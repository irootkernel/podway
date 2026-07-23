#!/usr/bin/env python3
"""Run and validate durable, freshness-checked Phase 0 verification evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
import platform
import re
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import tomllib
from typing import Any
import uuid


def verification_root() -> Path:
    return Path(__file__).resolve().parent.parent


ROOT = verification_root()
REPORT_RELATIVE = Path("artifacts/phase0/verification-report.json")
REPORT_POINTER_RELATIVE = Path("artifacts/phase0/verification-report.pointer.json")
RUNS_RELATIVE = Path("artifacts/phase0/verification-logs")
SOURCE_MANIFEST_NAME = "source-manifest.json"
RUN_REPORT_NAME = "verification-report.json"
REPORT_SCHEMA_VERSION = "podway.phase0.verification-report/v4"
REPORT_POINTER_SCHEMA_VERSION = "podway.phase0.verification-report-pointer/v1"
SOURCE_MANIFEST_SCHEMA_VERSION = "podway.phase0.source-manifest/v1"
ATTESTATION_SCHEMA_VERSION = "podway.phase0.verification-attestation/v1"
CANONICALIZATION = "podway.canonical-json/v1"
ATTESTATION_DIRECTORY = Path("contracts/evidence")
ATTESTATION_PREFIX = "phase0-verification-"
# Reports are evidence for the preceding 24 hours only.  The bound is deliberately
# short enough that receipts cannot be replayed as indefinitely-fresh evidence.
MAX_REPORT_AGE_SECONDS = 24 * 60 * 60
DIGEST_RE = re.compile(r"^[a-f0-9]{64}$")
RUN_ID_RE = re.compile(r"^[a-f0-9]{32}$")
UTC_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")

# These are every repository input class consumed by the fixed gates.  Only
# regular files under these roots are attested; mutable output trees are never
# treated as verification inputs.
INPUT_DIRECTORIES = (
    ".cargo",
    "contracts",
    "crates",
    "docs",
    "presets",
    "quality",
    "release",
    "schemas",
    "spec",
    "tests",
    "tools",
)
REQUIRED_INPUT_FILES = (
    "Cargo.toml",
    "Cargo.lock",
    "LICENSE",
    "Makefile",
    "README.md",
    "RELEASE_NOTES.md",
    "deny.toml",
)
OPTIONAL_INPUT_FILES = (
    ".clippy.toml",
    ".rustfmt.toml",
    "clippy.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
    "rustfmt.toml",
)
EXCLUDED_COMPONENTS = frozenset(
    {
        ".cache",
        ".gjc",
        ".git",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        ".tox",
        "__pycache__",
        "artifacts",
        "cache",
        "caches",
        "log",
        "logs",
        "target",
    }
)
EXCLUDED_PREFIXES = (
    ("contracts", "evidence"),
    ("contracts", "handoffs"),
    ("contracts", "locks"),
)
EXCLUDED_SUFFIXES = (".cache", ".log", ".pyc", ".pyo")
MEBIBYTE = 1024 * 1024
MAX_REPORT_JSON_BYTES = 8 * MEBIBYTE
MAX_SOURCE_MANIFEST_JSON_BYTES = 8 * MEBIBYTE
MAX_SOURCE_MANIFEST_FILES = 20_000
MAX_SOURCE_INPUT_FILE_BYTES = 64 * MEBIBYTE
MAX_SOURCE_INPUT_BYTES = 512 * MEBIBYTE
MAX_GATE_STDOUT_BYTES = 32 * MEBIBYTE
MAX_GATE_STDERR_BYTES = 32 * MEBIBYTE
MAX_GATE_TOTAL_OUTPUT_BYTES = 48 * MEBIBYTE
MAX_PROBE_STDOUT_BYTES = MEBIBYTE
MAX_PROBE_STDERR_BYTES = MEBIBYTE
MAX_PROBE_TOTAL_OUTPUT_BYTES = 2 * MEBIBYTE
PROBE_TERMINATION_GRACE_SECONDS = 5
PROBE_TIMEOUT_SECONDS = 30
GATE_TERMINATION_GRACE_SECONDS = 5
GATE_TIMEOUT_SECONDS = (
    15 * 60,
    5 * 60,
    5 * 60,
    5 * 60,
    15 * 60,
    30 * 60,
    15 * 60,
    5 * 60,
    30 * 60,
    30 * 60,
    30 * 60,
    60 * 60,
    30 * 60,
    60 * 60,
    5 * 60,
    5 * 60,
)
GATE_EXECUTABLES = (
    "cargo",
    "cargo-deny",
    "python3",
    "rustc",
    "rustfmt",
    "cargo-clippy",
    "cargo-fmt",
)
RUSTUP_TOOL_EXECUTABLES = ("cargo", "rustc", "rustfmt", "cargo-clippy", "cargo-fmt", "clippy-driver")
EXECUTABLE_IDENTITY_NAMES = (
    GATE_EXECUTABLES + ("rustup",) + tuple(f"rustup:{name}" for name in RUSTUP_TOOL_EXECUTABLES)
)
GATES: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("01-cargo-fmt", ("cargo", "fmt", "--all", "--", "--check")),
    ("02-sync-docs-assets", ("python3", "tools/sync_docs_assets.py", "--check")),
    ("02-verify-docs", ("python3", "tools/verify_docs.py")),
    ("03-test-layout", ("python3", "tools/verify_test_layout.py", "--check")),
    ("04-cargo-check", ("cargo", "check", "--workspace", "--all-targets", "--locked")),
    ("05-cargo-clippy", ("cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings")),
    ("06-cargo-deny", ("cargo", "deny", "check")),
    ("06-quality-contracts", ("python3", "tools/verify_quality_contracts.py")),
    ("07-architecture", ("cargo", "test", "--workspace", "--test", "arch_*", "--locked")),
    ("08-unit", ("cargo", "test", "--workspace", "--lib", "--bins", "--locked")),
    ("09-doc", ("cargo", "test", "--workspace", "--doc", "--locked")),
    ("10-integration", ("cargo", "test", "--workspace", "--test", "int_*", "--locked")),
    ("11-fuzzing", ("python3", "tools/run_fuzzing.py")),
    ("12-e2e", ("python3", "tools/run_e2e.py")),
    ("13-verify-contracts", ("python3", "tools/verify_contracts.py", "--all")),
    ("14-verify-verification-runner", ("python3", "tools/verify_verification_runner.py")),
)


class VerificationError(Exception):
    """A verification provenance invariant was violated."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def fail(code: str, message: str) -> None:
    raise VerificationError(code, message)


def canonical_json(value: Any) -> bytes:
    try:
        return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")
    except (TypeError, ValueError, UnicodeEncodeError) as error:
        fail("invalid_json_value", f"cannot canonicalize JSON value: {error}")


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            for block in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(block)
    except OSError as error:
        fail("unreadable_file", f"cannot hash {path.as_posix()}: {error}")
    return digest.hexdigest()


def require_object(value: Any, label: str, keys: set[str] | None = None) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail("invalid_schema", f"{label} must be an object")
    if keys is not None and set(value) != keys:
        fail("invalid_schema", f"{label} has unexpected or missing fields")
    return value


def require_list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        fail("invalid_schema", f"{label} must be a list")
    return value


def require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        fail("invalid_schema", f"{label} must be a non-empty string")
    return value


