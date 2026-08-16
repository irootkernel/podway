#!/usr/bin/env python3
"""Fail-closed eligibility checks for the reduced patch release gate."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
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


class PatchReleaseError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise PatchReleaseError(message)


def git(*arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=ROOT,
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


def changed_paths(base: str) -> set[str]:
    output = git("diff", "--name-only", "--diff-filter=ACMRTD", f"{base}..HEAD")
    return {line for line in output.splitlines() if line}


def baseline_bytes(base: str, path: str) -> bytes:
    completed = subprocess.run(
        ["git", "show", f"{base}:{path}"],
        cwd=ROOT,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        fail(f"tested baseline does not contain {path}")
    return completed.stdout


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


def check(base: str, confirmed: str) -> dict[str, Any]:
    if confirmed != "yes":
        fail("PRIOR_MAKE_TEST_PASSED must equal yes")
    if git("status", "--porcelain=v1", "--untracked-files=normal"):
        fail("patch release eligibility requires a clean Git worktree")
    git("rev-parse", "--verify", f"{base}^{{commit}}")
    if subprocess.run(
        ["git", "merge-base", "--is-ancestor", base, "HEAD"], cwd=ROOT, check=False
    ).returncode != 0:
        fail("PATCH_BASE_COMMIT must be an ancestor of HEAD")

    old = workspace_version(baseline_bytes(base, "Cargo.toml"), "baseline Cargo.toml")
    new = workspace_version((ROOT / "Cargo.toml").read_bytes(), "current Cargo.toml")
    require_patch_bump(old, new)

    paths = changed_paths(base)
    allowed = ALWAYS_ALLOWED | VERSION_BOUND
    allowed.update(
        path
        for path in paths
        if path.startswith("crates/") and path.endswith("/Cargo.toml")
    )
    unexpected = sorted(paths - allowed)
    if unexpected:
        fail(f"tested candidate has non-release changes: {unexpected}")
    required = {"Cargo.toml", "contracts/contract-manifest-v1.json", "tools/release_archive.py"}
    missing = sorted(required - paths)
    if missing:
        fail(f"patch release omits required version-bound changes: {missing}")
    for manifest in sorted((ROOT / "crates").glob("*/Cargo.toml")):
        normalize_manifest(
            manifest.read_bytes(),
            new,
            f"current {manifest.relative_to(ROOT).as_posix()}",
        )
    for lock_path in (ROOT / "Cargo.lock", ROOT / "fuzz/Cargo.lock"):
        normalize_lock(
            lock_path.read_bytes(),
            new,
            f"current {lock_path.relative_to(ROOT).as_posix()}",
        )
    for path in sorted(paths & (VERSION_BOUND | {p for p in paths if p.endswith("/Cargo.toml")})):
        validate_version_bound_file(
            path,
            baseline_bytes(base, path),
            (ROOT / path).read_bytes(),
            old,
            new,
        )
    return {"base_commit": base, "mode": "check", "ok": True, "version": new}


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
    return {"mode": "self-test", "ok": True, "sentinels": 21}


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
