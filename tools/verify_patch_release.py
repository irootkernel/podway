#!/usr/bin/env python3
"""Fail-closed eligibility checks for the reduced patch release gate."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import tomllib
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
ALWAYS_ALLOWED = {"README.md", "RELEASE_NOTES.md"}
VERSION_BOUND = {
    "Cargo.toml",
    "Cargo.lock",
    "fuzz/Cargo.lock",
    "contracts/contract-manifest-v1.json",
    "tools/release_archive.py",
}
PRODUCT_PACKAGES = {
    "podway-cli",
    "podway-config",
    "podway-core",
    "podway-daemon",
    "podway-git",
    "podway-presets",
    "podway-protocol",
    "podway-service",
    "podway-store",
}
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
GIT_OBJECT = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})")


class PatchReleaseError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise PatchReleaseError(message)


def git_at(root: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=root,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        fail(completed.stderr.strip() or f"git {' '.join(arguments)} failed")
    return completed.stdout.strip()


def parse_version(value: str, label: str) -> tuple[int, int, int]:
    match = VERSION.fullmatch(value)
    if match is None:
        fail(f"{label} must be a stable semantic version: {value!r}")
    return tuple(int(part) for part in match.groups())  # type: ignore[return-value]


def require_patch_bump(old: str, new: str) -> None:
    old_parts = parse_version(old, "baseline product version")
    new_parts = parse_version(new, "current product version")
    if new_parts != (old_parts[0], old_parts[1], old_parts[2] + 1):
        fail(f"reduced release requires an exact +1 patch bump: {old} -> {new}")


def toml_bytes(data: bytes, label: str) -> dict[str, Any]:
    try:
        value = tomllib.loads(data.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as error:
        fail(f"{label} is invalid TOML: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a TOML table")
    return value


def workspace_version(data: bytes, label: str) -> str:
    value = toml_bytes(data, label)
    try:
        version = value["workspace"]["package"]["version"]
    except (KeyError, TypeError) as error:
        fail(f"{label} omits workspace.package.version: {error}")
    if not isinstance(version, str):
        fail(f"{label} workspace.package.version must be a string")
    return version


def normalize_manifest(data: bytes, version: str, label: str) -> dict[str, Any]:
    value = toml_bytes(data, label)
    package = value.get("package")
    if isinstance(package, dict) and "version" in package:
        if package["version"] != version:
            fail(f"{label} package version does not equal {version}")
        package["version"] = "<product-version>"
    workspace = value.get("workspace")
    if isinstance(workspace, dict):
        package = workspace.get("package")
        if isinstance(package, dict) and "version" in package:
            if package["version"] != version:
                fail(f"{label} workspace version does not equal {version}")
            package["version"] = "<product-version>"
    return value


def normalize_lock(data: bytes, version: str, label: str) -> dict[str, Any]:
    value = toml_bytes(data, label)
    packages = value.get("package")
    if not isinstance(packages, list):
        fail(f"{label} omits package entries")
    for package in packages:
        if not isinstance(package, dict):
            fail(f"{label} contains a malformed package entry")
        name = package.get("name")
        if name in PRODUCT_PACKAGES:
            if package.get("version") != version:
                fail(f"{label} {name} version does not equal {version}")
            package["version"] = "<product-version>"
    return value


def changed_paths(root: Path, base: str) -> set[str]:
    output = git_at(root, "diff", "--name-only", "--diff-filter=ACMRTD", f"{base}..HEAD")
    return {line for line in output.splitlines() if line}


def revision_bytes(root: Path, revision: str, path: str) -> bytes:
    completed = subprocess.run(
        ["git", "show", f"{revision}:{path}"],
        cwd=root,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        fail(f"{revision} does not contain {path}")
    return completed.stdout


def git_entry(root: Path, revision: str, path: str) -> tuple[str, str] | None:
    completed = subprocess.run(
        ["git", "ls-tree", "-z", revision, "--", path],
        cwd=root,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        fail(completed.stderr.decode(errors="replace").strip() or f"cannot inspect {revision}:{path}")
    if not completed.stdout:
        return None
    entries = [entry for entry in completed.stdout.split(b"\0") if entry]
    if len(entries) != 1:
        fail(f"{revision}:{path} does not resolve to exactly one Git entry")
    try:
        metadata, entry_path = entries[0].split(b"\t", 1)
        mode, kind, _object_id = metadata.decode("ascii").split(" ", 2)
        decoded_path = entry_path.decode("utf-8")
    except (UnicodeError, ValueError) as error:
        fail(f"cannot decode Git entry for {revision}:{path}: {error}")
    if decoded_path != path:
        fail(f"Git entry path mismatch for {revision}:{path}")
    return mode, kind


def require_regular_git_file(root: Path, revision: str, path: str) -> tuple[str, str]:
    entry = git_entry(root, revision, path)
    if entry is None:
        fail(f"{revision} does not contain {path}")
    if entry != ("100644", "blob"):
        fail(f"{revision}:{path} must be a non-executable regular Git blob, found {entry}")
    return entry


def require_regular_worktree_file(root: Path, path: str) -> None:
    candidate = root / path
    try:
        mode = candidate.lstat().st_mode
    except OSError as error:
        fail(f"current {path} is not readable: {error}")
    if not stat.S_ISREG(mode):
        fail(f"current {path} must be a regular non-symlink file")


def current_manifest_paths(root: Path) -> list[str]:
    output = git_at(root, "ls-tree", "-r", "--name-only", "HEAD", "--", "crates")
    return sorted(
        path
        for path in output.splitlines()
        if path.startswith("crates/") and path.endswith("/Cargo.toml")
    )


def validate_version_bound_file(path: str, base: bytes, current: bytes, old: str, new: str) -> None:
    if path.endswith("Cargo.toml"):
        if normalize_manifest(base, old, f"baseline {path}") != normalize_manifest(
            current, new, f"current {path}"
        ):
            fail(f"{path} changes more than the product version")
        return
    if path.endswith("Cargo.lock"):
        if normalize_lock(base, old, f"baseline {path}") != normalize_lock(
            current, new, f"current {path}"
        ):
            fail(f"{path} changes more than Podway package versions")
        return
    if path == "contracts/contract-manifest-v1.json":
        before = json.loads(base)
        after = json.loads(current)
        if before.get("product_version") != old or after.get("product_version") != new:
            fail("contract manifest product version is inconsistent")
        before["product_version"] = after["product_version"] = "<product-version>"
        before["digest"] = after["digest"] = "<manifest-digest>"
        if before != after:
            fail("contract manifest changes more than version-bound identity")
        return
    if path == "tools/release_archive.py":
        if current.decode("utf-8").replace(new, old) != base.decode("utf-8"):
            fail("release_archive.py changes more than product-version literals")
        return
    fail(f"unsupported version-bound path: {path}")


def check(base: str, confirmed: str, root: Path = ROOT) -> dict[str, Any]:
    if confirmed != "yes":
        fail("PRIOR_MAKE_TEST_PASSED must equal yes")
    if git_at(root, "status", "--porcelain=v1", "--untracked-files=normal"):
        fail("patch release eligibility requires a clean Git worktree")
    resolved_base = git_at(root, "rev-parse", "--verify", f"{base}^{{commit}}")
    if GIT_OBJECT.fullmatch(base) is None or base != resolved_base:
        fail("PATCH_BASE_COMMIT must be the full immutable commit identity")
    if subprocess.run(
        ["git", "merge-base", "--is-ancestor", resolved_base, "HEAD"], cwd=root, check=False
    ).returncode != 0:
        fail("PATCH_BASE_COMMIT must be an ancestor of HEAD")

    require_regular_git_file(root, resolved_base, "Cargo.toml")
    require_regular_git_file(root, "HEAD", "Cargo.toml")
    require_regular_worktree_file(root, "Cargo.toml")
    old = workspace_version(revision_bytes(root, resolved_base, "Cargo.toml"), "baseline Cargo.toml")
    new = workspace_version(revision_bytes(root, "HEAD", "Cargo.toml"), "current Cargo.toml")
    require_patch_bump(old, new)

    paths = changed_paths(root, resolved_base)
    allowed = ALWAYS_ALLOWED | VERSION_BOUND
    allowed.update(
        path
        for path in paths
        if path.startswith("crates/") and path.endswith("/Cargo.toml")
    )
    unexpected = sorted(paths - allowed)
    if unexpected:
        fail(f"tested candidate has non-release changes: {unexpected}")
    for path in sorted(paths):
        current_entry = require_regular_git_file(root, "HEAD", path)
        require_regular_worktree_file(root, path)
        baseline_entry = git_entry(root, resolved_base, path)
        if baseline_entry is not None and baseline_entry != current_entry:
            fail(f"{path} changes Git object type or mode")
        if baseline_entry is None and path not in ALWAYS_ALLOWED:
            fail(f"tested baseline does not contain {path}")
    required = {"Cargo.toml", "contracts/contract-manifest-v1.json", "tools/release_archive.py"}
    missing = sorted(required - paths)
    if missing:
        fail(f"patch release omits required version-bound changes: {missing}")
    for manifest in current_manifest_paths(root):
        require_regular_git_file(root, "HEAD", manifest)
        require_regular_worktree_file(root, manifest)
        normalize_manifest(
            revision_bytes(root, "HEAD", manifest),
            new,
            f"current {manifest}",
        )
    for lock_path in ("Cargo.lock", "fuzz/Cargo.lock"):
        require_regular_git_file(root, "HEAD", lock_path)
        require_regular_worktree_file(root, lock_path)
        normalize_lock(
            revision_bytes(root, "HEAD", lock_path),
            new,
            f"current {lock_path}",
        )
    for path in sorted(paths & (VERSION_BOUND | {p for p in paths if p.endswith("/Cargo.toml")})):
        validate_version_bound_file(
            path,
            revision_bytes(root, resolved_base, path),
            revision_bytes(root, "HEAD", path),
            old,
            new,
        )
    return {
        "base_commit": resolved_base,
        "mode": "check",
        "ok": True,
        "prior_make_test_passed": True,
        "version": new,
    }


def write_fixture_version(root: Path, version: str) -> None:
    lock = f'[[package]]\nname = "podway-core"\nversion = "{version}"\n'
    (root / "Cargo.toml").write_text(
        f'[workspace.package]\nversion = "{version}"\n', encoding="utf-8"
    )
    (root / "Cargo.lock").write_text(lock, encoding="utf-8")
    (root / "fuzz/Cargo.lock").write_text(lock, encoding="utf-8")
    (root / "crates/podway-core/Cargo.toml").write_text(
        f'[package]\nname = "podway-core"\nversion = "{version}"\n', encoding="utf-8"
    )
    (root / "contracts/contract-manifest-v1.json").write_text(
        json.dumps({"digest": f"sha256:{version}", "product_version": version}) + "\n",
        encoding="utf-8",
    )
    (root / "tools/release_archive.py").write_text(
        f'PRODUCT_VERSION = "{version}"\n', encoding="utf-8"
    )


def patch_release_fixture(root: Path, mutation: str = "valid") -> str:
    for directory in (
        "contracts",
        "crates/podway-core",
        "fuzz",
        "tools",
    ):
        (root / directory).mkdir(parents=True, exist_ok=True)
    git_at(root, "init", "--quiet")
    git_at(root, "config", "user.email", "patch-release@example.invalid")
    git_at(root, "config", "user.name", "Patch Release Fixture")
    (root / "README.md").write_text("fixture\n", encoding="utf-8")
    (root / "RELEASE_NOTES.md").write_text("fixture\n", encoding="utf-8")
    write_fixture_version(root, "1.2.3")
    git_at(root, "add", "--all")
    git_at(root, "commit", "--quiet", "-m", "baseline")
    baseline = git_at(root, "rev-parse", "HEAD")

    write_fixture_version(root, "1.2.4")
    if mutation == "lock-symlink":
        external = root.parent / f"{root.name}-external-Cargo.lock"
        external.write_text(
            '[[package]]\nname = "podway-core"\nversion = "1.2.4"\n',
            encoding="utf-8",
        )
        (root / "Cargo.lock").unlink()
        (root / "Cargo.lock").symlink_to(external)
    elif mutation == "archive-executable":
        (root / "tools/release_archive.py").chmod(0o755)
    elif mutation == "unexpected-source":
        source = root / "crates/podway-core/src/lib.rs"
        source.parent.mkdir(parents=True)
        source.write_text("pub fn unexpected() {}\n", encoding="utf-8")
    elif mutation != "valid":
        fail(f"unknown patch release fixture mutation: {mutation}")
    git_at(root, "add", "--all")
    git_at(root, "commit", "--quiet", "-m", "release candidate")
    return baseline


def expect_patch_rejection(action: Any, label: str) -> None:
    try:
        action()
    except PatchReleaseError:
        return
    fail(f"patch release self-test accepted {label}")


def self_test() -> dict[str, Any]:
    if parse_version("1.2.3", "fixture") != (1, 2, 3):
        fail("semantic-version parser is inconsistent")
    for invalid in ("1.2", "v1.2.3", "1.2.3-rc.1", "01.2.3"):
        try:
            parse_version(invalid, "fixture")
        except PatchReleaseError:
            pass
        else:
            fail(f"semantic-version parser accepted {invalid!r}")
    require_patch_bump("1.2.3", "1.2.4")
    for old, new in (("1.2.3", "1.3.0"), ("1.2.3", "2.0.0"), ("1.2.3", "1.2.5")):
        try:
            require_patch_bump(old, new)
        except PatchReleaseError:
            pass
        else:
            fail(f"patch-bump check accepted {old} -> {new}")
    baseline_manifest = b'[package]\nname = "podway-core"\nversion = "1.2.3"\n'
    current_manifest = b'[package]\nname = "podway-core"\nversion = "1.2.4"\n'
    validate_version_bound_file(
        "crates/podway-core/Cargo.toml",
        baseline_manifest,
        current_manifest,
        "1.2.3",
        "1.2.4",
    )
    lock = b'[[package]]\nname = "podway-core"\nversion = "1.2.4"\n\n[[package]]\nname = "podway-fuzz"\nversion = "0.1.0"\n'
    normalize_lock(lock, "1.2.4", "fixture Cargo.lock")
    current_version = workspace_version((ROOT / "Cargo.toml").read_bytes(), "Cargo.toml")
    for manifest in sorted((ROOT / "crates").glob("*/Cargo.toml")):
        normalize_manifest(
            manifest.read_bytes(),
            current_version,
            manifest.relative_to(ROOT).as_posix(),
        )
    for lock_path in (ROOT / "Cargo.lock", ROOT / "fuzz/Cargo.lock"):
        normalize_lock(
            lock_path.read_bytes(),
            current_version,
            lock_path.relative_to(ROOT).as_posix(),
        )
    with tempfile.TemporaryDirectory(prefix="podway-patch-release-") as temporary:
        fixture_root = Path(temporary)
        valid = fixture_root / "valid"
        valid.mkdir()
        baseline = patch_release_fixture(valid)
        result = check(baseline, "yes", valid)
        if result["base_commit"] != baseline or result["prior_make_test_passed"] is not True:
            fail("valid patch release fixture returned incomplete gate identity")
        expect_patch_rejection(
            lambda: check("HEAD~1", "yes", valid),
            "a symbolic baseline",
        )
        expect_patch_rejection(
            lambda: check(baseline[:12], "yes", valid),
            "an abbreviated baseline",
        )
        expect_patch_rejection(
            lambda: check(baseline, "no", valid),
            "a missing prior make-test confirmation",
        )
        for name, mutation in (
            ("lock-symlink", "lock-symlink"),
            ("archive-executable", "archive-executable"),
            ("unexpected-source", "unexpected-source"),
        ):
            fixture = fixture_root / name
            fixture.mkdir()
            mutated_baseline = patch_release_fixture(fixture, mutation)
            expect_patch_rejection(
                lambda fixture=fixture, mutated_baseline=mutated_baseline: check(
                    mutated_baseline, "yes", fixture
                ),
                mutation,
            )
    return {"mode": "self-test", "ok": True, "sentinels": 28}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="mode", required=True)
    subparsers.add_parser("self-test")
    check_parser = subparsers.add_parser("check")
    check_parser.add_argument("--base", required=True)
    check_parser.add_argument("--confirmed", required=True)
    arguments = parser.parse_args()
    try:
        result = self_test() if arguments.mode == "self-test" else check(arguments.base, arguments.confirmed)
    except (PatchReleaseError, OSError, UnicodeError, json.JSONDecodeError) as error:
        print(json.dumps({"error": str(error), "mode": arguments.mode, "ok": False}), file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
