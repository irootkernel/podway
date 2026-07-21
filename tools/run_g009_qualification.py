#!/usr/bin/env python3
"""Fail-closed Apple-Silicon G009 qualification runner.

There is intentionally no aggregate command: characterization, human approval, RC freeze,
and unseen holdout remain separate irreversible checkpoints.
"""
from __future__ import annotations
import argparse
import base64
import binascii
import io
import json
import re
import os
import select
import resource
import platform
import shutil
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import struct
import stat
import tomllib
import plistlib
import zipfile
from pathlib import Path
from typing import Any

from g009_common import (
    CONTROLLER_ROOT,
    EVIDENCE_ROOT,
    ROOT,
    TARGET,
    QualificationError,
    archive_root,
    atomic_immutable_json,
    bounded_bytes,
    bounded_process,
    bounded_regular_tree,
    canonical_json,
    content_addressed_json,
    fail,
    host_manifest,
    load_json,
    load_json_bytes,
    profile_target_tuple,
    require_candidate_root,
    require_native_host,
    safe_relative,
    sha256_bytes,
    sha256_file,
    target_tuple,
)
from g009_performance import SAMPLES, WARMUPS, characterize as calculate_baseline, evaluate_holdout, thresholds
from g009_release import inspect_archive, rc_input_root, require_bound_file, resolve_rc_input, verify_rc_consumption, verify_role_signatures

QUALIFICATION_ZIP_MEMBER_MAX_BYTES = 64 * 1024 * 1024
QUALIFICATION_ZIP_AGGREGATE_MAX_BYTES = 256 * 1024 * 1024
QUALIFICATION_ZIP_MEMBER_COUNT_MAX = 4096


def _preflight_qualification_bundle(bundle: zipfile.ZipFile) -> None:
    aggregate = 0
    infos = bundle.infolist()
    if len(infos) > QUALIFICATION_ZIP_MEMBER_COUNT_MAX:
        fail("qualification bundle member count exceeds frozen limit")
    for info in infos:
        if (
            info.is_dir()
            or info.file_size < 0
            or info.file_size > QUALIFICATION_ZIP_MEMBER_MAX_BYTES
        ):
            fail("qualification bundle member exceeds uncompressed size limit")
        aggregate += info.file_size
        if aggregate > QUALIFICATION_ZIP_AGGREGATE_MAX_BYTES:
            fail("qualification bundle aggregate exceeds uncompressed size limit")


# User input never supplies executable vectors. These logical gate identifiers are the
# complete subprocess allowlist; profile declarations are checked against this map.
GATES: dict[str, tuple[tuple[str, ...], ...]] = {
    "G009-GATE-FORMAT": (("cargo", "+1.85.0", "fmt", "--all", "--", "--check"),),
    "G009-GATE-CHECK": (("cargo", "+1.85.0", "check", "--workspace", "--all-targets", "--target", TARGET),),
    "G009-GATE-CLIPPY": (("cargo", "+1.85.0", "clippy", "--workspace", "--all-targets", "--all-features", "--target", TARGET, "--", "-D", "warnings"),),
    "G009-GATE-NATIVE-TESTS": (("cargo", "+1.85.0", "test", "--workspace", "--all-targets", "--target", TARGET),),
    "G009-GATE-CONTRACTS": (("python3", str(CONTROLLER_ROOT / "tools/run_verification.py"), "--run"),),
    "G009-GATE-G005": (("python3", str(CONTROLLER_ROOT / "tools/run_g005_vertical.py")),),
    "G009-GATE-G008": (("python3", str(CONTROLLER_ROOT / "tools/run_g008_dogfood.py")),),
    "G009-GATE-CRASH": (
        ("python3", str(CONTROLLER_ROOT / "tools/verify_g009_qualification.py"), "--crash-registry", "quality/crash-boundaries-v1.json"),
        ("cargo", "+1.85.0", "test", "-p", "podway-store", "--test", "phase2_crash_matrix", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase4_execution", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase4_registry", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase5_reset_runtime", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-service", "--test", "phase8_crash_boundaries", "--target", TARGET),
    ),
    "G009-GATE-OBS": (
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase8_observability", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-service", "--test", "phase8_observability", "--target", TARGET),
    ),
    "G009-GATE-SECURITY": (
        ("cargo", "+1.85.0", "test", "-p", "podway-cli", "--test", "phase4_commands", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase4_endpoint", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase4_registry", "--target", TARGET),
    ),
    "G009-GATE-MIGRATION": (
        ("cargo", "+1.85.0", "test", "-p", "podway-store", "--test", "phase2_schema_codec", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-store", "--test", "phase2_integrity", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-store", "--test", "phase2_reset_lifecycle", "--target", TARGET),
    ),
    "G009-GATE-AUDIT": (("cargo", "+1.85.0", "audit", "--deny", "warnings"),),
    "G009-GATE-DENY": (("cargo", "+1.85.0", "deny", "check", "advisories", "bans", "licenses", "sources"),),
    "G009-GATE-COVERAGE": (("cargo", "+1.85.0", "llvm-cov", "report", "--target", TARGET, "--summary-only"),),
    "G009-GATE-FUZZ": (),
}
CONTROLLER_SOURCE_IDS = (
    ".github/workflows/release.yml",
    ".github/workflows/release-final-review.yml",
    ".github/workflows/release-publish.yml",
    "tools/g009_common.py",
    "tools/g009_performance.py",
    "tools/g009_release.py",
    "tools/run_g009_qualification.py",
    "tools/run_verification.py",
    "tools/run_g005_vertical.py",
    "tools/run_g008_dogfood.py",
    "tools/verify_g009_qualification.py",
    "tools/g009_publication.py",
)
FUZZ_TARGETS = ("frame_decoder", "request_envelope", "response_additive", "config_procedure", "canonical_json", "selector")
QUALIFICATION_PROFILES = {
    "aarch64-apple-darwin": "release/g009-qualification-v1.json",
}
FUZZ_POLICY_MODES = frozenset({"rc", "local_smoke"})
FUZZ_SEED_FIELDS = {"name", "target", "sha256", "base64"}
def fuzz_seeds(profile_data: dict[str, Any]) -> list[dict[str, Any]]:
    fuzz = profile_data.get("fuzz")
    seeds = fuzz.get("seeds") if isinstance(fuzz, dict) else None
    if not isinstance(seeds, list) or len(seeds) != len(FUZZ_TARGETS):
        fail("fuzz seed declarations are incomplete")
    normalized: list[dict[str, Any]] = []
    for target, seed in zip(FUZZ_TARGETS, seeds):
        if not isinstance(seed, dict) or set(seed) != FUZZ_SEED_FIELDS:
            fail("fuzz seed declaration schema drift")
        name, encoded, digest = seed["name"], seed["base64"], seed["sha256"]
        if seed["target"] != target or name != f"{target}-valid-v1" or not isinstance(encoded, str) or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
            fail("fuzz seed declaration binding drift")
        try:
            data = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError, binascii.Error):
            fail("fuzz seed encoding is invalid")
        if base64.b64encode(data).decode("ascii") != encoded or len(data) < 16 or sha256_bytes(data) != digest:
            fail("fuzz seed bytes are not exact and nontrivial")
        normalized.append({"name": name, "target": target, "sha256": digest, "base64": encoded, "bytes": data})
    return normalized
IDENTITY_COMMANDS = (("git", "status", "--porcelain"), ("git", "rev-parse", "HEAD"),
                     ("git", "rev-parse", "HEAD^{tree}"), ("rustc", "+1.85.0", "--version"),
                     ("cargo", "+1.85.0", "--version"))
LABEL = "dev.podway.podwayd"
def qualification_scratch_root() -> Path:
    raw = os.environ.get("G009_SCRATCH_ROOT")
    if raw is None:
        fail("qualification scratch root is unavailable")
    root = Path(raw)
    if not root.is_absolute() or root.is_symlink() or not root.is_dir():
        fail("qualification scratch root is unsafe")
    resolved = root.resolve()
    candidate = require_candidate_root()
    protected = (candidate, CONTROLLER_ROOT.resolve(), *_protected_roots()[:2])
    if any(resolved == path or resolved.is_relative_to(path) or path.is_relative_to(resolved) for path in protected):
        fail("qualification scratch root overlaps an immutable root")
    return resolved
def resolved_tool_argv(argv: tuple[str, ...]) -> tuple[str, ...]:
    if len(argv) > 1 and argv[0] in {"cargo", "rustc"} and argv[1].startswith("+"):
        rustup = shutil.which("rustup")
        if rustup is None:
            fail("rustup is required for pinned Rust commands")
        return (rustup, "run", argv[1][1:], argv[0], *argv[2:])
    return argv
def _candidate_source_roots(candidate_root: Path) -> tuple[Path, ...]:
    writable_outputs = {".git", "artifacts", "target"}
    nested_writable_outputs = {"fuzz": {"artifacts", "corpus", "target"}}
    roots = []
    try:
        entries = sorted(candidate_root.iterdir())
    except OSError as exc:
        fail(f"candidate source root cannot be enumerated: {exc}")
    for entry in entries:
        if entry.name in writable_outputs:
            if entry.is_symlink() or (
                entry.name != ".git" and not entry.is_dir()
            ) or (entry.name == ".git" and not (entry.is_dir() or entry.is_file())):
                fail("candidate writable output root is unsafe")
            continue
        if entry.is_symlink() or not (entry.is_dir() or entry.is_file()):
            fail("candidate source entry is unsafe")
        if entry.name in nested_writable_outputs:
            for child in sorted(entry.iterdir()):
                if child.name in nested_writable_outputs[entry.name]:
                    if child.is_symlink() or not child.is_dir():
                        fail("candidate nested writable output root is unsafe")
                    continue
                if child.is_symlink() or not (child.is_dir() or child.is_file()):
                    fail("candidate nested source entry is unsafe")
                roots.append(child)
        else:
            roots.append(entry)
    return tuple(roots)


def _protected_roots() -> tuple[Path, ...]:
    immutable_raw = os.environ.get("G009_IMMUTABLE_INPUT_ROOT")
    if immutable_raw is None:
        fail("protected RC input root is unavailable")
    immutable_root = Path(immutable_raw)
    if not immutable_root.is_absolute() or immutable_root.is_symlink() or not immutable_root.is_dir():
        fail("protected RC input root is unsafe")
    candidate_root = require_candidate_root()
    protected_roots = (
        CONTROLLER_ROOT.resolve(),
        immutable_root.resolve(),
        *_candidate_source_roots(candidate_root),
    )
    if any(
        left == right or left.is_relative_to(right) or right.is_relative_to(left)
        for index, left in enumerate(protected_roots[:2])
        for right in protected_roots[index + 1:2]
    ) or any(
        candidate_root == root
        or candidate_root.is_relative_to(root)
        or root.is_relative_to(candidate_root)
        for root in protected_roots[:2]
    ):
        fail("protected roots must be disjoint from controlled roots")
    return protected_roots
class _CandidateSourceWatch:
    def __init__(self) -> None:
        self._queue = select.kqueue()
        self._descriptors: list[int] = []
        paths: list[Path] = []
        stack = list(_candidate_source_roots(require_candidate_root()))
        while stack:
            path = stack.pop()
            paths.append(path)
            if path.is_dir():
                children = list(os.scandir(path))
                if len(paths) + len(stack) + len(children) > 8192:
                    fail("candidate source watch exceeds frozen entry limit")
                for child in children:
                    child_path = Path(child.path)
                    if child.is_symlink():
                        fail("candidate source watch encountered a symlink")
                    stack.append(child_path)
            elif not path.is_file() or path.stat().st_nlink != 1:
                fail("candidate source file is non-regular or hard-linked")
        soft_limit, hard_limit = resource.getrlimit(resource.RLIMIT_NOFILE)
        required_limit = len(paths) + 64
        if soft_limit < required_limit:
            if hard_limit < required_limit:
                fail("candidate source watch exceeds descriptor hard limit")
            resource.setrlimit(resource.RLIMIT_NOFILE, (required_limit, hard_limit))
        try:
            changes = (
                select.KQ_NOTE_WRITE
                | select.KQ_NOTE_DELETE
                | select.KQ_NOTE_RENAME
                | select.KQ_NOTE_LINK
                | select.KQ_NOTE_REVOKE
            )
            for path in paths:
                descriptor = os.open(path, os.O_EVTONLY | os.O_NOFOLLOW)
                self._descriptors.append(descriptor)
                self._queue.control(
                    [
                        select.kevent(
                            descriptor,
                            filter=select.KQ_FILTER_VNODE,
                            flags=select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                            fflags=changes,
                        )
                    ],
                    0,
                    0,
                )
        except BaseException:
            self.close()
            raise

    def verify_unchanged(self) -> None:
        if self._queue.control(None, max(1, len(self._descriptors)), 0):
            fail("candidate source changed during qualification execution")

    def close(self) -> None:
        for descriptor in self._descriptors:
            os.close(descriptor)
        self._descriptors.clear()
        self._queue.close()
def _candidate_source_manifest() -> dict[str, Any]:
    entries: list[dict[str, Any]] = []
    candidate_root = require_candidate_root()
    for root in _candidate_source_roots(candidate_root):
        if root.is_file():
            entries.append(
                {
                    "path": root.relative_to(candidate_root).as_posix(),
                    "type": "regular",
                    "mode": root.stat().st_mode & 0o777,
                    "sha256": sha256_file(root),
                    "bytes": root.stat().st_size,
                }
            )
            continue
        remaining = 8192 - len(entries)
        if remaining < 1:
            fail("candidate source manifest exceeds frozen entry limit")
        for relative, path, size in bounded_regular_tree(
            root,
            member_limit=remaining,
            path_depth=64,
            path_length=4096,
            label="candidate source",
        ):
            entries.append(
                {
                    "path": (root.relative_to(candidate_root) / relative).as_posix(),
                    "type": "regular",
                    "mode": path.stat().st_mode & 0o777,
                    "sha256": sha256_file(path),
                    "bytes": size,
                }
            )
            if len(entries) > 8192:
                fail("candidate source manifest exceeds frozen entry limit")
    entries.sort(key=lambda item: item["path"])
    if len({item["path"] for item in entries}) != len(entries):
        fail("candidate source manifest contains duplicate paths")
    encoded = canonical_json({"entries": entries})
    return {"sha256": sha256_bytes(encoded), "entries": len(entries)}
def _candidate_commit_tree() -> tuple[str, str]:
    candidate_root = require_candidate_root()
    values: list[str] = []
    for revision in ("HEAD", "HEAD^{tree}"):
        result = _completed_bounded(
            ("git", "-C", str(candidate_root), "rev-parse", revision),
            cwd=CONTROLLER_ROOT,
            env={"PATH": os.environ.get("PATH", "")},
        )
        if result.returncode != 0:
            fail("candidate Git identity cannot be read")
        values.append(result.stdout.decode("ascii", "strict").strip())
    return values[0], values[1]


def _candidate_source_files(candidate_root: Path) -> set[Path]:
    files: set[Path] = set()
    for root in _candidate_source_roots(candidate_root):
        if root.is_file():
            files.add(root.resolve())
            continue
        for _, path, _ in bounded_regular_tree(
            root, member_limit=8192 - len(files), path_depth=64, path_length=4096,
            label="candidate source files",
        ):
            files.add(path.resolve())
            if len(files) > 8192:
                fail("candidate source files exceed frozen entry limit")
    return files


def _materialized_source_manifest(source_root: Path, source_files: set[Path]) -> dict[str, Any]:
    entries: list[dict[str, Any]] = []
    for source in source_files:
        relative = source.relative_to(source_root)
        if source.is_symlink() or not source.is_file():
            fail("materialized candidate source entry is unsafe")
        entries.append(
            {
                "path": relative.as_posix(),
                "type": "regular",
                "mode": source.stat().st_mode & 0o777,
                "sha256": sha256_file(source),
                "bytes": source.stat().st_size,
            }
        )
    entries.sort(key=lambda item: item["path"])
    return {
        "sha256": sha256_bytes(canonical_json({"entries": entries})),
        "entries": len(entries),
    }


def _materialize_candidate_source(destination: Path) -> tuple[set[Path], dict[str, Any]]:
    candidate_root = require_candidate_root()
    source_files = _candidate_source_files(candidate_root)
    if destination.exists() or destination.is_symlink():
        fail("materialized candidate source destination is unsafe")
    destination.mkdir(mode=0o755)
    materialized: set[Path] = set()
    for source in source_files:
        relative = source.relative_to(candidate_root)
        target = destination / relative
        target.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
        copied = bounded_process(
            sandboxed_candidate_argv(("/bin/cp", "-p", str(source), str(target))),
            cwd=CONTROLLER_ROOT,
            env={"PATH": os.environ.get("PATH", "")},
            timeout=30,
            stream_limit=FUZZ_STREAM_MAX_BYTES,
            aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
            allow_descendants=False,
        )
        if copied["terminal_mode"] != "success":
            fail("bounded candidate source materialization failed")
        if target.is_symlink() or not target.is_file() or sha256_file(target) != sha256_file(source):
            fail("protected source materialization differs from candidate source")
        materialized.add(target)
    expected = _candidate_source_manifest()
    if _materialized_source_manifest(destination, materialized) != expected:
        fail("materialized candidate source identity differs from trusted candidate source")
    return materialized, expected


def _verify_materialized_candidate_source(
    source_root: Path, source_files: set[Path], expected: dict[str, Any], allowed_outputs: set[Path] | None = None,
) -> None:
    if _candidate_source_manifest() != expected:
        fail("trusted candidate source changed during qualification execution")
    if _materialized_source_manifest(source_root, source_files) != expected:
        fail("materialized candidate source changed during qualification execution")
    allowed = {path.resolve() for path in (allowed_outputs or set())}
    actual = {
        path.resolve() for _, path, _ in bounded_regular_tree(
            source_root, member_limit=8192, path_depth=64, path_length=4096, label="materialized candidate baseline"
        )
    }
    if actual != {path.resolve() for path in source_files} | allowed:
        fail("materialized candidate source has an unclassified addition")


def _validate_metadata_build_inputs(metadata: bytes, source_root: Path, source_files: set[Path]) -> None:
    try:
        parsed = json.loads(metadata.decode("utf-8"))
        packages = parsed["packages"]
    except (KeyError, TypeError, UnicodeDecodeError, ValueError) as exc:
        fail(f"locked fuzz metadata is malformed: {exc}")
    if not isinstance(packages, list) or not packages:
        fail("locked fuzz metadata has no packages")
    allowed = {path.resolve() for path in source_files}
    for package in packages:
        if not isinstance(package, dict):
            fail("locked fuzz metadata package is malformed")
        if package.get("source") is not None:
            continue
        raw_manifest = package.get("manifest_path")
        targets = package.get("targets")
        if not isinstance(raw_manifest, str) or not isinstance(targets, list):
            fail("locked fuzz metadata package inputs are malformed")
        manifest = Path(raw_manifest).resolve()
        if manifest not in allowed or not manifest.is_relative_to(source_root):
            fail("locked fuzz metadata manifest is outside protected source materialization")
        for target in targets:
            raw_source = target.get("src_path") if isinstance(target, dict) else None
            if not isinstance(raw_source, str):
                fail("locked fuzz metadata target source is malformed")
            source = Path(raw_source).resolve()
            if source not in allowed or not source.is_relative_to(source_root):
                fail("locked fuzz metadata target source is outside protected source materialization")
def _isolated_cargo_home(destination: Path) -> tuple[Path, tuple[Path, ...]]:
    home = destination / "cargo-home"
    home.mkdir(mode=0o755)
    (home / "config.toml").write_text("[net]\noffline = true\n", encoding="utf-8")
    configured_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    if not configured_home.is_absolute():
        fail("configured Cargo home is not absolute")
    cache_roots: list[Path] = []
    for name in ("registry", "git"):
        source = configured_home / name
        if not source.exists():
            continue
        if source.is_symlink() or not source.is_dir():
            fail("configured Cargo cache is unsafe")
        resolved = source.resolve()
        (home / name).symlink_to(resolved, target_is_directory=True)
        cache_roots.append(resolved)
    return home, tuple(cache_roots)


def _cargo_config_read_denials(source_root: Path) -> tuple[Path, ...]:
    denied: list[Path] = []
    cursor = source_root.parent
    while True:
        denied.extend((cursor / ".cargo" / "config.toml", cursor / ".cargo" / "config"))
        if cursor == cursor.parent:
            break
        cursor = cursor.parent
    return tuple(denied)
def _excluded_candidate_build_inputs() -> tuple[Path, ...]:
    candidate_root = require_candidate_root()
    return (
        candidate_root / "artifacts",
        candidate_root / "target",
        candidate_root / "fuzz" / "artifacts",
        candidate_root / "fuzz" / "corpus",
        candidate_root / "fuzz" / "target",
    )
def _validate_candidate_cargo_lookup_paths() -> None:
    candidate_root = require_candidate_root()
    lookup_starts = (candidate_root, candidate_root / "fuzz")
    checked: set[Path] = set()
    for start in lookup_starts:
        cursor = start
        while True:
            if not cursor.is_relative_to(candidate_root):
                fail("candidate Cargo lookup path escapes candidate root")
            if cursor not in checked:
                checked.add(cursor)
                cargo_directory = cursor / ".cargo"
                if cargo_directory.is_symlink():
                    fail("candidate Cargo configuration directory is unsafe")
                for name in ("config.toml", "config"):
                    cargo_config = cargo_directory / name
                    if cargo_config.exists() or cargo_config.is_symlink():
                        fail("candidate Cargo configuration is forbidden during qualification")
            if cursor == candidate_root:
                break
            cursor = cursor.parent


def _candidate_manifest_paths(candidate_root: Path) -> dict[Path, dict[str, Any]]:
    manifests: dict[Path, dict[str, Any]] = {}
    for root in _candidate_source_roots(candidate_root):
        paths = [root] if root.is_file() else [
            path
            for _, path, _ in bounded_regular_tree(
                root,
                member_limit=8192,
                path_depth=64,
                path_length=4096,
                label="candidate build surface",
            )
        ]
        for path in paths:
            if path.name == "build.rs":
                fail("candidate build scripts are forbidden during qualification")
            if path.name != "Cargo.toml":
                continue
            try:
                manifest = tomllib.loads(bounded_bytes(path).decode("utf-8"))
            except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
                fail(f"candidate Cargo manifest is malformed: {exc}")
            package = manifest.get("package", {})
            library = manifest.get("lib", {})
            if (
                isinstance(package, dict)
                and package.get("build") not in (None, False)
            ) or (
                isinstance(library, dict)
                and library.get("proc-macro") is True
            ):
                fail("candidate-defined build hooks are forbidden during qualification")
            manifests[path.resolve()] = manifest
    return manifests


