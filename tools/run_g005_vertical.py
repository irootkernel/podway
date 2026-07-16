#!/usr/bin/env python3
"""Build the current daemon binary and run the ignored G005 public vertical test."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
TEST_NAME = "public_cli_production_vertical_covers_g005_lifecycle_recovery_replay_and_conflict"


def run(argv: list[str], *, env: dict[str, str] | None = None) -> None:
    completed = subprocess.run(argv, cwd=ROOT, env=env, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def cargo_target_directory() -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    target = Path(configured) if configured else Path("target")
    if not target.is_absolute():
        target = ROOT / target
    return target.resolve()

def main() -> int:
    run(["cargo", "build", "-p", "podway-daemon", "--bin", "podwayd"])
    daemon = cargo_target_directory() / "debug" / "podwayd"
    if not daemon.is_file():
        raise SystemExit(f"current podwayd artifact is missing: {daemon}")

    environment = os.environ.copy()
    environment["PODWAYD_TEST_BINARY"] = str(daemon)
    run(
        [
            "cargo",
            "test",
            "-p",
            "podway-cli",
            "--test",
            "phase4_production_vertical",
            TEST_NAME,
            "--",
            "--ignored",
            "--nocapture",
        ],
        env=environment,
    )
    print(
        json.dumps(
            {
                "binary": str(daemon),
                "goal": "G005",
                "ok": True,
                "test": TEST_NAME,
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
