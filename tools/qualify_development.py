#!/usr/bin/env python3
"""Freeze source before development gates, build and nonpublishable packaging."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

import release_archive

ROOT = Path(__file__).resolve().parents[1]
STEPS = (
    ("make", "test"),
    ("make", "release-prepare"),
    ("make", "test-fuzzing"),
    ("rustup", "run", "1.97.1", "cargo", "build", "--release", "--locked", "-p", "podway-cli", "--bin", "podway"),
)


def require_frozen(snapshot: str) -> None:
    if release_archive.source_snapshot_sha256() != snapshot:
        raise release_archive.ReleaseError("development source changed after it was frozen; affected checks must run on one unchanged snapshot")


def validate_checks(value: object, snapshot: str, binary: Path) -> None:
    if not isinstance(value, dict) or value.get("schema") != "podway.development-checks/v1" or value.get("source_snapshot_sha256") != snapshot:
        raise release_archive.ReleaseError("development checks do not bind the original frozen source")
    checks = value.get("checks")
    if not isinstance(checks, list) or len(checks) != len(STEPS):
        raise release_archive.ReleaseError("development checks are incomplete")
    for check, argv in zip(checks, STEPS):
        if not isinstance(check, dict) or check.get("argv") != list(argv) or check.get("exit_code") != 0 or any(check.get(field) != snapshot for field in ("source_before", "source_after")):
            raise release_archive.ReleaseError("development check command, exit or source binding differs")
    if value.get("binary_sha256") != hashlib.sha256(binary.read_bytes()).hexdigest():
        raise release_archive.ReleaseError("development binary differs from the checked build")


def main() -> int:
    if sys.flags.optimize:
        raise SystemExit("development qualification requires Python assertions")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    release_archive.require_native_host()
    if not args.output_dir.is_absolute() or args.output_dir.exists() or not args.output_dir.parent.is_dir():
        raise SystemExit("a fresh absolute owned output directory with an existing parent is required")
    snapshot = release_archive.source_snapshot_sha256()
    args.output_dir.mkdir(mode=0o700)
    receipt = args.output_dir / "development-checks.json"
    report = {"schema": "podway.development-checks/v1", "source_snapshot_sha256": snapshot, "checks": []}
    def save() -> None:
        receipt.write_text(json.dumps(report, indent=2) + "\n")
    save()
    for argv in STEPS:
        require_frozen(snapshot)
        code = subprocess.call(argv, cwd=ROOT)
        after = release_archive.source_snapshot_sha256()
        report["checks"].append({"argv": list(argv), "exit_code": code, "source_before": snapshot, "source_after": after})
        save()
        if code:
            raise SystemExit(code)
        require_frozen(snapshot)
    target = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    binary = (ROOT / target / "release/podway").resolve()
    report["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
    save()
    validate_checks(report, snapshot, binary)
    commands = (
        ["tools/release_archive.py", "package", "--podway", str(binary), "--artifact-class", "development-qualification", "--gate", "development", "--allow-dirty", "--development-source-snapshot-sha256", snapshot, "--development-checks", str(receipt)],
        ["tools/qualify_distribution.py", "qualify", "--gate", "development"],
        ["tools/create_dolgorae_handoff.py", "create", "--gate", "development"],
        ["tools/verify_release_bundle.py", "verify", "--gate", "development"],
    )
    for command in commands:
        require_frozen(snapshot)
        subprocess.run([sys.executable, *command, "--output-dir", str(args.output_dir)], cwd=ROOT, check=True)
        require_frozen(snapshot)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
