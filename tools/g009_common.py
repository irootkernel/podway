#!/usr/bin/env python3
"""Fail-closed primitives for the local-only G009 qualification evidence."""
from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import stat
import tempfile
from fractions import Fraction
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_ROOT = ROOT / "artifacts" / "g009"
TARGET = "aarch64-apple-darwin"
ARCHIVE_ROOT = "podway-0.1.0-aarch64-apple-darwin"
MAX_JSON_BYTES = 16 * 1024 * 1024
MAX_FILE_BYTES = 128 * 1024 * 1024
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")

class QualificationError(RuntimeError):
    pass

def fail(message: str) -> None:
    raise QualificationError(message)

def _no_duplicate_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON key: {key}")
        result[key] = value
    return result

def _reject_constant(value: str) -> None:
    fail(f"non-finite JSON value: {value}")

def bounded_bytes(path: Path, limit: int = MAX_FILE_BYTES) -> bytes:
    try:
        size = path.stat().st_size
    except OSError as exc:
        fail(f"cannot stat {path}: {exc}")
    if size > limit:
        fail(f"read exceeds {limit} byte limit: {path}")
    try:
        with path.open("rb") as handle:
            data = handle.read(limit + 1)
    except OSError as exc:
        fail(f"cannot read {path}: {exc}")
    if len(data) > limit:
        fail(f"read exceeds {limit} byte limit: {path}")
    return data

def load_json(path: Path, limit: int = MAX_JSON_BYTES) -> Any:
    raw = bounded_bytes(path, limit)
    try:
        return json.loads(raw.decode("utf-8"), object_pairs_hook=_no_duplicate_object,
                          parse_constant=_reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(f"invalid JSON {path}: {exc}")

def _canonical(value: Any) -> str:
    if value is None: return "null"
    if value is True: return "true"
    if value is False: return "false"
    if isinstance(value, int): return str(value)
    if isinstance(value, float): fail("floating point values are forbidden in authoritative JSON")
    if isinstance(value, str): return json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    if isinstance(value, list): return "[" + ",".join(_canonical(item) for item in value) + "]"
    if isinstance(value, dict):
        if not all(isinstance(key, str) for key in value): fail("JSON object key is not a string")
        return "{" + ",".join(_canonical(key) + ":" + _canonical(value[key]) for key in sorted(value)) + "}"
    fail(f"non-JSON value: {type(value).__name__}")

def canonical_json(value: Any) -> bytes:
    return _canonical(value).encode("utf-8")

def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()

def sha256_file(path: Path) -> str:
    return sha256_bytes(bounded_bytes(path))

def fraction_value(value: Any, label: str) -> Fraction:
    if not isinstance(value, dict) or set(value) != {"numerator", "denominator"}:
        fail(f"{label} must be {{numerator,denominator}}")
    numerator, denominator = value["numerator"], value["denominator"]
    if not isinstance(numerator, int) or not isinstance(denominator, int) or denominator <= 0:
        fail(f"{label} has invalid fraction")
    return Fraction(numerator, denominator)

def fraction_json(value: Fraction) -> dict[str, int]:
    return {"numerator": value.numerator, "denominator": value.denominator}

def safe_relative(path: str) -> PurePosixPath:
    candidate = PurePosixPath(path)
    if not path or candidate.is_absolute() or ".." in candidate.parts or any(part in ("", ".") for part in candidate.parts):
        fail(f"unsafe relative path: {path!r}")
    return candidate

def safe_extract_member(name: str) -> PurePosixPath:
    candidate = safe_relative(name)
    if candidate.parts[0] != ARCHIVE_ROOT:
        fail(f"archive member outside required root: {name}")
    return candidate

def atomic_immutable_json(path: Path, value: Any) -> str:
    if not path.is_relative_to(EVIDENCE_ROOT): fail(f"evidence path outside {EVIDENCE_ROOT}: {path}")
    payload = canonical_json(value)
    digest = sha256_bytes(payload)
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        if bounded_bytes(path) != payload: fail(f"immutable evidence already differs: {path}")
        return digest
    fd, temp_name = tempfile.mkstemp(prefix=".g009-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(payload); handle.flush(); os.fsync(handle.fileno())
        os.chmod(temp_name, stat.S_IRUSR | stat.S_IRGRP | stat.S_IROTH)
        try: os.link(temp_name, path)
        except FileExistsError:
            if bounded_bytes(path) != payload: fail(f"immutable evidence race differs: {path}")
        finally: os.unlink(temp_name)
    finally:
        if os.path.exists(temp_name): os.unlink(temp_name)
    return digest

def content_addressed_json(category: str, value: Any) -> tuple[Path, str]:
    payload = canonical_json(value); digest = sha256_bytes(payload)
    path = EVIDENCE_ROOT / category / f"{digest}.json"
    atomic_immutable_json(path, value)
    return path, digest

def host_manifest() -> dict[str, str]:
    return {"machine": platform.machine(), "system": platform.system(), "platform": platform.platform()}

def require_arm64_host(target: str) -> None:
    if target != TARGET: fail(f"only target {TARGET} is accepted")
    host = host_manifest()
    if host["system"] != "Darwin" or host["machine"] != "arm64":
        fail(f"requires native Darwin arm64 host, got {host['system']} {host['machine']}")

def require_digest(value: Any, label: str) -> str:
    if not isinstance(value, str) or not SHA256_RE.fullmatch(value): fail(f"invalid {label} SHA-256")
    return value

def require_object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict): fail(f"{label} must be an object")
    return value
