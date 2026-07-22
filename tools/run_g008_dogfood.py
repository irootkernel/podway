#!/usr/bin/env python3
"""Build current binaries and run the ignored Phase 7 four-preset dogfood suite."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
from run_g005_vertical import produce_daemon_build_receipt

def verification_root() -> Path:
    return Path(__file__).resolve().parents[1]


ROOT = verification_root()
TEST_NAME = "public_cli_dogfoods_all_four_presets_with_retry_return_and_next_evidence"
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
    daemon = cargo_target_directory() / "debug" / "podwayd"
    receipt = produce_daemon_build_receipt(ROOT, daemon)

    environment = os.environ.copy()
    environment["PODWAYD_TEST_BINARY"] = str(daemon.resolve())
    environment["PODWAYD_BUILD_RECEIPT"] = str(receipt.resolve())
    completed = run(
        [
            "cargo",
            "test",
            "-p",
            "podway-cli",
            "--test",
            "e2e_phase4_production_vertical",
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
        if result.get("retry", 0) < 1 or result.get("return", 0) < 1:
            raise SystemExit(f"{preset} did not cover retry and return")
        if result.get("next_checks", 0) < 1 or result.get("commands", 0) < 1:
            raise SystemExit(f"{preset} did not record command and next evidence")
        if not isinstance(result.get("stage_topology"), list) or not result["stage_topology"]:
            raise SystemExit(f"{preset} did not emit ordered stage topology evidence")
        if not isinstance(result.get("readiness_millis"), int) or result["readiness_millis"] > 10_000:
            raise SystemExit(f"{preset} did not emit bounded readiness measurement")

    print(
        json.dumps(
            {
                "binary": str(daemon.resolve()),
                "goal": "G008",
                "ok": True,
                "receipt": str(receipt.resolve()),
                "test": TEST_NAME,
                "conformanceCells": 12,
                "scenarios": scenarios,
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
