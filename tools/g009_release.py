"""G009 archive integrity and RC intent validation."""
from __future__ import annotations
import hashlib
import subprocess
import zipfile
from pathlib import Path
from typing import Any
from g009_common import ARCHIVE_ROOT, EVIDENCE_ROOT, QualificationError, bounded_bytes, canonical_json, fail, load_json, require_digest, safe_extract_member, sha256_bytes, sha256_file

MAX_ARCHIVE_MEMBERS = 256
MAX_ARCHIVE_UNCOMPRESSED = 512 * 1024 * 1024

def _repo() -> Path:
    return Path(__file__).resolve().parents[1]


def _input(rc: dict[str, Any], role: str) -> Path:
    matches = [item for item in rc["inputs"] if item["role"] == role]
    if len(matches) != 1:
        fail(f"RC lacks exactly one {role} input")
    return (_repo() / matches[0]["path"]).resolve()


def _current_source() -> dict[str, str]:
    outputs: dict[str, str] = {}
    for label, argv in (("commit", ("git", "rev-parse", "HEAD")), ("tree", ("git", "rev-parse", "HEAD^{tree}"))):
        result = subprocess.run(argv, cwd=_repo(), capture_output=True, check=False)
        if result.returncode != 0:
            fail(f"cannot resolve checked-out source {label}")
        outputs[label] = result.stdout.decode("ascii", "strict").strip()
    return outputs


def _verify_approvals(rc: dict[str, Any]) -> None:
    characterization = _input(rc, "characterization")
    characterization_value = load_json(characterization)
    baseline = load_json(_input(rc, "baseline"))
    thresholds = load_json(_input(rc, "thresholds"))
    approvals = load_json(_input(rc, "approvals"))
    contract = load_json(_input(rc, "signer-contract"))
    if not isinstance(characterization_value, dict) or characterization_value.get("schema") != "podway.g009.characterization/v1" or characterization_value.get("target") != rc["target"] or not isinstance(baseline, dict) or not isinstance(thresholds, dict):
        fail("RC performance inputs are malformed")
    from g009_performance import characterize, thresholds as derive_thresholds
    if characterize(characterization_value.get("workloads")) != baseline:
        fail("RC characterization baseline is not mechanically exact")
    if derive_thresholds(baseline) != thresholds:
        fail("RC thresholds are not mechanically derived")
    if not isinstance(contract, dict) or contract.get("schema") != "podway.g009.approval-signers/v1" or set(contract) != {"schema", "keyring", "signers"}:
        fail("approval signer contract is not exact")
    keyring = Path(contract["keyring"])
    signers = contract.get("signers")
    if not keyring.is_file() or keyring.is_symlink() or not isinstance(signers, list) or len(signers) != 3:
        fail("approval trust root is unavailable")
    by_role = {item.get("role"): item for item in signers if isinstance(item, dict)}
    if set(by_role) != {"owner", "E", "F"} or any(set(item) != {"role", "signer", "fingerprint"} or not all(isinstance(item.get(key), str) and item[key] for key in ("signer", "fingerprint")) for item in by_role.values()):
        fail("approval signer roles are incomplete")
    if not isinstance(approvals, dict) or approvals.get("schema") != "podway.g009.approvals/v1" or approvals.get("characterization_sha256") != sha256_file(characterization) or not isinstance(approvals.get("approvals"), list) or len(approvals["approvals"]) != 3:
        fail("approval bundle is stale or incomplete")
    baseline_digest = sha256_bytes(canonical_json(baseline))
    thresholds_digest = sha256_bytes(canonical_json(thresholds))
    roles: set[str] = set()
    signers_seen: set[str] = set()
    for approval in approvals["approvals"]:
        if not isinstance(approval, dict) or set(approval) != {"role", "signer", "fingerprint", "characterization_sha256", "baseline_sha256", "thresholds_sha256", "payload", "signature"}:
            fail("approval has mutable or missing fields")
        role = approval["role"]
        expected = by_role.get(role)
        if expected is None or approval["signer"] != expected["signer"] or approval["fingerprint"] != expected["fingerprint"] or approval["characterization_sha256"] != sha256_file(characterization) or approval["baseline_sha256"] != baseline_digest or approval["thresholds_sha256"] != thresholds_digest:
            fail("approval binding or signer contract mismatch")
        payload, signature = Path(approval["payload"]), Path(approval["signature"])
        if not payload.is_file() or payload.is_symlink() or not signature.is_file() or signature.is_symlink():
            fail("approval detached signature inputs are unsafe")
        statement = canonical_json({"role": role, "signer": approval["signer"], "fingerprint": approval["fingerprint"], "characterization_sha256": approval["characterization_sha256"], "baseline_sha256": baseline_digest, "thresholds_sha256": thresholds_digest})
        if bounded_bytes(payload) != statement:
            fail("approval payload is not the exact bound statement")
        verified = subprocess.run(("gpgv", "--keyring", str(keyring), "--status-fd", "1", str(signature), str(payload)), capture_output=True, check=False)
        if verified.returncode != 0 or expected["fingerprint"] not in verified.stdout.decode("utf-8", "replace"):
            fail("detached approval signature did not verify against trust root")
        if role in roles or approval["signer"] in signers_seen:
            fail("approval roles/signers must be distinct")
        roles.add(role)
        signers_seen.add(approval["signer"])
    if roles != {"owner", "E", "F"}:
        fail("missing explicit owner/E/F approvals")


def verify_rc_consumption(path: Path) -> dict[str, Any]:
    resolved = path.resolve()
    digest = sha256_file(resolved)
    if not resolved.is_relative_to(EVIDENCE_ROOT.resolve()) or resolved.is_symlink() or resolved.name != f"{digest}.json":
        fail("RC must be an immutable digest-addressed evidence artifact")
    rc = load_rc(resolved)
    if _current_source() != {"commit": rc["source"]["commit"], "tree": rc["source"]["tree"]}:
        fail("checked-out source commit/tree differs from immutable RC")
    _verify_approvals(rc)
    return rc

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
    repo = _repo()
    required_roles = {"profile", "characterization", "baseline", "thresholds", "approvals", "signer-contract", "lockfile", "interfaces", "handoffs", "crash-registry", "fuzz-policy", "observability-policy", "podway", "podwayd"}
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
