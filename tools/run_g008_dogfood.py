#!/usr/bin/env python3
"""Build current binaries and run the four-preset product-binary smoke suite."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys

def verification_root() -> Path:
    return Path(__file__).resolve().parents[1]


ROOT = verification_root()
TEST_NAME = (
    "e2e_phase4_production_vertical::"
    "public_cli_starts_all_four_presets_and_reports_first_action"
)
EVIDENCE_PREFIX = "G008_DOGFOOD_EVIDENCE="


def cargo_target_directory() -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    target = Path(configured) if configured else Path("target")
    if not target.is_absolute():
        target = ROOT / target
    return target.resolve()


def run(argv: list[str], *, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        argv,
        cwd=ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        sys.stdout.write(completed.stdout)
        sys.stderr.write(completed.stderr)
        raise SystemExit(completed.returncode)
    return completed


def main() -> int:
    run(["cargo", "build", "-p", "podway-daemon", "--bin", "podwayd"])
    daemon = (cargo_target_directory() / "debug" / "podwayd").resolve()
    if not daemon.is_file():
        raise SystemExit(f"current podwayd artifact is missing: {daemon}")

    environment = os.environ.copy()
    environment["PODWAYD_TEST_BINARY"] = str(daemon)
    completed = run(
        [
            "cargo",
            "test",
            "-p",
            "podway-cli",
            "--test",
            "e2e_suite",
            TEST_NAME,
            "--",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ],
        env=environment,
    )
    combined = completed.stdout + completed.stderr
    evidence_lines = [
        line.split(EVIDENCE_PREFIX, 1)[1]
        for line in combined.splitlines()
        if EVIDENCE_PREFIX in line
    ]
    if len(evidence_lines) != 1:
        raise SystemExit("dogfood test did not emit exactly one structured evidence record")
    scenarios = json.loads(evidence_lines[0])
    required = {"sw-dev", "bug-fix", "docs-only", "analysis"}
    if set(scenarios) != required:
        raise SystemExit("dogfood evidence does not cover the four shipped presets")
    for preset, result in scenarios.items():
        if result.get("next_checks", 0) < 1 or result.get("commands", 0) < 1:
            raise SystemExit(f"{preset} did not record command and next evidence")
        if not isinstance(result.get("stage_topology"), list) or not result["stage_topology"]:
            raise SystemExit(f"{preset} did not emit ordered stage topology evidence")
        if not isinstance(result.get("readiness_millis"), int) or result["readiness_millis"] > 10_000:
            raise SystemExit(f"{preset} did not emit bounded readiness measurement")

    print(
        json.dumps(
            {
                "binary": str(daemon),
                "goal": "G008",
                "ok": True,
                "test": TEST_NAME,
                "conformanceCells": 4,
                "scenarios": scenarios,
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
