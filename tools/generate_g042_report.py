#!/usr/bin/env python3
"""Generate the terminal G042 api-package-test-report from real command exit codes.

The G042 report is a terminal gate that nothing else validates, so this
generator reproduces the frozen artifact's exact shape while recording only
authentic results: it runs the seven qualification commands verbatim (cargo
and verifier invocations) from the repository root, captures each real exit
code, and derives the aggregate ``result.status`` from those codes. No verdict
is ever hard-coded. Source bindings are recomputed from current disk.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
from pathlib import Path
import shlex
import subprocess
import sys
from typing import Any

from g009_common import canonical_json, sha256_file
from verify_g009_qualification import G036_TARGET

ROOT = Path(__file__).resolve().parents[1]
REPORT_PATH = ROOT / "artifacts/g042/g042-test-report.json"

SCOPE = "G036 evidence hardening plus store/daemon review-blocker remediation"

# The seven qualification commands, run verbatim from the repository root.
COMMANDS: tuple[list[str], ...] = (
    ["python3", "tools/verify_g009_qualification.py", "--self-test"],
    [
        "python3", "tools/verify_g009_qualification.py",
        "--product-acceptance-matrix", "release/product-acceptance-matrix-v1.json",
    ],
    [
        "python3", "tools/verify_g009_qualification.py",
        "--g036-test-report", "artifacts/g036/g036-test-report.json",
    ],
    ["cargo", "test", "--workspace", "--locked"],
    ["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
    ["cargo", "fmt", "--all", "--", "--check"],
    ["git", "diff", "--check"],
)

# Exact evidence files whose current digests are bound into the report.
SOURCE_BINDING_PATHS: tuple[str, ...] = (
    "artifacts/g036/g036-test-report.json",
    "release/product-acceptance-matrix-v1.json",
    "release/g009-release-policy-v1.json",
    "release/migration-evidence-v1.json",
    "tools/verify_g009_qualification.py",
)

ADVERSARIAL_COVERAGE: tuple[str, ...] = (
    "ambient product-tree write denial",
    "IP network denial with Unix-domain socket-only bind/connect",
    "immutable checksum-verified vendor materialization",
    "exact test-binary/function replay markers",
    "publication destination replacement race",
    "concurrent publication winner identity",
    "cleanup-fault residual recovery",
    "session-clear terminal replay",
    "scheduler final-owner unregister",
    "observability drop/failure accounting",
    "normal and sandbox TMPDIR Unix-socket path bounds",
)

UNSUPPORTED_TARGETS: tuple[str, ...] = (
    "x86_64-apple-darwin",
    "Rosetta translation",
    "universal/fat Mach-O",
)


def _run_command(argv: list[str]) -> dict[str, Any]:
    """Run one command from the repository root and record its real exit code."""
    sys.stderr.write(f"[g042] running: {shlex.join(argv)}\n")
    sys.stderr.flush()
    completed = subprocess.run(argv, cwd=ROOT, check=False)
    exit_code = completed.returncode
    return {
        "argv": list(argv),
        "exitCode": exit_code,
        "verdict": "passed" if exit_code == 0 else "failed",
    }


def _source_bindings() -> dict[str, str]:
    bindings: dict[str, str] = {}
    for relative in SOURCE_BINDING_PATHS:
        candidate = ROOT / relative
        if candidate.is_symlink() or not candidate.is_file():
            raise SystemExit(f"G042 source binding is absent or unsafe: {relative}")
        bindings[relative] = sha256_file(candidate)
    return bindings


def build_report(command_receipts: list[dict[str, Any]]) -> dict[str, Any]:
    status = "passed" if all(receipt["exitCode"] == 0 for receipt in command_receipts) else "failed"
    return {
        "schemaVersion": 1,
        "kind": "api-package-test-report",
        "storyId": "G042",
        "generatedAt": datetime.now(timezone.utc).isoformat(),
        "target": dict(G036_TARGET),
        "scope": SCOPE,
        "result": {"status": status},
        "commands": command_receipts,
        "sourceBindings": _source_bindings(),
        "adversarialCoverage": list(ADVERSARIAL_COVERAGE),
        "unsupportedTargets": list(UNSUPPORTED_TARGETS),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="run nothing; print the resolved command list and exit",
    )
    args = parser.parse_args()

    if args.dry_run:
        print(f"G042 report output: {REPORT_PATH}")
        print(f"G042 resolved command list ({len(COMMANDS)} commands, cwd={ROOT}):")
        for argv in COMMANDS:
            print(f"  {shlex.join(argv)}")
        print("G042 source bindings:")
        for relative in SOURCE_BINDING_PATHS:
            print(f"  {relative}")
        return 0

    command_receipts = [_run_command(list(argv)) for argv in COMMANDS]
    report = build_report(command_receipts)
    REPORT_PATH.parent.mkdir(parents=True, exist_ok=True)
    payload = canonical_json(report) + b"\n"
    REPORT_PATH.write_bytes(payload)
    digest = sha256_file(REPORT_PATH)
    print(f"{REPORT_PATH} {digest} {report['result']['status']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
