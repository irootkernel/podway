#!/usr/bin/env python3
"""Build Podway binaries and run every binary-backed end-to-end target."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess

from run_g005_vertical import cargo_target_directory, produce_daemon_build_receipt, verification_root


ROOT = verification_root()


def run(argv: list[str], *, env: dict[str, str] | None = None) -> None:
    completed = subprocess.run(argv, cwd=ROOT, env=env, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def main() -> int:
    run(
        [
            "cargo",
            "build",
            "--locked",
            "-p",
            "podway-cli",
            "--bin",
            "podway",
            "-p",
            "podway-daemon",
            "--bin",
            "podwayd",
        ]
    )
    target = cargo_target_directory()
    podway = (target / "debug" / "podway").resolve()
    podwayd = (target / "debug" / "podwayd").resolve()
    if not podway.is_file() or not podwayd.is_file():
        raise SystemExit("cargo build did not produce both Podway binaries")
    receipt_path = produce_daemon_build_receipt(ROOT, podwayd)
    environment = os.environ.copy()
    environment["PODWAYD_TEST_BINARY"] = str(podwayd)
    environment["PODWAYD_BUILD_RECEIPT"] = str(receipt_path.resolve())
    run(
        [
            "cargo",
            "test",
            "--workspace",
            "--test",
            "e2e_*",
            "--locked",
            "--",
            "--include-ignored",
            "--test-threads=1",
        ],
        env=environment,
    )
    print(
        json.dumps(
            {
                "binaries": {"podway": str(podway), "podwayd": str(podwayd)},
                "mode": "e2e",
                "ok": True,
                "receipt": str(receipt_path.resolve()),
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
