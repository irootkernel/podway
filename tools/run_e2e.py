#!/usr/bin/env python3
"""Build Podway binaries and run all or one exact binary-backed end-to-end test."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess

from run_g005_vertical import cargo_target_directory, produce_daemon_build_receipt, verification_root


ROOT = verification_root()
EXACT_TEST_RE = re.compile(
    r"^(?P<package>[A-Za-z0-9_-]+)::(?P<target>e2e_[A-Za-z0-9_]+)::"
    r"(?P<function>[A-Za-z0-9_]+(?:::[A-Za-z0-9_]+)*)$"
)


def run(argv: list[str], *, env: dict[str, str] | None = None) -> None:
    completed = subprocess.run(argv, cwd=ROOT, env=env, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def parse_exact_test(value: str) -> tuple[str, str, str]:
    match = EXACT_TEST_RE.fullmatch(value)
    if match is None:
        raise argparse.ArgumentTypeError(
            "exact test must be PACKAGE::e2e_TARGET::QUALIFIED_FUNCTION using Cargo/Rust identifiers"
        )
    return match.group("package"), match.group("target"), match.group("function")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--exact-test",
        type=parse_exact_test,
        metavar="PACKAGE::TARGET::FUNCTION",
        help="run one exact binary-backed E2E test after preparing canonical build evidence",
    )
    arguments = parser.parse_args()
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
    if arguments.exact_test is None:
        test_command = [
            "cargo",
            "test",
            "--workspace",
            "--test",
            "e2e_*",
            "--locked",
            "--",
            "--include-ignored",
            "--test-threads=1",
        ]
    else:
        package, test_target, function = arguments.exact_test
        test_command = [
            "cargo",
            "test",
            "-p",
            package,
            "--test",
            test_target,
            function,
            "--locked",
            "--",
            "--exact",
            "--include-ignored",
            "--test-threads=1",
        ]
    run(test_command, env=environment)
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
