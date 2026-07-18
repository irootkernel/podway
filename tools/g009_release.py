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
    required = {"schema", "target", "minimum_macos", "rust", "source", "host", "inputs", "signing", "archive_root", "binaries"}
    if not isinstance(rc, dict) or set(rc) != required or rc.get("schema") != "podway.g009.rc-intent/v1": fail("not an exact G009 RC intent")
    if rc.get("target") != "aarch64-apple-darwin" or rc.get("rust") != "1.85.0" or rc.get("archive_root") != ARCHIVE_ROOT: fail("RC identity drift")
    source, host = rc.get("source"), rc.get("host")
    if not isinstance(source, dict) or set(source) != {"commit", "tree", "tools"} or not all(isinstance(source.get(key), str) and source[key] for key in ("commit", "tree")):
        fail("RC source identity is malformed")
    if not isinstance(host, dict) or host.get("system") != "Darwin" or host.get("machine") != "arm64": fail("RC host identity is malformed")
    signing = rc.get("signing")
    if not isinstance(signing, dict) or signing.get("posture") not in ("unsigned-internal", "signed-public"):
        fail("invalid RC signing posture")
    if signing["posture"] == "unsigned-internal" and signing != {"posture": "unsigned-internal", "codesign": "not_attempted_missing_credentials", "notarization": "not_attempted_missing_credentials", "stapling": "not_applicable_zip", "gatekeeper": "not_claimed"}:
        fail("unsigned RC has invalid codesign status")
    if signing["posture"] == "signed-public":
        fail("signed-public requires an external credentialed release implementation")
    inputs = rc.get("inputs")
    repo = Path(__file__).resolve().parents[1]
    required_roles = {"profile", "baseline", "thresholds", "approvals", "signer-contract", "lockfile", "interfaces", "handoffs", "crash-registry", "fuzz-policy", "observability-policy", "podway", "podwayd"}
    if not isinstance(inputs, list) or {item.get("role") for item in inputs if isinstance(item, dict)} != required_roles or len(inputs) != len(required_roles): fail("RC has incomplete input roles")
    for item in inputs:
        if not isinstance(item, dict) or set(item) != {"role", "path", "sha256"}: fail("RC has malformed input")
        expected = require_digest(item.get("sha256"), f"RC {item['role']}")
        relative = item.get("path")
        if not isinstance(relative, str): fail(f"RC input {item['role']} has no relative path")
        candidate = (repo / relative).resolve()
        if not candidate.is_relative_to(repo) or candidate.is_symlink() or not candidate.is_file(): fail(f"RC input {item['role']} path is unsafe or missing")
        if sha256_bytes(bounded_bytes(candidate)) != expected: fail(f"stale or mutated RC input: {item['role']}")
    binaries = rc.get("binaries")
    if not isinstance(binaries, dict) or set(binaries) != {"podway", "podwayd"}: fail("RC lacks exact binary provenance")
    for name, bound in binaries.items():
        if not isinstance(bound, dict) or set(bound) != {"sha256", "provenance"} or bound["sha256"] != next(item["sha256"] for item in inputs if item["role"] == name) or not isinstance(bound["provenance"], dict):
            fail(f"RC binary binding malformed: {name}")
    return rc

def payload_manifest(members: dict[str, bytes]) -> dict[str, Any]:
    return {"schema": "podway.g009.payload-digests/v1", "members": [
        {"path": name, "sha256": sha256_bytes(data), "size": len(data)}
        for name, data in sorted(members.items()) if name != f"{ARCHIVE_ROOT}/payload-digests-v1.json"]}

def inspect_archive(path: Path, declared_members: set[str] | None = None) -> dict[str, Any]:
    if not path.is_file() or path.is_symlink(): fail(f"archive missing or unsafe: {path}")
    sidecar = path.with_name(path.name + ".sha256")
    if not sidecar.is_file() or sidecar.is_symlink(): fail("archive requires detached final checksum")
    try:
        text = bounded_bytes(sidecar, 1024).decode("ascii", "strict").strip().split()
        if len(text) != 1 or text[0] != sha256_bytes(bounded_bytes(path)): fail("detached final archive checksum mismatch")
        with zipfile.ZipFile(path) as archive:
            infos = archive.infolist()
            if not infos or len(infos) > MAX_ARCHIVE_MEMBERS: fail("archive member count invalid")
            if sum(info.file_size for info in infos) > MAX_ARCHIVE_UNCOMPRESSED: fail("archive exceeds uncompressed bound")
            names: set[str] = set(); members: dict[str, bytes] = {}
            for info in infos:
                mode = info.external_attr >> 16
                if info.is_dir() or info.filename in names or (mode & 0o170000) != 0o100000: fail("archive has duplicate, directory, or non-regular member")
                safe_extract_member(info.filename)
                if info.flag_bits & 0x1 or (mode & 0o777) not in (0o644, 0o755): fail("unsafe archive member metadata")
                if info.filename.startswith(f"{ARCHIVE_ROOT}/bin/") != ((mode & 0o777) == 0o755): fail("archive executable mode mismatch")
                names.add(info.filename); members[info.filename] = archive.read(info)
    except (OSError, zipfile.BadZipFile) as exc: fail(f"unsafe archive: {exc}")
    manifest_name = f"{ARCHIVE_ROOT}/payload-digests-v1.json"
    if manifest_name not in members: fail("archive missing internal payload manifest")
    manifest = load_json_from_bytes(members[manifest_name])
    expected = payload_manifest(members)
    if manifest != expected: fail("internal payload manifest mismatch or recursion")
    expected_names = {item["path"] for item in expected["members"]} | {manifest_name}
    if names != expected_names or (declared_members is not None and names != declared_members): fail("archive membership differs from declaration")
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