def require_integer(value: Any, label: str, minimum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        fail("invalid_schema", f"{label} must be an integer")
    if minimum is not None and value < minimum:
        fail("invalid_schema", f"{label} must be at least {minimum}")
    return value


def require_digest(value: Any, label: str) -> str:
    digest = require_string(value, label)
    if DIGEST_RE.fullmatch(digest) is None:
        fail("invalid_schema", f"{label} must be a lowercase SHA-256 digest")
    return digest


def normalized_relative_path(value: Any, label: str) -> Path:
    text = require_string(value, label)
    if "\\" in text:
        fail("unsafe_path", f"{label} must use POSIX separators")
    parts = text.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        fail("unsafe_path", f"{label} must be a normalized relative path")
    candidate = PurePosixPath(text)
    if candidate.is_absolute() or not candidate.parts:
        fail("unsafe_path", f"{label} must be a normalized relative path")
    return Path(*candidate.parts)


def is_under(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def checked_path(root: Path, relative: Path, label: str) -> Path:
    root = root.resolve()
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            fail("unsafe_path", f"{label} contains a symlink: {relative.as_posix()}")
    if not is_under(current.resolve(strict=False), root):
        fail("unsafe_path", f"{label} escapes the repository root")
    return current


def require_regular_file(path: Path, label: str) -> None:
    try:
        mode = path.stat().st_mode
    except OSError as error:
        fail("missing_file", f"cannot stat {label}: {error}")
    if not stat.S_ISREG(mode):
        fail("unsafe_path", f"{label} is not a regular file")


def require_directory(path: Path, label: str) -> None:
    try:
        mode = path.stat().st_mode
    except OSError as error:
        fail("missing_directory", f"cannot stat {label}: {error}")
    if not stat.S_ISDIR(mode):
        fail("unsafe_path", f"{label} is not a directory")


def is_excluded(relative: Path) -> bool:
    parts = relative.parts
    if any(part in EXCLUDED_COMPONENTS for part in parts):
        return True
    if any(parts[: len(prefix)] == prefix for prefix in EXCLUDED_PREFIXES):
        return True
    return relative.name.endswith(EXCLUDED_SUFFIXES)


def add_regular_file(root: Path, relative: Path, entries: list[dict[str, Any]], total_bytes: int) -> int:
    path = checked_path(root, relative, "source input")
    require_regular_file(path, f"source input {relative.as_posix()}")
    metadata = opened_regular_file_metadata(
        path, f"source input {relative.as_posix()}", maximum_bytes=MAX_SOURCE_INPUT_FILE_BYTES
    )
    size = metadata["size"]
    if len(entries) >= MAX_SOURCE_MANIFEST_FILES:
        fail("source_manifest_limit", f"source-input manifest exceeds {MAX_SOURCE_MANIFEST_FILES} files")
    total_bytes += size
    if total_bytes > MAX_SOURCE_INPUT_BYTES:
        fail("source_manifest_limit", f"source-input manifest exceeds {MAX_SOURCE_INPUT_BYTES} bytes")
    entries.append({"path": relative.as_posix(), "sha256": metadata["sha256"], "size": size})
    return total_bytes


def collect_directory_inputs(root: Path, directory_name: str, entries: list[dict[str, Any]], total_bytes: int) -> int:
    directory_relative = normalized_relative_path(directory_name, "input directory")
    directory = checked_path(root, directory_relative, "input directory")
    if not directory.exists():
        return total_bytes
    require_directory(directory, f"input directory {directory_name}")

    for current_name, child_directories, child_files in os.walk(directory, followlinks=False):
        current = Path(current_name)
        relative_current = current.relative_to(root)
        retained_directories: list[str] = []
        for child_name in sorted(child_directories):
            child_relative = relative_current / child_name
            if is_excluded(child_relative):
                continue
            child = current / child_name
            if child.is_symlink():
                fail("unsafe_path", f"source input directory contains a symlink: {child_relative.as_posix()}")
            require_directory(child, f"source input directory {child_relative.as_posix()}")
            retained_directories.append(child_name)
        child_directories[:] = retained_directories

        for child_name in sorted(child_files):
            child_relative = relative_current / child_name
            if is_excluded(child_relative):
                continue
            child = current / child_name
            if child.is_symlink():
                fail("unsafe_path", f"source input contains a symlink: {child_relative.as_posix()}")
            require_regular_file(child, f"source input {child_relative.as_posix()}")
            total_bytes = add_regular_file(root, child_relative, entries, total_bytes)
    return total_bytes


def build_source_manifest(root: Path) -> dict[str, Any]:
    entries: list[dict[str, Any]] = []
    total_bytes = 0
    for file_name in REQUIRED_INPUT_FILES:
        relative = normalized_relative_path(file_name, "required input file")
        path = checked_path(root, relative, "required input file")
        if not path.exists():
            fail("missing_file", f"required source input is missing: {file_name}")
        total_bytes = add_regular_file(root, relative, entries, total_bytes)
    for file_name in OPTIONAL_INPUT_FILES:
        relative = normalized_relative_path(file_name, "optional input file")
        path = checked_path(root, relative, "optional input file")
        if path.exists():
            total_bytes = add_regular_file(root, relative, entries, total_bytes)
    for directory_name in INPUT_DIRECTORIES:
        total_bytes = collect_directory_inputs(root, directory_name, entries, total_bytes)

    entries.sort(key=lambda entry: entry["path"])
    paths = [entry["path"] for entry in entries]
    if len(paths) != len(set(paths)):
        fail("invalid_manifest", "source-input manifest contains duplicate paths")
    return {"schema_version": SOURCE_MANIFEST_SCHEMA_VERSION, "inputs": entries}


def source_manifest_bytes(manifest: dict[str, Any]) -> bytes:
    validate_source_manifest_shape(manifest)
    content = canonical_json(manifest)
    if len(content) > MAX_SOURCE_MANIFEST_JSON_BYTES:
        fail("source_manifest_limit", f"source manifest JSON exceeds {MAX_SOURCE_MANIFEST_JSON_BYTES} bytes")
    return content


def validate_source_manifest_shape(manifest: Any) -> None:
    value = require_object(manifest, "source manifest", {"schema_version", "inputs"})
    if value["schema_version"] != SOURCE_MANIFEST_SCHEMA_VERSION:
        fail("invalid_schema", "source manifest schema version is invalid")
    entries = require_list(value["inputs"], "source manifest inputs")
    if len(entries) > MAX_SOURCE_MANIFEST_FILES:
        fail("source_manifest_limit", f"source-input manifest exceeds {MAX_SOURCE_MANIFEST_FILES} files")
    prior_path: str | None = None
    total_bytes = 0
    for index, raw_entry in enumerate(entries):
        entry = require_object(raw_entry, f"source manifest input {index}", {"path", "sha256", "size"})
        path = normalized_relative_path(entry["path"], f"source manifest input {index} path")
        if is_excluded(path):
            fail("invalid_manifest", f"source manifest includes excluded path: {path.as_posix()}")
        if prior_path is not None and path.as_posix() <= prior_path:
            fail("invalid_manifest", "source manifest inputs must be sorted and unique")
        prior_path = path.as_posix()
        require_digest(entry["sha256"], f"source manifest input {index} digest")
        size = require_integer(entry["size"], f"source manifest input {index} size", minimum=0)
        if size > MAX_SOURCE_INPUT_FILE_BYTES:
            fail("source_manifest_limit", f"source manifest input {index} exceeds {MAX_SOURCE_INPUT_FILE_BYTES} bytes")
        total_bytes += size
        if total_bytes > MAX_SOURCE_INPUT_BYTES:
            fail("source_manifest_limit", f"source-input manifest exceeds {MAX_SOURCE_INPUT_BYTES} bytes")


def absolute_environment_path(value: str, label: str) -> Path:
    path = Path(value)
    if not path.is_absolute():
        fail("unsafe_environment", f"{label} must be an absolute path")
    return path


def normalized_path_environment(root: Path) -> str:
    raw_path = os.environ.get("PATH", os.defpath)
    entries: list[str] = []
    for entry in raw_path.split(os.pathsep):
        candidate = root if not entry else Path(entry)
        if not candidate.is_absolute():
            candidate = root / candidate
        entries.append(os.path.abspath(candidate))
    if not entries:
        fail("unsafe_environment", "PATH must contain at least one entry")
    return os.pathsep.join(entries)


def safe_environment(root: Path) -> dict[str, str]:
    home = os.environ.get("HOME") or str(Path.home())
    home_path = absolute_environment_path(home, "HOME")
    cargo_home = os.environ.get("CARGO_HOME") or str(home_path / ".cargo")
    rustup_home = os.environ.get("RUSTUP_HOME") or str(home_path / ".rustup")
    temporary_directory = os.environ.get("TMPDIR") or tempfile.gettempdir()
    for key, value in (
        ("CARGO_HOME", cargo_home),
        ("HOME", home),
        ("RUSTUP_HOME", rustup_home),
        ("TMPDIR", temporary_directory),
    ):
        absolute_environment_path(value, key)
    locale = os.environ.get("LC_ALL") or os.environ.get("LC_CTYPE") or os.environ.get("LANG") or "C"
    environment = {
        "CARGO_HOME": cargo_home,
        "HOME": home,
        "LANG": locale,
        "LC_ALL": locale,
        "LC_CTYPE": locale,
        "PATH": normalized_path_environment(root),
        "PYTHONNOUSERSITE": "1",
        "RUSTUP_HOME": rustup_home,
        "TEMP": temporary_directory,
        "TMP": temporary_directory,
        "TMPDIR": temporary_directory,
        "TZ": "UTC",
    }
    if os.name == "nt":
        system_root = os.environ.get("SYSTEMROOT") or os.environ.get("SystemRoot")
        if not system_root:
            fail("unsafe_environment", "SYSTEMROOT is required on Windows")
        environment["SYSTEMROOT"] = system_root
    return environment


def decode_tool_output(value: bytes, label: str) -> str:
    try:
        text = value.decode("utf-8")
    except UnicodeDecodeError as error:
        fail("toolchain_identity_failed", f"{label} did not produce UTF-8 output: {error}")
    if not text:
        fail("toolchain_identity_failed", f"{label} produced no output")
    return text


def opened_regular_file_metadata(path: Path, label: str, maximum_bytes: int | None = None) -> dict[str, Any]:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        fail("unreadable_file", f"cannot open {label}: {error}")
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            fail("unsafe_path", f"{label} is not a regular file")
        if maximum_bytes is not None and metadata.st_size > maximum_bytes:
            fail("resource_limit", f"{label} exceeds its {maximum_bytes}-byte ceiling")
        digest = hashlib.sha256()
        total_bytes = 0
        while True:
            block = os.read(descriptor, 1024 * 1024)
            if not block:
                break
            total_bytes += len(block)
            if maximum_bytes is not None and total_bytes > maximum_bytes:
                fail("resource_limit", f"{label} exceeds its {maximum_bytes}-byte ceiling")
            digest.update(block)
        final_metadata = os.fstat(descriptor)
        if final_metadata.st_size != metadata.st_size or total_bytes != metadata.st_size:
            fail("input_changed", f"{label} changed while it was read")
        return {"path": path.as_posix(), "sha256": digest.hexdigest(), "size": metadata.st_size}
    except OSError as error:
        fail("unreadable_file", f"cannot read {label}: {error}")
    finally:
        os.close(descriptor)


def expected_config_inputs(root: Path, environment: dict[str, str]) -> list[tuple[str, Path]]:
    locations: list[tuple[str, Path]] = []
    for ancestor in (root.resolve(), *root.resolve().parents):
        for name in ("config", "config.toml"):
            locations.append(("cargo-project", ancestor / ".cargo" / name))
    cargo_home = absolute_environment_path(environment["CARGO_HOME"], "CARGO_HOME")
    for name in ("config", "config.toml"):
        locations.append(("cargo-home", cargo_home / name))
    rustup_home = absolute_environment_path(environment["RUSTUP_HOME"], "RUSTUP_HOME")
    locations.append(("rustup-home", rustup_home / "settings.toml"))
    return locations


def collect_config_inputs(root: Path, environment: dict[str, str]) -> tuple[list[dict[str, Any]], str | None]:
    records: list[dict[str, Any]] = []
    for origin, candidate in expected_config_inputs(root, environment):
        lexical_path = candidate.as_posix()
        try:
            resolved = candidate.resolve(strict=True)
        except FileNotFoundError:
            records.append(
                {
                    "origin": origin,
                    "path": lexical_path,
                    "resolved_path": candidate.resolve(strict=False).as_posix(),
                    "state": "absent",
                }
            )
            continue
        except OSError as error:
            return records, f"cannot resolve configuration input {lexical_path}: {error}"
        try:
            metadata = opened_regular_file_metadata(resolved, f"configuration input {lexical_path}")
        except VerificationError as error:
            return records, error.message
        records.append(
            {
                "origin": origin,
                "path": lexical_path,
                "resolved_path": resolved.as_posix(),
                "sha256": metadata["sha256"],
                "size": metadata["size"],
                "state": "file",
            }
        )
    return records, None


def collect_executable_identity(
    root: Path, command: str, environment: dict[str, str]
) -> tuple[dict[str, Any] | None, str | None]:
    executable = shutil.which(command, path=environment["PATH"])
    if executable is None:
        return None, f"cannot resolve executable {command} from the attested PATH"
    candidate = Path(executable)
    if not candidate.is_absolute():
        candidate = root / candidate
    try:
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        return None, f"cannot resolve executable {command}: {error}"
    try:
        metadata = opened_regular_file_metadata(resolved, f"executable {command}")
    except VerificationError as error:
        return None, error.message
    return {
        "name": command,
        "path": metadata["path"],
        "sha256": metadata["sha256"],
        "size": metadata["size"],
    }, None

def collect_rustup_tool_identity(
    root: Path, command: str, environment: dict[str, str]
) -> tuple[dict[str, Any] | None, str | None]:
    probe = run_environment_probe(root, ("rustup", "which", command), environment)
    if probe["failure"] is not None:
        return None, f"rustup which {command} {probe['failure']}"
    if probe["exit_code"] != 0:
        return None, f"rustup which {command} exited with {probe['exit_code']}"
    try:
        location = decode_tool_output(probe["stdout"], f"rustup which {command}").strip()
    except VerificationError as error:
        return None, error.message
    if not location or "\n" in location or "\r" in location:
        return None, f"rustup which {command} produced an invalid executable path"
    target = Path(location)
    if not target.is_absolute():
        return None, f"rustup which {command} produced a non-absolute executable path"
    try:
        resolved = target.resolve(strict=True)
        metadata = opened_regular_file_metadata(resolved, f"rustup tool executable {command}")
    except VerificationError as error:
        return None, error.message
    except OSError as error:
        return None, f"cannot resolve rustup tool executable {command}: {error}"
    return {
        "name": f"rustup:{command}",
        "path": metadata["path"],
        "sha256": metadata["sha256"],
        "size": metadata["size"],
    }, None


def collect_environment(root: Path, environment: dict[str, str]) -> tuple[dict[str, Any], str | None]:
    identity: dict[str, Any] = {
        "architecture": platform.machine(),
        "cargo_v": "unavailable",
        "config_inputs": [],
        "executables": [],
        "platform": platform.platform(),
        "python": sys.version,
        "rustc_vv": "unavailable",
        "variables": dict(environment),
    }
    if not all(identity[field] for field in ("architecture", "platform", "python")):
        return identity, "platform or Python identity was unavailable"

    config_inputs, config_error = collect_config_inputs(root, environment)
    identity["config_inputs"] = config_inputs
    if config_error is not None:
        return identity, config_error

    for command in GATE_EXECUTABLES:
        executable, executable_error = collect_executable_identity(root, command, environment)
        if executable_error is not None:
            return identity, executable_error
        identity["executables"].append(executable)
    rustup_path = shutil.which("rustup", path=environment["PATH"])
    if rustup_path is not None:
        executable, executable_error = collect_executable_identity(root, "rustup", environment)
        if executable_error is not None:
            return identity, executable_error
        try:
            cargo_uses_rustup = os.path.samefile(identity["executables"][0]["path"], executable["path"])
        except OSError as error:
            return identity, f"cannot compare cargo and rustup executable identities: {error}"
        if cargo_uses_rustup:
            identity["executables"].append(executable)
            for command in RUSTUP_TOOL_EXECUTABLES:
                executable, executable_error = collect_rustup_tool_identity(root, command, environment)
                if executable_error is not None:
                    return identity, executable_error
                identity["executables"].append(executable)
    for field, argv in (("rustc_vv", ("rustc", "-Vv")), ("cargo_v", ("cargo", "-V"))):
        probe = run_environment_probe(root, argv, environment)
        if probe["failure"] is not None:
            identity[field] = f"unavailable: {probe['failure']}"
            return identity, f"{' '.join(argv)} {probe['failure']}"
        if probe["exit_code"] != 0:
            identity[field] = f"unavailable: exit {probe['exit_code']}"
            return identity, f"{' '.join(argv)} exited with {probe['exit_code']}"
        try:
            identity[field] = decode_tool_output(probe["stdout"], " ".join(argv))
        except VerificationError as error:
            identity[field] = f"unavailable: {error.message}"
            return identity, error.message
    return identity, None


def expected_run_directory(run_id: str) -> Path:
    return RUNS_RELATIVE / run_id


def expected_source_manifest_relative(run_id: str) -> Path:
    return expected_run_directory(run_id) / SOURCE_MANIFEST_NAME


def expected_run_report_relative(run_id: str) -> Path:
    return expected_run_directory(run_id) / RUN_REPORT_NAME


def expected_log_relative(run_id: str, index: int, stream: str) -> Path:
    if stream not in {"stdout", "stderr"}:
        fail("invalid_log", f"unsupported log stream: {stream}")
    return expected_run_directory(run_id) / f"{GATES[index][0]}.{stream}.log"


def directory_open_flags() -> int:
    return os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)


def fsync_descriptor(descriptor: int, label: str) -> None:
    try:
        os.fsync(descriptor)
    except OSError as error:
        fail("durability_failed", f"cannot fsync {label}: {error}")


def open_directory_component(parent_descriptor: int, name: str, label: str, create: bool) -> int:
    try:
        descriptor = os.open(name, directory_open_flags(), dir_fd=parent_descriptor)
    except FileNotFoundError:
        if not create:
            fail("missing_directory", f"cannot open {label}: directory is missing")
        try:
            os.mkdir(name, mode=0o700, dir_fd=parent_descriptor)
            fsync_descriptor(parent_descriptor, f"parent of {label}")
        except FileExistsError:
            pass
        except OSError as error:
            fail("unsafe_path", f"cannot create {label}: {error}")
        try:
            descriptor = os.open(name, directory_open_flags(), dir_fd=parent_descriptor)
        except OSError as error:
            fail("unsafe_path", f"cannot open newly created {label}: {error}")
    except OSError as error:
        fail("unsafe_path", f"cannot open {label}: {error}")
    try:
        mode = os.fstat(descriptor).st_mode
    except OSError as error:
        os.close(descriptor)
        fail("unsafe_path", f"cannot inspect {label}: {error}")
    if not stat.S_ISDIR(mode):
        os.close(descriptor)
        fail("unsafe_path", f"{label} is not a directory")
    return descriptor


def open_anchored_directory(root: Path, relative: Path, label: str, create: bool) -> int:
    try:
        descriptor = os.open(root, directory_open_flags())
    except OSError as error:
        fail("unsafe_path", f"cannot open repository root for {label}: {error}")
    try:
        if not stat.S_ISDIR(os.fstat(descriptor).st_mode):
            fail("unsafe_path", "repository root is not a directory")
        for component in relative.parts:
            child = open_directory_component(descriptor, component, f"{label} component {component}", create)
            os.close(descriptor)
            descriptor = child
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def fsync_relative_directory(root: Path, relative: Path, label: str) -> None:
    descriptor = open_anchored_directory(root, relative, label, create=False)
    try:
        fsync_descriptor(descriptor, label)
    finally:
        os.close(descriptor)


def create_run_directory(root: Path) -> str:
    runs_descriptor = open_anchored_directory(root, RUNS_RELATIVE, "verification log directory", create=True)
    try:
        fsync_descriptor(runs_descriptor, "verification log directory")
        for _ in range(16):
            run_id = uuid.uuid4().hex
            try:
                os.mkdir(run_id, mode=0o700, dir_fd=runs_descriptor)
            except FileExistsError:
                continue
            except OSError as error:
                fail("unsafe_path", f"cannot create verification run directory: {error}")
            run_descriptor = open_directory_component(
                runs_descriptor, run_id, "verification run directory", create=False
            )
            try:
                fsync_descriptor(run_descriptor, "verification run directory")
            finally:
                os.close(run_descriptor)
            fsync_descriptor(runs_descriptor, "verification log directory")
            return run_id
    finally:
        os.close(runs_descriptor)
    fail("run_id_collision", "could not allocate a unique verification run identifier")


def atomic_write(root: Path, relative: Path, content: bytes, label: str) -> None:
    if not relative.name or relative.name in {".", ".."}:
        fail("unsafe_path", f"{label} has an invalid destination name")
    parent_descriptor = open_anchored_directory(root, relative.parent, f"{label} parent", create=True)
    temporary_name = f".{relative.name}.{uuid.uuid4().hex}.tmp"
    temporary_created = False
    descriptor = -1
    try:
        try:
            destination = os.stat(relative.name, dir_fd=parent_descriptor, follow_symlinks=False)
        except FileNotFoundError:
            destination = None
        except OSError as error:
            fail("unsafe_path", f"cannot inspect {label} destination: {error}")
        if destination is not None and not stat.S_ISREG(destination.st_mode):
            fail("unsafe_path", f"{label} destination is not a regular file")
        descriptor = os.open(
            temporary_name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0),
            0o600,
            dir_fd=parent_descriptor,
        )
        temporary_created = True
        with os.fdopen(descriptor, "wb") as handle:
            descriptor = -1
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(
            temporary_name,
            relative.name,
            src_dir_fd=parent_descriptor,
            dst_dir_fd=parent_descriptor,
        )
        temporary_created = False
        fsync_descriptor(parent_descriptor, f"{label} parent directory")
    except BaseException:
        if descriptor >= 0:
            os.close(descriptor)
        if temporary_created:
            try:
                os.unlink(temporary_name, dir_fd=parent_descriptor)
                fsync_descriptor(parent_descriptor, f"{label} parent directory")
            except OSError:
                pass
        raise
    finally:
        os.close(parent_descriptor)


