#!/usr/bin/env python3
"""Build Podway binaries and run all or one exact binary-backed end-to-end test."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess

from run_g005_vertical import cargo_target_directory, verification_root


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
        help="run one exact binary-backed E2E test after building the product binaries",
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
    # macOS validates a newly built executable on its first launch and serializes that
    # validation machine-wide, so a busy machine can stall a first exec beyond the
    # bounded daemon-install windows. One launch per binary absorbs that cost before
    # any test pays it inside a bounded window.
    run([str(podway), "version"])
    run([str(podwayd), "version"])

    v2rel003_debug_target = target / "e2e-v2rel003-development"
    run(
        [
            "cargo",
            "build",
            "--locked",
            "-p",
            "podway-daemon",
            "--bin",
            "podwayd",
            "--features",
            "development-v2-admission",
            "--target-dir",
            str(v2rel003_debug_target),
        ]
    )
    v2rel003_debug_daemon = (v2rel003_debug_target / "debug" / "podwayd").resolve()

    v2rel003_release_target = target / "e2e-v2rel003-release"
    run(
        [
            "cargo",
            "build",
            "--locked",
            "--release",
            "-p",
            "podway-daemon",
            "--bin",
            "podwayd",
            "--features",
            "development-v2-admission",
            "--target-dir",
            str(v2rel003_release_target),
        ]
    )
    v2rel003_release_daemon = (v2rel003_release_target / "release" / "podwayd").resolve()
    for binary in (v2rel003_debug_daemon, v2rel003_release_daemon):
        if not binary.is_file():
            raise SystemExit(f"cargo build did not produce expected Podway daemon: {binary}")
        run([str(binary), "version"])

    environment = os.environ.copy()
    environment["PODWAYD_TEST_BINARY"] = str(podwayd)
    environment["PODWAY_V2REL003_CLI"] = str(podway)
    environment["PODWAY_V2REL003_DAEMON_DEBUG"] = str(v2rel003_debug_daemon)
    environment["PODWAY_V2REL003_DAEMON_RELEASE"] = str(v2rel003_release_daemon)
    environment["PODWAY_V2REL003_QUALIFIER"] = str((ROOT / "tools" / "dev_runtime.py").resolve())
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
                "binaries": {
                    "podway": str(podway),
                    "podwayd": str(podwayd),
                    "podwayd_v2rel003_debug": str(v2rel003_debug_daemon),
                    "podwayd_v2rel003_release": str(v2rel003_release_daemon),
                },
                "mode": "e2e",
                "ok": True,
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
