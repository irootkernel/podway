#!/usr/bin/env python3
"""Build the single Podway CLI in both profiles and run binary-backed E2E tests."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import select
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
EXACT_TEST_RE = re.compile(
    r"^(?P<package>[A-Za-z0-9_-]+)::(?P<target>e2e_[A-Za-z0-9_]+)::"
    r"(?P<function>[A-Za-z0-9_]+(?:::[A-Za-z0-9_]+)*)$"
)


def run(argv: list[str], *, env: dict[str, str] | None = None) -> None:
    completed = subprocess.run(argv, cwd=ROOT, env=env, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def cargo_target_directory() -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    target = Path(configured) if configured else ROOT / "target"
    if not target.is_absolute():
        target = ROOT / target
    return target.resolve()


def parse_exact_test(value: str) -> tuple[str, str, str]:
    match = EXACT_TEST_RE.fullmatch(value)
    if match is None:
        raise argparse.ArgumentTypeError(
            "exact test must be PACKAGE::e2e_TARGET::QUALIFIED_FUNCTION using Cargo/Rust identifiers"
        )
    return match.group("package"), match.group("target"), match.group("function")


def run_exact(argv: list[str], environment: dict[str, str], *, timeout: int = 7200) -> None:
    """Require libtest to execute one test, rather than accept an empty filter."""
    results = []
    with subprocess.Popen(
        argv, cwd=ROOT, env=environment, stdout=subprocess.PIPE, start_new_session=True
    ) as child:
        assert child.stdout is not None
        end = time.monotonic() + timeout
        pending = b""
        try:
            while True:
                if time.monotonic() >= end:
                    raise SystemExit("exact test exceeded its execution deadline")
                if not select.select([child.stdout], [], [], max(0, min(1, end - time.monotonic())))[0]:
                    continue
                block = os.read(child.stdout.fileno(), 65536)
                if not block:
                    break
                pending += block
                while b"\n" in pending:
                    raw, pending = pending.split(b"\n", 1)
                    if len(raw) > 65536:
                        raise SystemExit("exact test output line exceeds its bound")
                    line = raw.decode("utf-8", errors="replace")
                    print(line, flush=True)
                    if line.startswith("test result:"):
                        results.append(line.strip())
                        if len(results) > 1:
                            raise SystemExit("exact selection produced multiple test results")
                if len(pending) > 65536:
                    raise SystemExit("exact test output line exceeds its bound")
            if pending:
                print(pending.decode("utf-8", errors="replace"), flush=True)
            code = child.wait(timeout=max(0, end - time.monotonic()))
        finally:
            # Also terminate descendants that retained the pipe or outlived a
            # failed controller. The group belongs only to this exact test.
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait(timeout=5)
    if code != 0:
        raise SystemExit(code)
    if len(results) != 1 or re.fullmatch(
        r"test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; \d+ filtered out; finished in .+",
        results[0],
    ) is None:
        raise SystemExit("exact E2E selection must execute exactly one passed test")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--exact-test",
        type=parse_exact_test,
        metavar="PACKAGE::TARGET::FUNCTION",
        help="run one exact binary-backed E2E test after building the product binaries",
    )
    arguments = parser.parse_args()
    run(["cargo", "build", "--locked", "-p", "podway-cli", "--bin", "podway"])
    run(["cargo", "build", "--locked", "--release", "-p", "podway-cli", "--bin", "podway"])
    target = cargo_target_directory()
    podway = (target / "debug" / "podway").resolve()
    release_podway = (target / "release" / "podway").resolve()
    for binary in (podway, release_podway):
        if not binary.is_file():
            raise SystemExit(f"cargo build did not produce expected Podway CLI: {binary}")
        run([str(binary), "version"])
    environment = os.environ.copy()
    environment["PODWAY_RELEASE_TEST_BINARY"] = str(release_podway)
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
    if arguments.exact_test is None:
        run(test_command, env=environment)
    else:
        run_exact(test_command, environment)
    print(
        json.dumps(
            {
                "binaries": {
                    "podway": str(podway),
                    "podway_release": str(release_podway),
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