def open_new_log_file(root: Path, relative: Path, label: str) -> Any:
    parent_descriptor = open_anchored_directory(root, relative.parent, f"{label} parent", create=False)
    try:
        descriptor = os.open(
            relative.name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0),
            0o600,
            dir_fd=parent_descriptor,
        )
    except OSError as error:
        os.close(parent_descriptor)
        fail("gate_log_failed", f"cannot create {label}: {error}")
    os.close(parent_descriptor)
    return os.fdopen(descriptor, "wb")


def log_metadata(root: Path, relative: Path, label: str) -> dict[str, Any]:
    path = checked_path(root, relative, label)
    require_regular_file(path, label)
    try:
        size = path.stat().st_size
    except OSError as error:
        fail("unreadable_file", f"cannot stat {label}: {error}")
    return {"bytes": size, "path": relative.as_posix(), "sha256": digest_file(path)}


def terminate_process_group(process: subprocess.Popen[bytes], grace_seconds: float) -> None:
    if os.name == "nt":
        if process.poll() is not None:
            return
        try:
            process.send_signal(signal.CTRL_BREAK_EVENT)
            process.wait(timeout=grace_seconds)
            return
        except (OSError, subprocess.TimeoutExpired):
            try:
                process.kill()
                process.wait(timeout=grace_seconds)
            except (OSError, subprocess.TimeoutExpired):
                pass
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    except OSError:
        return
    try:
        process.wait(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except OSError:
        pass
    try:
        process.wait(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        pass


def append_bounded_probe_output(captured: bytearray, value: bytes, stream_limit: int, total_bytes: int) -> bool:
    retained = min(
        len(value),
        max(0, stream_limit - len(captured)),
        max(0, MAX_PROBE_TOTAL_OUTPUT_BYTES - total_bytes),
    )
    if retained:
        captured.extend(value[:retained])
    return retained != len(value)


def run_environment_probe(root: Path, argv: tuple[str, ...], environment: dict[str, str]) -> dict[str, Any]:
    deadline = time.monotonic() + PROBE_TIMEOUT_SECONDS
    stdout = bytearray()
    stderr = bytearray()
    try:
        process = subprocess.Popen(
            argv,
            cwd=root,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            shell=False,
            start_new_session=os.name != "nt",
            creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0,
        )
    except OSError as error:
        return {
            "exit_code": 127,
            "failure": f"probe_launch_error: {error}",
            "stderr": b"",
            "stdout": b"",
        }

    if process.stdout is None or process.stderr is None:
        terminate_process_group(process, PROBE_TERMINATION_GRACE_SECONDS)
        return {
            "exit_code": 125,
            "failure": "probe_capture_failed",
            "stderr": b"",
            "stdout": b"",
        }

    failure: str | None = None
    termination_drain_deadline: float | None = None
    pipe_drain_deadline: float | None = None
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ, "stdout")
    selector.register(process.stderr, selectors.EVENT_READ, "stderr")
    try:
        while selector.get_map():
            now = time.monotonic()
            if failure is None and now >= deadline:
                failure = "probe_timeout"
                terminate_process_group(process, PROBE_TERMINATION_GRACE_SECONDS)
                termination_drain_deadline = time.monotonic() + PROBE_TERMINATION_GRACE_SECONDS
            active_deadline = termination_drain_deadline if failure is not None else deadline
            events = selector.select(timeout=min(0.1, max(0.0, active_deadline - now)))
            for key, _ in events:
                stream_name = key.data
                try:
                    block = os.read(key.fileobj.fileno(), 64 * 1024)
                except OSError as error:
                    failure = f"probe_output_read_failed: {error}"
                    terminate_process_group(process, PROBE_TERMINATION_GRACE_SECONDS)
                    termination_drain_deadline = time.monotonic() + PROBE_TERMINATION_GRACE_SECONDS
                    continue
                if not block:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                if stream_name == "stdout":
                    overflowed = append_bounded_probe_output(
                        stdout, block, MAX_PROBE_STDOUT_BYTES, len(stdout) + len(stderr)
                    )
                else:
                    overflowed = append_bounded_probe_output(
                        stderr, block, MAX_PROBE_STDERR_BYTES, len(stdout) + len(stderr)
                    )
                if overflowed and failure is None:
                    failure = "probe_output_overflow"
                    terminate_process_group(process, PROBE_TERMINATION_GRACE_SECONDS)
                    termination_drain_deadline = time.monotonic() + PROBE_TERMINATION_GRACE_SECONDS
            now = time.monotonic()
            if process.poll() is not None and pipe_drain_deadline is None:
                pipe_drain_deadline = now + PROBE_TERMINATION_GRACE_SECONDS
            if pipe_drain_deadline is not None and now >= pipe_drain_deadline and selector.get_map():
                if failure is None:
                    failure = "probe_pipe_drain_timeout"
                    terminate_process_group(process, PROBE_TERMINATION_GRACE_SECONDS)
                    termination_drain_deadline = time.monotonic() + PROBE_TERMINATION_GRACE_SECONDS
                for key in list(selector.get_map().values()):
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
            if (
                termination_drain_deadline is not None
                and time.monotonic() >= termination_drain_deadline
                and selector.get_map()
            ):
                for key in list(selector.get_map().values()):
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
        if failure is None:
            try:
                exit_code = process.wait(timeout=max(0.0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                failure = "probe_timeout"
                terminate_process_group(process, PROBE_TERMINATION_GRACE_SECONDS)
                exit_code = 124
            except OSError as error:
                failure = f"probe_wait_failed: {error}"
                exit_code = 125
            else:
                return {
                    "exit_code": exit_code,
                    "failure": None,
                    "stderr": bytes(stderr),
                    "stdout": bytes(stdout),
                }
        else:
            exit_code = 124 if failure == "probe_timeout" else 125
        return {
            "exit_code": exit_code,
            "failure": failure,
            "stderr": bytes(stderr),
            "stdout": bytes(stdout),
        }
    finally:
        selector.close()
        for pipe in (process.stdout, process.stderr):
            if pipe is not None and not pipe.closed:
                pipe.close()


def write_bounded_output(
    handle: Any,
    value: bytes,
    stream_bytes: int,
    total_bytes: int,
    stream_limit: int,
) -> tuple[int, int, bool]:
    retained = min(len(value), max(0, stream_limit - stream_bytes), max(0, MAX_GATE_TOTAL_OUTPUT_BYTES - total_bytes))
    if retained:
        handle.write(value[:retained])
        stream_bytes += retained
        total_bytes += retained
    return stream_bytes, total_bytes, retained != len(value)


def run_gate(
    root: Path,
    run_id: str,
    index: int,
    environment: dict[str, str],
) -> dict[str, Any]:
    gate_name, argv = GATES[index]
    stdout_relative = expected_log_relative(run_id, index, "stdout")
    stderr_relative = expected_log_relative(run_id, index, "stderr")
    start = time.monotonic_ns()
    deadline = time.monotonic() + GATE_TIMEOUT_SECONDS[index]
    exit_code = 127
    termination = "launch_error"
    stdout_bytes = 0
    stderr_bytes = 0
    total_bytes = 0
    try:
        with open_new_log_file(root, stdout_relative, "gate stdout log") as stdout_handle, open_new_log_file(
            root, stderr_relative, "gate stderr log"
        ) as stderr_handle:
            try:
                process = subprocess.Popen(
                    argv,
                    cwd=root,
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    shell=False,
                    start_new_session=os.name != "nt",
                    creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0,
                )
            except OSError as error:
                message = f"verification runner could not launch {argv[0]}: {error}\n".encode("utf-8")
                _, _, _ = write_bounded_output(stderr_handle, message, stderr_bytes, total_bytes, MAX_GATE_STDERR_BYTES)
            else:
                if process.stdout is None or process.stderr is None:
                    terminate_process_group(process, GATE_TERMINATION_GRACE_SECONDS)
                    fail("gate_log_failed", f"cannot capture output from {gate_name}")
                selector = selectors.DefaultSelector()
                selector.register(process.stdout, selectors.EVENT_READ, (stdout_handle, "stdout"))
                selector.register(process.stderr, selectors.EVENT_READ, (stderr_handle, "stderr"))
                pipe_drain_deadline: float | None = None
                termination_drain_deadline: float | None = None
                try:
                    while selector.get_map():
                        now = time.monotonic()
                        if termination == "launch_error" and now >= deadline:
                            termination = "deadline_exceeded"
                            terminate_process_group(process, GATE_TERMINATION_GRACE_SECONDS)
                            termination_drain_deadline = time.monotonic() + GATE_TERMINATION_GRACE_SECONDS
                        active_deadline = termination_drain_deadline if termination_drain_deadline is not None else deadline
                        events = selector.select(timeout=min(0.1, max(0.0, active_deadline - now)))
                        for key, _ in events:
                            stream_handle, stream_name = key.data
                            try:
                                block = os.read(key.fileobj.fileno(), 64 * 1024)
                            except OSError as error:
                                terminate_process_group(process, GATE_TERMINATION_GRACE_SECONDS)
                                fail("gate_log_failed", f"cannot read {gate_name} {stream_name}: {error}")
                            if not block:
                                selector.unregister(key.fileobj)
                                key.fileobj.close()
                                continue
                            if stream_name == "stdout":
                                stdout_bytes, total_bytes, overflowed = write_bounded_output(
                                    stream_handle, block, stdout_bytes, total_bytes, MAX_GATE_STDOUT_BYTES
                                )
                            else:
                                stderr_bytes, total_bytes, overflowed = write_bounded_output(
                                    stream_handle, block, stderr_bytes, total_bytes, MAX_GATE_STDERR_BYTES
                                )
                            if overflowed and termination == "launch_error":
                                termination = "output_limit_exceeded"
                                terminate_process_group(process, GATE_TERMINATION_GRACE_SECONDS)
                                termination_drain_deadline = time.monotonic() + GATE_TERMINATION_GRACE_SECONDS
                        if process.poll() is not None and pipe_drain_deadline is None:
                            pipe_drain_deadline = time.monotonic() + GATE_TERMINATION_GRACE_SECONDS
                        if (
                            termination_drain_deadline is not None
                            and time.monotonic() >= termination_drain_deadline
                            and selector.get_map()
                        ):
                            for key in list(selector.get_map().values()):
                                selector.unregister(key.fileobj)
                                key.fileobj.close()
                        if pipe_drain_deadline is not None and time.monotonic() >= pipe_drain_deadline and selector.get_map():
                            if termination == "launch_error":
                                termination = "pipe_drain_timeout"
                                terminate_process_group(process, GATE_TERMINATION_GRACE_SECONDS)
                                termination_drain_deadline = time.monotonic() + GATE_TERMINATION_GRACE_SECONDS
                            for key in list(selector.get_map().values()):
                                selector.unregister(key.fileobj)
                                key.fileobj.close()
                    if termination == "launch_error":
                        try:
                            exit_code = process.wait(timeout=max(0.0, deadline - time.monotonic()))
                        except subprocess.TimeoutExpired:
                            termination = "deadline_exceeded"
                            terminate_process_group(process, GATE_TERMINATION_GRACE_SECONDS)
                            exit_code = 124
                        else:
                            termination = "completed"
                    elif termination == "deadline_exceeded":
                        exit_code = 124
                    else:
                        exit_code = 125
                finally:
                    selector.close()
                    for pipe in (process.stdout, process.stderr):
                        if pipe is not None and not pipe.closed:
                            pipe.close()
            stdout_handle.flush()
            stderr_handle.flush()
            os.fsync(stdout_handle.fileno())
            os.fsync(stderr_handle.fileno())
    except OSError as error:
        fail("gate_log_failed", f"cannot create raw logs for {gate_name}: {error}")
    fsync_relative_directory(root, expected_run_directory(run_id), "verification run directory")
    duration_ms = max(0, (time.monotonic_ns() - start) // 1_000_000)
    status = "passed" if exit_code == 0 and termination == "completed" else "failed"
    return {
        "argv": list(argv),
        "cwd": ".",
        "duration_ms": duration_ms,
        "exit_code": exit_code,
        "status": status,
        "stderr": log_metadata(root, stderr_relative, f"{gate_name} stderr log"),
        "stdout": log_metadata(root, stdout_relative, f"{gate_name} stdout log"),
        "termination": termination,
    }


def current_utc_timestamp() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def report_content_digest(report: dict[str, Any]) -> str:
    identity = require_object(report.get("report_identity"), "report identity")
    normalized_identity = dict(identity)
    normalized_identity.pop("digest", None)
    normalized_report = dict(report)
    normalized_report["report_identity"] = normalized_identity
    return digest_bytes(canonical_json(normalized_report))


def build_report(
    run_id: str,
    started_at_utc: str,
    environment: dict[str, Any],
    source_manifest_digest: str,
    source_file_count: int,
    commands: list[dict[str, Any]],
    status: str,
) -> dict[str, Any]:
    report: dict[str, Any] = {
        "commands": commands,
        "environment": environment,
        "report_artifact": {
            "path": expected_run_report_relative(run_id).as_posix(),
        },
        "report_identity": {
            "algorithm": "sha256",
            "canonicalization": CANONICALIZATION,
            "digest": "0" * 64,
        },
        "run_id": run_id,
        "schema_version": REPORT_SCHEMA_VERSION,
        "source_manifest": {
            "file_count": source_file_count,
            "path": expected_source_manifest_relative(run_id).as_posix(),
            "sha256": source_manifest_digest,
        },
        "started_at_utc": started_at_utc,
        "status": status,
    }
    report["report_identity"]["digest"] = report_content_digest(report)
    return report
def build_report_pointer(report: dict[str, Any], report_digest: str) -> dict[str, Any]:
    return {
        "report_path": report["report_artifact"]["path"],
        "report_self_digest": report["report_identity"]["digest"],
        "report_sha256": report_digest,
        "run_id": report["run_id"],
        "schema_version": REPORT_POINTER_SCHEMA_VERSION,
        "source_manifest_sha256": report["source_manifest"]["sha256"],
    }


def validate_report_pointer(pointer: Any, report: dict[str, Any], report_digest: str) -> None:
    value = require_object(
        pointer,
        "verification report pointer",
        {
            "report_path",
            "report_self_digest",
            "report_sha256",
            "run_id",
            "schema_version",
            "source_manifest_sha256",
        },
    )
    if value["schema_version"] != REPORT_POINTER_SCHEMA_VERSION:
        fail("invalid_schema", "verification report pointer schema version is invalid")
    if value["run_id"] != report["run_id"]:
        fail("canonical_pointer_replaced", "canonical report pointer run ID does not match the report")
    if value["report_path"] != report["report_artifact"]["path"]:
        fail("canonical_pointer_replaced", "canonical report pointer path does not match the report")
    if require_digest(value["report_sha256"], "canonical report pointer report digest") != report_digest:
        fail("canonical_pointer_replaced", "canonical report pointer digest does not match the report")
    if require_digest(value["report_self_digest"], "canonical report pointer self digest") != report["report_identity"]["digest"]:
        fail("canonical_pointer_replaced", "canonical report pointer self digest does not match the report")
    if require_digest(value["source_manifest_sha256"], "canonical report pointer source digest") != report["source_manifest"]["sha256"]:
        fail("canonical_pointer_replaced", "canonical report pointer source digest does not match the report")


def validate_log_metadata(value: Any, label: str, expected_relative: Path, root: Path, maximum_bytes: int) -> int:
    metadata = require_object(value, label, {"bytes", "path", "sha256"})
    relative = normalized_relative_path(metadata["path"], f"{label} path")
    if relative != expected_relative:
        fail("unexpected_data", f"{label} path does not match the fixed log location")
    expected_path = checked_path(root, expected_relative, label)
    require_regular_file(expected_path, label)
    size = require_integer(metadata["bytes"], f"{label} byte length", minimum=0)
    if size > maximum_bytes:
        fail("resource_limit", f"{label} exceeds its {maximum_bytes}-byte ceiling")
    if expected_path.stat().st_size != size:
        fail("log_tampered", f"{label} byte length does not match")
    digest = require_digest(metadata["sha256"], f"{label} digest")
    if digest_file(expected_path) != digest:
        fail("log_tampered", f"{label} digest does not match")
    return size


def validate_command_list(root: Path, run_id: str, commands: Any, report_status: str) -> set[str]:
    records = require_list(commands, "report commands")
    if len(records) > len(GATES):
        fail("invalid_schema", "report commands contains too many entries")
    expected_files = {RUN_REPORT_NAME, SOURCE_MANIFEST_NAME}
    failure_seen = False
    for index, raw_record in enumerate(records):
        record = require_object(
            raw_record,
            f"report command {index}",
            {"argv", "cwd", "duration_ms", "exit_code", "status", "stderr", "stdout", "termination"},
        )
        argv = require_list(record["argv"], f"report command {index} argv")
        if argv != list(GATES[index][1]) or not all(isinstance(item, str) and item for item in argv):
            fail("command_drift", f"report command {index} does not match the fixed gate")
        if record["cwd"] != ".":
            fail("command_drift", f"report command {index} cwd must be repository root")
        require_integer(record["duration_ms"], f"report command {index} duration", minimum=0)
        exit_code = require_integer(record["exit_code"], f"report command {index} exit code")
        termination = record["termination"]
        if termination not in {
            "completed",
            "deadline_exceeded",
            "launch_error",
            "output_limit_exceeded",
            "pipe_drain_timeout",
        }:
            fail("invalid_schema", f"report command {index} termination is invalid")
        if termination == "completed":
            expected_status = "passed" if exit_code == 0 else "failed"
        else:
            expected_status = "failed"
            expected_exit = {
                "deadline_exceeded": 124,
                "launch_error": 127,
                "output_limit_exceeded": 125,
                "pipe_drain_timeout": 125,
            }[termination]
            if exit_code != expected_exit:
                fail("invalid_schema", f"report command {index} exit code does not match its termination")
        if record["status"] != expected_status:
            fail("invalid_schema", f"report command {index} status does not match its exit code and termination")
        if failure_seen:
            fail("invalid_schema", "report recorded a command after a failed gate")
        if expected_status == "failed":
            failure_seen = True
        stdout_relative = expected_log_relative(run_id, index, "stdout")
        stderr_relative = expected_log_relative(run_id, index, "stderr")
        stdout_size = validate_log_metadata(
            record["stdout"], f"report command {index} stdout", stdout_relative, root, MAX_GATE_STDOUT_BYTES
        )
        stderr_size = validate_log_metadata(
            record["stderr"], f"report command {index} stderr", stderr_relative, root, MAX_GATE_STDERR_BYTES
        )
        if stdout_size + stderr_size > MAX_GATE_TOTAL_OUTPUT_BYTES:
            fail("resource_limit", f"report command {index} logs exceed the total output ceiling")
        expected_files.add(stdout_relative.name)
        expected_files.add(stderr_relative.name)

    if report_status == "passed":
        if len(records) != len(GATES) or failure_seen:
            fail("verification_failed", "a passed report must contain every successful fixed gate")
    elif report_status != "failed":
        fail("invalid_schema", "report status is invalid")
    return expected_files


def validate_run_directory(root: Path, run_id: str, expected_files: set[str]) -> None:
    descriptor = open_anchored_directory(
        root, expected_run_directory(run_id), "verification run directory", create=False
    )
    try:
        entries = os.listdir(descriptor)
        actual_names = set(entries)
        if actual_names != expected_files:
            fail("unexpected_data", "verification run directory has missing or unexpected files")
        for name in entries:
            try:
                mode = os.stat(name, dir_fd=descriptor, follow_symlinks=False).st_mode
            except OSError as error:
                fail("unreadable_file", f"cannot inspect verification run data {name}: {error}")
            if not stat.S_ISREG(mode):
                fail("unsafe_path", f"verification run data {name} is not a regular file")
    except OSError as error:
        fail("unreadable_file", f"cannot read verification run directory: {error}")
    finally:
        os.close(descriptor)


def parse_json_no_duplicates(raw: bytes, label: str) -> Any:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                fail("invalid_schema", f"{label} contains a duplicate object key: {key}")
            result[key] = value
        return result

    try:
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=reject_duplicates, parse_constant=lambda constant: fail("invalid_schema", f"{label} contains invalid JSON constant: {constant}"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail("invalid_json", f"cannot parse {label}: {error}")
    if raw != canonical_json(value):
        fail("noncanonical_json", f"{label} is not canonical JSON")
    return value


def read_json_no_duplicates(path: Path, label: str, maximum_bytes: int = MAX_REPORT_JSON_BYTES) -> Any:
    require_regular_file(path, label)
    try:
        if path.stat().st_size > maximum_bytes:
            fail("resource_limit", f"{label} exceeds its {maximum_bytes}-byte JSON ceiling")
        raw = path.read_bytes()
    except OSError as error:
        fail("unreadable_file", f"cannot read {label}: {error}")
    if len(raw) > maximum_bytes:
        fail("resource_limit", f"{label} exceeds its {maximum_bytes}-byte JSON ceiling")
    return parse_json_no_duplicates(raw, label)


def read_anchored_json_no_duplicates(
    root: Path,
    relative: Path,
    label: str,
    require_single_link: bool = False,
    maximum_bytes: int = MAX_REPORT_JSON_BYTES,
) -> Any:
    parent_descriptor = open_anchored_directory(root, relative.parent, f"{label} parent", create=False)
    try:
        descriptor = os.open(
            relative.name,
            os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0),
            dir_fd=parent_descriptor,
        )
    except OSError as error:
        os.close(parent_descriptor)
        fail("unsafe_path", f"cannot open {label}: {error}")
    os.close(parent_descriptor)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            fail("unsafe_path", f"{label} is not a regular file")
        if metadata.st_size > maximum_bytes:
            fail("resource_limit", f"{label} exceeds its {maximum_bytes}-byte JSON ceiling")
        if require_single_link and metadata.st_nlink != 1:
            fail("unsafe_path", f"{label} must not be a hard-linked canonical pointer")
        chunks: list[bytes] = []
        total_bytes = 0
        while True:
            block = os.read(descriptor, min(1024 * 1024, maximum_bytes - total_bytes + 1))
            if not block:
                break
            total_bytes += len(block)
            if total_bytes > maximum_bytes:
                fail("resource_limit", f"{label} exceeds its {maximum_bytes}-byte JSON ceiling")
            chunks.append(block)
    except OSError as error:
        fail("unreadable_file", f"cannot read {label}: {error}")
    finally:
        os.close(descriptor)
    return parse_json_no_duplicates(b"".join(chunks), label)


def require_absolute_path_string(value: Any, label: str) -> str:
    path = require_string(value, label)
    if not Path(path).is_absolute():
        fail("invalid_schema", f"{label} must be an absolute path")
    return path


def validate_environment_shape(value: Any, root: Path) -> None:
    environment = require_object(
        value,
        "report environment",
        {
            "architecture",
            "cargo_v",
            "config_inputs",
            "executables",
            "platform",
            "python",
            "rustc_vv",
            "variables",
        },
    )
    for field in ("architecture", "cargo_v", "platform", "python", "rustc_vv"):
        require_string(environment[field], f"report environment {field}")

    variables = require_object(environment["variables"], "report environment variables")
    expected_variable_names = {
        "CARGO_HOME",
        "HOME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "PATH",
        "PYTHONNOUSERSITE",
        "RUSTUP_HOME",
        "TEMP",
        "TMP",
        "TMPDIR",
        "TZ",
    }
    actual_variable_names = set(variables)
    if (
        actual_variable_names != expected_variable_names
        and actual_variable_names != expected_variable_names | {"SYSTEMROOT"}
    ):
        fail("invalid_schema", "report environment variables are not the exact safe environment")
    for name, item in variables.items():
        require_string(item, f"report environment variable {name}")
    for name in ("CARGO_HOME", "HOME", "RUSTUP_HOME", "TEMP", "TMP", "TMPDIR"):
        require_absolute_path_string(variables[name], f"report environment variable {name}")
    if variables["PYTHONNOUSERSITE"] != "1":
        fail("invalid_schema", "report environment must disable Python user-site imports")

    config_inputs = require_list(environment["config_inputs"], "report environment configuration inputs")
    expected_inputs = expected_config_inputs(root, variables)
    if len(config_inputs) != len(expected_inputs):
        fail("invalid_schema", "report environment must record every configuration discovery location")
    for index, raw_record in enumerate(config_inputs):
        expected_origin, expected_candidate = expected_inputs[index]
        expected_path = expected_candidate.as_posix()
        record = require_object(raw_record, f"report environment configuration input {index}")
        if record.get("origin") != expected_origin or record.get("path") != expected_path:
            fail("invalid_schema", "report environment configuration input origin or path is invalid")
        require_absolute_path_string(record["path"], f"report environment configuration input {index} path")
        require_absolute_path_string(
            record.get("resolved_path"), f"report environment configuration input {index} resolved path"
        )
        state = record.get("state")
        if state == "absent":
            require_object(
                record,
                f"report environment configuration input {index}",
                {"origin", "path", "resolved_path", "state"},
            )
        elif state == "file":
            require_object(
                record,
                f"report environment configuration input {index}",
                {"origin", "path", "resolved_path", "sha256", "size", "state"},
            )
            require_digest(record["sha256"], f"report environment configuration input {index} digest")
            require_integer(record["size"], f"report environment configuration input {index} size", minimum=0)
        else:
            fail("invalid_schema", f"report environment configuration input {index} state is invalid")

    executables = require_list(environment["executables"], "report environment executables")
    if len(executables) > len(EXECUTABLE_IDENTITY_NAMES):
        fail("invalid_schema", "report environment has too many executable identities")
    for index, raw_record in enumerate(executables):
        record = require_object(
            raw_record,
            f"report environment executable {index}",
            {"name", "path", "sha256", "size"},
        )
        if record["name"] != EXECUTABLE_IDENTITY_NAMES[index]:
            fail("invalid_schema", "report environment executable order is invalid")
        require_absolute_path_string(record["path"], f"report environment executable {index} path")
        require_digest(record["sha256"], f"report environment executable {index} digest")
        require_integer(record["size"], f"report environment executable {index} size", minimum=0)


def validate_report_shape(report: Any, root: Path) -> dict[str, Any]:
    value = require_object(
        report,
        "verification report",
        {
            "commands",
            "environment",
            "report_artifact",
            "report_identity",
            "run_id",
            "schema_version",
            "source_manifest",
            "started_at_utc",
            "status",
        },
    )
    if value["schema_version"] != REPORT_SCHEMA_VERSION:
        fail("invalid_schema", "verification report schema version is invalid")
    run_id = require_string(value["run_id"], "report run ID")
    if RUN_ID_RE.fullmatch(run_id) is None:
        fail("invalid_schema", "report run ID is invalid")
    timestamp = require_string(value["started_at_utc"], "report timestamp")
    if UTC_RE.fullmatch(timestamp) is None:
        fail("invalid_schema", "report timestamp must be UTC to whole-second precision")
    try:
        report_time = datetime.strptime(timestamp, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as error:
        fail("invalid_schema", f"report timestamp is invalid: {error}")
    now = datetime.now(timezone.utc)
    if report_time > now:
        fail("report_from_future", "report timestamp is in the future")
    if (now - report_time).total_seconds() > MAX_REPORT_AGE_SECONDS:
        fail("report_stale", f"report is older than the {MAX_REPORT_AGE_SECONDS}-second freshness window")
    if value["status"] not in {"passed", "failed"}:
        fail("invalid_schema", "report status is invalid")

    validate_environment_shape(value["environment"], root)

    report_artifact = require_object(value["report_artifact"], "report artifact", {"path"})
    report_relative = normalized_relative_path(report_artifact["path"], "report artifact path")
    if report_relative != expected_run_report_relative(run_id):
        fail("unexpected_data", "report artifact path does not match its run ID")

    source = require_object(value["source_manifest"], "report source manifest", {"file_count", "path", "sha256"})
    require_integer(source["file_count"], "report source manifest file count", minimum=0)
    relative = normalized_relative_path(source["path"], "report source manifest path")
    if relative != expected_source_manifest_relative(run_id):
        fail("unexpected_data", "report source manifest path does not match its run ID")
    require_digest(source["sha256"], "report source manifest digest")

    identity = require_object(value["report_identity"], "report identity", {"algorithm", "canonicalization", "digest"})
    if identity["algorithm"] != "sha256" or identity["canonicalization"] != CANONICALIZATION:
        fail("invalid_schema", "report identity algorithm or canonicalization is invalid")
    digest = require_digest(identity["digest"], "report identity digest")
    if digest != report_content_digest(value):
        fail("report_tampered", "report content digest does not match")
    return value


def validate_source_manifest(root: Path, report: dict[str, Any]) -> None:
    run_id = report["run_id"]
    source = report["source_manifest"]
    relative = expected_source_manifest_relative(run_id)
    path = checked_path(root, relative, "published source manifest")
    manifest = read_json_no_duplicates(path, "published source manifest", MAX_SOURCE_MANIFEST_JSON_BYTES)
    validate_source_manifest_shape(manifest)
    raw = canonical_json(manifest)
    if digest_bytes(raw) != source["sha256"]:
        fail("source_tampered", "published source manifest digest does not match the report")
    entries = manifest["inputs"]
    if len(entries) != source["file_count"]:
        fail("source_tampered", "published source manifest file count does not match the report")
    current = build_source_manifest(root)
    if current != manifest:
        fail("source_stale", "current source inputs do not match the published source manifest")


def validate_environment(root: Path, report: dict[str, Any]) -> None:
    current, error = collect_environment(root, safe_environment(root))
    if error is not None:
        fail("toolchain_identity_failed", error)
    if current != report["environment"]:
        fail("environment_drift", "current safe environment, executable, configuration, or toolchain identity differs from the report")


def read_published_report(root: Path) -> tuple[dict[str, Any], str]:
    report = validate_report_shape(
        read_anchored_json_no_duplicates(root, REPORT_RELATIVE, "verification report", require_single_link=True),
        root,
    )
    report_content = canonical_json(report)
    report_digest = digest_bytes(report_content)
    artifact_relative = expected_run_report_relative(report["run_id"])
    artifact = validate_report_shape(
        read_anchored_json_no_duplicates(root, artifact_relative, "immutable verification report"),
        root,
    )
    if canonical_json(artifact) != report_content:
        fail("canonical_pointer_replaced", "canonical report does not exactly match its immutable run report")
    pointer = read_anchored_json_no_duplicates(
        root, REPORT_POINTER_RELATIVE, "verification report pointer", require_single_link=True
    )
    validate_report_pointer(pointer, report, report_digest)
    return report, report_digest


def validate_published_report_binding(root: Path, report: dict[str, Any], report_digest: str) -> None:
    current, current_digest = read_published_report(root)
    if current_digest != report_digest or current["run_id"] != report["run_id"]:
        fail("canonical_pointer_replaced", "canonical report changed while verification evidence was checked")


def check_report(root: Path) -> tuple[dict[str, Any], str]:
    report, report_digest = read_published_report(root)
    expected_files = validate_command_list(root, report["run_id"], report["commands"], report["status"])
    validate_run_directory(root, report["run_id"], expected_files)
    validate_source_manifest(root, report)
    validate_environment(root, report)
    # Revalidate both mutable classes after their counterpart's probes.  The last
    # source comparison is immediately before pointer stability is checked.
    validate_source_manifest(root, report)
    validate_environment(root, report)
    validate_source_manifest(root, report)
    validate_published_report_binding(root, report, report_digest)
    if report["status"] != "passed":
        fail("verification_failed", "verification report status is not passed")
    return report, report_digest


def workspace_product_version(root: Path) -> str:
    manifest_path = checked_path(root, Path("Cargo.toml"), "workspace manifest")
    require_regular_file(manifest_path, "workspace manifest")
    try:
        with manifest_path.open("rb") as handle:
            manifest = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail("invalid_manifest", f"cannot read workspace product version: {error}")
    workspace = require_object(manifest.get("workspace"), "workspace manifest workspace")
    package = require_object(workspace.get("package"), "workspace manifest package")
    return require_string(package.get("version"), "workspace product version")


def build_attestation(root: Path, report: dict[str, Any], report_digest: str) -> dict[str, Any]:
    if report["status"] != "passed":
        fail("verification_failed", "only a passed verification report can be attested")
    attestation = {
        "commands": [
            {
                "argv": command["argv"],
                "exit_code": command["exit_code"],
                "status": command["status"],
                "termination": command["termination"],
            }
            for command in report["commands"]
        ],
        "product_version": workspace_product_version(root),
        "report": {
            "schema_version": report["schema_version"],
            "self_digest": report["report_identity"]["digest"],
            "sha256": report_digest,
        },
        "schema_version": ATTESTATION_SCHEMA_VERSION,
        "source_manifest": {
            "file_count": report["source_manifest"]["file_count"],
            "sha256": report["source_manifest"]["sha256"],
        },
        "status": "passed",
    }
    return validate_attestation_shape(attestation)


def validate_attestation_shape(value: Any) -> dict[str, Any]:
    attestation = require_object(
        value,
        "verification attestation",
        {"commands", "product_version", "report", "schema_version", "source_manifest", "status"},
    )
    if attestation["schema_version"] != ATTESTATION_SCHEMA_VERSION:
        fail("invalid_schema", "verification attestation schema version is invalid")
    require_string(attestation["product_version"], "verification attestation product version")
    if attestation["status"] != "passed":
        fail("verification_failed", "verification attestation status is not passed")

    report = require_object(
        attestation["report"],
        "verification attestation report",
        {"schema_version", "self_digest", "sha256"},
    )
    if report["schema_version"] != REPORT_SCHEMA_VERSION:
        fail("invalid_schema", "attested verification report schema version is invalid")
    require_digest(report["self_digest"], "attested verification report self digest")
    require_digest(report["sha256"], "attested verification report digest")

    source = require_object(
        attestation["source_manifest"],
        "verification attestation source manifest",
        {"file_count", "sha256"},
    )
    require_integer(source["file_count"], "verification attestation source file count", minimum=1)
    require_digest(source["sha256"], "verification attestation source manifest digest")

    commands = require_list(attestation["commands"], "verification attestation commands")
    if len(commands) != len(GATES):
        fail("invalid_schema", "verification attestation must cover every fixed gate")
    for index, raw_command in enumerate(commands):
        command = require_object(
            raw_command,
            f"verification attestation command {index}",
            {"argv", "exit_code", "status", "termination"},
        )
        argv = require_list(command["argv"], f"verification attestation command {index} argv")
        if argv != list(GATES[index][1]):
            fail("invalid_schema", f"verification attestation command {index} does not match the fixed gate")
        if command["exit_code"] != 0 or command["status"] != "passed" or command["termination"] != "completed":
            fail("verification_failed", f"verification attestation command {index} did not pass")
    return attestation


def publish_attestation(root: Path, report: dict[str, Any], report_digest: str) -> tuple[Path, str]:
    attestation = build_attestation(root, report, report_digest)
    content = canonical_json(attestation)
    digest = digest_bytes(content)
    relative = ATTESTATION_DIRECTORY / f"{ATTESTATION_PREFIX}{digest}.json"
    atomic_write(root, relative, content, "verification attestation destination")
    path = checked_path(root, relative, "verification attestation destination")
    os.chmod(path, 0o644, follow_symlinks=False)
    return relative, digest


def publish_report(root: Path, report: dict[str, Any]) -> str:
    validate_report_shape(report, root)
    content = canonical_json(report)
    if len(content) > MAX_REPORT_JSON_BYTES:
        fail("resource_limit", f"verification report exceeds its {MAX_REPORT_JSON_BYTES}-byte JSON ceiling")
    atomic_write(
        root,
        expected_run_report_relative(report["run_id"]),
        content,
        "immutable verification report destination",
    )
    atomic_write(root, REPORT_RELATIVE, content, "verification report destination")
    report_digest = digest_bytes(content)
    atomic_write(
        root,
        REPORT_POINTER_RELATIVE,
        canonical_json(build_report_pointer(report, report_digest)),
        "verification report pointer destination",
    )
    return report_digest


def run_verification(root: Path) -> tuple[dict[str, Any], bool, str]:
    started_at_utc = current_utc_timestamp()
    run_id = create_run_directory(root)
    manifest = build_source_manifest(root)
    manifest_content = source_manifest_bytes(manifest)
    source_manifest_relative = expected_source_manifest_relative(run_id)
    atomic_write(root, source_manifest_relative, manifest_content, "source manifest destination")
    source_manifest_digest = digest_bytes(manifest_content)

    gate_environment = safe_environment(root)
    environment, environment_error = collect_environment(root, gate_environment)
    commands: list[dict[str, Any]] = []
    status = "failed" if environment_error is not None else "passed"
    if environment_error is None:
        for index in range(len(GATES)):
            command = run_gate(root, run_id, index, gate_environment)
            commands.append(command)
            if command["status"] != "passed":
                status = "failed"
                break
        if status == "passed" and build_source_manifest(root) != manifest:
            status = "failed"

    report = build_report(
        run_id=run_id,
        started_at_utc=started_at_utc,
        environment=environment,
        source_manifest_digest=source_manifest_digest,
        source_file_count=len(manifest["inputs"]),
        commands=commands,
        status=status,
    )
    report_digest = publish_report(root, report)
    if status == "passed":
        report, report_digest = check_report(root)
    return report, status == "passed", report_digest


def receipt(mode: str, ok: bool, **details: Any) -> None:
    print(canonical_json({"mode": mode, "ok": ok, **details}).decode("utf-8"))


def receipt_details(report: dict[str, Any], report_digest: str) -> dict[str, Any]:
    return {
        "report_path": report["report_artifact"]["path"],
        "report_self_digest": report["report_identity"]["digest"],
        "report_sha256": report_digest,
        "run_id": report["run_id"],
        "source_manifest_sha256": report["source_manifest"]["sha256"],
        "status": report["status"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--run", action="store_true", help="run fixed verification gates and publish a durable report")
    mode.add_argument("--check", action="store_true", help="validate the published report, source inputs, environment, and raw logs")
    mode.add_argument("--attest", action="store_true", help="publish a host-neutral, content-addressed attestation from the current passed report")
    arguments = parser.parse_args()
    selected_mode = "run" if arguments.run else "attest" if arguments.attest else "check"
    try:
        if arguments.run:
            report, ok, report_digest = run_verification(ROOT)
            receipt(selected_mode, ok, **receipt_details(report, report_digest))
            return 0 if ok else 1
        report, report_digest = check_report(ROOT)
        if arguments.attest:
            path, digest = publish_attestation(ROOT, report, report_digest)
            receipt(
                selected_mode,
                True,
                attestation_path=path.as_posix(),
                attestation_sha256=digest,
                **receipt_details(report, report_digest),
            )
            return 0
        receipt(selected_mode, True, **receipt_details(report, report_digest))
        return 0
    except (VerificationError, OSError, ValueError, RuntimeError) as error:
        code = error.code if isinstance(error, VerificationError) else "verification_runner_failed"
        message = error.message if isinstance(error, VerificationError) else str(error)
        receipt(selected_mode, False, error={"code": code, "message": message})
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
