#!/usr/bin/env python3
"""Build the current daemon binary and run the ignored G005 public vertical test."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def verification_root() -> Path:
    return Path(__file__).resolve().parents[1]


ROOT = verification_root()
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


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def daemon_source_inputs(root: Path) -> dict[str, str]:
    inputs: dict[str, str] = {}
    roots = [root / "Cargo.toml", root / "Cargo.lock", root / "presets", root / "spec"]
    roots.extend(
        path
        for crate in (
            "podway-core", "podway-protocol", "podway-config", "podway-presets",
            "podway-store", "podway-git", "podway-service", "podway-daemon",
        )
        for path in (root / "crates" / crate / "Cargo.toml", root / "crates" / crate / "src")
    )
    for source_root in roots:
        if source_root.is_symlink() or not source_root.exists():
            raise SystemExit(f"daemon receipt input must be a present non-symlink path: {source_root}")
        candidates = [source_root] if source_root.is_file() else sorted(source_root.rglob("*"))
        for path in candidates:
            if path.is_symlink():
                raise SystemExit(f"daemon receipt input must not be a symlink: {path}")
            if not path.is_file() or (path.suffix not in {".rs", ".toml", ".yaml", ".yml", ".sql"} and path.name != "Cargo.lock"):
                continue
            relative = path.relative_to(root).as_posix()
            if relative in inputs:
                raise SystemExit(f"daemon receipt input is duplicated: {relative}")
            inputs[relative] = sha256_file(path)
    return dict(sorted(inputs.items()))


def tool_identity(tool_id: str) -> dict[str, str]:
    rustup = shutil.which("rustup")
    if rustup is None:
        raise SystemExit("rustup is required to resolve the pinned Rust toolchain")
    resolved = subprocess.run(
        [rustup, "which", tool_id],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if resolved.returncode != 0:
        raise SystemExit(resolved.stderr.strip() or f"rustup could not resolve {tool_id}")
    path = Path(resolved.stdout.strip()).resolve()
    if path.is_symlink() or not path.is_file():
        raise SystemExit(f"{tool_id} must resolve to a regular non-symlink executable")
    version = subprocess.run(
        [str(path), "--version"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if version.returncode != 0 or not version.stdout.strip():
        raise SystemExit(version.stderr.strip() or f"cannot identify {tool_id} version")
    return {"path": str(path), "sha256": sha256_file(path), "version": version.stdout.strip()}


def produce_daemon_build_receipt(root: Path, daemon: Path) -> Path:
    daemon = daemon.resolve()
    if not daemon.is_file():
        raise SystemExit(f"current podwayd artifact is missing: {daemon}")
    receipt = daemon.with_name("podwayd.build-receipt.json")
    payload = {
        "binary": str(daemon),
        "binary_sha256": sha256_file(daemon),
        "inputs": daemon_source_inputs(root),
        "schema": "podway.daemon-build-receipt/v1",
        "toolchain": {tool_id: tool_identity(tool_id) for tool_id in ("cargo", "rustc")},
    }
    receipt.write_text(json.dumps(payload, separators=(",", ":"), sort_keys=True) + "\n", encoding="utf-8")
    return receipt


def main() -> int:
    run(["cargo", "build", "-p", "podway-daemon", "--bin", "podwayd"])
    daemon = cargo_target_directory() / "debug" / "podwayd"
    receipt = produce_daemon_build_receipt(ROOT, daemon)

    environment = os.environ.copy()
    environment["PODWAYD_TEST_BINARY"] = str(daemon.resolve())
    environment["PODWAYD_BUILD_RECEIPT"] = str(receipt.resolve())
    run(
        [
            "cargo", "test", "-p", "podway-cli", "--test", "e2e_phase4_production_vertical",
            TEST_NAME, "--", "--ignored", "--nocapture",
        ],
        env=environment,
    )
    print(json.dumps({"binary": str(daemon.resolve()), "goal": "G005", "ok": True,
                      "receipt": str(receipt.resolve()), "test": TEST_NAME}, separators=(",", ":"), sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
