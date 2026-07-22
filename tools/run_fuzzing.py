#!/usr/bin/env python3
"""Run the bounded, reproducible protocol fuzzing release gate."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent.parent
FUZZ_TOOLCHAIN = "nightly-2026-07-17"
FUZZ_CARGO_VERSION = "cargo-fuzz 0.13.2"
FUZZ_RUNS = 100_000
FUZZ_SEED = 0x50D0A7
FUZZ_TARGETS = ("frame_decoder", "request_envelope")

VALID_REQUEST = (
    b'{"protocol":"podway.ipc/v1","request_id":"00000000-0000-4000-8000-000000000001",'
    b'"client":{"name":"podway-cli","version":"0.1.0","pid":1},"operation":"query",'
    b'"command":"status","options":{"detach":false,"wait_timeout_ms":0},"payload":{}}'
)


class FuzzGateError(RuntimeError):
    """The fuzzing gate could not run with its pinned dependencies."""


def run(
    command: list[str],
    *,
    environment: dict[str, str],
    capture: bool = False,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )


def rustup_executable(name: str) -> str:
    result = subprocess.run(
        ["rustup", "which", "--toolchain", FUZZ_TOOLCHAIN, name],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        raise FuzzGateError(
            f"Rust toolchain {FUZZ_TOOLCHAIN} is required; "
            f"install it with `rustup toolchain install {FUZZ_TOOLCHAIN} --profile minimal`"
        )
    return result.stdout.strip()


def seed_corpus(directory: Path, target: str) -> None:
    directory.mkdir(mode=0o700)
    (directory / "malformed-json").write_bytes(b"{}")
    if target == "frame_decoder":
        framed = len(VALID_REQUEST).to_bytes(4, byteorder="big") + VALID_REQUEST
        (directory / "valid-request-frame").write_bytes(framed)
    else:
        (directory / "valid-request-envelope").write_bytes(VALID_REQUEST)


def main() -> int:
    cargo = rustup_executable("cargo")
    rustc = rustup_executable("rustc")
    environment = os.environ.copy()
    environment["RUSTC"] = rustc

    try:
        version = run([cargo, "fuzz", "--version"], environment=environment, capture=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise FuzzGateError(
            f"{FUZZ_CARGO_VERSION} is required; install it with "
            f"`cargo install cargo-fuzz --version 0.13.2 --locked`"
        ) from error
    if version != FUZZ_CARGO_VERSION:
        raise FuzzGateError(f"expected {FUZZ_CARGO_VERSION}, found {version}")

    completed: list[dict[str, int | str]] = []
    with tempfile.TemporaryDirectory(prefix="podway-fuzz-gate-") as temporary:
        temporary_root = Path(temporary)
        for index, target in enumerate(FUZZ_TARGETS):
            corpus = temporary_root / target
            seed_corpus(corpus, target)
            command = [
                cargo,
                "fuzz",
                "run",
                target,
                str(corpus),
                "--",
                f"-runs={FUZZ_RUNS}",
                f"-seed={FUZZ_SEED + index}",
                "-timeout=5",
                "-rss_limit_mb=2048",
                "-verbosity=0",
                "-print_final_stats=1",
            ]
            try:
                run(command, environment=environment, capture=True)
            except subprocess.CalledProcessError as error:
                detail = "\n".join(part for part in (error.stdout, error.stderr) if part)
                raise FuzzGateError(f"target {target} failed\n{detail}") from error
            completed.append({"runs": FUZZ_RUNS, "seed": FUZZ_SEED + index, "target": target})

    print(
        json.dumps(
            {
                "cargo_fuzz": FUZZ_CARGO_VERSION,
                "ok": True,
                "targets": completed,
                "toolchain": FUZZ_TOOLCHAIN,
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except FuzzGateError as error:
        raise SystemExit(f"fuzzing gate failed: {error}") from error
