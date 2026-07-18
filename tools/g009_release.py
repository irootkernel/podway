"""G009 archive integrity and RC intent validation."""
from __future__ import annotations
import hashlib
import zipfile
from pathlib import Path
from typing import Any
from g009_common import ARCHIVE_ROOT, QualificationError, bounded_bytes, fail, load_json, require_digest, safe_extract_member, sha256_bytes

MAX_ARCHIVE_MEMBERS = 256
MAX_ARCHIVE_UNCOMPRESSED = 512 * 1024 * 1024

def load_rc(path: Path) -> dict[str, Any]:
    rc = load_json(path)
    if not isinstance(rc, dict) or rc.get("schema") != "podway.g009.rc-intent/v1": fail("not a G009 RC intent")
    if rc.get("target") != "aarch64-apple-darwin": fail("RC target is not aarch64-apple-darwin")
    signing = rc.get("signing")
    if not isinstance(signing, dict) or signing.get("posture") not in ("unsigned-internal", "signed-public"):
        fail("invalid RC signing posture")
    if signing["posture"] == "unsigned-internal" and signing.get("codesign") != "not_attempted_missing_credentials":
        fail("unsigned RC has invalid codesign status")
    inputs = rc.get("inputs")
    repo = Path(__file__).resolve().parents[1]
    if not isinstance(inputs, list) or not inputs: fail("RC has no bound inputs")
    roles: set[str] = set()
    for item in inputs:
        if not isinstance(item, dict) or not isinstance(item.get("role"), str) or item["role"] in roles:
            fail("RC has malformed or duplicate input role")
        roles.add(item["role"])
        expected = require_digest(item.get("sha256"), f"RC {item['role']}")
        relative = item.get("path")
        if not isinstance(relative, str): fail(f"RC input {item['role']} has no relative path")
        candidate = (repo / relative).resolve()
        if not candidate.is_relative_to(repo) or not candidate.is_file(): fail(f"RC input {item['role']} path is unsafe or missing")
        if sha256_bytes(bounded_bytes(candidate)) != expected: fail(f"stale or mutated RC input: {item['role']}")
    return rc

def payload_manifest(members: dict[str, bytes]) -> dict[str, Any]:
    return {"schema": "podway.g009.payload-digests/v1", "members": [
        {"path": name, "sha256": sha256_bytes(data), "size": len(data)}
        for name, data in sorted(members.items()) if name != f"{ARCHIVE_ROOT}/payload-digests-v1.json"]}

def inspect_archive(path: Path) -> dict[str, Any]:
    if not path.is_file(): fail(f"archive missing: {path}")
    try:
        with zipfile.ZipFile(path) as archive:
            infos = archive.infolist()
            if not infos or len(infos) > MAX_ARCHIVE_MEMBERS: fail("archive member count invalid")
            if sum(info.file_size for info in infos) > MAX_ARCHIVE_UNCOMPRESSED: fail("archive exceeds uncompressed bound")
            names: set[str] = set(); members: dict[str, bytes] = {}
            for info in infos:
                if info.is_dir() or info.filename in names: fail("archive has duplicate or directory member")
                safe_extract_member(info.filename)
                if info.flag_bits & 0x1: fail("encrypted archive member")
                names.add(info.filename); members[info.filename] = archive.read(info)
    except (OSError, zipfile.BadZipFile) as exc: fail(f"unsafe archive: {exc}")
    manifest_name = f"{ARCHIVE_ROOT}/payload-digests-v1.json"
    if manifest_name not in members: fail("archive missing internal payload manifest")
    manifest = load_json_from_bytes(members[manifest_name])
    expected = payload_manifest(members)
    if manifest != expected: fail("internal payload manifest mismatch or recursion")
    sidecar = path.with_name(path.name + ".sha256")
    if sidecar.is_file():
        text = bounded_bytes(sidecar, 1024).decode("ascii", "strict").strip().split()
        if len(text) != 1 or text[0] != sha256_bytes(bounded_bytes(path)):
            fail("detached final archive checksum mismatch")
    return {"archive_sha256": sha256_bytes(bounded_bytes(path)), "members": expected["members"]}

def load_json_from_bytes(data: bytes) -> Any:
    import json
    from g009_common import _no_duplicate_object, _reject_constant
    try: return json.loads(data.decode("utf-8"), object_pairs_hook=_no_duplicate_object, parse_constant=_reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc: fail(f"invalid internal JSON: {exc}")

def require_bound_file(rc: dict[str, Any], role: str, path: Path) -> None:
    inputs = rc.get("inputs")
    if not isinstance(inputs, list): fail("RC has no inputs")
    matching = [item for item in inputs if isinstance(item, dict) and item.get("role") == role]
    if len(matching) != 1: fail(f"RC requires exactly one bound {role}")
    expected = require_digest(matching[0].get("sha256"), f"RC {role}")
    if sha256_bytes(bounded_bytes(path)) != expected: fail(f"stale or mutated {role}")
