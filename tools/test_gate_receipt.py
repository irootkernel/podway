#!/usr/bin/env python3
"""Record or validate a local make-test result for the exact source and toolchain."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_RECEIPT = ROOT / "target/podway-test-gate-v1.json"
SCHEMA = "podway.test-gate-receipt/v1"


class ReceiptError(RuntimeError):
    pass


def run(*arguments: str) -> bytes:
    completed = subprocess.run(
        arguments,
        cwd=ROOT,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        message = completed.stderr.decode("utf-8", errors="replace").strip()
        raise ReceiptError(message or f"command failed: {' '.join(arguments)}")
    return completed.stdout


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def source_fingerprint() -> str:
    listed = run("git", "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    relative_paths = sorted(path for path in listed.split(b"\0") if path)
    digest = hashlib.sha256()
    for relative_bytes in relative_paths:
        try:
            relative = relative_bytes.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ReceiptError("source path is not UTF-8") from error
        path = ROOT / relative
        metadata = path.lstat()
        digest.update(len(relative_bytes).to_bytes(8, "big"))
        digest.update(relative_bytes)
        if stat.S_ISLNK(metadata.st_mode):
            payload = os.fsencode(os.readlink(path))
            kind = b"symlink"
        elif stat.S_ISREG(metadata.st_mode):
            payload = path.read_bytes()
            kind = b"executable" if metadata.st_mode & 0o111 else b"file"
        else:
            raise ReceiptError(f"source entry is not a regular file or symlink: {relative}")
        digest.update(len(kind).to_bytes(8, "big"))
        digest.update(kind)
        digest.update(len(payload).to_bytes(8, "big"))
        digest.update(payload)
    return f"sha256:{digest.hexdigest()}"


def tool_identity(name: str) -> dict[str, str]:
    selected = shutil.which(name)
    if selected is None:
        raise ReceiptError(f"{name} is not available on PATH")
    path = Path(selected).resolve()
    if not path.is_file():
        raise ReceiptError(f"{name} does not resolve to a regular file")
    version = run(str(path), "--version").decode("utf-8", errors="strict").strip()
    return {"path": str(path), "sha256": sha256_file(path), "version": version}


def current_state() -> dict[str, Any]:
    commit = run("git", "rev-parse", "--verify", "HEAD").decode("ascii").strip()
    return {
        "cargo_lock_sha256": sha256_file(ROOT / "Cargo.lock"),
        "source": {"commit": commit, "fingerprint": source_fingerprint()},
        "toolchain": {name: tool_identity(name) for name in ("cargo", "rustc")},
    }


def expected_receipt() -> dict[str, Any]:
    return {
        "gate": "make test",
        "schema": SCHEMA,
        "status": "passed",
        **current_state(),
    }


def load_receipt(path: Path) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file():
        raise ReceiptError("test gate receipt is missing")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ReceiptError(f"test gate receipt is unreadable: {error}") from error
    if not isinstance(value, dict):
        raise ReceiptError("test gate receipt is not an object")
    return value


def invalidate(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.exists():
        raise ReceiptError("test gate receipt path is not a file")


def record(path: Path) -> None:
    receipt = expected_receipt()
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=path.parent, delete=False
    ) as output:
        temporary = Path(output.name)
        json.dump(receipt, output, sort_keys=True, separators=(",", ":"))
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)


def self_test() -> int:
    with tempfile.TemporaryDirectory(prefix="podway-test-gate-") as temporary_name:
        path = Path(temporary_name) / "receipt.json"
        record(path)
        recorded = load_receipt(path)
        expected_fields = {
            "cargo_lock_sha256",
            "gate",
            "schema",
            "source",
            "status",
            "toolchain",
        }
        if set(recorded) != expected_fields:
            raise ReceiptError("recorded receipt did not validate")
        tampered = dict(recorded)
        tampered["status"] = "failed"
        path.write_text(json.dumps(tampered), encoding="utf-8")
        if load_receipt(path) == recorded:
            raise ReceiptError("tampered receipt unexpectedly validated")
        invalidate(path)
        if path.exists():
            raise ReceiptError("receipt invalidation did not remove the file")
    return 3


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("check", "invalidate", "record", "self-test"))
    parser.add_argument("--receipt", type=Path, default=DEFAULT_RECEIPT)
    arguments = parser.parse_args()
    path = arguments.receipt.resolve()
    try:
        if arguments.mode == "self-test":
            result = {"mode": "self-test", "ok": True, "sentinels": self_test()}
        elif arguments.mode == "invalidate":
            invalidate(path)
            result = {"mode": "invalidate", "ok": True}
        elif arguments.mode == "record":
            record(path)
            result = {"mode": "record", "ok": True, "receipt": str(path)}
        else:
            if load_receipt(path) != expected_receipt():
                raise ReceiptError(
                    "test gate receipt does not match the current source and toolchain"
                )
            result = {"mode": "check", "ok": True, "receipt": str(path), "reused": True}
    except (OSError, ReceiptError) as error:
        print(
            json.dumps(
                {"error": str(error), "mode": arguments.mode, "ok": False},
                sort_keys=True,
                separators=(",", ":"),
            )
        )
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