def _dependency_tables(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    tables: list[dict[str, Any]] = []
    for name in ("dependencies", "dev-dependencies", "build-dependencies", "replace"):
        value = manifest.get(name)
        if isinstance(value, dict):
            tables.append(value)
    workspace = manifest.get("workspace")
    if isinstance(workspace, dict) and isinstance(workspace.get("dependencies"), dict):
        tables.append(workspace["dependencies"])
    target = manifest.get("target")
    if isinstance(target, dict):
        for target_value in target.values():
            if not isinstance(target_value, dict):
                continue
            for name in ("dependencies", "dev-dependencies", "build-dependencies"):
                value = target_value.get(name)
                if isinstance(value, dict):
                    tables.append(value)
    patch = manifest.get("patch")
    if isinstance(patch, dict):
        tables.extend(value for value in patch.values() if isinstance(value, dict))
    return tables


def _active_fuzz_lockfile() -> dict[str, str]:
    candidate_root = require_candidate_root()
    lockfile = candidate_root / "fuzz" / "Cargo.lock"
    manifest = candidate_root / "fuzz" / "Cargo.toml"
    if (
        lockfile.is_symlink()
        or not lockfile.is_file()
        or manifest.is_symlink()
        or not manifest.is_file()
    ):
        fail("active fuzz lockfile or manifest is absent or unsafe")
    try:
        fuzz_manifest = tomllib.loads(bounded_bytes(manifest).decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        fail(f"candidate fuzz manifest is malformed: {exc}")
    if fuzz_manifest.get("workspace") != {}:
        fail("fuzz manifest must be a standalone empty workspace")
    return {"path": "fuzz/Cargo.lock", "sha256": sha256_file(lockfile)}


def _validate_candidate_build_surface() -> None:
    candidate_root = require_candidate_root()
    forbidden_environment = (
        "CARGO_CONFIG",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTFLAGS",
    )
    active_hooks = [
        name
        for name, value in os.environ.items()
        if value
        and (
            name in forbidden_environment
            or name.startswith("CARGO_ALIAS_")
            or (
                name.startswith("CARGO_TARGET_")
                and (name.endswith("_RUNNER") or name.endswith("_RUSTFLAGS"))
            )
        )
    ]
    if active_hooks:
        fail("candidate Cargo executable hook environment is forbidden during qualification")
    cargo_config = candidate_root / ".cargo"
    if cargo_config.exists() or cargo_config.is_symlink():
        fail("candidate-local Cargo configuration is forbidden during qualification")
    _validate_candidate_cargo_lookup_paths()
    manifests = _candidate_manifest_paths(candidate_root)
    if (candidate_root / "fuzz" / "Cargo.toml").resolve() not in manifests:
        fail("candidate fuzz manifest is outside the protected source set")
    for manifest_path, manifest in manifests.items():
        for table in _dependency_tables(manifest):
            for dependency_name, dependency in table.items():
                if not isinstance(dependency_name, str):
                    fail("candidate dependency name is malformed")
                if not isinstance(dependency, dict) or "path" not in dependency:
                    continue
                relative = dependency["path"]
                if not isinstance(relative, str) or not relative:
                    fail("candidate path dependency is malformed")
                dependency_root = (manifest_path.parent / relative).resolve()
                dependency_manifest = (dependency_root / "Cargo.toml").resolve()
                if (
                    not dependency_root.is_relative_to(candidate_root)
                    or dependency_manifest not in manifests
                ):
                    fail("candidate path dependency escapes the protected source set")
                dependency_package = manifests[dependency_manifest].get("package", {})
                dependency_library = manifests[dependency_manifest].get("lib", {})
                if (
                    isinstance(dependency_package, dict)
                    and isinstance(dependency_library, dict)
                    and dependency_library.get("proc-macro") is True
                ):
                    fail("candidate proc-macro dependency is forbidden during qualification")
    _active_fuzz_lockfile()


def _completed_from_capture(
    argv: tuple[str, ...], captured: dict[str, Any],
) -> subprocess.CompletedProcess[bytes]:
    terminal_mode = captured["terminal_mode"]
    if terminal_mode in {"success", "nonzero_exit"}:
        returncode = captured["exit_code"]
    elif terminal_mode == "signal":
        returncode = -(captured["signal"] or signal.SIGKILL)
    else:
        returncode = 124
    assert isinstance(returncode, int)
    return subprocess.CompletedProcess(
        argv,
        returncode,
        captured["stdout"],
        captured["stderr"],
    )


def _completed_bounded(
    argv: tuple[str, ...],
    *,
    cwd: Path = ROOT,
    env: dict[str, str] | None = None,
    timeout: float = 30,
    allow_descendants: bool = False,
) -> subprocess.CompletedProcess[bytes]:
    captured = bounded_process(
        argv,
        cwd=cwd,
        env=env or {"PATH": os.environ.get("PATH", "")},
        timeout=timeout,
        stream_limit=FUZZ_STREAM_MAX_BYTES,
        aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
        allow_descendants=allow_descendants,
    )
    return _completed_from_capture(argv, captured)


def _sandboxed_completed(
    argv: tuple[str, ...], *, cwd: Path, env: dict[str, str], timeout: float = 30,
) -> subprocess.CompletedProcess[bytes]:
    environment = dict(env)
    scratch = qualification_scratch_root()
    for name in ("home", "tmp"):
        (scratch / name).mkdir(mode=0o700, exist_ok=True)
    environment.setdefault("HOME", str(scratch / "home"))
    environment.setdefault("TMPDIR", str(scratch / "tmp"))
    return _completed_bounded(
        sandboxed_candidate_argv(argv),
        cwd=cwd,
        env=environment,
        timeout=timeout,
        allow_descendants=True,
    )

def _github_command_channels() -> tuple[Path, ...]:
    channels = []
    for name in ("GITHUB_ENV", "GITHUB_OUTPUT", "GITHUB_PATH", "GITHUB_STATE", "GITHUB_STEP_SUMMARY"):
        raw = os.environ.get(name)
        if raw is None:
            continue
        path = Path(raw)
        if not path.is_absolute() or any(character in raw for character in ('"', "\n", "\r")):
            fail(f"{name} cannot be represented in the sandbox policy")
        channels.append(path)
    return tuple(channels)


def sandboxed_candidate_argv(
    argv: tuple[str, ...], *, allow_process_fork: bool = True,
    read_only_paths: tuple[Path, ...] = (),
    read_denied_paths: tuple[Path, ...] = (),
) -> tuple[str, ...]:
    protected_roots = _protected_roots()
    candidate_root = require_candidate_root()
    scratch_root = qualification_scratch_root()
    protected_paths = list(protected_roots)
    for path in read_only_paths:
        if not path.is_absolute() or path.is_symlink():
            fail("sandbox read-only path is unsafe")
        protected_paths.append(path.resolve())
    rendered_roots = [str(path) for path in protected_paths]
    rendered_scratch = str(scratch_root)
    if any(character in root for root in (*rendered_roots, str(candidate_root), rendered_scratch) for character in ('"', "\n", "\r")):
        fail("protected path cannot be represented in the sandbox policy")
    deny_rules = (
        f'(deny file-write* (literal "{candidate_root}"))'
        f'(deny file-write* (subpath "{candidate_root}"))'
    )
    deny_rules += "".join(
        f'(deny file-write* (literal "{root}"))'
        f'(deny file-write* (subpath "{root}"))'
        for root in rendered_roots
    )
    deny_rules += "".join(
        f'(deny file-write* (literal "{path}"))' for path in _github_command_channels()
    )
    deny_rules += f'(deny file-link (subpath "{candidate_root}"))'
    deny_rules += "".join(
        f'(deny file-link (literal "{root}"))'
        f'(deny file-link (subpath "{root}"))'
        for root in rendered_roots
    )
    for path in read_denied_paths:
        if not path.is_absolute() or path.is_symlink():
            fail("sandbox read-denial path is unsafe")
        rendered = str(path)
        if any(character in rendered for character in ('"', "\n", "\r")):
            fail("sandbox read-denial path cannot be represented in the policy")
        deny_rules += f'(deny file-read* (literal "{rendered}"))'
        deny_rules += f'(deny file-read* (subpath "{rendered}"))'
    process_rule = "(allow process-fork)" if allow_process_fork else "(deny process-fork)"
    profile_text = (
        "(version 1)(deny default)"
        "(allow process-exec)(allow process-info*)(allow file-read*)(allow sysctl-read)"
        f'(allow file-write* (subpath "{rendered_scratch}"))'
        f"{process_rule}{deny_rules}"
    )
    return ("/usr/bin/sandbox-exec", "-p", profile_text, *argv)


def sandboxed_fuzz_execution_argv(
    argv: tuple[str, ...], *, corpus: Path, scratch: Path,
) -> tuple[str, ...]:
    protected_roots = (require_candidate_root(), *_protected_roots())
    for path in (corpus, scratch):
        if not path.is_absolute() or path.is_symlink() or not path.is_dir():
            fail("fuzz execution writable path is unsafe")
        if any(path.resolve() == root or path.resolve().is_relative_to(root) for root in protected_roots):
            fail("fuzz execution writable path overlaps an immutable root")
    rendered = tuple(str(path) for path in (corpus, scratch))
    if any(any(character in path for character in ('"', "\n", "\r")) for path in rendered):
        fail("fuzz execution writable path cannot be represented in the policy")
    profile_text = (
        "(version 1)(deny default)"
        "(allow process-exec)(allow process-info*)"
        "(allow file-read*)(allow sysctl-read)"
        f'(allow file-write* (subpath "{rendered[0]}"))'
        f'(allow file-write* (subpath "{rendered[1]}"))'
    )
    return ("/usr/bin/sandbox-exec", "-p", profile_text, *argv)


def assert_descendant_write_protection() -> None:
    protected_roots = _protected_roots()
    controller_root, immutable_root = protected_roots[:2]
    candidate_root = require_candidate_root()
    candidate_probe = next(
        (path for path in _candidate_source_roots(candidate_root) if path.is_file()),
        None,
    )
    controller_probe = controller_root / "tools" / "run_g009_qualification.py"
    immutable_probe = next(
        (
            path
            for path in sorted(immutable_root.iterdir())
            if not path.is_symlink() and path.is_file()
        ),
        None,
    )
    if not controller_probe.is_file() or immutable_probe is None or candidate_probe is None:
        fail("protected-write sentinel inputs are unavailable")
    source_before = _candidate_source_manifest()
    transient = qualification_scratch_root() / ".g009-transient-create-delete-sentinel"
    descriptor = os.open(transient, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    os.close(descriptor)
    transient.unlink()
    if transient.exists():
        fail("qualification scratch transient sentinel persisted")
    child = (
        "import os, sys\n"
        "from pathlib import Path\n"
        "root, probe = map(Path, sys.argv[1:])\n"
        "attempts = {\n"
        "    'new-root-child': lambda: (root / '.g009-build-write').write_bytes(b'x'),\n"
        "    'existing-file-mutation': lambda: os.open(probe, os.O_WRONLY),\n"
        "    'rename': lambda: os.rename(probe, root / '.g009-test-rename'),\n"
        "    'symlink': lambda: os.symlink(probe, root / '.g009-daemon-link'),\n"
        "    'workload-write': lambda: (root / '.g009-workload-write').write_bytes(b'x'),\n"
        "}\n"
        "for label, attempt in attempts.items():\n"
        "    try:\n"
        "        descriptor = attempt()\n"
        "    except OSError:\n"
        "        continue\n"
        "    if isinstance(descriptor, int): os.close(descriptor)\n"
        "    raise SystemExit(f'candidate write was permitted: {label}')\n"
        "if any((root / name).exists() or (root / name).is_symlink() for name in (\n"
        "    '.g009-build-write', '.g009-test-rename', '.g009-daemon-link', '.g009-workload-write',\n"
        ")):\n"
        "    raise SystemExit('candidate write attempt persisted')\n"
    )
    result = _sandboxed_completed(
        (sys.executable, "-c", child, str(candidate_root), str(candidate_probe)),
        cwd=ROOT,
        env={"PATH": os.environ.get("PATH", "")},
        timeout=15,
    )
    if result.returncode != 0 or _candidate_source_manifest() != source_before:
        fail("sandbox recursive candidate-write sentinel failed")


def run_allowed(
    argv: tuple[str, ...],
    *,
    cwd: Path = ROOT,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
) -> subprocess.CompletedProcess[bytes]:
    environment = {"PATH": os.environ.get("PATH", "")}
    if env is not None:
        environment.update(env)
    environment["G009_CANDIDATE_ROOT"] = str(require_candidate_root())
    scratch = qualification_scratch_root()
    for name in ("cargo-home", "home", "tmp"):
        (scratch / name).mkdir(mode=0o700, exist_ok=True)
    environment.setdefault("CARGO_HOME", str(scratch / "cargo-home"))
    environment.setdefault("HOME", str(scratch / "home"))
    environment.setdefault("TMPDIR", str(scratch / "tmp"))
    _validate_candidate_build_surface()
    source_before = _candidate_source_manifest()
    identity_before = _candidate_commit_tree()
    resolved = resolved_tool_argv(argv)
    watch = _CandidateSourceWatch()
    try:
        with tempfile.TemporaryDirectory(prefix="g009-cargo-target-", dir=qualification_scratch_root()) as target_dir:
            environment["CARGO_TARGET_DIR"] = target_dir
            result = bounded_process(
                sandboxed_candidate_argv(resolved),
                cwd=cwd,
                env=environment,
                timeout=timeout or 900,
                stream_limit=FUZZ_STREAM_MAX_BYTES,
                aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
                allow_descendants=True,
            )
        watch.verify_unchanged()
        if _candidate_source_manifest() != source_before:
            fail("candidate source bytes changed during Cargo command")
        if _candidate_commit_tree() != identity_before:
            fail("candidate commit or tree changed during Cargo command")
    finally:
        watch.close()
    return _completed_from_capture(argv, result)

WORKLOAD_ADAPTER_IDS = frozenset({"G009-W01", "G009-W02", "G009-W03", "G009-W04", "G009-W05", "G009-W06", "G009-W07"})
WORKLOAD_COMMANDS = {
    "G009-W01": [["podwayd", "--service"]],
    "G009-W02": [["podway", "status"], ["podway", "next"]],
    "G009-W03": [["podway", "start", "--procedure", ".g009-procedure.yaml", "--task", "G009-linked"]],
    "G009-W04": [["podway", "set", "target-audience", "updated"]],
    "G009-W05": [["podway", "attach", "draft-reference", ".g009-artifact.bin"]],
    "G009-W06": [["podway", "status"]],
    "G009-W07": [["podway", "set", "target-audience", "<65536-byte-string>"]],
}
def _w07_generator(item: dict[str, Any]) -> dict[str, Any]:
    if item.get("input_generator_ref") != "release/g009-release-policy-v1.json#/w07_generator" or "input_generator" in item:
        fail("W07 must use the sole controller-owned generator reference")
    contract = item.get("adapter_contract")
    if contract != {
        "id": "G009-W07",
        "argv_prefix": ["podway", "set", "target-audience"],
        "argument_index": 2,
        "argument_source": "generated_utf8_string",
    }:
        fail("W07 adapter contract drift")
    policy = load_json(CONTROLLER_ROOT / "release/g009-release-policy-v1.json")
    if not isinstance(policy, dict) or policy.get("schema") != "podway.g009.release-policy/v1":
        fail("W07 release policy is invalid")
    generated = policy.get("w07_generator")
    if not isinstance(generated, dict) or set(generated) != {
        "id",
        "authoritative_fields",
        "algorithm",
        "code_point",
        "utf8_byte_length",
        "digest",
        "version",
        "reject_non_authoritative_generator_fields",
    }:
        fail("W07 input generator is incomplete")
    digest = generated["digest"]
    if (
        generated["id"] != "G009-W07"
        or generated["authoritative_fields"] != [
            "algorithm",
            "code_point",
            "utf8_byte_length",
            "digest.algorithm",
            "digest.derivation",
            "digest.hex",
            "version",
        ]
        or generated["reject_non_authoritative_generator_fields"] is not True
        or generated["algorithm"] != "repeat-utf8-code-point"
        or generated["version"] != 1
        or not isinstance(digest, dict)
        or set(digest) != {"algorithm", "derivation", "hex"}
        or digest.get("algorithm") != "sha256"
        or digest.get("derivation") != "sha256(generated_utf8_bytes)"
        or not isinstance(digest.get("hex"), str)
    ):
        fail("W07 input generator contract drift")
    code_point = generated["code_point"]
    if not isinstance(code_point, str) or not code_point.startswith("U+"):
        fail("W07 code point is malformed")
    try:
        scalar = chr(int(code_point[2:], 16))
        encoded = scalar.encode("utf-8")
    except (TypeError, ValueError, UnicodeError):
        fail("W07 code point is malformed")
    length = generated["utf8_byte_length"]
    if not isinstance(length, int) or length <= 0 or length % len(encoded):
        fail("W07 byte length is not generated from its UTF-8 code point")
    payload = encoded * (length // len(encoded))
    if sha256_bytes(payload) != digest["hex"]:
        fail("W07 generator digest does not bind generated UTF-8 bytes")
    return {
        "code_point": code_point,
        "utf8_byte_length": length,
        "sha256": digest["hex"],
        "argument_index": contract["argument_index"],
        "bytes": payload,
    }



def profile(path: Path) -> dict[str, Any]:
    value = load_json(path)
    required = {"archive", "release_policy", "release_evidence", "evidence", "fuzz", "gates", "external_prerequisites", "invalidation",
                "minimum_macos", "minimum_macos_scope", "observability", "performance", "profile", "release_profile",
                "rust", "schema", "signing_postures", "target", "version", "workloads", "workflow_checkpoints"}
    if not isinstance(value, dict) or set(value) != required: fail("profile schema is not closed")
    if value["evidence"] != {"committed_results_permitted": False, "local_root": "artifacts/g009",
                             "schema": "podway.g009.checkpoint/v1", "status_values": ["not-run", "pass", "fail", "blocked"],
                             "untrusted_mutable_pointers_forbidden": True, "fresh_run_identity_required": True,
                             "exact_command_result_schema_required": True}: fail("evidence checkpoint schema drift")
    if value["release_evidence"] != {
        "current_public_package": {
            "posture": "unsigned-not-notarized",
            "codesign": "not_attempted_missing_credentials",
            "notarization": "not_attempted_missing_credentials",
            "stapling": "not_applicable_zip",
            "gatekeeper": "not_claimed",
            "release_notes_asset": "RELEASE_NOTES.md",
            "release_notes_must_document_status": True,
            "status_frozen_for_current_release": True,
        },
        "developer_id_and_notarization": {
            "policy_ref": "release/g009-release-policy-v1.json#/signing_evidence/developer_id_and_notarization",
            "qualification_requirement": False,
            "detached_human_release_step": True,
        },
    }:
        fail("release signing evidence policy drift")
    if value["signing_postures"] != {
        "unsigned-internal": {
            "codesign": "not_attempted_missing_credentials",
            "gatekeeper": "not_claimed",
            "notarization": "not_attempted_missing_credentials",
            "stapling": "not_applicable_zip",
        }
    }:
        fail("release candidate signing posture drift")
    target = value["target"]
    if value["schema"] != "podway.g009.qualification/v1" or value["version"] != 1:
        fail("unsupported profile")
    profile_target_tuple(target)
    if not isinstance(value["archive"], dict) or value["archive"].get("root") != archive_root(target["triple"]) + "/":
        fail("profile archive root does not bind its native target tuple")
    if value["rust"] != {"channel": "1.85.0", "version": "1.85.0"}: fail("profile Rust identity is not 1.85.0")
    if value["release_profile"] != {"codegen-units": 1, "lto": "thin", "panic": "abort", "strip": "symbols"}: fail("release flags drift")
    perf = value["performance"]
    if not isinstance(perf, dict) or perf.get("warmups") != WARMUPS or perf.get("characterization_samples") != SAMPLES or perf.get("holdout_samples") != SAMPLES or perf.get("rounding_permitted") is not False: fail("performance protocol drift")
    workloads = value["workloads"]
    if not isinstance(workloads, list) or len(workloads) != 7 or len({item.get("id") for item in workloads if isinstance(item, dict)}) != 7: fail("profile must define exactly seven unique workloads")
    for item in workloads:
        if not isinstance(item, dict) or not isinstance(item.get("name"), str):
            fail("malformed workload declaration")
        workload_id = item.get("id")
        if workload_id not in WORKLOAD_ADAPTER_IDS or item.get("adapter_id") != workload_id:
            fail("workload has no matching native adapter")
        if item.get("measured_commands") != WORKLOAD_COMMANDS[workload_id]:
            fail("workload command contract differs from its native adapter")
        if workload_id == "G009-W07":
            _w07_generator(item)
        if not isinstance(item.get("hard_bounds"), dict) or not all(isinstance(item["hard_bounds"].get(key), int) and item["hard_bounds"][key] > 0 for key in ("max_completion_ms", "max_rss_mib")): fail("malformed workload hard bounds")
    fuzz = value["fuzz"]
    expected_fuzz_keys = {"toolchain", "sanitizer_env", "runner", "change_budget", "corpus_root", "pre_rc", "rc", "seeds", "local_smoke", "surfaces"}
    if not isinstance(fuzz, dict) or set(fuzz) != expected_fuzz_keys or fuzz.get("corpus_root") != "artifacts/g009/fuzz/corpus" or fuzz.get("surfaces") != list(FUZZ_TARGETS):
        fail("fuzz target contract drift")
    if fuzz.get("toolchain") != {"channel": "nightly-2026-07-17", "rustc": "1.99.0-nightly (3d50c25bc 2026-07-16)"}:
        fail("fuzz toolchain contract drift")
    if fuzz.get("sanitizer_env") != {"ASAN_OPTIONS": "quarantine_size_mb=16:thread_local_quarantine_size_kb=64:detect_odr_violation=0"}:
        fail("fuzz sanitizer environment drift")
    expected_runner = {
        "schema": "podway.g009.fuzz-runner/v1",
        "stream_bytes": FUZZ_STREAM_MAX_BYTES,
        "aggregate_bytes": FUZZ_AGGREGATE_MAX_BYTES,
        "post_kill_drain_seconds": 2,
        "archive_materialization_bytes": FUZZ_ARCHIVE_MATERIALIZATION_MAX_BYTES,
        "corpus_member_count": FUZZ_CORPUS_MEMBER_COUNT,
        "corpus_member_bytes": FUZZ_CORPUS_MEMBER_MAX_BYTES,
        "corpus_aggregate_bytes": FUZZ_CORPUS_AGGREGATE_MAX_BYTES,
        "corpus_path_depth": FUZZ_CORPUS_PATH_MAX_DEPTH,
        "corpus_path_length": FUZZ_CORPUS_PATH_MAX_LENGTH,
        "manifest_bytes": FUZZ_MANIFEST_MAX_BYTES,
        "fuzz_dependency_materialization_bytes": FUZZ_DEPENDENCY_MATERIALIZATION_MAX_BYTES,
    }
    if fuzz.get("runner") != expected_runner:
        fail("fuzz runner schema drift")
    expected_fuzz_policies = {
        "rc": {"rss_limit_mb": 512, "seconds_per_target": 3600, "timeout_seconds": 5},
        "local_smoke": {"rss_limit_mb": 512, "seconds_per_target": 5, "timeout_seconds": 5},
    }
    if fuzz.get("pre_rc") != {"seconds_per_target": 600} or fuzz.get("change_budget") != {"seconds_per_target": 60} or any(
        fuzz.get(mode) != limits for mode, limits in expected_fuzz_policies.items()
    ):
        fail("fuzz budget contract drift")
    fuzz_seeds(value)
    gates = value["gates"]
    if not isinstance(gates, list) or {item.get("id") for item in gates if isinstance(item, dict)} != set(GATES) or len(gates) != len(GATES):
        fail("profile gate declarations drift from the runner allowlist")
    for item in gates:
        dispatch = item.get("dispatch") if isinstance(item, dict) else None
        if not isinstance(dispatch, dict) or dispatch != {"command": "full-gates", "only": item["id"], "required_args": ["--rc", "--only"]}:
            fail("profile gate dispatch declaration is not executable")
    checkpoints = value["workflow_checkpoints"]
    checkpoint_ids = {"G009-GATE-PREFLIGHT", "G009-GATE-PERFORMANCE", "G009-GATE-PACKAGE", "G009-GATE-LIFECYCLE", "G009-GATE-FINAL-001"}
    if not isinstance(checkpoints, list) or {item.get("id") for item in checkpoints if isinstance(item, dict)} != checkpoint_ids:
        fail("workflow checkpoint replacements drift")
    for item in checkpoints:
        dispatch = item.get("dispatch") if isinstance(item, dict) else None
        if not isinstance(dispatch, dict) or not isinstance(dispatch.get("command"), str) or not dispatch["command"] or not isinstance(dispatch.get("required_args"), list) or not dispatch["required_args"]:
            fail("workflow checkpoint replacement is incomplete")
    return value
def _frozen_profile_for_target(target: str) -> dict[str, Any]:
    profile_name = QUALIFICATION_PROFILES.get(target)
    if profile_name is None:
        fail(f"unsupported qualification profile target: {target}")
    frozen = profile(ROOT / profile_name)
    if frozen["target"]["triple"] != target:
        fail("frozen qualification profile target differs from requested target")
    return frozen




def identity_manifest(require_clean: bool = True) -> dict[str, Any]:
    outputs: dict[tuple[str, ...], bytes] = {}
    for argv in IDENTITY_COMMANDS:
        result = run_allowed(argv)
        if result.returncode != 0: fail(f"identity command failed: {' '.join(argv)}")
        outputs[argv] = result.stdout
    if require_clean and outputs[("git", "status", "--porcelain")]: fail("source tree is dirty")
    def text(argv: tuple[str, ...]) -> str: return outputs[argv].decode("utf-8", "strict").strip()
    def tool(name: str, argv: tuple[str, ...]) -> dict[str, str]:
        rustup = shutil.which("rustup")
        if rustup is None or "1.85.0" not in text(argv):
            fail(f"{name} is not the pinned 1.85.0 tool")
        located = _completed_bounded(
            (rustup, "which", "--toolchain", "1.85.0", name),
            cwd=ROOT,
            env={"PATH": os.environ.get("PATH", "")},
        )
        if located.returncode != 0:
            fail(f"cannot locate pinned {name}")
        binary = Path(located.stdout.decode("utf-8", "strict").strip()).resolve()
        return {"id": name, "version": text(argv), "path": str(binary), "path_sha256": sha256_file(binary)}
    return {"commit": text(("git", "rev-parse", "HEAD")), "tree": text(("git", "rev-parse", "HEAD^{tree}")), "tools": [tool("cargo", ("cargo", "+1.85.0", "--version")), tool("rustc", ("rustc", "+1.85.0", "--version"))]}


def _public_source(source: dict[str, Any]) -> dict[str, Any]:
    tools = source.get("tools")
    if not isinstance(tools, list):
        fail("source tool manifest is missing")
    return {
        "commit": source["commit"],
        "tree": source["tree"],
        "tools": [
            {
                "id": tool["id"],
                "version": tool["version"],
                "path_sha256": tool["path_sha256"],
            }
            for tool in tools
        ],
    }


def controller_source_bindings() -> list[dict[str, str]]:
    return [
        {"id": source_id, "path_sha256": sha256_file(CONTROLLER_ROOT / source_id)}
        for source_id in CONTROLLER_SOURCE_IDS
    ]


def require_native_tool(path: Path, tool_id: str, target: str) -> None:
    expected = target_tuple(target)
    def has_required_slice(executable: Path) -> bool:
        observed = _completed_bounded(
            ("/usr/bin/lipo", "-archs", str(executable)),
            cwd=ROOT,
            env={"PATH": os.environ.get("PATH", "")},
        )
        architectures = observed.stdout.decode("ascii", "strict").strip().split() if observed.returncode == 0 else []
        return expected["mach_o_arch"] in architectures

    if has_required_slice(path):
        return
    launcher = bounded_bytes(path, 64 * 1024).decode("utf-8", "strict")
    if not launcher.startswith("#!"):
        fail(f"release tool lacks a native {expected['mach_o_arch']} executable slice: {tool_id}")
    interpreter_tokens = launcher.splitlines()[0][2:].strip().split()
    if not interpreter_tokens:
        fail(f"release tool has an invalid script interpreter: {tool_id}")
    interpreter = Path(interpreter_tokens[0])
    if interpreter == Path("/usr/bin/env"):
        if len(interpreter_tokens) != 2 or shutil.which(interpreter_tokens[1]) is None:
            fail(f"release tool script interpreter is unresolved: {tool_id}")
        interpreter = Path(shutil.which(interpreter_tokens[1]) or "")
    if not interpreter.is_absolute() or not has_required_slice(interpreter.resolve()):
        fail(f"release tool script interpreter is not {expected['mach_o_arch']}-native: {tool_id}")
    for target_path in re.findall(r'\bexec\s+"([^"]+)"', launcher):
        executable = Path(target_path)
        if not executable.is_absolute() or not executable.is_file() or not has_required_slice(executable.resolve()):
            fail(f"release tool script exec target is not {expected['mach_o_arch']}-native: {tool_id}")


def release_tool_manifest(source: dict[str, Any], target: str) -> dict[str, Any]:
    stable = {tool["id"]: tool for tool in source["tools"]}
    rustup_raw = shutil.which("rustup")
    if rustup_raw is None:
        fail("rustup is unavailable")
    rustup = Path(rustup_raw).resolve()
    llvm_cov = Path(shutil.which("cargo-llvm-cov") or "")
    candidates: list[tuple[str, Path, tuple[str, ...] | None]] = [
        ("rustc", Path(stable["rustc"]["path"]), None),
        ("cargo", Path(stable["cargo"]["path"]), None),
        ("rustup", rustup, (str(rustup), "--version")),
        ("cargo-audit", Path(shutil.which("cargo-audit") or ""), None),
        ("cargo-deny", Path(shutil.which("cargo-deny") or ""), None),
        ("cargo-llvm-cov", llvm_cov, (str(llvm_cov), "llvm-cov", "--version")),
        ("cargo-fuzz", Path(shutil.which("cargo-fuzz") or ""), None),
        ("python3", Path(sys.executable).resolve(), (sys.executable, "--version")),
        ("git", Path("/usr/bin/git"), ("/usr/bin/git", "--version")),
        ("gpgv", Path(shutil.which("gpgv") or ""), None),
        ("lipo", Path("/usr/bin/lipo"), None),
        ("sysctl", Path("/usr/sbin/sysctl"), None),
        ("ps", Path("/bin/ps"), None),
        ("launchctl", Path("/bin/launchctl"), None),
        ("bash", Path("/bin/bash"), ("/bin/bash", "--version")),
        ("sandbox-exec", Path("/usr/bin/sandbox-exec"), None),
    ]
    for tool_id, channel, name in (
        ("rustc-nightly", "nightly-2026-07-17", "rustc"),
        ("cargo-nightly", "nightly-2026-07-17", "cargo"),
    ):
        located = _completed_bounded(
            (str(rustup), "which", "--toolchain", channel, name),
            cwd=ROOT,
            env={"PATH": os.environ.get("PATH", "")},
        )
        if located.returncode != 0:
            fail(f"{tool_id} is unavailable")
        path = Path(located.stdout.decode("utf-8", "strict").strip()).resolve()
        candidates.append((tool_id, path, (str(rustup), "run", channel, name, "--version")))
    records: list[dict[str, str]] = []
    system_version = f"macos-{platform.mac_ver()[0]}"
    for tool_id, supplied, version_argv in candidates:
        path = supplied.resolve()
        if not str(supplied) or not path.is_file():
            fail(f"release tool is unsafe or missing: {tool_id}")
        if version_argv is None and tool_id in stable:
            version = stable[tool_id]["version"]
        elif version_argv is None and tool_id in {"lipo", "sysctl", "ps", "launchctl", "sandbox-exec"}:
            version = system_version
        else:
            argv = version_argv or (str(path), "--version")
            observed = _completed_bounded(
                argv,
                cwd=ROOT,
                env={"PATH": os.environ.get("PATH", "")},
            )
            if observed.returncode != 0:
                fail(f"cannot identify release tool: {tool_id}")
            version = (observed.stdout or observed.stderr).decode("utf-8", "strict").splitlines()[0].strip()
        if not version:
            fail(f"release tool version is empty: {tool_id}")
        require_native_tool(path, tool_id, target)
        records.append({
            "id": tool_id,
            "version": version,
            "path_sha256": sha256_file(path),
            "architecture": target_tuple(target)["mach_o_arch"],
        })
    records.sort(key=lambda item: item["id"])
    return {
        "schema": "podway.g009.release-tool-manifest/v1",
        "source": _public_source(source),
        "tools": records,
        "controller_sources": controller_source_bindings(),
    }


def evidence(category: str, value: dict[str, Any]) -> tuple[Path, str]:
    run_identity = os.environ.get("G009_QUALIFICATION_RUN_ID")
    if not isinstance(run_identity, str) or not re.fullmatch(r"[0-9a-f]{64}", run_identity):
        fail("G009_QUALIFICATION_RUN_ID must bind every qualification checkpoint")
    record = dict(value)
    record.setdefault("schema", "podway.g009.checkpoint/v1")
    record.setdefault("host", host_manifest())
    record.setdefault("run_identity", run_identity)
    if record["run_identity"] != run_identity:
        fail("checkpoint attempted to override qualification run identity")
    return content_addressed_json(category, record)


def _bound_path(role: str, path: Path) -> Path:
    root = rc_input_root(role)
    supplied = path if path.is_absolute() else root / path
    resolved = supplied.resolve()
    if supplied.is_symlink() or not resolved.is_relative_to(root) or not resolved.is_file():
        fail(f"bound input {role} is unsafe or outside its authoritative root")
    return resolved

def _bound(role: str, path: Path) -> dict[str, str]:
    resolved = _bound_path(role, path)
    return {"role": role, "path": str(resolved.relative_to(rc_input_root(role))), "sha256": sha256_file(resolved)}






def _native_scratch_source() -> tuple[tempfile.TemporaryDirectory[str], Path, set[Path], dict[str, Any]]:
    holder = tempfile.TemporaryDirectory(prefix="g009-source-", dir=qualification_scratch_root())
    source_root = Path(holder.name) / "source"
    try:
        source_files, manifest = _materialize_candidate_source(source_root)
    except BaseException:
        holder.cleanup()
        raise
    return holder, source_root, source_files, manifest


def _run(argv: tuple[str, ...], cwd: Path, env: dict[str, str], timeout: float = 15,
         read_only_paths: tuple[Path, ...] = ()) -> subprocess.CompletedProcess[bytes]:
    captured = bounded_process(
        sandboxed_candidate_argv(
            argv, read_only_paths=read_only_paths,
        ),
        cwd=cwd, env=env, timeout=timeout,
        stream_limit=FUZZ_STREAM_MAX_BYTES, aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
        allow_descendants=True,
    )
    result = _completed_from_capture(argv, captured)
    if captured["terminal_mode"] == "timeout": fail(f"native workload command timed out: {argv[0]}")
    if result.returncode != 0: fail(f"native workload command failed ({result.returncode}): {' '.join(argv[:3])}")
    return result


def _socket_paths(env: dict[str, str]) -> tuple[Path, ...]:
    return (Path(env["TMPDIR"]) / f"podway-{os.getuid()}" / "podwayd.sock",)


def _daemon_group_exists(process: subprocess.Popen[bytes]) -> bool:
    try:
        os.killpg(process.pid, 0)
    except ProcessLookupError:
        return False
    return True


def _wait_for_daemon_group(process: subprocess.Popen[bytes], deadline: float) -> bool:
    while _daemon_group_exists(process) and time.monotonic() < deadline:
        process.poll()
        time.sleep(0.01)
    if process.poll() is None:
        try:
            process.wait(timeout=max(0, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            return False
    return not _daemon_group_exists(process)


def _terminate_daemon(process: subprocess.Popen[bytes], timeout: float = 10) -> bool:
    if not _daemon_group_exists(process):
        if process.poll() is None:
            process.wait(timeout=2)
        return True
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return process.poll() is not None
    if _wait_for_daemon_group(process, time.monotonic() + timeout):
        return True
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    if not _wait_for_daemon_group(process, time.monotonic() + 2):
        fail("podwayd process group survived SIGKILL")
    return False
def _drain_and_close_daemon_pipes(process: subprocess.Popen[bytes]) -> None:
    if process.stdin is not None:
        try:
            process.stdin.close()
        except OSError:
            pass
    for pipe in (process.stdout, process.stderr):
        if pipe is None:
            continue
        try:
            pipe.read()
        except (OSError, ValueError):
            pass
        finally:
            try:
                pipe.close()
            except OSError:
                pass


def _cleanup_failed_daemon_start(process: subprocess.Popen[bytes]) -> None:
    try:
        _terminate_daemon(process, timeout=5)
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            fail("podwayd did not exit after failed startup cleanup")
    finally:
        _drain_and_close_daemon_pipes(process)




def _start_daemon(podwayd: Path, cwd: Path, env: dict[str, str]) -> tuple[subprocess.Popen[bytes], Path]:
    candidates = _socket_paths(env)
    if any(candidate.exists() for candidate in candidates):
        fail("refusing a pre-existing Podway socket before workload startup")
    process = subprocess.Popen(
        sandboxed_candidate_argv((str(podwayd), "--service"), allow_process_fork=False),
        cwd=cwd,
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, env=env, start_new_session=True)
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if process.poll() is not None:
                fail("podwayd exited before socket readiness")
            ready = [candidate for candidate in candidates if candidate.exists()]
            if len(ready) == 1:
                return process, ready[0]
            if len(ready) > 1:
                fail("podwayd created multiple socket candidates")
            time.sleep(0.01)
        fail("podwayd did not create its socket")
    except BaseException:
        _cleanup_failed_daemon_start(process)
        raise


def _stop_daemon(process: subprocess.Popen[bytes], socket: Path) -> None:
    graceful = _terminate_daemon(process)
    if not graceful:
        fail("podwayd ignored SIGTERM")
    if socket.exists():
        fail("podwayd left socket after termination")

def _prepare_task(podway: Path, workspace: Path, env: dict[str, str]) -> Path:
    procedure = workspace / ".g009-procedure.yaml"
    procedure.write_text(
        """schema: podway.procedure/v1
id: g009-benchmark
version: "1"
name: G009 benchmark
description: Deterministic release qualification fixture.
stages:
  - id: benchmark
    title: Benchmark
    items:
      - id: target-audience
        type: text
        prompt: Target audience.
        required: true
        min_length: 1
      - id: draft-reference
        type: artifact
        prompt: Draft reference.
        required: false
rework:
  allow_return_to: any_previous
""",
        encoding="utf-8",
    )
    _run((str(podway), "init"), workspace, env)
    _run(
        (str(podway), "start", "--procedure", procedure.name, "--task", "G009"),
        workspace,
        env,
    )
    return procedure

def _adapter_commands(workload_id: str, podway: Path, workspace: Path, w07: dict[str, Any] | None = None,
                      podwayd: Path | None = None) -> tuple[tuple[str, ...], ...]:
    artifact = workspace / ".g009-artifact.bin"; artifact.write_bytes(b"g009-artifact-v1\n" * 4096)
    if workload_id == "G009-W01":
        if podwayd is None: fail("W01 daemon adapter is absent")
        return ((str(podwayd), "--service"),)
    if workload_id == "G009-W02": return ((str(podway), "status"), (str(podway), "next"))
    if workload_id == "G009-W03": return ((str(podway), "start", "--procedure", ".g009-procedure.yaml", "--task", "G009-linked"),)
    if workload_id == "G009-W04": return ((str(podway), "set", "target-audience", "updated"),)
    if workload_id == "G009-W05": return ((str(podway), "attach", "draft-reference", artifact.name),)
    if workload_id == "G009-W06": return ((str(podway), "status"),)
    if workload_id == "G009-W07":
        if w07 is None: fail("W07 generator is absent")
        return ((str(podway), "set", "target-audience", w07["bytes"].decode("utf-8", "strict")),)
    fail(f"unknown workload adapter: {workload_id}")

def _rss_kib(pid: int) -> int:
    sample = _completed_bounded(
        ("/bin/ps", "-o", "rss=", "-p", str(pid)),
        env={"PATH": os.environ.get("PATH", "")},
        timeout=5,
    )
    value = sample.stdout.strip()
    if sample.returncode != 0 or not value.isdigit():
        fail(f"cannot sample RSS for live process {pid}")
    return int(value)

def _measure(argvs: tuple[tuple[str, ...], ...], cwd: Path, env: dict[str, str], bound: dict[str, int],
             allow_rejection: bool = False, daemon: subprocess.Popen[bytes] | None = None) -> dict[str, Any]:
    started = time.monotonic_ns(); stdout = bytearray(); stderr = bytearray(); exit_code = 0
    cli_peak_kib = 0; daemon_peak_kib = 0
    for argv in argvs:
        if daemon is not None and daemon.poll() is not None: fail("podwayd exited unexpectedly during workload")
        def observe(pid: int) -> None:
            nonlocal cli_peak_kib, daemon_peak_kib
            cli_peak_kib = max(cli_peak_kib, _rss_kib(pid))
            if daemon is not None:
                if daemon.poll() is not None: fail("podwayd exited unexpectedly during workload")
                daemon_peak_kib = max(daemon_peak_kib, _rss_kib(daemon.pid))
        captured = bounded_process(
            sandboxed_candidate_argv(argv), cwd=cwd, env=env,
            timeout=bound["max_completion_ms"] / 1000,
            stream_limit=FUZZ_STREAM_MAX_BYTES, aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
            observer=observe,
            allow_descendants=True,
        )
        completed = _completed_from_capture(argv, captured)
        if captured["terminal_mode"] == "timeout":
            fail("workload command timed out")
        if completed.returncode != 0 and not allow_rejection:
            fail(f"native workload command failed ({completed.returncode}): {' '.join(argv[1:3])}")
        if allow_rejection and completed.returncode != 0 and (completed.returncode != 2 or b"validation" not in completed.stderr.lower()):
            fail("maximum-input command did not return the exact validation rejection")
        exit_code = completed.returncode
        stdout.extend(completed.stdout); stderr.extend(completed.stderr)
    elapsed = time.monotonic_ns() - started
    peak_kib = max(cli_peak_kib, daemon_peak_kib)
    if peak_kib <= 0:
        fail("workload completed without a valid RSS sample")
    if elapsed > bound["max_completion_ms"] * 1_000_000 or peak_kib > bound["max_rss_mib"] * 1024:
        fail("workload exceeded frozen resource bound")
    return {"elapsed_ns": elapsed, "rss_kib": peak_kib,
            "process_rss_kib": {"cli_peak": cli_peak_kib, "daemon_peak": daemon_peak_kib},
            "exit_code": exit_code, "stdout_sha256": sha256_bytes(bytes(stdout)),
            "stderr_sha256": sha256_bytes(bytes(stderr)), "value": {"numerator": elapsed, "denominator": 1}}
def _collect(profile_data: dict[str, Any], bin_dir: Path, phase: str) -> dict[str, Any]:
    podway, podwayd = (bin_dir / "podway").resolve(), (bin_dir / "podwayd").resolve()
    if not all(path.is_file() and os.access(path, os.X_OK) for path in (podway, podwayd)): fail("prebuilt podway binaries are missing or not executable")
    fixture_digest = sha256_bytes(canonical_json({"schema": "podway.g009.fixture-manifest/v1", "fixture": "g009-safe-synthetic-fixture-v1", "adapters": sorted(WORKLOAD_ADAPTER_IDS)}))
    w07 = _w07_generator(next(item for item in profile_data["workloads"] if item["id"] == "G009-W07"))
    assert_descendant_write_protection()
    def one(item: dict[str, Any]) -> dict[str, Any]:
        holder, workspace, source_files, source_manifest = _native_scratch_source()
        daemon: subprocess.Popen[bytes] | None = None
        socket: Path | None = None
        linked_files: set[Path] | None = None
        linked_manifest: dict[str, Any] | None = None
        try:
            fixture = Path(holder.name) / "fixture"; fixture.mkdir(mode=0o700)
            home, temporary = fixture / "home", fixture / "tmp"
            home.mkdir(mode=0o700)
            temporary.mkdir(mode=0o700)
            for relative in (
                Path("Library/Application Support/Podway"),
                Path("Library/LaunchAgents"),
                Path("Library/Logs/Podway"),
            ):
                directory = home / relative
                directory.mkdir(parents=True, mode=0o700)
                os.chmod(directory, 0o700)
            env = {"HOME": str(home), "TMPDIR": str(temporary), "PATH": os.environ.get("PATH", "")}
            if item["id"] == "G009-W01":
                started = time.monotonic_ns()
                daemon, socket = _start_daemon(podwayd, workspace, env)
                elapsed = time.monotonic_ns() - started
                rss_kib = _rss_kib(daemon.pid)
                if elapsed > item["hard_bounds"]["max_completion_ms"] * 1_000_000 or rss_kib > item["hard_bounds"]["max_rss_mib"] * 1024:
                    fail("cold start exceeded frozen resource bound")
                return {"elapsed_ns": elapsed, "rss_kib": rss_kib,
                        "process_rss_kib": {"cli_peak": 0, "daemon_peak": rss_kib},
                        "exit_code": 0, "stdout_sha256": sha256_bytes(b""), "stderr_sha256": sha256_bytes(b""),
                        "value": {"numerator": elapsed, "denominator": 1}}
            daemon, socket = _start_daemon(podwayd, workspace, env)
            procedure = _prepare_task(podway, workspace, env)
            if item["id"] == "G009-W06":
                for index in range(32):
                    _run((str(podway), "set", "target-audience", f"growth-{index}"), workspace, env)
            linked = Path(holder.name) / "linked"
            measured_workspace = workspace
            if item["id"] == "G009-W03":
                linked_files, linked_manifest = _materialize_candidate_source(linked)
                _run((str(podway), "init"), linked, env)
                linked_procedure = linked / procedure.name
                linked_procedure.write_bytes(procedure.read_bytes())
                measured_workspace = linked
            return _measure(
                _adapter_commands(item["id"], podway, measured_workspace, w07, podwayd),
                measured_workspace,
                env,
                item["hard_bounds"],
                item["id"] == "G009-W07",
                daemon,
            )
        finally:
            try:
                if daemon is not None and socket is not None: _stop_daemon(daemon, socket)
            finally:
                if linked_files is not None and linked_manifest is not None:
                    _verify_materialized_candidate_source(linked, linked_files, linked_manifest)
                _verify_materialized_candidate_source(workspace, source_files, source_manifest)
                holder.cleanup()
    workloads: dict[str, Any] = {}
    for item in profile_data["workloads"]:
        warm = [one(item) for _ in range(WARMUPS)]
        measured = [one(item) for _ in range(SAMPLES)]
        workloads[item["id"]] = {
            "kind": "latency",
            "warmups": [entry["value"] for entry in warm],
            "samples": [entry["value"] for entry in measured],
            "resource": {
                "hard_bounds": item["hard_bounds"],
                "warmups": warm,
                "samples": measured,
            },
            "fixture_manifest_sha256": sha256_bytes(
                canonical_json(
                    {
                        "schema": "podway.g009.fixture-manifest/v1",
                        "adapter": item["id"],
                        "commands": item["measured_commands"],
                    }
                )
            ),
            "workload_name": item["name"],
            "adapter_id": item["adapter_id"],
            "measured_commands": item["measured_commands"],
            **(
                {"input_generator": {key: value for key, value in w07.items() if key != "bytes"}}
                if item["id"] == "G009-W07"
                else {}
            ),
        }
    return {"schema": "podway.g009.characterization/v1", "phase": phase,
            "target": profile_data["target"]["triple"],
            "warmups": WARMUPS, "samples": SAMPLES, "fixture_sha256": fixture_digest, "workloads": workloads}


def _rc_evidence(rc_path: Path, checkpoint_id: str, **value: Any) -> tuple[Path, str]:
    rc = verify_rc_consumption(rc_path)
    record = {"checkpoint_id": checkpoint_id, "status": "pass", "rc_sha256": sha256_file(rc_path),
              "source": rc["source"], "target": rc["target"], "blockers": [], **value}
    return evidence(checkpoint_id.lower(), record)
def _rc_profile(rc: dict[str, Any]) -> dict[str, Any]:
    profile_data = profile(resolve_rc_input(rc, "profile"))
    if profile_target_tuple(profile_data["target"]) != target_tuple(rc["target"]):
        fail("RC profile target tuple differs from RC target")
    if rc["target_tuple"] != target_tuple(rc["target"]):
        fail("RC target tuple differs from RC target")
    if rc["archive_root"] != archive_root(rc["target"]):
        fail("RC archive root differs from profile target tuple")
    return profile_data



def preflight(args: argparse.Namespace) -> None:
    rc_path = Path(args.rc)
    _validate_candidate_build_surface()
    source_before = _candidate_source_manifest()
    rc = verify_rc_consumption(rc_path)
    profile_path = resolve_rc_input(rc, "profile")
    _rc_profile(rc)
    require_native_host(rc["target"])
    if _candidate_source_manifest() != source_before:
        fail("candidate source changed during preflight")
    out, digest = _rc_evidence(rc_path, "G009-GATE-PREFLIGHT",
                                profile_sha256=sha256_file(profile_path), source_manifest=identity_manifest())
    print(f"{out} {digest}")


def _require_characterization(value: Any, target: str) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("schema") != "podway.g009.characterization/v1" or value.get("target") != target or value.get("warmups") != WARMUPS or value.get("samples") != SAMPLES:
        fail("invalid characterization")
    calculated = calculate_baseline(value.get("workloads"))
    if value.get("baseline") != calculated:
        fail("characterization baseline is not mechanically exact")
    return value


def characterize(args: argparse.Namespace) -> None:
    p = profile(Path(args.profile))
    if p["target"]["triple"] != args.target:
        fail("characterization target differs from profile target tuple")
    require_native_host(args.target)
    if args.warmups != WARMUPS or args.samples != SAMPLES: fail("G009 requires exactly 5 warmups and 30 samples")
    measured = _collect(p, Path(args.bin_dir).resolve(), "characterization")
    measured["profile_sha256"] = sha256_file(Path(args.profile)); measured["source"] = identity_manifest()
    measured["baseline"] = calculate_baseline(measured["workloads"])
    out, digest = evidence("performance/characterization", measured)
    print(f"{out} {digest}")


def _verified_approvals(approvals_path: Path, signer_contract_path: Path, profile_path: Path, characterization: Path, baseline: dict[str, Any], threshold: dict[str, Any]) -> list[dict[str, Any]]:
    contract, bundle = load_json(signer_contract_path), load_json(approvals_path)
    if not isinstance(contract, dict) or contract.get("schema") != "podway.g009.approval-signers/v1" or set(contract) != {"schema", "keyring", "keyring_sha256", "signers"}:
        fail("approval signer contract is not exact")
    keyring = Path(contract["keyring"])
    keyring_sha256 = contract["keyring_sha256"]
    signers = contract["signers"]
    if (
        not keyring.is_file() or keyring.is_symlink()
        or not isinstance(keyring_sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", keyring_sha256)
        or sha256_file(keyring) != keyring_sha256
        or not isinstance(signers, list) or len(signers) != 3
    ):
        fail("approval trust root is unavailable or differs from its contract digest")
    by_role = {item.get("role"): item for item in signers if isinstance(item, dict)}
    if set(by_role) != {"owner", "E", "F"} or any(set(item) != {"role", "signer", "fingerprint"} or not all(isinstance(item[key], str) and item[key] for key in ("signer", "fingerprint")) for item in by_role.values()):
        fail("approval signer roles are incomplete")
    if not isinstance(bundle, dict) or set(bundle) != {"schema", "profile_sha256", "characterization_sha256", "approval_keyring_sha256", "approvals"} or bundle.get("schema") != "podway.g009.approvals/v1" or bundle.get("profile_sha256") != sha256_file(profile_path) or bundle.get("characterization_sha256") != sha256_file(characterization) or bundle.get("approval_keyring_sha256") != keyring_sha256 or not isinstance(bundle.get("approvals"), list) or len(bundle["approvals"]) != 3:
        fail("approval bundle is stale, unbound to its keyring, or incomplete")
    baseline_digest, threshold_digest = sha256_bytes(canonical_json(baseline)), sha256_bytes(canonical_json(threshold))
    profile_digest = sha256_file(profile_path)
    seen_roles: set[str] = set(); seen_signers: set[str] = set()
    for approval in bundle["approvals"]:
        if not isinstance(approval, dict) or set(approval) != {"role", "signer", "fingerprint", "profile_sha256", "characterization_sha256", "baseline_sha256", "thresholds_sha256", "payload", "signature"}:
            fail("approval has mutable or missing fields")
        role = approval["role"]; expected = by_role.get(role)
        if expected is None or approval["signer"] != expected["signer"] or approval["fingerprint"] != expected["fingerprint"] or approval["profile_sha256"] != profile_digest or approval["characterization_sha256"] != sha256_file(characterization) or approval["baseline_sha256"] != baseline_digest or approval["thresholds_sha256"] != threshold_digest:
            fail("approval binding or signer contract mismatch")
        if role in seen_roles or approval["signer"] in seen_signers: fail("approval roles/signers must be distinct")
        payload, signature = Path(approval["payload"]), Path(approval["signature"])
        if not payload.is_file() or not signature.is_file() or payload.is_symlink() or signature.is_symlink():
            fail("approval detached signature inputs are unsafe")
        expected_payload = canonical_json({"role": role, "signer": approval["signer"], "fingerprint": approval["fingerprint"], "approval_keyring_sha256": keyring_sha256, "profile_sha256": profile_digest, "characterization_sha256": approval["characterization_sha256"], "baseline_sha256": baseline_digest, "thresholds_sha256": threshold_digest})
        if bounded_bytes(payload) != expected_payload:
            fail("approval payload is not the exact bound statement")
        check = _completed_bounded(
            ("gpgv", "--keyring", str(keyring), "--status-fd", "1", str(signature), str(payload)),
            env={"PATH": os.environ.get("PATH", "")},
        )
        valid = [
            line.split() for line in check.stdout.decode("utf-8", "strict").splitlines()
            if line.startswith("[GNUPG:] VALIDSIG ")
        ]
        if check.returncode != 0 or len(valid) != 1 or len(valid[0]) < 12 or valid[0][-1] != expected["fingerprint"]:
            fail("detached approval signature is not an exact primary VALIDSIG")
        seen_roles.add(role); seen_signers.add(approval["signer"])
    if seen_roles != {"owner", "E", "F"}: fail("missing explicit owner/E/F approvals")
    return bundle["approvals"]

def approve_baseline(args: argparse.Namespace) -> None:
    data = _require_characterization(load_json(Path(args.characterization)), profile(Path(args.profile))["target"]["triple"])
    if args.roles != "owner,E,F" or not args.approval or not args.signer_contract: fail("exact owner,E,F approvals and signer contract are required")
    approvals_path = Path(args.approval[0])
    if len(args.approval) != 1: fail("approvals must be supplied as one immutable bundle")
    baseline, threshold = data["baseline"], thresholds(data["baseline"])
    signer_contract = Path(args.signer_contract)
    approvals = _verified_approvals(approvals_path, signer_contract, Path(args.profile), Path(args.characterization), baseline, threshold)
    keyring_sha256 = load_json(signer_contract)["keyring_sha256"]
    for category, value in (("performance/baseline", baseline), ("performance/thresholds", threshold), ("performance/approvals", {"schema": "podway.g009.approvals/v1", "profile_sha256": sha256_file(Path(args.profile)), "characterization_sha256": sha256_file(Path(args.characterization)), "approval_keyring_sha256": keyring_sha256, "approvals": approvals})):
        out, digest = evidence(category, value); print(f"{out} {digest}")

def freeze_rc(args: argparse.Namespace) -> None:
    profile_path = _bound_path("profile", Path(args.profile))
    characterization_path = _bound_path("characterization", Path(args.characterization))
    baseline_path = _bound_path("baseline", Path(args.baseline))
    thresholds_path = _bound_path("thresholds", Path(args.thresholds))
    approval_path = _bound_path("approvals", Path(args.approvals))
    signer_contract = _bound_path("signer-contract", Path(args.signer_contract))
    p = profile(profile_path)
    target = p["target"]["triple"]
    require_native_host(target)
    source = identity_manifest()
    characterization = _require_characterization(load_json(characterization_path), target)
    baseline, approved = load_json(baseline_path), load_json(thresholds_path)
    if thresholds(baseline) != approved: fail("thresholds are not mechanically derived")
    if characterization.get("profile_sha256") != sha256_file(profile_path):
        fail("characterization profile binding differs from RC target profile")
    _verified_approvals(approval_path, signer_contract, profile_path, characterization_path, baseline, approved)
    posture = args.signing_posture
    if posture not in p["signing_postures"]: fail("unapproved signing posture")
    signing = {"posture": "unsigned-internal", "codesign": "not_attempted_missing_credentials", "notarization": "not_attempted_missing_credentials", "stapling": "not_applicable_zip", "gatekeeper": "not_claimed"}
    inputs = [_bound("profile", profile_path), _bound("characterization", characterization_path), _bound("baseline", baseline_path), _bound("thresholds", thresholds_path), _bound("approvals", approval_path), _bound("signer-contract", signer_contract), _bound("lockfile", ROOT / "Cargo.lock")]
    for raw in args.input:
        try: role, supplied = raw.split("=", 1)
        except ValueError: fail("--input must be ROLE=PATH")
        if not role or any(entry["role"] == role for entry in inputs): fail("duplicate RC input role")
        inputs.append(_bound(role, Path(supplied)))
    required = {"profile", "characterization", "baseline", "thresholds", "approvals", "signer-contract", "lockfile", "interfaces", "handoffs", "crash-registry", "fuzz-policy", "observability-policy", "podway", "podwayd"}
    if {entry["role"] for entry in inputs} != required: fail("RC does not bind all invalidation inputs and binaries")
    binaries = {name: {"sha256": next(item["sha256"] for item in inputs if item["role"] == name), "provenance": {"source": source, "target": target, "rust": "1.85.0"}} for name in ("podway", "podwayd")}
    intent = {"schema": "podway.g009.rc-intent/v1", "target": target, "target_tuple": target_tuple(target), "minimum_macos": p["minimum_macos"], "rust": "1.85.0", "source": source, "host": host_manifest(), "inputs": inputs, "signing": signing, "archive_root": archive_root(target), "binaries": binaries}
    out, digest = evidence("rc", intent); print(f"{out} {digest}")


def holdout(args: argparse.Namespace) -> None:
    rc = verify_rc_consumption(Path(args.rc))
    require_native_host(rc["target"])
    if args.warmups != WARMUPS or args.samples != SAMPLES: fail("holdout requires exactly 5 warmups and 30 samples")
    profile_path = resolve_rc_input(rc, "profile"); baseline = load_json(resolve_rc_input(rc, "baseline")); approved = load_json(resolve_rc_input(rc, "thresholds"))
    p = _rc_profile(rc)
    measured = _collect(p, Path(args.bin_dir).resolve(), "holdout")
    decision = evaluate_holdout(measured["workloads"], baseline, approved)
    measured["decision"] = decision
    if not decision["passed"]: fail("unseen holdout does not meet frozen thresholds")
    out, digest = _rc_evidence(Path(args.rc), "G009-GATE-PERFORMANCE", holdout=measured)
    print(f"{out} {digest}")


FUZZ_STREAM_MAX_BYTES = 1024 * 1024
FUZZ_AGGREGATE_MAX_BYTES = 2 * FUZZ_STREAM_MAX_BYTES
FUZZ_CORPUS_MEMBER_COUNT = 4096
FUZZ_CORPUS_MEMBER_MAX_BYTES = 1024 * 1024
FUZZ_CORPUS_AGGREGATE_MAX_BYTES = 8 * 1024 * 1024
FUZZ_CORPUS_PATH_MAX_DEPTH = 8
FUZZ_CORPUS_PATH_MAX_LENGTH = 256
FUZZ_MANIFEST_MAX_BYTES = 512 * 1024
FUZZ_DEPENDENCY_MATERIALIZATION_MAX_BYTES = 16 * 1024 * 1024
FUZZ_ARCHIVE_MATERIALIZATION_MAX_BYTES = 64 * 1024 * 1024
FUZZ_BUILD_ALLOWANCE_SECONDS = 300

def _fuzz_limits(profile_data: dict[str, Any], policy: dict[str, Any]) -> dict[str, int]:
    runner = profile_data["fuzz"]["runner"]
    return {
        "stream_bytes": runner["stream_bytes"], "aggregate_bytes": runner["aggregate_bytes"],
        "max_total_time": policy["seconds_per_target"], "timeout_seconds": policy["timeout_seconds"],
        "rss_limit_mb": policy["rss_limit_mb"], "corpus_member_count": runner["corpus_member_count"],
        "corpus_member_bytes": runner["corpus_member_bytes"],
        "corpus_aggregate_bytes": runner["corpus_aggregate_bytes"],
        "corpus_path_depth": runner["corpus_path_depth"],
        "corpus_path_length": runner["corpus_path_length"], "manifest_bytes": runner["manifest_bytes"],
        "fuzz_dependency_materialization_bytes": runner["fuzz_dependency_materialization_bytes"],
        "archive_materialization_bytes": runner["archive_materialization_bytes"],
    }

def _write_fuzz_blob(data: bytes) -> dict[str, Any]:
    digest = sha256_bytes(data)
    root = EVIDENCE_ROOT.resolve()
    path = EVIDENCE_ROOT / "fuzz" / "blobs" / f"{digest}.bin"
    if EVIDENCE_ROOT.is_symlink():
        fail("fuzz evidence root is unsafe")
    for directory in (EVIDENCE_ROOT, EVIDENCE_ROOT / "fuzz", path.parent):
        if directory.exists():
            if directory.is_symlink() or not directory.is_dir():
                fail("fuzz blob path is unsafe")
        else:
            directory.mkdir(mode=0o755)
    if path.parent.resolve() != root / "fuzz" / "blobs":
        fail("fuzz blob path escapes evidence root")
    if path.exists():
        if path.is_symlink() or not path.is_file() or bounded_bytes(path, FUZZ_STREAM_MAX_BYTES) != data:
            fail("immutable fuzz blob already differs")
    else:
        fd, temporary = tempfile.mkstemp(prefix=".g009-fuzz-", dir=path.parent)
        try:
            with os.fdopen(fd, "wb") as handle:
                handle.write(data)
                handle.flush()
                os.fsync(handle.fileno())
            os.chmod(temporary, 0o444)
            try:
                os.link(temporary, path)
            except FileExistsError:
                if path.is_symlink() or not path.is_file() or bounded_bytes(path, FUZZ_STREAM_MAX_BYTES) != data:
                    fail("immutable fuzz blob race differs")
            finally:
                os.unlink(temporary)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    if not path.is_file() or path.is_symlink() or sha256_file(path) != digest:
        fail("fuzz blob write or binding is absent")
    return {"path": str(path.relative_to(EVIDENCE_ROOT)), "bytes": len(data), "sha256": digest}

def _fuzz_executable_sha256(executable: Path) -> str:
    if (
        executable.is_symlink()
        or not executable.is_file()
        or executable.stat().st_nlink != 1
    ):
        fail("built fuzz target executable is absent or unsafe")
    return sha256_file(executable)
def _require_fuzz_executable_unchanged(executable: Path, pre_run_sha256: str) -> None:
    if _fuzz_executable_sha256(executable) != pre_run_sha256:
        fail("fuzz target executable changed during untrusted execution")



def _stream_fuzz_command(
    argv: tuple[str, ...], *, cwd: Path, env: dict[str, str], timeout: float,
    cargo: Path, cargo_fuzz: Path, native_target: str,
) -> dict[str, Any]:
    if not cargo.is_file() or cargo.is_symlink() or not cargo_fuzz.is_file() or cargo_fuzz.is_symlink():
        fail("recorded Cargo or cargo-fuzz executable is unsafe")
    target = argv[3]
    with tempfile.TemporaryDirectory(prefix="g009-fuzz-isolated-", dir=qualification_scratch_root()) as temporary:
        isolated = Path(temporary).resolve()
        source_root = (isolated / "source").resolve()
        source_files, source_manifest = _materialize_candidate_source(source_root)
        snapshot_cwd = source_root / cwd.relative_to(require_candidate_root())
        if not snapshot_cwd.is_dir():
            fail("protected fuzz source materialization is incomplete")
        (source_root / ".cargo").mkdir(mode=0o755)
        (source_root / ".cargo" / "config.toml").write_text("[net]\noffline = true\n", encoding="utf-8")
        cargo_home, cache_roots = _isolated_cargo_home(isolated)
        environment = dict(env)
        environment.update({
            "CARGO_HOME": str(cargo_home),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TARGET_DIR": str(isolated / "target"),
            "HOME": str(isolated / "home"),
            "TMPDIR": str(isolated / "scratch"),
        })
        environment["PATH"] = f"{cargo.parent}{os.pathsep}{environment.get('PATH', '')}"
        environment["CARGO"] = str(cargo)
        Path(environment["HOME"]).mkdir(mode=0o755)
        Path(environment["TMPDIR"]).mkdir(mode=0o755)
        target_tree = Path(environment["CARGO_TARGET_DIR"])
        common_sandbox = {
            "read_only_paths": (source_root, *cache_roots),
            "read_denied_paths": (*_cargo_config_read_denials(source_root), *_excluded_candidate_build_inputs()),
        }
        locked_resolution = bounded_process(
            sandboxed_candidate_argv(
                (str(cargo), "metadata", "--manifest-path", "Cargo.toml", "--locked", "--offline", "--format-version", "1"),
                **common_sandbox,
            ),
            cwd=snapshot_cwd, env=environment, timeout=FUZZ_BUILD_ALLOWANCE_SECONDS,
            stream_limit=FUZZ_STREAM_MAX_BYTES, aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
            allow_descendants=True,
        )
        if locked_resolution["terminal_mode"] != "success":
            fail("fuzz dependency resolution did not consume the active lockfile")
        _validate_metadata_build_inputs(locked_resolution["stdout"], source_root, source_files)
        build = bounded_process(
            sandboxed_candidate_argv(
                (str(cargo_fuzz), "fuzz", "build", target),
                **common_sandbox,
            ),
            cwd=snapshot_cwd, env=environment, timeout=FUZZ_BUILD_ALLOWANCE_SECONDS,
            stream_limit=FUZZ_STREAM_MAX_BYTES, aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
            allow_descendants=True,
        )
        if build["terminal_mode"] != "success":
            fail(f"fuzz target build failed: {target}")
        executable = target_tree / native_target / "release" / target
        pre_run_binary_sha256 = _fuzz_executable_sha256(executable)
        execution_argv = (str(executable), argv[4], *argv[6:])
        captured = bounded_process(
            sandboxed_fuzz_execution_argv(
                execution_argv, corpus=Path(argv[4]), scratch=Path(environment["TMPDIR"]),
            ),
            cwd=Path(argv[4]), env=environment, timeout=timeout - FUZZ_BUILD_ALLOWANCE_SECONDS,
            stream_limit=FUZZ_STREAM_MAX_BYTES, aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
            allow_descendants=False,
        )
        _require_fuzz_executable_unchanged(executable, pre_run_binary_sha256)
        captured["execution_binary_sha256"] = pre_run_binary_sha256
        captured["execution_argv"] = [
            f"sha256:{pre_run_binary_sha256}",
            f"controller/{_fuzz_corpus_identity(Path(argv[4]))}",
            *argv[6:],
        ]
        _verify_materialized_candidate_source(source_root, source_files, source_manifest)
        return captured

def _fuzz_corpus_identity(corpus: Path) -> str:
    root = qualification_scratch_root() / "fuzz" / "corpus"
    try:
        relative = corpus.resolve().relative_to(root.resolve())
    except ValueError as exc:
        raise QualificationError("fuzz corpus escapes qualification scratch root") from exc
    if len(relative.parts) != 1:
        fail("fuzz corpus identity is malformed")
    return f"artifacts/g009/fuzz/corpus/{relative.as_posix()}"


def _corpus_manifest(
    target: str, corpus: Path, phase: str, run_identity: str, limits: dict[str, int],
) -> dict[str, Any]:
    if phase not in {"before", "after"} or corpus.is_symlink() or not corpus.is_dir():
        fail("fuzz corpus manifest input is unsafe")
    files: list[dict[str, Any]] = []
    aggregate = 0
    members = bounded_regular_tree(
        corpus,
        member_limit=limits["corpus_member_count"],
        path_depth=limits["corpus_path_depth"],
        path_length=limits["corpus_path_length"],
        label="fuzz corpus",
    )
    for relative, member, size in members:
        if size < 0 or size > limits["corpus_member_bytes"]:
            fail("fuzz corpus member violates frozen bounds")
        aggregate += size
        if aggregate > limits["corpus_aggregate_bytes"]:
            fail("fuzz corpus aggregate exceeds frozen bound")
        files.append({"path": relative, "sha256": sha256_file(member), "bytes": size})
    record = {
        "schema": "podway.g009.fuzz-corpus-manifest/v1", "target": target, "phase": phase,
        "corpus": _fuzz_corpus_identity(corpus), "files": files, "run_identity": run_identity,
    }
    encoded = canonical_json({**record, "host": host_manifest()})
    if len(encoded) > limits["manifest_bytes"]:
        fail("fuzz corpus manifest exceeds frozen bound")
    path, digest = evidence("fuzz-corpus-manifests", record)
    return {"path": str(path.relative_to(EVIDENCE_ROOT)), "sha256": digest, "bytes": len(encoded)}
def _materialize_fuzz_seed(corpus: Path, seed: dict[str, Any], index: int) -> dict[str, Any]:
    filename = f"{index:02d}-{seed['name']}.seed"
    path = corpus / filename
    if path.exists() or path.is_symlink():
        fail("fresh fuzz corpus seed path is occupied")
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o444)
    except FileExistsError:
        fail("fresh fuzz corpus seed path raced")
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(seed["bytes"])
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise
    if path.is_symlink() or not path.is_file() or bounded_bytes(path, FUZZ_STREAM_MAX_BYTES) != seed["bytes"] or sha256_file(path) != seed["sha256"]:
        fail("materialized fuzz seed differs from frozen declaration")
    return {"name": seed["name"], "target": seed["target"], "path": filename, "sha256": seed["sha256"], "bytes": len(seed["bytes"])}


def _fuzz_provenance(profile_data: dict[str, Any], fuzz_env: dict[str, str]) -> dict[str, Any]:
    toolchain = profile_data["fuzz"]["toolchain"]
    rustup = shutil.which("rustup")
    if rustup is None:
        fail("rustup is required for fuzz provenance")
    tools: list[dict[str, str]] = []
    for name in ("rustc", "cargo"):
        located = bounded_process(
            sandboxed_candidate_argv((rustup, "which", "--toolchain", toolchain["channel"], name)),
            cwd=ROOT, env={"PATH": os.environ.get("PATH", "")}, timeout=30,
            stream_limit=FUZZ_STREAM_MAX_BYTES, aggregate_limit=FUZZ_AGGREGATE_MAX_BYTES,
            allow_descendants=True,
        )
        if located["terminal_mode"] != "success":
            fail(f"cannot locate fuzz {name}")
        path = Path(located["stdout"].decode("utf-8", "strict").strip()).resolve()
        if not path.is_file():
            fail(f"fuzz {name} path is missing")
        tools.append({"id": name, "path": str(path), "sha256": sha256_file(path)})
    cargo_fuzz = shutil.which("cargo-fuzz", path=fuzz_env["PATH"])
    if cargo_fuzz is None:
        fail("cargo-fuzz is required for fuzz provenance")
    cargo_fuzz_path = Path(cargo_fuzz).resolve()
    if not cargo_fuzz_path.is_file():
        fail("cargo-fuzz path is missing")
    tools.append({"id": "cargo-fuzz", "path": str(cargo_fuzz_path), "sha256": sha256_file(cargo_fuzz_path)})
    candidate_sources = [
        ROOT / "Cargo.lock",
        ROOT / "fuzz" / "Cargo.lock",
        ROOT / "fuzz" / "Cargo.toml",
    ]
    candidate_sources.extend(
        ROOT / "fuzz" / "fuzz_targets" / f"{target}.rs" for target in FUZZ_TARGETS
    )
    controller_sources = [
        Path(__file__).resolve(),
        CONTROLLER_ROOT / "tools" / "g009_common.py",
    ]
    if any(not source.is_file() for source in (*candidate_sources, *controller_sources)):
        fail("fuzz source binding is absent")
    sources = [
        {
            "root": "candidate",
            "path": str(source.relative_to(ROOT)),
            "sha256": sha256_file(source),
        }
        for source in candidate_sources
    ]
    sources.extend(
        {
            "root": "controller",
            "path": str(source.relative_to(CONTROLLER_ROOT)),
            "sha256": sha256_file(source),
        }
        for source in controller_sources
    )
    active_lockfile = _active_fuzz_lockfile()
    return {
        "source": identity_manifest(),
        "profile_sha256": sha256_bytes(canonical_json(profile_data)),
        "toolchain": {
            "channel": toolchain["channel"],
            "rustc": toolchain["rustc"],
            "tools": tools,
        },
        "sources": sources,
        "candidate_source_manifest": _candidate_source_manifest(),
        "active_lockfile": active_lockfile,
    }
def _recorded_fuzz_tool(provenance: dict[str, Any], name: str) -> Path:
    tools = provenance.get("toolchain", {}).get("tools")
    if not isinstance(tools, list):
        fail("fuzz provenance tools are malformed")
    matches = [item for item in tools if isinstance(item, dict) and item.get("id") == name]
    if len(matches) != 1:
        fail(f"fuzz provenance lacks exactly one {name} executable")
    path, digest = matches[0].get("path"), matches[0].get("sha256")
    if not isinstance(path, str) or not isinstance(digest, str):
        fail(f"fuzz provenance {name} executable is malformed")
    executable = Path(path)
    if executable.is_symlink() or not executable.is_file() or sha256_file(executable) != digest:
        fail(f"recorded fuzz {name} executable changed")
    return executable


def _fuzz_policy(profile_data: dict[str, Any], policy_mode: str) -> dict[str, Any]:
    if not isinstance(policy_mode, str) or policy_mode not in FUZZ_POLICY_MODES:
        fail("fuzz policy mode is not allowlisted")
    policy = profile_data.get("fuzz", {}).get(policy_mode)
    if not isinstance(policy, dict):
        fail("fuzz policy is absent")
    return policy

def _fuzz_receipt(target: str, native_target: str, corpus: Path, argv: tuple[str, ...], captured: dict[str, Any],
                  provenance: dict[str, Any], policy_mode: str, policy: dict[str, Any], limits: dict[str, int],
                  before: dict[str, Any], after: dict[str, Any], seed_manifest: dict[str, Any], run_identity: str) -> dict[str, Any]:
    if target not in FUZZ_TARGETS or not argv[:4] == ("cargo", "fuzz", "run", target):
        fail("fuzz receipt target or argv binding is malformed")
    elapsed_ns = captured.get("elapsed_ns")
    budget_seconds = policy["seconds_per_target"]
    if not isinstance(elapsed_ns, int) or isinstance(elapsed_ns, bool) or elapsed_ns < 0:
        fail("fuzz command elapsed time is malformed")
    if (
        captured.get("terminal_mode") == "success"
        and elapsed_ns < budget_seconds * 1_000_000_000
    ):
        fail("successful fuzz command did not consume its exact policy execution budget")
    stdout, stderr = _write_fuzz_blob(captured["stdout"]), _write_fuzz_blob(captured["stderr"])
    stdout["overflow"], stderr["overflow"] = captured["stdout_overflow"], captured["stderr_overflow"]
    status = "pass" if captured["terminal_mode"] == "success" else "fail"
    receipt = {"schema": "podway.g009.fuzz-receipt/v2", "policy_mode": policy_mode,
               "profile_sha256": provenance["profile_sha256"], "native_target": native_target,
               "target": target, "corpus": _fuzz_corpus_identity(corpus),
               "argv": ["cargo", "fuzz", "run", target, f"controller/{_fuzz_corpus_identity(corpus)}", "--",
                        f"-max_total_time={budget_seconds}", f"-timeout={policy['timeout_seconds']}",
                        f"-rss_limit_mb={policy['rss_limit_mb']}"],
               "limits": limits,
               "execution": {"budget_seconds": budget_seconds, "elapsed_ns": elapsed_ns,
                             "completed": captured["terminal_mode"] == "success",
                             "binary_sha256": captured["execution_binary_sha256"],
                             "argv": captured["execution_argv"]},
               "stdout": stdout, "stderr": stderr, "corpus_manifests": {"before": before, "after": after},
               "seed_manifest": seed_manifest, "initialization": {"seed_corpus_files": 1, "requires_captured_output": True},
               "terminal_mode": captured["terminal_mode"], "termination_reason": captured["termination_reason"],
               "exit_code": captured["exit_code"], "signal": captured["signal"], "timeout": captured["timeout"],
               "status": status, "provenance": provenance, "run_identity": run_identity}
    path, digest = evidence("fuzz-receipts", receipt)
    return {"path": str(path.relative_to(EVIDENCE_ROOT)), "sha256": digest, "status": status, "target": target}


def _run_fuzz_gate_under_watch(profile_data: dict[str, Any], policy_mode: str) -> dict[str, Any]:
    policy, toolchain = _fuzz_policy(profile_data, policy_mode), profile_data["fuzz"]["toolchain"]
    rustup = shutil.which("rustup")
    if rustup is None:
        fail("rustup is required for fuzz qualification")
    fuzz_env = {"PATH": f"{Path(rustup).resolve().parent}{os.pathsep}{os.environ.get('PATH', '')}",
                "RUSTUP_TOOLCHAIN": toolchain["channel"], "ASAN_OPTIONS": profile_data["fuzz"]["sanitizer_env"]["ASAN_OPTIONS"]}
    rustc = run_allowed(("rustc", "--version"), env=fuzz_env)
    if rustc.returncode != 0 or rustc.stdout.decode("utf-8", "strict").strip() != f"rustc {toolchain['rustc']}":
        fail("installed fuzz toolchain differs from the frozen profile")
    provenance = _fuzz_provenance(profile_data, fuzz_env)
    cargo = _recorded_fuzz_tool(provenance, "cargo")
    cargo_fuzz = _recorded_fuzz_tool(provenance, "cargo-fuzz")
    root = qualification_scratch_root() / "fuzz" / "corpus"
    root.mkdir(parents=True, exist_ok=True)
    if root.is_symlink() or not root.is_dir() or root.resolve() != root.absolute():
        fail("fuzz corpus root is unsafe")
    results: list[dict[str, Any]] = []
    limits = _fuzz_limits(profile_data, policy)
    run_identity = os.environ.get("G009_QUALIFICATION_RUN_ID")
    if not isinstance(run_identity, str) or not re.fullmatch(r"[0-9a-f]{64}", run_identity):
        fail("G009_QUALIFICATION_RUN_ID must bind fuzz evidence")
    seeds = fuzz_seeds(profile_data)
    for target, seed in zip(FUZZ_TARGETS, seeds):
        corpus = Path(tempfile.mkdtemp(prefix=f"{target}-", dir=root))
        materialized = _materialize_fuzz_seed(corpus, seed, 0)
        seed_manifest = {
            "profile_sha256": provenance["profile_sha256"],
            "seeds": [materialized],
        }
        argv = ("cargo", "fuzz", "run", target, str(corpus), "--", f"-max_total_time={policy['seconds_per_target']}",
                f"-timeout={policy['timeout_seconds']}", f"-rss_limit_mb={policy['rss_limit_mb']}")
        before = _corpus_manifest(target, corpus, "before", run_identity, limits)
        captured = _stream_fuzz_command(
            argv,
            cwd=ROOT / "fuzz",
            env=fuzz_env,
            timeout=(
                FUZZ_BUILD_ALLOWANCE_SECONDS
                + policy["seconds_per_target"]
                + policy["timeout_seconds"]
            ),
            cargo=cargo,
            cargo_fuzz=cargo_fuzz,
            native_target=profile_data["target"]["triple"],
        )
        after = _corpus_manifest(target, corpus, "after", run_identity, limits)
        receipt = _fuzz_receipt(
            target, profile_data["target"]["triple"], corpus, argv, captured, provenance, policy_mode,
            policy, limits, before, after, seed_manifest, run_identity,
        )
        results.append({"target": target, "corpus": _fuzz_corpus_identity(corpus), "receipt": receipt, "status": receipt["status"]})
    if len(results) != len(FUZZ_TARGETS) or {item["target"] for item in results} != set(FUZZ_TARGETS):
        fail("fuzz gate receipt set is not exactly the six bound targets")
    if any(item["receipt"].get("target") != item["target"] or item["receipt"].get("status") != item["status"]
           for item in results):
        fail("fuzz gate contains malformed or mismatched target receipts")
    if len({item["receipt"]["sha256"] for item in results}) != len(FUZZ_TARGETS):
        fail("fuzz gate contains duplicate target receipts")
    return {"policy_mode": policy_mode, "profile_sha256": provenance["profile_sha256"],
            "native_target": profile_data["target"]["triple"], "provenance": provenance,
            "commands": results, "run_identity": run_identity}
def _run_fuzz_gate(profile_data: dict[str, Any], policy_mode: str) -> dict[str, Any]:
    _validate_candidate_build_surface()
    watch = _CandidateSourceWatch()
    identity_before = identity_manifest()
    manifest_before = _candidate_source_manifest()
    try:
        result = _run_fuzz_gate_under_watch(profile_data, policy_mode)
        watch.verify_unchanged()
        if identity_manifest() != identity_before:
            fail("candidate identity changed during fuzz qualification")
        if _candidate_source_manifest() != manifest_before:
            fail("candidate source manifest changed during fuzz qualification")
        if result["provenance"]["source"] != identity_before:
            fail("fuzz provenance source differs from the watched candidate identity")
        if result["provenance"]["candidate_source_manifest"] != manifest_before:
            fail("fuzz provenance manifest differs from the watched candidate source")
        return result
    finally:
        watch.close()

def run_gate(gate_id: str, profile_data: dict[str, Any]) -> dict[str, Any]:
    commands = GATES.get(gate_id)
    if commands is None: fail(f"gate is not allowlisted: {gate_id}")
    target = profile_data["target"]["triple"]
    commands = tuple(tuple(target if item == TARGET else item for item in argv) for argv in commands)
    if gate_id == "G009-GATE-FUZZ":
        fuzz = _run_fuzz_gate(profile_data, "rc")
        return {
            "gate_id": gate_id,
            "policy_mode": fuzz["policy_mode"],
            "profile_sha256": fuzz["profile_sha256"],
            "native_target": fuzz["native_target"],
            "provenance": fuzz["provenance"],
            "commands": fuzz["commands"],
            "run_identity": fuzz["run_identity"],
            "status": "pass" if all(item["status"] == "pass" for item in fuzz["commands"]) else "fail",
        }
    results = []
    if gate_id == "G009-GATE-COVERAGE":
        collection = ("cargo", "+1.85.0", "llvm-cov", "--workspace", "--all-targets", "--target", target)
        result = run_allowed(collection)
        results.append({"argv": list(collection), "exit_code": result.returncode, "stdout_sha256": sha256_bytes(result.stdout),
                        "stderr_sha256": sha256_bytes(result.stderr), "status": "pass" if result.returncode == 0 else "fail",
                        "phase": "isolated-current-run-collection"})
    for argv in commands:
        result = run_allowed(argv)
        results.append({"argv": list(argv), "exit_code": result.returncode, "stdout_sha256": sha256_bytes(result.stdout),
                        "stderr_sha256": sha256_bytes(result.stderr), "status": "pass" if result.returncode == 0 else "fail",
                        **({"phase": "current-run-report"} if gate_id == "G009-GATE-COVERAGE" else {})})
    return {"gate_id": gate_id, "commands": results, "status": "pass" if all(item["status"] == "pass" for item in results) else "fail"}

def full_gates(args: argparse.Namespace) -> None:
    selected = args.only.split(",") if args.only else list(GATES)
    if not selected or len(set(selected)) != len(selected): fail("gate selection is empty or duplicates a gate")
    unknown = [gate for gate in selected if gate not in GATES]
    if unknown: fail(f"gate is not allowlisted: {unknown[0]}")
    rc_path = Path(args.rc)
    rc = verify_rc_consumption(rc_path)
    require_native_host(rc["target"])
    profile_data = _rc_profile(rc)
    assert_descendant_write_protection()
    results = []
    frozen_controller_sources = controller_source_bindings()
    for gate in selected:
        if controller_source_bindings() != frozen_controller_sources:
            fail("controller source changed during qualification")
        rc = verify_rc_consumption(rc_path)
        require_native_host(rc["target"])
        _rc_profile(rc)
        results.append(run_gate(gate, profile_data))
        if controller_source_bindings() != frozen_controller_sources:
            fail("candidate execution changed controller source")
    if any(item["status"] != "pass" for item in results): fail("one or more real gates failed")
    out, digest = _rc_evidence(Path(args.rc), "G009-GATE-GATES", results=results)
    print(f"{out} {digest}")


def local_fuzz(args: argparse.Namespace) -> None:
    if args.policy_mode != "local_smoke":
        fail("local fuzz invocation only permits local_smoke mode")
    profile_data = profile(Path(args.profile))
    require_native_host(profile_data["target"]["triple"])
    assert_descendant_write_protection()
    fuzz = _run_fuzz_gate(profile_data, args.policy_mode)
    gate = {
        "gate_id": "G009-GATE-FUZZ",
        "policy_mode": fuzz["policy_mode"],
        "profile_sha256": fuzz["profile_sha256"],
        "native_target": fuzz["native_target"],
        "provenance": fuzz["provenance"],
        "commands": fuzz["commands"],
        "run_identity": fuzz["run_identity"],
        "status": "pass" if all(item["status"] == "pass" for item in fuzz["commands"]) else "fail",
    }
    if gate["status"] != "pass":
        fail("local fuzz gate failed")
    out, digest = evidence("g009-local-fuzz-gate", gate)
    print(f"{out} {digest}")

def _verify_release_binary_bytes(raw: bytes, name: str, target: str) -> None:
    if len(raw) < 32 or raw[:4] != b"\xcf\xfa\xed\xfe":
        fail(f"{name} is not a thin 64-bit Mach-O")
    if target_tuple(target)["mach_o_arch"] != "arm64":
        fail("release binary target is not arm64")
    expected_cpu_type = 0x0100000C
    if struct.unpack_from("<I", raw, 4)[0] != expected_cpu_type:
        fail(f"{name} Mach-O architecture differs from native target tuple")
    if struct.unpack_from("<I", raw, 12)[0] != 0x2:
        fail(f"{name} is not an MH_EXECUTE Mach-O executable")
    commands, offset = struct.unpack_from("<I", raw, 16)[0], 32
    has_macos_target = False
    for _ in range(commands):
        if offset + 8 > len(raw): fail(f"{name} has truncated Mach-O load commands")
        command, size = struct.unpack_from("<II", raw, offset)
        if size < 8 or offset + size > len(raw): fail(f"{name} has invalid Mach-O load command")
        has_macos_target |= command in (0x24, 0x32)
        offset += size
    if not has_macos_target: fail(f"{name} lacks a macOS deployment load command")


def _verify_release_binary(path: Path, name: str, target: str,
                           read_only_paths: tuple[Path, ...] = ()) -> None:
    _verify_release_binary_bytes(bounded_bytes(path, 1024 * 1024), name, target)
    result = _run(
        (str(path), "--version"), ROOT, {"PATH": os.environ.get("PATH", "")},
        read_only_paths=read_only_paths,
    )
    if result.stdout.decode("utf-8", "strict").strip() != f"{name} 0.1.0":
        fail(f"{name} version is not 0.1.0")


def _snapshot_protected_paths(snapshots: dict[str, Path]) -> tuple[Path, ...]:
    paths = tuple(snapshots.values())
    if set(snapshots) != {"podway", "podwayd"} or not paths or len({path.parent for path in paths}) != 1:
        fail("RC snapshot set is incomplete or split across directories")
    return (paths[0].parent, *paths)


def _rc_binary_snapshots(rc: dict[str, Any], bin_dir: Path) -> tuple[tempfile.TemporaryDirectory[str], dict[str, Path], dict[str, bytes]]:
    holder = tempfile.TemporaryDirectory(prefix="g009-rc-binaries-", dir=qualification_scratch_root())
    root = Path(holder.name)
    snapshots: dict[str, Path] = {}
    bytes_by_name: dict[str, bytes] = {}
    try:
        for name in ("podway", "podwayd"):
            supplied = bin_dir / name
            if supplied.is_symlink() or not supplied.is_file():
                fail(f"missing or unsafe executable {name}")
            candidate = supplied.resolve()
            require_bound_file(rc, name, candidate)
            if not candidate.is_relative_to(bin_dir):
                fail(f"package executable escapes its declared directory: {name}")
            snapshot = bounded_bytes(candidate)
            if sha256_bytes(snapshot) != rc["binaries"][name]["sha256"]:
                fail(f"package binary differs from RC: {name}")
            destination = root / name
            descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o500)
            try:
                with os.fdopen(descriptor, "wb") as handle:
                    handle.write(snapshot)
                    handle.flush()
                    os.fsync(handle.fileno())
            except BaseException:
                destination.unlink(missing_ok=True)
                raise
            if destination.is_symlink() or bounded_bytes(destination) != snapshot:
                fail(f"RC snapshot materialization failed: {name}")
            snapshots[name], bytes_by_name[name] = destination, snapshot
        protected = _snapshot_protected_paths(snapshots)
        for name, snapshot_path in snapshots.items():
            _verify_release_binary(snapshot_path, name, rc["target"], protected)
        return holder, snapshots, bytes_by_name
    except BaseException:
        holder.cleanup()
        raise


def _assert_snapshot_rewrite_sentinels(snapshots: dict[str, Path], bytes_by_name: dict[str, bytes]) -> dict[str, Any]:
    child = (
        "import os,sys\n"
        "target,sibling=map(os.fspath,sys.argv[1:])\n"
        "for operation in ('overwrite','rename','symlink'):\n"
        " try:\n"
        "  if operation=='overwrite': open(target,'wb').write(b'g009-rewrite')\n"
        "  elif operation=='rename': os.rename(sibling,target)\n"
        "  else: os.symlink(sibling,target)\n"
        " except OSError: continue\n"
        " raise SystemExit('snapshot rewrite permitted: '+operation)\n"
    )
    protected = _snapshot_protected_paths(snapshots)
    rows: list[dict[str, str]] = []
    for name, target in snapshots.items():
        sibling_name = "podwayd" if name == "podway" else "podway"
        result = _completed_bounded(
            sandboxed_candidate_argv(
                (sys.executable, "-c", child, str(target), str(snapshots[sibling_name])),
                read_only_paths=protected,
            ),
            cwd=ROOT, env={"PATH": os.environ.get("PATH", "")},
        )
        if result.returncode != 0:
            fail(f"RC snapshot {name} rewrite sentinel did not deny every operation")
        if bounded_bytes(target) != bytes_by_name[name]:
            fail(f"RC snapshot {name} changed during rewrite sentinel")
        rows.append({"target": name, "sibling": sibling_name, "operations": "overwrite,rename,symlink"})
    return {
        "description": "sandboxed candidate self/sibling overwrite, rename, and symlink attempts are denied before packaging",
        "rows": rows,
    }
def _assert_snapshot_bytes(snapshots: dict[str, Path], bytes_by_name: dict[str, bytes], stage: str) -> None:
    for name, snapshot in snapshots.items():
        if bounded_bytes(snapshot) != bytes_by_name[name]:
            fail(f"RC snapshot {name} changed {stage}")




def _archive_member_bytes(podway: Path, binary_bytes: dict[str, bytes], target: str,
                          protected_snapshots: tuple[Path, ...]) -> dict[str, bytes]:
    root = archive_root(target)
    members = {f"{root}/bin/podway": binary_bytes["podway"],
               f"{root}/bin/podwayd": binary_bytes["podwayd"],
               f"{root}/LICENSE": bounded_bytes(ROOT / "sot/LICENSE"),
               f"{root}/README.md": bounded_bytes(ROOT / "README.md"),
               f"{root}/RELEASE_NOTES.md": bounded_bytes(ROOT / "RELEASE_NOTES.md")}
    with tempfile.TemporaryDirectory(prefix="g009-completions-", dir=qualification_scratch_root()) as raw:
        directory = Path(raw)
        for shell in ("bash", "zsh", "fish"):
            result = _run((str(podway), "completions", shell), ROOT, {"HOME": str(directory), "TMPDIR": str(directory), "PATH": os.environ.get("PATH", "")}, read_only_paths=protected_snapshots)
            if not result.stdout: fail(f"empty {shell} completion output")
            members[f"{root}/share/completions/podway.{shell}"] = result.stdout
    for source_root, archive_prefix in ((ROOT / "presets", "share/podway/presets"), (ROOT / "schemas", "share/podway/schemas")):
        if source_root.is_symlink() or not source_root.is_dir():
            fail(f"missing or unsafe shipped directory: {source_root}")
        resolved_root = source_root.resolve()
        if not resolved_root.is_relative_to(ROOT.resolve()):
            fail("shipped directory escapes candidate root")
        for source in sorted(path for path in source_root.rglob("*") if path.is_file()):
            if source.is_symlink() or not source.resolve().is_relative_to(resolved_root):
                fail("shipped member escapes its declared root")
            relative = source.relative_to(source_root).as_posix()
            safe_relative(relative)
            members[f"{root}/{archive_prefix}/{relative}"] = bounded_bytes(source)
    return members

def _deterministic_zip(path: Path, members: dict[str, bytes], target: str) -> None:
    if not path.parent.resolve().is_relative_to(qualification_scratch_root()):
        fail("archive output escapes qualification scratch root")
    manifest_path = f"{archive_root(target)}/payload-digests-v1.json"
    manifest = {"schema": "podway.g009.payload-digests/v1", "members": [
        {"path": name, "sha256": sha256_bytes(data), "size": len(data)}
        for name, data in sorted(members.items())]}
    members = dict(members)
    members[manifest_path] = canonical_json(manifest)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9, strict_timestamps=True) as archive:
            for name in sorted(members):
                safe_relative(name)
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.external_attr = ((0o100755 if name.startswith(f"{archive_root(target)}/bin/") else 0o100644) << 16)
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, members[name], compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
        if path.exists():
            if bounded_bytes(path) != bounded_bytes(temporary): fail(f"refusing mismatched existing archive: {path}")
            temporary.unlink()
        else:
            os.replace(temporary, path)
    finally:
        if temporary.exists(): temporary.unlink()

def _write_checksum(path: Path) -> None:
    sidecar = path.with_name(path.name + ".sha256")
    payload = (sha256_file(path) + "\n").encode("ascii")
    if sidecar.exists():
        if bounded_bytes(sidecar, 1024) != payload: fail("refusing mismatched existing detached checksum")
        return
    temporary = sidecar.with_name(f".{sidecar.name}.{os.getpid()}.tmp")
    try:
        temporary.write_bytes(payload)
        os.replace(temporary, sidecar)
    finally:
        if temporary.exists(): temporary.unlink()

def package(args: argparse.Namespace) -> None:
    rc = verify_rc_consumption(Path(args.rc))
    require_native_host(rc["target"])
    profile_data = _rc_profile(rc)
    archive_input, bin_input = Path(args.archive), Path(args.bin_dir)
    if archive_input.is_symlink() or bin_input.is_symlink():
        fail("package paths may not be symlinks")
    archive, bin_dir = archive_input.resolve(), bin_input.resolve()
    scratch = qualification_scratch_root()
    if not archive.is_relative_to(scratch / "package") or not bin_dir.is_relative_to(scratch / "cargo" / "target"):
        fail("package inputs escape the qualification scratch allowlist")
    holder, snapshots, bytes_by_name = _rc_binary_snapshots(rc, bin_dir)
    try:
        sentinels = _assert_snapshot_rewrite_sentinels(snapshots, bytes_by_name)
        members = _archive_member_bytes(
            snapshots["podway"], bytes_by_name, rc["target"], _snapshot_protected_paths(snapshots),
        )
        _assert_snapshot_bytes(snapshots, bytes_by_name, "during candidate completion execution")
        required = profile_data["archive"].get("members")
        if not isinstance(required, list) or any(not isinstance(item, str) for item in required): fail("profile archive member declaration is invalid")
        root = archive_root(rc["target"])
        declared_roots = {f"{root}/{item}" for item in required}
        actual = set(members) | {f"{root}/payload-digests-v1.json"}
        if any(name not in declared_roots and not any(name.startswith(root + "/") for root in declared_roots) for name in actual):
            fail("archive contains an undeclared member")
        if any(root not in actual and not any(name.startswith(root + "/") for name in actual) for root in declared_roots):
            fail("archive omits a declared member or descendant")
        _deterministic_zip(archive, members, rc["target"])
        _write_checksum(archive)
        report = inspect_archive(archive, actual, target=rc["target"])
        snapshot_binding = {
            name: {"sha256": sha256_bytes(data), "bytes": len(data)}
            for name, data in bytes_by_name.items()
        }
        out, digest = _rc_evidence(
            Path(args.rc), "G009-GATE-PACKAGE", archive=report, signing=rc["signing"],
            rc_binary_snapshots=snapshot_binding, rewrite_sentinels=sentinels,
        )
        print(f"{out} {digest}")
    finally:
        holder.cleanup()


LAUNCHCTL = Path("/bin/launchctl")
def _lifecycle_identity(uid: int, runtime: Path) -> dict[str, Any]:
    runtime_path = runtime.resolve(strict=True)
    if uid < 0 or not runtime_path.is_absolute() or runtime_path != runtime:
        fail("lifecycle identity inputs are unsafe")
    target = f"gui/{uid}/{LABEL}"
    socket_path = runtime_path / f"podway-{uid}" / "podwayd.sock"
    return {
        "uid": uid,
        "target": target,
        "runtime_path": str(runtime_path),
        "socket_path": str(socket_path),
    }



def _launchctl_print(uid: int, environment: dict[str, str]) -> subprocess.CompletedProcess[bytes]:
    if not LAUNCHCTL.is_file() or LAUNCHCTL.is_symlink():
        fail("exact /bin/launchctl is unavailable")
    return _completed_bounded((str(LAUNCHCTL), "print", f"gui/{uid}/{LABEL}"), env=environment)

def _launchctl_not_loaded(result: subprocess.CompletedProcess[bytes], uid: int) -> bool:
    current = (
        f'Bad request.\nCould not find service "{LABEL}" in domain for user gui: {uid}'
    ).encode()
    legacy = f'Could not find service "{LABEL}" in domain for gui/{uid}'.encode()
    return (
        result.stdout == b""
        and (
            result.returncode == 113 and result.stderr in {current, current + b"\n"}
            or result.returncode == 3 and result.stderr in {legacy, legacy + b"\n"}
        )
    )


def _launchctl_absent(uid: int, environment: dict[str, str] | None = None) -> dict[str, Any]:
    result = _launchctl_print(uid, environment or {"PATH": os.environ.get("PATH", "")})
    if result.returncode == 0:
        fail(f"refusing pre-existing LaunchAgent {LABEL}")
    if not _launchctl_not_loaded(result, uid):
        fail("cannot establish exact LaunchAgent absence safely")
    return {
        "target": f"gui/{uid}/{LABEL}",
        "exit_code": result.returncode,
        "stdout_sha256": sha256_bytes(result.stdout),
        "stderr_sha256": sha256_bytes(result.stderr),
        "stderr": result.stderr.decode("utf-8", "strict"),
    }


def _wait_for_launchctl_absence(uid: int, environment: dict[str, str], timeout: float = 10) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = _launchctl_print(uid, environment)
        if _launchctl_not_loaded(result, uid):
            return {
                "target": f"gui/{uid}/{LABEL}",
                "exit_code": result.returncode,
                "stdout_sha256": sha256_bytes(result.stdout),
                "stderr_sha256": sha256_bytes(result.stderr),
                "stderr": result.stderr.decode("utf-8", "strict"),
            }
        if result.returncode != 0:
            fail("launchctl print returned an unrecognized status during cleanup")
        time.sleep(0.05)
    fail("launchctl bootout did not establish bounded label absence")


def _safe_extract_archive(archive: Path, destination: Path, target: str) -> Path:
    inspected = inspect_archive(archive, target=target)
    archive_bytes = bounded_bytes(archive)
    if sha256_bytes(archive_bytes) != inspected["archive_sha256"]:
        fail("archive changed after validation")
    with zipfile.ZipFile(io.BytesIO(archive_bytes)) as bundle:
        for info in bundle.infolist():
            name = info.filename
            if info.is_dir() or not name.startswith(archive_root(target) + "/") or ".." in Path(name).parts: fail("unsafe archive member")
            extraction_path = (destination / name).resolve()
            if not extraction_path.is_relative_to(destination.resolve()): fail("unsafe extraction destination")
            extraction_path.parent.mkdir(parents=True, exist_ok=True)
            extraction_path.write_bytes(bundle.read(info))
            mode = (info.external_attr >> 16) & 0o777
            os.chmod(extraction_path, mode)
            if (extraction_path.stat().st_mode & 0o777) != mode: fail("archive mode was not preserved on extraction")
    return destination / archive_root(target)


def _lifecycle_file_identity(path: Path, role: str, expected_bytes: bytes | None = None,
                             expected_mode: int | None = None) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file():
        fail(f"lifecycle {role} is not a regular non-symlink file")
    mode, data = path.stat().st_mode & 0o777, bounded_bytes(path)
    if (expected_bytes is not None and data != expected_bytes) or (expected_mode is not None and mode != expected_mode):
        fail(f"lifecycle {role} identity changed")
    return {"role": role, "path": str(path), "sha256": sha256_bytes(data), "mode": mode, "bytes": len(data)}
def _lifecycle_canonical_identity(path: Path, role: str, expected_bytes: bytes | None = None,
                                  expected_mode: int | None = None) -> dict[str, Any]:
    if (
        not path.is_absolute()
        or str(path) != str(Path(str(path)))
        or path.is_symlink()
    ):
        fail(f"lifecycle {role} path is unsafe")
    resolved = path.resolve(strict=True)
    if resolved != path or any(parent.is_symlink() for parent in path.parents):
        fail(f"lifecycle {role} path has a lexical or symlink escape")
    return _lifecycle_file_identity(path, role, expected_bytes, expected_mode)


def _lifecycle_generation(staged: Path, identity: str, metadata: dict[str, Any],
                          log_path: Path) -> tuple[str, bytes]:
    def xml_escape(value: str) -> str:
        return value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;").replace("'", "&apos;")

    plist_without_generation = (
        '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0">\n<dict>\n'
        f"  <key>Label</key>\n  <string>{LABEL}</string>\n  <key>PodwayDaemonSha256</key>\n  <string>{identity}</string>\n"
        f"\n  <key>ProgramArguments</key>\n  <array>\n    <string>{xml_escape(str(staged))}</string>\n    <string>--service</string>\n  </array>\n\n  <key>RunAtLoad</key>\n  <true/>\n\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n\n  <key>ThrottleInterval</key>\n  <integer>5</integer>\n\n  <key>ProcessType</key>\n  <string>Background</string>\n\n  <key>StandardOutPath</key>\n  <string>{xml_escape(str(log_path))}</string>\n\n  <key>StandardErrorPath</key>\n  <string>{xml_escape(str(log_path))}</string>\n</dict>\n</plist>\n"
    ).encode("utf-8")
    preimage = {
        "version": metadata["version"], "label": metadata["label"],
        "daemon_binary": metadata["daemon_binary"], "daemon_identity": metadata["daemon_identity"],
        "installed_at": metadata["installed_at"], "updated_at": metadata["updated_at"],
    }
    generation = sha256_bytes(
        plist_without_generation + b"\n"
        + json.dumps(preimage, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    )
    label = f"  <string>{LABEL}</string>".encode()
    authenticated = plist_without_generation.replace(
        label,
        label + f"\n  <key>PodwayGeneration</key>\n  <string>{generation}</string>\n".encode(),
        1,
    )
    return generation, authenticated


def _lifecycle_service_metadata_bytes(metadata: dict[str, Any]) -> bytes:
    fields = (
        "version", "label", "daemon_binary", "daemon_identity", "installed_at",
        "updated_at", "publication_state", "generation",
    )
    if set(metadata) != set(fields):
        fail("lifecycle metadata keys are incomplete")
    return json.dumps(
        {field: metadata[field] for field in fields},
        separators=(",", ":"), ensure_ascii=False,
    ).encode("utf-8") + b"\n"


def _validate_lifecycle_install(plist: Path, metadata: Path, support: Path, wrapper: Path,
                                log_path: Path) -> tuple[Path, dict[str, Any], dict[str, Any]]:
    plist_identity = _lifecycle_canonical_identity(plist, "plist", expected_mode=0o600)
    metadata_identity = _lifecycle_canonical_identity(metadata, "metadata", expected_mode=0o600)
    try:
        plist_data, metadata_data = plistlib.loads(bounded_bytes(plist)), load_json(metadata)
        arguments = plist_data["ProgramArguments"]
    except (KeyError, TypeError, ValueError, OSError) as exc:
        raise QualificationError("lifecycle publication metadata is invalid") from exc
    wrapper_digest = sha256_file(wrapper)
    expected_staged = support / ".podway-daemons-v1" / wrapper_digest
    if (not isinstance(metadata_data, dict) or set(metadata_data) != {"version", "label", "daemon_binary", "daemon_identity", "installed_at", "updated_at", "publication_state", "generation"}
            or arguments != [str(expected_staged), "--service"] or metadata_data.get("daemon_binary") != str(expected_staged)
            or metadata_data.get("daemon_identity") != wrapper_digest or metadata_data.get("label") != LABEL
            or metadata_data.get("publication_state") != "receipt_durable" or metadata_data.get("version") != 1
            or not all(isinstance(metadata_data.get(field), int) and metadata_data[field] >= 0 for field in ("installed_at", "updated_at"))
            or metadata_data["updated_at"] < metadata_data["installed_at"] or not expected_staged.is_relative_to(support / ".podway-daemons-v1")
            or expected_staged.resolve(strict=True) != expected_staged):
        fail("lifecycle install did not publish canonical digest-named staged binding")
    if bounded_bytes(metadata) != _lifecycle_service_metadata_bytes(metadata_data):
        fail("lifecycle metadata is not exact compact ordered service.json bytes")
    generation, expected_plist = _lifecycle_generation(expected_staged, wrapper_digest, metadata_data, log_path)
    if (bounded_bytes(plist) != expected_plist or metadata_data.get("generation") != generation
            or plist_data.get("Label") != LABEL or plist_data.get("PodwayGeneration") != generation
            or plist_data.get("PodwayDaemonSha256") != wrapper_digest):
        fail("lifecycle install generation or byte-exact plist identity is not reconstructed")
    staged_identity = _lifecycle_canonical_identity(expected_staged, "staged_wrapper", bounded_bytes(wrapper), 0o700)
    return expected_staged, {"plist": plist_identity, "metadata": metadata_identity, "staged_wrapper": staged_identity}, metadata_data


def _lifecycle_sandbox(
    root: Path, home: Path, runtime: Path, podwayd: Path,
) -> tuple[Path, Path, bytes, bytes]:
    candidate_root = require_candidate_root()
    wrapper, profile_path = root / "podwayd-sandbox-wrapper", root / "podwayd-service.sb"
    allowed = (runtime, root / "scratch")
    for path in allowed:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    rendered, candidate = tuple(str(path.resolve()) for path in allowed), str(candidate_root)
    if any(any(character in value for character in ('"', "\\", "\n", "\r")) for value in (*rendered, candidate)):
        fail("lifecycle sandbox path cannot be represented safely")
    profile_bytes = (
        "(version 1)(deny default)(allow process-exec)(allow process-info*)(allow file-read*)(allow sysctl-read)"
        + "".join(f'(allow file-write* (subpath "{path}"))' for path in rendered)
        + f'(deny file-write* (literal "{candidate}"))(deny file-write* (subpath "{candidate}"))'
        + f'(deny file-link (subpath "{candidate}"))'
    ).encode("utf-8")
    wrapper_bytes = (
        "#!/bin/sh\n"
        f"exec /usr/bin/sandbox-exec -f {shlex.quote(str(profile_path))} "
        f"{shlex.quote(str(podwayd))} \"$@\"\n"
    ).encode("utf-8")
    profile_path.write_bytes(profile_bytes); wrapper.write_bytes(wrapper_bytes)
    os.chmod(profile_path, 0o600); os.chmod(wrapper, 0o700)
    _lifecycle_file_identity(profile_path, "controller_profile", profile_bytes, 0o600)
    _lifecycle_file_identity(wrapper, "controller_wrapper", wrapper_bytes, 0o700)
    return wrapper, profile_path, wrapper_bytes, profile_bytes


def _lifecycle_candidate_completed(
    argv: tuple[str, ...], *, cwd: Path, env: dict[str, str], writable: tuple[Path, ...],
    protected: tuple[Path, ...], timeout: float = 30,
) -> subprocess.CompletedProcess[bytes]:
    policy = _lifecycle_command_policy(writable, protected)
    return _completed_bounded(("/usr/bin/sandbox-exec", "-p", policy, *argv), cwd=cwd, env=env, timeout=timeout)


def _lifecycle_command_policy(writable: tuple[Path, ...], protected: tuple[Path, ...]) -> str:
    rendered_writable, rendered_protected = [str(path.resolve()) for path in writable], [str(path.resolve()) for path in protected]
    if any(any(character in value for character in ('"', "\\", "\n", "\r")) for value in (*rendered_writable, *rendered_protected)):
        fail("lifecycle sandbox path cannot be represented safely")
    ancestor_denials = tuple(sorted({path.parent.resolve() for path in protected} - set(writable)))
    return (
        "(version 1)(deny default)(allow process-exec)(allow process-info*)(allow process-fork)(allow file-read*)(allow sysctl-read)"
        + "".join(f'(allow file-write* (subpath "{path}"))' for path in rendered_writable)
        + "".join(f'(deny file-write* (literal "{path}"))(deny file-write* (subpath "{path}"))(deny file-link (literal "{path}"))(deny file-link (subpath "{path}"))' for path in rendered_protected)
        + "".join(f'(deny file-write* (literal "{path}"))(deny file-link (literal "{path}"))' for path in ancestor_denials)
    )
def _lifecycle_policy_inputs(writable: tuple[Path, ...], protected: tuple[Path, ...]) -> dict[str, list[str]]:
    return {
        "writable": [str(path.resolve()) for path in writable],
        "protected": [str(path.resolve()) for path in protected],
    }



def _lifecycle_protected_snapshot(identities: dict[str, dict[str, Any]]) -> dict[str, Any]:
    artifacts = {role: _lifecycle_canonical_identity(Path(identity["path"]), role, expected_mode=identity["mode"]) for role, identity in identities.items()}
    if any(artifacts[role]["sha256"] != identity["sha256"] for role, identity in identities.items()):
        fail("lifecycle protected artifact identity changed")
    ancestors: list[dict[str, Any]] = []
    for role, identity in artifacts.items():
        cursor = Path(identity["path"]).parent
        while True:
            if cursor.is_symlink() or not cursor.is_dir() or cursor.resolve(strict=True) != cursor:
                fail("lifecycle protected artifact ancestor is unsafe")
            stat = cursor.stat()
            ancestors.append({"role": role, "path": str(cursor), "mode": stat.st_mode & 0o777, "device": stat.st_dev, "inode": stat.st_ino})
            if cursor == cursor.parent:
                break
            cursor = cursor.parent
    return {"artifacts": artifacts, "canonical_ancestors": sorted(ancestors, key=lambda item: (item["path"], item["role"]))}
def _assert_lifecycle_policy_denies_mutations(profile_text: str, candidate_root: Path, staged: Path,
                                              identities: dict[str, dict[str, Any]], boundary: str) -> dict[str, Any]:
    operations = ("transient_create_then_delete", "existing_overwrite", "rename", "symlink")
    child = (
        "import json,os,sys\nfrom pathlib import Path\nop=sys.argv[1]; target=Path(sys.argv[2]); source=Path(sys.argv[3])\ntry:\n"
        " if op=='transient_create_then_delete': target.write_bytes(b'x'); target.unlink()\n"
        " elif op=='existing_overwrite': source.write_bytes(b'x')\n"
        " elif op=='rename': os.rename(source,target)\n"
        " else: os.symlink(source,target)\n"
        "except OSError as exc: print(json.dumps({'denied':True,'errno':exc.errno,'diagnostic':str(exc)})); raise SystemExit(0)\n"
        "raise SystemExit('lifecycle sandbox permitted '+op)\n"
    )
    rows: list[dict[str, Any]] = []
    for target_class, root, existing in (("candidate_root", candidate_root, candidate_root / "Cargo.toml"), ("staged_publication", staged.parent, staged)):
        for operation in operations:
            target = root / f".g009-{boundary}-{target_class}-{operation}"
            result = _completed_bounded(("/usr/bin/sandbox-exec", "-p", profile_text, sys.executable, "-c", child, operation, str(target), str(existing)), cwd=CONTROLLER_ROOT, env={"PATH": os.environ.get("PATH", "")})
            try:
                detail = json.loads(result.stdout.decode("utf-8", "strict"))
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                raise QualificationError("lifecycle mutation probe output is malformed") from exc
            row = {"boundary": boundary, "target_class": target_class, "operation": operation, **detail}
            if result.returncode != 0 or set(row) != {"boundary", "target_class", "operation", "denied", "errno", "diagnostic"} or row["denied"] is not True or not isinstance(row["errno"], int) or not row["diagnostic"] or row["errno"] not in {1, 13, 30}:
                fail("lifecycle sandbox mutation sentinel failed")
            rows.append(row)
    if _lifecycle_protected_snapshot(identities)["artifacts"] != identities:
        fail("lifecycle mutation probe changed a protected identity")
    return {"policy_sha256": sha256_bytes(profile_text.encode("utf-8")), "operations": rows}

def _launchctl_bound_state(uid: int, environment: dict[str, str], plist: Path, staged: Path) -> dict[str, Any] | None:
    observed = _launchctl_print(uid, environment)
    if _launchctl_not_loaded(observed, uid):
        return None
    if observed.returncode != 0 or observed.stderr != b"":
        fail("launchctl print did not return cleanly")
    try:
        text = observed.stdout.decode("utf-8", "strict")
    except UnicodeDecodeError as exc:
        raise QualificationError("launchctl print is not UTF-8") from exc
    target = f"gui/{uid}/{LABEL}"
    patterns = (
        rf"(?m)^{re.escape(target)} = \{{$",
        rf"(?m)^\s*path = {re.escape(str(plist))}$",
        rf"(?m)^\s*program = {re.escape(str(staged))}$",
        rf"(?ms)^\s*arguments = \{{\s*{re.escape(str(staged))}\s*--service\s*\}}$",
        r"(?m)^\s*state = running$",
    )
    pids = re.findall(r"(?m)^\s*pid = ([1-9]\d*)$", text)
    if not all(re.search(pattern, text) for pattern in patterns) or len(pids) != 1:
        fail("launchctl state is not bound to the exact target, plist, staged executable, and running process")
    return {"target": target, "plist_path": str(plist), "program": str(staged), "arguments": [str(staged), "--service"], "state": "running", "pid": int(pids[0]), "stdout_sha256": sha256_bytes(observed.stdout), "stderr_sha256": sha256_bytes(observed.stderr), "stdout_base64": base64.b64encode(observed.stdout).decode("ascii"), "stderr_base64": base64.b64encode(observed.stderr).decode("ascii")}
def _lifecycle_runtime_socket(runtime: Path, uid: int) -> dict[str, Any]:
    expected = runtime / f"podway-{uid}" / "podwayd.sock"
    present = [path for path in _socket_paths({"TMPDIR": str(runtime)}) if path.exists()]
    if present != [expected]:
        fail("lifecycle socket is not the exact UID-specific runtime socket")
    status, runtime_status = expected.stat(), runtime.stat()
    if (
        not stat.S_ISSOCK(status.st_mode)
        or status.st_uid != uid
        or runtime_status.st_uid != uid
        or status.st_mode & 0o077
        or runtime_status.st_mode & 0o077
    ):
        fail("lifecycle socket/runtime owner or mode is unsafe")
    return {
        "socket_path": str(expected), "socket_owner_uid": status.st_uid,
        "socket_mode": status.st_mode & 0o777, "runtime_path": str(runtime),
        "runtime_owner_uid": runtime_status.st_uid, "runtime_mode": runtime_status.st_mode & 0o777,
        "target_uid": uid,
    }
def _lifecycle_unexpected_exit_relaunch(uid: int, environment: dict[str, str], plist: Path,
                                        staged: Path, runtime: Path) -> dict[str, Any]:
    before = _launchctl_bound_state(uid, environment, plist, staged)
    if before is None:
        fail("lifecycle unexpected-exit probe lacks a running service")
    expected_socket = runtime / f"podway-{uid}" / "podwayd.sock"
    os.kill(before["pid"], signal.SIGKILL)
    try:
        stale_status = expected_socket.stat()
    except OSError as exc:
        raise QualificationError("unexpected exit did not leave an observable stale socket") from exc
    if not stat.S_ISSOCK(stale_status.st_mode) or stale_status.st_uid != uid:
        fail("unexpected exit stale socket is not the exact service-owned socket")
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        after = _launchctl_bound_state(uid, environment, plist, staged)
        if after is not None and after["pid"] != before["pid"]:
            return {
                "signal": signal.SIGKILL, "before_pid": before["pid"], "after_pid": after["pid"],
                "launchctl": after, "runtime_socket": _lifecycle_runtime_socket(runtime, uid),
                "stale_socket_recovery": {
                    "stale_socket_path": str(expected_socket), "stale_socket_owner_uid": stale_status.st_uid,
                    "recovered_socket_path": str(expected_socket), "recovered": True,
                },
            }
        time.sleep(0.05)
    fail("lifecycle unexpected exit did not relaunch the staged service")
def _lifecycle_stop_defeats_keepalive(uid: int, environment: dict[str, str]) -> dict[str, Any]:
    first = _launchctl_absent(uid, environment)
    time.sleep(5.1)
    second = _launchctl_absent(uid, environment)
    return {"first_absence": first, "post_throttle_absence": second, "throttle_interval_seconds": 5}

def _controller_lifecycle_cleanup(uid: int, environment: dict[str, str], plist: Path,
                                  metadata: Path, staged: Path | None) -> dict[str, Any]:
    if staged is None:
        fail("controller cleanup lacks a validated staged path")
    observed = _launchctl_bound_state(uid, environment, plist, staged)
    bootout = _completed_bounded((str(LAUNCHCTL), "bootout", f"gui/{uid}/{LABEL}"), env=environment)
    if bootout.returncode not in (0, 3, 113):
        fail("controller bootout failed")
    absence = _wait_for_launchctl_absence(uid, environment)
    verified = [plist, metadata, staged]
    for path in verified:
        if path.is_symlink() or not path.resolve().is_relative_to(plist.parents[3].resolve()):
            fail("controller cleanup refused an unverified lifecycle path")
        path.unlink(missing_ok=True)
    if any(path.exists() or path.is_symlink() for path in verified):
        fail("controller cleanup left verified lifecycle paths")
    return {"launchctl_path": str(LAUNCHCTL), "bootout_exit_code": bootout.returncode,
            "observed_state": observed, "absence": absence, "removed_paths": [str(path) for path in verified]}
def lifecycle(args: argparse.Namespace) -> None:
    rc = verify_rc_consumption(Path(args.rc))
    require_native_host(rc["target"])
    _rc_profile(rc)
    archive = Path(args.archive)
    inspected = inspect_archive(archive, target=rc["target"])
    if not args.require_clean_user:
        fail("lifecycle requires --require-clean-user")
    uid = os.getuid()

    _launchctl_absent(uid)
    with tempfile.TemporaryDirectory(prefix="g009-lifecycle-", dir=qualification_scratch_root()) as raw:
        root = Path(raw)
        home = root / "home"
        home.mkdir(mode=0o700)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        lifecycle_identity = _lifecycle_identity(uid, runtime)
        launch_agents = home / "Library" / "LaunchAgents"
        launch_agents.mkdir(parents=True)
        plist = launch_agents / f"{LABEL}.plist"
        if plist.exists():
            fail("isolated HOME already has Podway label")
        extracted = _safe_extract_archive(archive, root / "extract", rc["target"])
        podway, podwayd = extracted / "bin" / "podway", extracted / "bin" / "podwayd"
        _verify_release_binary(podway, "podway", rc["target"])
        _verify_release_binary(podwayd, "podwayd", rc["target"])
        if sha256_file(podway) != rc["binaries"]["podway"]["sha256"] or sha256_file(podwayd) != rc["binaries"]["podwayd"]["sha256"]:
            fail("lifecycle archive executables differ from RC")
        if inspected["archive_sha256"] != sha256_file(archive):
            fail("lifecycle archive digest changed after inspection")
        holder, workspace, source_files, source_manifest = _native_scratch_source()
        wrapper, profile_path, wrapper_bytes, profile_bytes = _lifecycle_sandbox(root, home, runtime, podwayd)
        support, metadata = home / "Library" / "Application Support" / "Podway", home / "Library" / "Application Support" / "Podway" / "service.json"
        logs = home / "Library" / "Logs" / "Podway"
        support.mkdir(parents=True, mode=0o700); logs.mkdir(parents=True, mode=0o700)
        workload_output = workspace / ".g009-lifecycle-outputs"; workload_output.mkdir(mode=0o700)
        controls = (wrapper, profile_path, podwayd, podway, require_candidate_root())
        frozen = {"controller_wrapper": _lifecycle_canonical_identity(wrapper, "controller_wrapper", wrapper_bytes, 0o700),
                  "controller_profile": _lifecycle_canonical_identity(profile_path, "controller_profile", profile_bytes, 0o600),
                  "extracted_archived_daemon": _lifecycle_canonical_identity(podwayd, "extracted_archived_daemon"),
                  "extracted_archived_cli": _lifecycle_canonical_identity(podway, "extracted_archived_cli")}
        qualification_install = {
            "route": "install-qualification-wrapper",
            "wrapper_path": str(wrapper),
            "wrapper_sha256": sha256_bytes(wrapper_bytes),
            "sandbox_profile_path": str(profile_path),
            "sandbox_profile_sha256": sha256_bytes(profile_bytes),
            "archived_daemon_path": str(podwayd),
            "archived_daemon_sha256": sha256_file(podwayd),
        }
        commands = [(
            str(podway), "daemon", qualification_install["route"],
            "--wrapper-path", qualification_install["wrapper_path"],
            "--wrapper-sha256", qualification_install["wrapper_sha256"],
            "--sandbox-profile-path", qualification_install["sandbox_profile_path"],
            "--sandbox-profile-sha256", qualification_install["sandbox_profile_sha256"],
            "--archived-daemon-path", qualification_install["archived_daemon_path"],
            "--archived-daemon-sha256", qualification_install["archived_daemon_sha256"],
        ),
                    (str(podway), "daemon", "stop"), (str(podway), "daemon", "start"),
                    (str(podway), "daemon", "status"), (str(podway), "daemon", "restart"),
                    (str(podway), "daemon", "logs", "--lines", "1"),
                    (str(podway), "daemon", "uninstall")]
        receipts: list[dict[str, Any]] = []
        environment = {"HOME": str(home), "TMPDIR": str(runtime), "PATH": os.environ.get("PATH", "")}
        primary_error: BaseException | None = None; cleanup_errors: list[str] = []
        staged: Path | None = None; policy_probe: dict[str, Any] | None = None; cleanup_receipt: dict[str, Any] | None = None
        try:
            marker = workspace / ".g009-preserve"; marker.write_text("preserve\n", encoding="utf-8")
            for argv in commands:
                operation = argv[2]
                before_protected = controls + ((plist, metadata, staged) if staged else ())
                if operation in {"start", "restart"}:
                    if staged is None:
                        fail("lifecycle start preceded staged installation")
                    for role, identity in frozen.items():
                        observed = _lifecycle_canonical_identity(Path(identity["path"]), role, expected_mode=identity["mode"])
                        if observed["sha256"] != identity["sha256"]:
                            fail("lifecycle control artifact changed before launch")
                if operation == "install-qualification-wrapper":
                    writable_paths = (support, launch_agents)
                    protected_paths = controls
                elif operation == "uninstall":
                    writable_paths = (home, runtime, workload_output)
                    protected_paths = controls
                else:
                    writable_paths = (runtime, logs, workload_output)
                    protected_paths = before_protected
                before_snapshot = _lifecycle_protected_snapshot(frozen)
                result = _lifecycle_candidate_completed(
                    argv, cwd=workspace, timeout=30, env=environment,
                    writable=writable_paths,
                    protected=protected_paths,
                )
                receipt = {"argv": list(argv), "exit_code": result.returncode, "stdout_sha256": sha256_bytes(result.stdout), "stderr_sha256": sha256_bytes(result.stderr),
                           "protected_before": before_snapshot}
                receipts.append(receipt)
                if result.returncode != 0:
                    fail(f"lifecycle command failed: {' '.join(argv[1:3])}")
                if operation == "install-qualification-wrapper":
                    staged, installed, metadata_data = _validate_lifecycle_install(plist, metadata, support, wrapper, logs / "podwayd.log")
                    frozen.update({"plist": installed["plist"], "staged_wrapper": installed["staged_wrapper"], "metadata": installed["metadata"]})
                    idempotent = _lifecycle_candidate_completed(
                        argv, cwd=workspace, timeout=30, env=environment,
                        writable=writable_paths, protected=protected_paths,
                    )
                    if idempotent.returncode != 0:
                        fail("lifecycle idempotent install command failed")
                    staged, installed, metadata_data = _validate_lifecycle_install(plist, metadata, support, wrapper, logs / "podwayd.log")
                    frozen.update({"plist": installed["plist"], "staged_wrapper": installed["staged_wrapper"], "metadata": installed["metadata"]})
                    state = _launchctl_bound_state(uid, environment, plist, staged)
                    if state is None:
                        fail("lifecycle install did not bootstrap the exact running staged binding")
                    idempotent_receipt = {
                        "argv": list(argv), "exit_code": idempotent.returncode,
                        "stdout_sha256": sha256_bytes(idempotent.stdout),
                        "stderr_sha256": sha256_bytes(idempotent.stderr),
                        "launchctl": state, "running": True, "pid": state["pid"],
                        "runtime_socket": _lifecycle_runtime_socket(runtime, uid),
                    }
                    service_probe = _assert_lifecycle_policy_denies_mutations(
                        bounded_bytes(profile_path).decode("utf-8", "strict"), require_candidate_root(), staged, frozen, "staged_service",
                    )
                    command_inputs = _lifecycle_policy_inputs(
                        (runtime, logs, workload_output), controls + (plist, metadata, staged),
                    )
                    command_policy = _lifecycle_command_policy(
                        tuple(Path(path) for path in command_inputs["writable"]),
                        tuple(Path(path) for path in command_inputs["protected"]),
                    )
                    command_probe = _assert_lifecycle_policy_denies_mutations(
                        command_policy, require_candidate_root(), staged, frozen, "command_policy",
                    )
                    command_probe["inputs"] = command_inputs
                    policy_probe = {"service_profile": service_probe, "command_policy": command_probe}
                    relaunch = _lifecycle_unexpected_exit_relaunch(uid, environment, plist, staged, runtime)
                    receipt.update({
                        **installed, "program_arguments": [str(staged), "--service"], "launchctl": state,
                        "running": True, "staged_path": str(staged), "pid": state["pid"],
                        "runtime_socket": _lifecycle_runtime_socket(runtime, uid),
                        "login_load": {"plist_run_at_load": "true", "bootstrap_target": state["target"], "bootstrap_pid": state["pid"]},
                        "idempotent_install": idempotent_receipt,
                        "unexpected_exit_relaunch": relaunch,
                    })
                if operation == "stop":
                    receipt.update({"launchctl_path": str(LAUNCHCTL), "absence": _launchctl_absent(uid, environment),
                                    "keepalive_defeated": _lifecycle_stop_defeats_keepalive(uid, environment)})
                if operation in {"start", "restart"}:
                    for role, identity in frozen.items():
                        observed = _lifecycle_canonical_identity(Path(identity["path"]), role, expected_mode=identity["mode"])
                        if observed["sha256"] != identity["sha256"]:
                            fail("lifecycle protected artifact changed by candidate command")
                    state = _launchctl_bound_state(uid, environment, plist, staged)
                    if state is None:
                        fail("lifecycle did not prove an actual running staged binding")
                    receipt.update({"launchctl": state, "running": True, "staged_path": str(staged), "pid": state["pid"], "runtime_socket": _lifecycle_runtime_socket(runtime, uid)})
                if operation == "uninstall":
                    absence = _launchctl_absent(uid, environment)
                    receipt.update({"absence": absence, "worktree_state": {
                        "marker_path": str(marker), "marker_sha256": sha256_file(marker),
                        "marker_bytes": marker.stat().st_size,
                    }})
                    if any(path is not None and (path.exists() or path.is_symlink()) for path in (plist, metadata, staged)):
                        fail("lifecycle uninstall left verified service artifacts")
                    for role in ("plist", "metadata", "staged_wrapper"):
                        frozen.pop(role)
                receipt["protected_after"] = _lifecycle_protected_snapshot(frozen)
            _verify_materialized_candidate_source(workspace, source_files, source_manifest, {marker})
            if plist.exists(): fail("lifecycle uninstall left plist bytes behind")
            if not marker.is_file() or marker.read_text(encoding="utf-8") != "preserve\n": fail("lifecycle did not preserve isolated source materialization")
        except BaseException as exc:
            primary_error = exc
        finally:
            try:
                cleanup_before = _lifecycle_protected_snapshot(frozen)
                cleanup = _lifecycle_candidate_completed((str(podway), "daemon", "uninstall"), cwd=workspace, timeout=30, env=environment,
                                                         writable=(home, runtime, workload_output), protected=controls + ((staged,) if staged else ()))
                if cleanup.returncode != 0:
                    fail("candidate cleanup uninstall failed")
                absence = _launchctl_absent(uid, environment)
                cleanup_after = _lifecycle_protected_snapshot(frozen)
                receipts.append({"argv": [str(podway), "daemon", "uninstall"], "exit_code": cleanup.returncode, "stdout_sha256": sha256_bytes(cleanup.stdout), "stderr_sha256": sha256_bytes(cleanup.stderr), "candidate_uninstall": True, "absence": absence,
                                 "protected_before": cleanup_before, "protected_after": cleanup_after})
            except BaseException as exc:
                cleanup_errors.append(f"candidate uninstall: {exc}")
            try:
                cleanup_receipt = _controller_lifecycle_cleanup(uid, environment, plist, metadata, staged)
                for role in ("controller_wrapper", "controller_profile", "extracted_archived_daemon", "extracted_archived_cli"):
                    identity = frozen[role]
                    observed = _lifecycle_file_identity(Path(identity["path"]), role, expected_mode=identity["mode"])
                    if observed["sha256"] != identity["sha256"]:
                        fail("lifecycle control artifact changed during cleanup")
                _verify_materialized_candidate_source(workspace, source_files, source_manifest, {marker})
                holder.cleanup()
            except BaseException as exc:
                cleanup_errors.append(f"controller cleanup: {exc}")
        if primary_error is not None:
            if cleanup_errors: raise QualificationError("lifecycle primary failure: " + str(primary_error) + "; cleanup failures: " + "; ".join(cleanup_errors)) from primary_error
            raise primary_error
        if cleanup_errors: fail("lifecycle cleanup failed: " + "; ".join(cleanup_errors))
        out, digest = _rc_evidence(
            Path(args.rc), "G009-GATE-LIFECYCLE", archive_sha256=sha256_file(archive),
            archive_members_sha256=sha256_bytes(canonical_json(inspected["members"])),
            binaries={"podway": sha256_file(podway), "podwayd": sha256_file(podwayd)},
            home_isolated=True, source_materialization_preserved=True,
            lifecycle_identity=lifecycle_identity, commands=receipts,
            lifecycle_sandbox={"controller_identities": frozen, "program_arguments": [str(staged), "--service"],
                               "qualification_install": qualification_install,
                               "policy_probe": policy_probe, "cleanup": cleanup_receipt,
                               "publication": {"staged_path": str(staged), "daemon_identity": sha256_file(wrapper),
                                               "metadata": metadata_data, "metadata_identity": installed["metadata"],
                                               "log_path": str(logs / "podwayd.log"),
                                               "launchctl_path": str(LAUNCHCTL)}},
        )
        print(f"{out} {digest}")


def _upstream_gate_ids(trace: dict[str, Any]) -> list[str]:
    if trace.get("schema") != "podway.g009.traceability/v1" or not isinstance(trace.get("rows"), list):
        fail("invalid traceability")
    expected: list[str] = []
    acceptance_ids: list[str] = []
    for row in trace["rows"]:
        if not isinstance(row, dict):
            fail("malformed traceability row")
        gate = row.get("executable_gate")
        row_id = row.get("id")
        if isinstance(row_id, str) and row_id.startswith("ACC-"):
            if row.get("exception_eligible") is not False or row_id in acceptance_ids:
                fail("acceptance exception semantics are not exact")
            acceptance_ids.append(row_id)
        if isinstance(row.get("contract_id"), str) and isinstance(gate, str) and gate != "G009-GATE-FINAL-001":
            if gate in expected:
                fail("upstream traceability gate is duplicated")
            expected.append(gate)
    if not expected or acceptance_ids != [f"ACC-{number:02d}" for number in range(1, 12)]:
        fail("traceability has incomplete upstream acceptance contracts")
    return expected


def acceptance_index(args: argparse.Namespace) -> None:
    rc_path, trace_path, root = Path(args.rc), Path(args.traceability), Path(args.evidence_root)
    rc = verify_rc_consumption(rc_path)
    if not root.resolve().is_relative_to(EVIDENCE_ROOT.resolve()):
        fail("evidence root escapes artifacts/g009")
    expected = _upstream_gate_ids(load_json(trace_path))
    run_identity = os.environ.get("G009_QUALIFICATION_RUN_ID")
    if not isinstance(run_identity, str) or not re.fullmatch(r"[0-9a-f]{64}", run_identity):
        fail("G009_QUALIFICATION_RUN_ID must bind acceptance index")
    found: dict[str, dict[str, Any]] = {}
    for raw in args.checkpoint:
        artifact = Path(raw).resolve()
        if not artifact.is_relative_to(root.resolve()) or not artifact.is_file() or artifact.is_symlink():
            fail("checkpoint artifact is unsafe or outside evidence root")
        payload = load_json(artifact)
        if (
            not isinstance(payload, dict) or payload.get("status") != "pass"
            or payload.get("run_identity") != run_identity
            or payload.get("rc_sha256") != sha256_file(rc_path) or payload.get("source") != rc["source"]
            or payload.get("target") != rc["target"] or payload.get("blockers") != []
        ):
            fail("checkpoint artifact is not a current pass envelope")
        gates = [payload.get("checkpoint_id")]
        if payload.get("checkpoint_id") == "G009-GATE-GATES":
            results = payload.get("results")
            if not isinstance(results, list) or not results:
                fail("gate checkpoint has no results")
            gates = [item.get("gate_id") for item in results if isinstance(item, dict) and item.get("status") == "pass"]
            if len(gates) != len(results):
                fail("gate checkpoint results are incomplete")
        for gate in gates:
            if not isinstance(gate, str) or gate in found:
                fail("checkpoint gate is missing or duplicated")
            found[gate] = {"gate_id": gate, "path": str(artifact.relative_to(root.resolve())), "sha256": sha256_file(artifact),
                           "rc_sha256": sha256_file(rc_path), "target": rc["target"], "source": rc["source"], "blockers": []}
    if list(found) != expected:
        fail("checkpoint artifacts do not exactly match upstream traceability gate order")
    out, digest = _rc_evidence(rc_path, "G009-GATE-ACCEPTANCE-INDEX",
                                traceability_sha256=sha256_file(trace_path), upstream_gate_ids=expected,
                                acceptance_ids=[f"ACC-{number:02d}" for number in range(1, 12)],
                                evidence=[found[gate] for gate in expected])
    print(f"{out} {digest}")


def _review_evidence(index: Path, rc_digest: str, source: dict[str, Any], target: str) -> list[dict[str, Any]]:
    value = load_json(index)
    if not isinstance(value, dict) or value.get("rc_sha256") != rc_digest or value.get("target") != target or value.get("source") != source or value.get("status") != "pass" or value.get("blockers") != []:
        fail("acceptance index is stale or mismatched")
    evidence_rows = value.get("evidence")
    if not isinstance(evidence_rows, list) or not evidence_rows:
        fail("acceptance index has no evidence")
    return evidence_rows
def _parse_roles(values: list[str], option: str) -> dict[str, str]:
    parsed: dict[str, str] = {}
    for value in values:
        role, separator, binding = value.partition("=")
        if not separator or role in parsed:
            fail(f"{option} must bind each role exactly once")
        parsed[role] = binding
    if list(parsed) != ["owner", "E", "F"]:
        fail(f"{option} requires ordered owner/E/F bindings")
    return parsed

def _safe_file(path: Path, root: Path, label: str) -> Path:
    if path.is_symlink():
        fail(f"{label} is a symlink")
    resolved = path.resolve(strict=True)
    if not resolved.is_relative_to(root.resolve()) or not resolved.is_file():
        fail(f"{label} escapes its trusted root")
    return resolved
def _validate_qualification_source(source: Any) -> dict[str, Any]:
    if (
        not isinstance(source, dict)
        or set(source) != {"commit", "tree", "tools"}
        or not all(isinstance(source.get(key), str) and re.fullmatch(r"[0-9a-f]{40}", source[key]) for key in ("commit", "tree"))
        or not isinstance(source.get("tools"), list)
        or len(source["tools"]) != 2
        or [item.get("id") if isinstance(item, dict) else None for item in source["tools"]] != ["cargo", "rustc"]
        or any(
            not isinstance(item, dict)
            or set(item) != {"id", "version", "path_sha256"}
            or not isinstance(item["version"], str)
            or not item["version"]
            or not isinstance(item["path_sha256"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", item["path_sha256"])
            for item in source["tools"]
        )
    ):
        fail("qualification source provenance is malformed")
    return source

def _qualification_descriptor(path: Path, data: bytes | None = None) -> dict[str, Any]:
    descriptor = load_json_bytes(data, "qualification bundle descriptor") if data is not None else load_json(path)
    required = {
        "schema",
        "qualification_archive_sha256",
        "acceptance_index_sha256",
        "rc_sha256",
        "traceability_sha256",
        "release_policy_sha256",
        "tool_manifest_sha256",
        "source",
        "target",
        "target_tuple",
    }
    if not isinstance(descriptor, dict) or set(descriptor) != required or descriptor["schema"] != "podway.g009.qualification-bundle/v1":
        fail("qualification bundle descriptor is malformed")
    digest_fields = (
        "qualification_archive_sha256",
        "acceptance_index_sha256",
        "rc_sha256",
        "traceability_sha256",
        "release_policy_sha256",
        "tool_manifest_sha256",
    )
    target = descriptor.get("target")
    if (
        not isinstance(target, str)
        or descriptor.get("target_tuple") != target_tuple(target)
        or not all(isinstance(descriptor[key], str) and re.fullmatch(r"[0-9a-f]{64}", descriptor[key]) for key in digest_fields)
    ):
        fail("qualification bundle descriptor has invalid identities")
    if sha256_file(CONTROLLER_ROOT / "release/g009-release-policy-v1.json") != descriptor["release_policy_sha256"]:
        fail("qualification release policy differs from the trusted controller")
    _validate_qualification_source(descriptor["source"])
    return descriptor
def _validate_final_archive_binding(
    archive_bytes: bytes, descriptor: dict[str, Any], final: dict[str, Any], strict: dict[str, Any],
) -> str:
    digest = sha256_bytes(archive_bytes)
    if any(value.get("qualification_archive_sha256") != digest for value in (descriptor, final, strict)):
        fail("qualification archive bytes do not bind every final-bundle authority")
    return digest


def _verify_review_inputs(args: argparse.Namespace) -> tuple[dict[str, Any], dict[str, str], dict[str, tuple[Path, Path]]]:
    descriptor_path = Path(args.qualification_bundle)
    descriptor = _qualification_descriptor(descriptor_path)
    root = descriptor_path.parent
    archive = _safe_file(root / "qualification-bundle.zip", root, "qualification archive")
    index = _safe_file(root / "acceptance-index.json", root, "acceptance index")
    archive_bytes = bounded_bytes(archive)
    if sha256_bytes(archive_bytes) != descriptor["qualification_archive_sha256"] or sha256_file(index) != descriptor["acceptance_index_sha256"]:
        fail("qualification bundle members do not bind descriptor")
    index_value = load_json(index)
    evidence_rows = index_value.get("evidence") if isinstance(index_value, dict) else None
    if (
        not isinstance(index_value, dict)
        or index_value.get("rc_sha256") != descriptor["rc_sha256"]
        or index_value.get("target") != descriptor["target"]
        or index_value.get("status") != "pass"
        or index_value.get("blockers") != []
        or not isinstance(evidence_rows, list)
        or not evidence_rows
    ):
        fail("qualification acceptance index is stale or malformed")
    policy_gate_ids = load_json(CONTROLLER_ROOT / "release/g009-release-policy-v1.json").get("acceptance_index", {}).get("required_upstream_gate_ids")
    if (
        not isinstance(policy_gate_ids, list)
        or index_value.get("checkpoint_id") != "G009-GATE-ACCEPTANCE-INDEX"
        or index_value.get("upstream_gate_ids") != policy_gate_ids
        or index_value.get("acceptance_ids") != [f"ACC-{number:02d}" for number in range(1, 12)]
        or len(evidence_rows) != len(policy_gate_ids)
        or [row.get("gate_id") for row in evidence_rows if isinstance(row, dict)] != policy_gate_ids
    ):
        fail("qualification acceptance index does not reconstruct the exact gate contract")
    expected_names = {
        "manifest.json",
        "rc.json",
        "traceability.json",
        "release-policy.json",
        "acceptance-index.json",
        "archive.zip",
        "archive.zip.sha256",
        "tool-manifest.json",
        "receipt.json",
    }
    for row in evidence_rows:
        relative = row.get("path") if isinstance(row, dict) else None
        digest = row.get("sha256") if isinstance(row, dict) else None
        if (
            not isinstance(row, dict)
            or set(row) != {"gate_id", "path", "sha256", "rc_sha256", "target", "source", "blockers"}
            or row["rc_sha256"] != descriptor["rc_sha256"]
            or row["target"] != descriptor["target"]
            or _public_source(row["source"]) != descriptor["source"]
            or row["blockers"] != []
            or not isinstance(relative, str)
            or Path(relative).is_absolute()
            or ".." in Path(relative).parts
            or not isinstance(digest, str)
            or not re.fullmatch(r"[0-9a-f]{64}", digest)
        ):
            fail("qualification evidence envelope binding differs")
        expected_names.add(f"evidence/{relative}")
    try:
        with zipfile.ZipFile(io.BytesIO(archive_bytes)) as bundle:
            _preflight_qualification_bundle(bundle)
            names = bundle.namelist()
            if len(names) != len(set(names)):
                fail("qualification bundle has duplicate members")
            fuzz_identities: set[str] = set()
            for row in evidence_rows:
                raw = bundle.read(f"evidence/{row['path']}")
                payload = load_json_bytes(raw, f"evidence/{row['path']}")
                if row["gate_id"] != "G009-GATE-FUZZ":
                    continue
                results = payload.get("results") if isinstance(payload, dict) else None
                matched = [
                    result for result in results if isinstance(result, dict)
                    and result.get("gate_id") == "G009-GATE-FUZZ"
                ] if isinstance(results, list) else []
                if len(matched) != 1:
                    fail("qualification fuzz result is missing or duplicated")
                for result in matched:
                    if not isinstance(result, dict):
                        fail("qualification fuzz result is malformed")
                    for command in result.get("commands", []):
                        receipt_ref = command.get("receipt") if isinstance(command, dict) else None
                        receipt_name = receipt_ref.get("path") if isinstance(receipt_ref, dict) else None
                        if not isinstance(receipt_name, str):
                            fail("qualification fuzz receipt reference is malformed")
                        receipt = load_json_bytes(bundle.read(f"evidence/{receipt_name}"), receipt_name)
                        identity = receipt.get("run_identity") if isinstance(receipt, dict) else None
                        if not isinstance(identity, str) or not re.fullmatch(r"[0-9a-f]{64}", identity):
                            fail("qualification fuzz run identity is malformed")
                        fuzz_identities.add(identity)
                        for stream_name in ("stdout", "stderr"):
                            stream = receipt.get(stream_name) if isinstance(receipt, dict) else None
                            if not isinstance(stream, dict) or not isinstance(stream.get("path"), str):
                                fail("qualification fuzz blob reference is malformed")
                            expected_names.add(stream["path"])
                        manifests = receipt.get("corpus_manifests") if isinstance(receipt, dict) else None
                        corpus = receipt.get("corpus") if isinstance(receipt, dict) else None
                        if not isinstance(manifests, dict) or not isinstance(corpus, str):
                            fail("qualification fuzz corpus reference is malformed")
                        for phase in ("before", "after"):
                            binding = manifests.get(phase)
                            manifest_name = binding.get("path") if isinstance(binding, dict) else None
                            if not isinstance(manifest_name, str):
                                fail("qualification fuzz manifest reference is malformed")
                            expected_names.add(manifest_name)
                            manifest = load_json_bytes(bundle.read(manifest_name), manifest_name)
                            if not isinstance(manifest, dict) or manifest.get("run_identity") != identity:
                                fail("qualification fuzz manifest run identity differs from receipt")
                            files = manifest.get("files") if isinstance(manifest, dict) else None
                            if not isinstance(files, list):
                                fail("qualification fuzz manifest file set is malformed")
                            for member in files:
                                relative = member.get("path") if isinstance(member, dict) else None
                                if not isinstance(relative, str):
                                    fail("qualification fuzz corpus member is malformed")
                                expected_names.add(f"fuzz-corpus/{corpus}/{relative}")
                        provenance = receipt.get("provenance") if isinstance(receipt, dict) else None
                        sources = provenance.get("sources") if isinstance(provenance, dict) else None
                        if not isinstance(sources, list):
                            fail("qualification fuzz source references are malformed")
                        for source in sources:
                            root_name = source.get("root") if isinstance(source, dict) else None
                            relative = source.get("path") if isinstance(source, dict) else None
                            if root_name not in {"candidate", "controller"} or not isinstance(relative, str):
                                fail("qualification fuzz source reference is malformed")
                            expected_names.add(f"fuzz-sources/{root_name}/{safe_relative(relative).as_posix()}")
            if fuzz_identities and len(fuzz_identities) != 1:
                fail("qualification fuzz receipts/manifests do not share one run identity")
            if len(names) != len(set(names)) or set(names) != expected_names:
                fail("qualification bundle membership is not the exact acceptance set")
            manifest = load_json_bytes(bundle.read("manifest.json"), "qualification bundle manifest")
            listed = manifest.get("members") if isinstance(manifest, dict) else None
            if (
                not isinstance(manifest, dict)
                or set(manifest) != {"schema", "members"}
                or manifest.get("schema") != "podway.g009.bundle-manifest/v1"
                or not isinstance(listed, list)
                or listed != [
                    {"path": member, "size": len(bundle.read(member)), "sha256": sha256_bytes(bundle.read(member))}
                    for member in sorted(set(names) - {"manifest.json"})
                ]
            ):
                fail("qualification bundle manifest is not the exact immutable member set")
            for row in evidence_rows:
                raw_evidence = bundle.read(f"evidence/{row['path']}")
                if sha256_bytes(raw_evidence) != row["sha256"]:
                    fail("qualification evidence member digest differs")
                payload = load_json_bytes(raw_evidence, f"evidence/{row['path']}")
                if (
                    not isinstance(payload, dict)
                    or payload.get("status") != "pass"
                    or payload.get("rc_sha256") != descriptor["rc_sha256"]
                    or payload.get("target") != descriptor["target"]
                    or payload.get("source") != row["source"]
                    or payload.get("blockers") != []
                ):
                    fail("qualification evidence payload is not a current pass envelope")
                if row["gate_id"] in GATES:
                    results = payload.get("results")
                    if not isinstance(results, list):
                        fail("qualification aggregate gate evidence is incomplete")
                    matched = [
                        item for item in results
                        if isinstance(item, dict)
                        and item.get("gate_id") == row["gate_id"]
                        and item.get("status") == "pass"
                    ]
                    if payload.get("checkpoint_id") != "G009-GATE-GATES" or len(matched) != 1:
                        fail("qualification aggregate gate evidence is incomplete")
                elif payload.get("checkpoint_id") != row["gate_id"]:
                    fail("qualification checkpoint identity differs")
            if sha256_bytes(bundle.read("tool-manifest.json")) != descriptor["tool_manifest_sha256"]:
                fail("qualification tool manifest digest differs")
            if sha256_bytes(bundle.read("release-policy.json")) != descriptor["release_policy_sha256"]:
                fail("qualification release policy digest differs")
            tool_manifest = load_json_bytes(bundle.read("tool-manifest.json"), "qualification tool manifest")
            expected_tool_ids = {
                "bash", "cargo", "cargo-audit", "cargo-deny", "cargo-fuzz",
                "cargo-llvm-cov", "cargo-nightly", "git", "gpgv", "launchctl",
                "lipo", "ps", "python3", "rustc", "rustc-nightly", "rustup",
                "sandbox-exec", "sysctl",
            }
            tools = tool_manifest.get("tools") if isinstance(tool_manifest, dict) else None
            controller_sources = tool_manifest.get("controller_sources") if isinstance(tool_manifest, dict) else None
            if (
                not isinstance(tool_manifest, dict)
                or set(tool_manifest) != {"schema", "source", "tools", "controller_sources"}
                or tool_manifest.get("schema") != "podway.g009.release-tool-manifest/v1"
                or tool_manifest.get("source") != descriptor["source"]
                or not isinstance(tools, list)
                or {item.get("id") for item in tools if isinstance(item, dict)} != expected_tool_ids
                or len(tools) != len(expected_tool_ids)
                or any(
                    not isinstance(item, dict)
                    or set(item) != {"id", "version", "path_sha256", "architecture"}
                    or item.get("architecture") != target_tuple(descriptor["target"])["mach_o_arch"]
                    or not isinstance(item.get("version"), str)
                    or not item["version"]
                    or not isinstance(item.get("path_sha256"), str)
                    or not re.fullmatch(r"[0-9a-f]{64}", item["path_sha256"])
                    for item in tools
                )
                or controller_sources != controller_source_bindings()
            ):
                fail("qualification tool manifest schema or exact set differs")
            policy_value = load_json_bytes(bundle.read("release-policy.json"), "qualification release policy")
            if not isinstance(policy_value, dict) or policy_value.get("schema") != "podway.g009.release-policy/v1":
                fail("qualification release policy schema differs")
    except (KeyError, zipfile.BadZipFile) as exc:
        fail(f"qualification bundle archive is invalid: {exc}")
    keyring = _safe_file(Path(args.reviewer_keyring), Path(args.reviewer_keyring).parent, "reviewer keyring")
    if not re.fullmatch(r"[0-9a-f]{64}", args.reviewer_keyring_sha256) or sha256_file(keyring) != args.reviewer_keyring_sha256:
        fail("reviewer keyring digest binding failed")
    fingerprints = _parse_roles(args.reviewer_fingerprint, "--reviewer-fingerprint")
    if any(not re.fullmatch(r"[0-9A-F]{40}", value) for value in fingerprints.values()) or len(set(fingerprints.values())) != 3:
        fail("reviewer fingerprints must be distinct uppercase 40-hex values")
    raw_attestations = _parse_roles(args.attestation, "--attestation")
    attestations: dict[str, tuple[Path, Path]] = {}
    statement = canonical_json({
        "qualification_archive_sha256": descriptor["qualification_archive_sha256"],
        "acceptance_index_sha256": descriptor["acceptance_index_sha256"],
        "rc_sha256": descriptor["rc_sha256"],
        "traceability_sha256": descriptor["traceability_sha256"],
        "release_policy_sha256": descriptor["release_policy_sha256"],
        "tool_manifest_sha256": descriptor["tool_manifest_sha256"],
    })
    for role, value in raw_attestations.items():
        payload_raw, separator, signature_raw = value.partition("=")
        if not separator:
            fail("--attestation must be ROLE=PAYLOAD=SIGNATURE")
        payload = _safe_file(Path(payload_raw), Path(payload_raw).parent, f"{role} payload")
        signature = _safe_file(Path(signature_raw), Path(signature_raw).parent, f"{role} signature")
        if bounded_bytes(payload) != statement:
            fail("reviewer attestation is not an exact bundle-bound statement")
        attestations[role] = (payload, signature)
    verify_role_signatures(
        [{"role": role, "payload": payload, "signature": signature} for role, (payload, signature) in attestations.items()],
        [{"role": role, "fingerprint": fingerprint} for role, fingerprint in fingerprints.items()],
        keyring,
    )
    return descriptor, fingerprints, attestations

def final_review(args: argparse.Namespace) -> None:
    descriptor, fingerprints, attestations = _verify_review_inputs(args)
    review = {
        "schema": "podway.g009.final-review/v2", "status": "passed",
        "qualification_bundle_sha256": sha256_file(Path(args.qualification_bundle)),
        "qualification_archive_sha256": descriptor["qualification_archive_sha256"],
        "acceptance_index_sha256": descriptor["acceptance_index_sha256"],
        "rc_sha256": descriptor["rc_sha256"],
        "traceability_sha256": descriptor["traceability_sha256"],
        "release_policy_sha256": descriptor["release_policy_sha256"],
        "tool_manifest_sha256": descriptor["tool_manifest_sha256"],
        "source": descriptor["source"],
        "target": descriptor["target"],
        "target_tuple": descriptor["target_tuple"],
        "reviewers": ["owner", "E", "F"], "reviewer_keyring_sha256": args.reviewer_keyring_sha256,
        "attestations": [{"role": role, "fingerprint": fingerprints[role],
                          "payload_sha256": sha256_file(attestations[role][0]),
                          "signature_sha256": sha256_file(attestations[role][1])}
                         for role in ("owner", "E", "F")],
        "blockers": [],
    }
    digest = sha256_bytes(canonical_json(review))
    output = EVIDENCE_ROOT / "final-review" / f"{digest}.json"
    atomic_immutable_json(output, review)
    print(f"{output} {digest}")

def _bundle_member(path: Path, root: Path, label: str) -> tuple[str, bytes]:
    resolved = _safe_file(path, root, f"bundle member {label}")
    return label, bounded_bytes(resolved)
def _write_immutable_file(path: Path, data: bytes, root: Path) -> None:
    if path.is_symlink() or not path.parent.resolve().is_relative_to(root.resolve()):
        fail("immutable output path is unsafe")
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        if not path.is_file() or bounded_bytes(path) != data:
            fail("immutable output already differs")
        return
    descriptor, temporary_name = tempfile.mkstemp(prefix=".g009-output-", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o444)
        try:
            os.link(temporary, path)
        except FileExistsError:
            if path.is_symlink() or not path.is_file() or bounded_bytes(path) != data:
                fail("immutable output race differs")
    finally:
        temporary.unlink(missing_ok=True)



def _write_bundle(output: Path, members: dict[str, bytes]) -> tuple[Path, str]:
    if output.is_symlink() or not output.parent.resolve().is_relative_to(EVIDENCE_ROOT.resolve()):
        fail("bundle output must be under controller evidence root")
    if sum(len(data) for data in members.values()) > FUZZ_ARCHIVE_MATERIALIZATION_MAX_BYTES:
        fail("bundle materialization exceeds frozen archive bound")
    manifest = {"schema": "podway.g009.bundle-manifest/v1", "members": [
        {"path": name, "size": len(data), "sha256": sha256_bytes(data)}
        for name, data in sorted(members.items())
    ]}
    payloads = dict(members); payloads["manifest.json"] = canonical_json(manifest)
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=".g009-bundle-", suffix=".zip", dir=output.parent)
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED, strict_timestamps=True) as archive:
            for name, data in sorted(payloads.items()):
                safe_relative(name)
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.external_attr = 0o100644 << 16
                archive.writestr(info, data)
        _write_immutable_file(output, bounded_bytes(temporary, FUZZ_ARCHIVE_MATERIALIZATION_MAX_BYTES), EVIDENCE_ROOT)
    finally:
        temporary.unlink(missing_ok=True)
    return output.resolve(), sha256_file(output)

def _bundle_fuzz_dependencies(
    members: dict[str, bytes], evidence_root: Path, target: str,
) -> None:
    """Freeze every fuzz dependency with the aggregate fuzz gate that references it."""
    candidate = require_candidate_root()
    materialized = 0
    maximum = _frozen_profile_for_target(target)["fuzz"]["runner"]["fuzz_dependency_materialization_bytes"]

    def add(name: str, data: bytes) -> None:
        nonlocal materialized
        if name in members and members[name] != data:
            fail("fuzz dependency path has contradictory bytes")
        materialized += 0 if name in members else len(data)
        if materialized > maximum:
            fail("fuzz dependency materialization exceeds frozen bound")
        members[name] = data
    for name, payload_bytes in list(members.items()):
        if not name.startswith("evidence/"):
            continue
        payload = load_json_bytes(payload_bytes, name)
        results = payload.get("results") if isinstance(payload, dict) else None
        matched = [
            result for result in results if isinstance(result, dict)
            and result.get("gate_id") == "G009-GATE-FUZZ"
        ] if isinstance(results, list) else []
        if not matched:
            continue
        if len(matched) != 1 or not isinstance(matched[0].get("commands"), list):
            fail("aggregate fuzz evidence is malformed")
        from verify_g009_qualification import validate_fuzz_gate
        validate_fuzz_gate(matched[0], evidence_root)
        for command in matched[0]["commands"]:
            binding = command.get("receipt") if isinstance(command, dict) else None
            receipt_name = binding.get("path") if isinstance(binding, dict) else None
            receipt_digest = binding.get("sha256") if isinstance(binding, dict) else None
            if not isinstance(receipt_name, str) or not isinstance(receipt_digest, str):
                fail("aggregate fuzz receipt reference is malformed")
            receipt_bytes = bounded_bytes(_safe_file(evidence_root / receipt_name, evidence_root, receipt_name), FUZZ_MANIFEST_MAX_BYTES)
            if sha256_bytes(receipt_bytes) != receipt_digest:
                fail("aggregate fuzz receipt digest differs")
            add(f"evidence/{receipt_name}", receipt_bytes)
    for name, receipt_bytes in list(members.items()):
        if not name.startswith("evidence/fuzz-receipts/"):
            continue
        receipt = load_json_bytes(receipt_bytes, name)
        if not isinstance(receipt, dict) or receipt.get("schema") != "podway.g009.fuzz-receipt/v2":
            fail("fuzz receipt selected for bundling is malformed")
        corpus = receipt.get("corpus")
        if not isinstance(corpus, str):
            fail("fuzz receipt corpus binding is malformed")
        corpus_path = qualification_scratch_root() / "fuzz" / "corpus" / Path(corpus).name
        for stream_name in ("stdout", "stderr"):
            stream = receipt.get(stream_name)
            if not isinstance(stream, dict) or not isinstance(stream.get("path"), str):
                fail("fuzz receipt blob reference is malformed")
            label = stream["path"]
            add(label, bounded_bytes(_safe_file(evidence_root / label, evidence_root, label), receipt["limits"]["stream_bytes"]))
        manifests = receipt.get("corpus_manifests")
        if not isinstance(manifests, dict) or set(manifests) != {"before", "after"}:
            fail("fuzz receipt corpus manifests are malformed")
        for phase in ("before", "after"):
            binding = manifests[phase]
            if not isinstance(binding, dict) or not isinstance(binding.get("path"), str):
                fail("fuzz receipt corpus manifest reference is malformed")
            manifest_name = binding["path"]
            manifest_bytes = bounded_bytes(_safe_file(evidence_root / manifest_name, evidence_root, manifest_name), receipt["limits"]["manifest_bytes"])
            if sha256_bytes(manifest_bytes) != binding.get("sha256"):
                fail("fuzz receipt corpus manifest digest differs")
            add(manifest_name, manifest_bytes)
            manifest = load_json_bytes(manifest_bytes, manifest_name)
            files = manifest.get("files") if isinstance(manifest, dict) else None
            if not isinstance(files, list):
                fail("fuzz corpus manifest file set is malformed")
            for member in files:
                if not isinstance(member, dict) or not isinstance(member.get("path"), str):
                    fail("fuzz corpus member is malformed")
                relative = safe_relative(member["path"])
                source = _safe_file(corpus_path / relative, corpus_path, "fuzz corpus member")
                data = bounded_bytes(source, receipt["limits"]["corpus_member_bytes"])
                if len(data) != member.get("bytes") or sha256_bytes(data) != member.get("sha256"):
                    fail("fuzz corpus member differs from its immutable manifest")
                add(f"fuzz-corpus/{corpus}/{relative.as_posix()}", data)
        provenance = receipt.get("provenance")
        sources = provenance.get("sources") if isinstance(provenance, dict) else None
        if not isinstance(sources, list):
            fail("fuzz receipt source set is malformed")
        for source_binding in sources:
            if (
                not isinstance(source_binding, dict)
                or set(source_binding) != {"root", "path", "sha256"}
                or source_binding.get("root") not in {"candidate", "controller"}
                or not isinstance(source_binding.get("path"), str)
            ):
                fail("fuzz receipt source binding is malformed")
            relative = safe_relative(source_binding["path"])
            source_root = candidate if source_binding["root"] == "candidate" else CONTROLLER_ROOT
            source = _safe_file(source_root / relative, source_root, "fuzz source")
            data = bounded_bytes(source, maximum - materialized)
            if sha256_bytes(data) != source_binding["sha256"]:
                fail("fuzz receipt source digest differs")
            add(f"fuzz-sources/{source_binding['root']}/{relative.as_posix()}", data)


def qualification_bundle(args: argparse.Namespace) -> None:
    rc = verify_rc_consumption(Path(args.rc))
    archive = Path(args.archive)
    report = inspect_archive(archive, target=rc["target"])
    index = Path(args.index)
    traceability = Path(args.traceability)
    policy = CONTROLLER_ROOT / "release/g009-release-policy-v1.json"
    evidence_root = Path(args.evidence_root)
    trusted_traceability = CONTROLLER_ROOT / "release/g009-traceability-v1.json"
    if (
        traceability.is_symlink()
        or not traceability.resolve().is_relative_to(CONTROLLER_ROOT.resolve())
        or sha256_file(traceability) != sha256_file(trusted_traceability)
    ):
        fail("qualification traceability is not the exact trusted semantic registry")
    index_value = load_json(index)
    if (
        not isinstance(index_value, dict)
        or index_value.get("traceability_sha256") != sha256_file(traceability)
    ):
        fail("acceptance index does not bind the bundled traceability registry")
    evidence_rows = _review_evidence(index, sha256_file(Path(args.rc)), rc["source"], rc["target"])
    public_source = _public_source(rc["source"])
    tool_manifest = release_tool_manifest(rc["source"], rc["target"])
    tool_manifest_bytes = canonical_json(tool_manifest)
    members = dict([
        _bundle_member(Path(args.rc), EVIDENCE_ROOT, "rc.json"),
        _bundle_member(traceability, CONTROLLER_ROOT, "traceability.json"),
        _bundle_member(policy, CONTROLLER_ROOT, "release-policy.json"),
        _bundle_member(index, evidence_root, "acceptance-index.json"),
        _bundle_member(archive, archive.parent, "archive.zip"),
        _bundle_member(archive.with_name(archive.name + ".sha256"), archive.parent, "archive.zip.sha256"),
    ])
    members["tool-manifest.json"] = tool_manifest_bytes
    for row in evidence_rows:
        name, data = _bundle_member(evidence_root / row["path"], evidence_root, f"evidence/{row['path']}")
        members[name] = data
    _bundle_fuzz_dependencies(members, evidence_root, rc["target"])
    members["receipt.json"] = canonical_json({
        "schema": "podway.g009.qualification-bundle-receipt/v1",
        "rc_sha256": sha256_file(Path(args.rc)),
        "archive_sha256": report["archive_sha256"],
        "index_sha256": sha256_file(index),
        "traceability_sha256": sha256_file(traceability),
        "release_policy_sha256": sha256_file(policy),
        "tool_manifest_sha256": sha256_bytes(tool_manifest_bytes),
        "source": public_source,
        "target": rc["target"],
        "target_tuple": target_tuple(rc["target"]),
    })
    out = Path(args.out)
    if out.is_symlink() or (out.exists() and not out.is_dir()) or not out.parent.resolve().is_relative_to(CONTROLLER_ROOT.resolve()):
        fail("qualification output directory is unsafe")
    out.mkdir(parents=True, exist_ok=True)
    if not out.resolve().is_relative_to(CONTROLLER_ROOT.resolve()):
        fail("qualification output directory escapes controller root")
    bundle, digest = _write_bundle(EVIDENCE_ROOT / "qualification" / f"{sha256_file(index)}.zip", members)
    for source_path, name in ((bundle, "qualification-bundle.zip"), (index, "acceptance-index.json")):
        destination = out / name
        _write_immutable_file(destination, bounded_bytes(source_path), CONTROLLER_ROOT)
    descriptor = {
        "schema": "podway.g009.qualification-bundle/v1",
        "qualification_archive_sha256": digest,
        "acceptance_index_sha256": sha256_file(index),
        "rc_sha256": sha256_file(Path(args.rc)),
        "traceability_sha256": sha256_file(traceability),
        "release_policy_sha256": sha256_file(policy),
        "tool_manifest_sha256": sha256_bytes(tool_manifest_bytes),
        "source": public_source,
        "target": rc["target"],
        "target_tuple": target_tuple(rc["target"]),
    }
    descriptor_path = out / "qualification-bundle.json"
    _write_immutable_file(descriptor_path, canonical_json(descriptor), CONTROLLER_ROOT)
    print(f"{descriptor_path.resolve()} {sha256_file(descriptor_path)}")

def final_bundle(args: argparse.Namespace) -> None:
    qualification = Path(args.qualification_bundle)
    index = Path(args.index)
    review = Path(args.review)
    traceability = Path(args.traceability)
    qualification = _safe_file(qualification, qualification.parent, "qualification bundle descriptor")
    qualification_bytes = bounded_bytes(qualification)
    descriptor = _qualification_descriptor(qualification, qualification_bytes)
    index = _safe_file(index, index.parent, "acceptance index")
    index_bytes = bounded_bytes(index)
    index_value = load_json_bytes(index_bytes, "acceptance index")
    review = _safe_file(review, review.parent, "final review")
    review_bytes = bounded_bytes(review)
    final = load_json_bytes(review_bytes, "final review")
    traceability = _safe_file(traceability, CONTROLLER_ROOT, "traceability")
    traceability_bytes = bounded_bytes(traceability)
    required_review = {
        "schema",
        "status",
        "qualification_bundle_sha256",
        "qualification_archive_sha256",
        "acceptance_index_sha256",
        "rc_sha256",
        "traceability_sha256",
        "release_policy_sha256",
        "tool_manifest_sha256",
        "source",
        "target",
        "target_tuple",
        "reviewers",
        "reviewer_keyring_sha256",
        "attestations",
        "blockers",
    }
    if (
        not isinstance(final, dict)
        or set(final) != required_review
        or final.get("schema") != "podway.g009.final-review/v2"
        or final.get("status") != "passed"
        or final.get("blockers") != []
        or final.get("reviewers") != ["owner", "E", "F"]
        or final.get("qualification_bundle_sha256") != sha256_bytes(qualification_bytes)
        or any(final.get(field) != descriptor[field] for field in (
            "qualification_archive_sha256",
            "acceptance_index_sha256",
            "rc_sha256",
            "traceability_sha256",
            "release_policy_sha256",
            "tool_manifest_sha256",
            "source",
            "target",
            "target_tuple",
        ))
        or sha256_bytes(index_bytes) != descriptor["acceptance_index_sha256"]
        or sha256_bytes(traceability_bytes) != descriptor["traceability_sha256"]
    ):
        fail("final review or release member binding is not passed and exact")
    supplied_attestations = _parse_roles(args.attestation, "--attestation")
    final_attestations = final.get("attestations")
    if not isinstance(final_attestations, list) or [item.get("role") for item in final_attestations if isinstance(item, dict)] != ["owner", "E", "F"]:
        fail("final review attestation set is incomplete")
    archive = _safe_file(
        qualification.parent / "qualification-bundle.zip",
        qualification.parent,
        "qualification archive",
    )
    qualification_archive = bounded_bytes(archive)
    try:
        with zipfile.ZipFile(io.BytesIO(qualification_archive)) as bundle:
            _preflight_qualification_bundle(bundle)
            policy_bytes = bundle.read("release-policy.json")
            tool_manifest_bytes = bundle.read("tool-manifest.json")
    except (KeyError, zipfile.BadZipFile) as exc:
        fail(f"qualification bundle policy/tool members are invalid: {exc}")
    if (
        sha256_bytes(policy_bytes) != descriptor["release_policy_sha256"]
        or sha256_bytes(tool_manifest_bytes) != descriptor["tool_manifest_sha256"]
    ):
        fail("qualification bundle policy/tool digests differ")
    members = {
        "qualification-bundle.json": qualification_bytes,
        "qualification-bundle.zip": qualification_archive,
        "acceptance-index.json": index_bytes,
        "final-review.json": review_bytes,
        "traceability.json": traceability_bytes,
        "release-policy.json": policy_bytes,
        "tool-manifest.json": tool_manifest_bytes,
    }
    for final_attestation, (role, value) in zip(final_attestations, supplied_attestations.items()):
        payload_raw, separator, signature_raw = value.partition("=")
        if not separator:
            fail("--attestation must be ROLE=PAYLOAD=SIGNATURE")
        payload = _safe_file(Path(payload_raw), Path(payload_raw).parent, f"{role} payload")
        signature = _safe_file(Path(signature_raw), Path(signature_raw).parent, f"{role} signature")
        payload_bytes = bounded_bytes(payload)
        signature_bytes = bounded_bytes(signature)
        if (
            final_attestation.get("role") != role
            or final_attestation.get("payload_sha256") != sha256_bytes(payload_bytes)
            or final_attestation.get("signature_sha256") != sha256_bytes(signature_bytes)
        ):
            fail("final bundle attestation differs from final review")
        members[f"attestations/{role}.payload"] = payload_bytes
        members[f"attestations/{role}.signature"] = signature_bytes
    strict_path = Path(args.strict_verifier_receipt)
    strict_path = _safe_file(strict_path, strict_path.parent, "strict verifier receipt")
    strict_bytes = bounded_bytes(strict_path)
    strict = load_json_bytes(strict_bytes, "strict verifier receipt")
    required_receipt = {
        "schema",
        "status",
        "qualification_bundle_sha256",
        "qualification_archive_sha256",
        "acceptance_index_sha256",
        "rc_sha256",
        "traceability_sha256",
        "release_policy_sha256",
        "tool_manifest_sha256",
        "final_review_sha256",
        "reviewer_keyring_sha256",
        "source",
        "target",
        "target_tuple",
        "attestations",
        "invocation_nonce",
    }
    if (
        not isinstance(strict, dict)
        or set(strict) != required_receipt
        or strict.get("schema") != "podway.g009.strict-verifier-receipt/v1"
        or strict.get("status") != "passed"
        or strict.get("qualification_bundle_sha256") != sha256_bytes(qualification_bytes)
        or strict.get("final_review_sha256") != sha256_bytes(review_bytes)
        or strict.get("reviewer_keyring_sha256") != final["reviewer_keyring_sha256"]
        or strict.get("attestations") != final_attestations
        or not isinstance(strict.get("invocation_nonce"), str)
        or not re.fullmatch(r"[0-9a-f]{64}", strict["invocation_nonce"])
        or strict.get("invocation_nonce") != index_value.get("run_identity")
        or any(strict.get(field) != descriptor[field] for field in (
            "qualification_archive_sha256",
            "acceptance_index_sha256",
            "rc_sha256",
            "traceability_sha256",
            "release_policy_sha256",
            "tool_manifest_sha256",
            "source",
            "target",
            "target_tuple",
        ))
    ):
        fail("strict-verifier receipt is stale or incomplete")
    _validate_final_archive_binding(qualification_archive, descriptor, final, strict)
    members["strict-verifier-receipt.json"] = strict_bytes
    members["receipt.json"] = canonical_json({
        "schema": "podway.g009.final-bundle-receipt/v1",
        "qualification_bundle_sha256": sha256_bytes(qualification_bytes),
        "index_sha256": sha256_bytes(index_bytes),
        "review_sha256": sha256_bytes(review_bytes),
        "traceability_sha256": sha256_bytes(traceability_bytes),
        "release_policy_sha256": descriptor["release_policy_sha256"],
        "tool_manifest_sha256": descriptor["tool_manifest_sha256"],
        "strict_verifier_receipt_sha256": sha256_bytes(strict_bytes),
        "reviewers": final["reviewers"],
        "attestations": final_attestations,
    })
    output, digest = _write_bundle(Path(args.output), members)
    try:
        with zipfile.ZipFile(output) as bundle:
            names = bundle.namelist()
            manifest = load_json_bytes(bundle.read("manifest.json"), "final bundle manifest")
    except (KeyError, zipfile.BadZipFile) as exc:
        fail(f"final bundle output is malformed: {exc}")
    expected_manifest = {"schema": "podway.g009.bundle-manifest/v1", "members": [
        {"path": name, "size": len(data), "sha256": sha256_bytes(data)}
        for name, data in sorted(members.items())
    ]}
    if len(names) != len(set(names)) or set(names) != set(members) | {"manifest.json"} or manifest != expected_manifest:
        fail("final bundle output member set or manifest differs")
    print(f"{output} {digest}")
def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__); sub = p.add_subparsers(dest="command", required=True)
    def command(name: str) -> argparse.ArgumentParser: return sub.add_parser(name)
    x=command("preflight"); x.add_argument("--rc", required=True); x.set_defaults(fn=preflight)
    x=command("characterize"); x.add_argument("--profile", required=True); x.add_argument("--target", required=True); x.add_argument("--warmups", type=int, required=True); x.add_argument("--samples", type=int, required=True); x.add_argument("--bin-dir", required=True); x.set_defaults(fn=characterize)
    x=command("approve-baseline"); x.add_argument("--profile", required=True); x.add_argument("--characterization", required=True); x.add_argument("--roles", required=True); x.add_argument("--approval", action="append"); x.add_argument("--signer-contract", required=True); x.set_defaults(fn=approve_baseline)
    x=command("freeze-rc"); x.add_argument("--profile", required=True); x.add_argument("--baseline", required=True); x.add_argument("--thresholds", required=True); x.add_argument("--characterization", required=True); x.add_argument("--approvals", required=True); x.add_argument("--signer-contract", required=True); x.add_argument("--input", action="append", default=[], metavar="ROLE=PATH"); x.add_argument("--signing-posture", required=True, choices=("unsigned-internal",)); x.set_defaults(fn=freeze_rc)
    x=command("holdout"); x.add_argument("--rc", required=True); x.add_argument("--warmups", type=int, required=True); x.add_argument("--samples", type=int, required=True); x.add_argument("--bin-dir", required=True); x.set_defaults(fn=holdout)
    x=command("full-gates"); x.add_argument("--rc", required=True); x.add_argument("--only"); x.set_defaults(fn=full_gates)
    x=command("local-fuzz"); x.add_argument("--profile", required=True); x.add_argument("--policy-mode", required=True, choices=tuple(sorted(FUZZ_POLICY_MODES))); x.set_defaults(fn=local_fuzz)
    x=command("package"); x.add_argument("--rc", required=True); x.add_argument("--archive", required=True); x.add_argument("--bin-dir", required=True); x.set_defaults(fn=package)
    x=command("lifecycle"); x.add_argument("--rc", required=True); x.add_argument("--archive", required=True); x.add_argument("--require-clean-user", action="store_true"); x.set_defaults(fn=lifecycle)
    x=command("acceptance-index"); x.add_argument("--rc", required=True); x.add_argument("--traceability", required=True); x.add_argument("--evidence-root", required=True); x.add_argument("--checkpoint", action="append", required=True); x.set_defaults(fn=acceptance_index)
    x=command("final-review"); x.add_argument("--qualification-bundle", required=True); x.add_argument("--reviewer-keyring", required=True); x.add_argument("--reviewer-keyring-sha256", required=True); x.add_argument("--reviewer-fingerprint", action="append", required=True); x.add_argument("--attestation", action="append", required=True); x.set_defaults(fn=final_review)
    x=command("qualification-bundle"); x.add_argument("--rc", required=True); x.add_argument("--traceability", required=True); x.add_argument("--index", required=True); x.add_argument("--archive", required=True); x.add_argument("--evidence-root", required=True); x.add_argument("--out", required=True); x.set_defaults(fn=qualification_bundle)
    x=command("final-bundle"); x.add_argument("--qualification-bundle", required=True); x.add_argument("--index", required=True); x.add_argument("--review", required=True); x.add_argument("--traceability", required=True); x.add_argument("--attestation", action="append", required=True); x.add_argument("--strict-verifier-receipt", required=True); x.add_argument("--output", required=True); x.set_defaults(fn=final_bundle)
    return p


def main() -> int:
    try:
        args = parser().parse_args()
        if args.command not in {"final-review", "final-bundle"}:
            require_candidate_root()
            supplied = os.environ.get("G009_QUALIFICATION_RUN_ID")
            if not isinstance(supplied, str) or not re.fullmatch(r"[0-9a-f]{64}", supplied):
                fail("G009_QUALIFICATION_RUN_ID must be a controller-created 64 lowercase hexadecimal invocation nonce")
        args.fn(args)
        return 0
    except QualificationError as exc:
        print(f"G009 qualification failed closed: {exc}", file=sys.stderr)
        return 2
    except (OSError, subprocess.SubprocessError) as exc: print(f"G009 qualification failed closed: {exc}", file=sys.stderr); return 2
if __name__ == "__main__": raise SystemExit(main())
