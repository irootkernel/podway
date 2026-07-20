"""G009 archive integrity and RC intent validation."""
from __future__ import annotations
import tomllib
from datetime import date
import io
import os
import re
import subprocess
import zipfile
from pathlib import Path
from typing import Any

from g009_common import CONTROLLER_ROOT, EVIDENCE_ROOT, ROOT, archive_root, bounded_bytes, canonical_json, fail, load_json, load_json_bytes, profile_target_tuple, require_digest, require_native_host, safe_extract_member, sha256_bytes, sha256_file, target_tuple

MAX_ARCHIVE_MEMBERS = 256
MAX_ARCHIVE_UNCOMPRESSED = 512 * 1024 * 1024

PROTECTED_INPUT_ROLES = frozenset({"characterization", "baseline", "thresholds", "approvals", "signer-contract"})


def rc_input_root(role: str) -> Path:
    if role == "profile":
        return CONTROLLER_ROOT
    if role not in PROTECTED_INPUT_ROLES:
        return ROOT
    raw = os.environ.get("G009_IMMUTABLE_INPUT_ROOT")
    if raw is None:
        fail("protected RC input root is unavailable")
    supplied = Path(raw)
    if not supplied.is_absolute() or supplied.is_symlink() or not supplied.is_dir():
        fail("protected RC input root is unsafe")
    resolved = supplied.resolve()
    roots = (CONTROLLER_ROOT.resolve(), ROOT.resolve())
    if any(
        resolved == root or resolved.is_relative_to(root) or root.is_relative_to(resolved)
        for root in roots
    ):
        fail("protected RC input root overlaps a controlled root")
    return resolved


def resolve_rc_input(rc: dict[str, Any], role: str) -> Path:
    matches = [item for item in rc["inputs"] if item["role"] == role]
    if len(matches) != 1:
        fail(f"RC lacks exactly one {role} input")
    relative = matches[0].get("path")
    if not isinstance(relative, str):
        fail(f"RC {role} input path is malformed")
    expected = require_digest(matches[0].get("sha256"), f"RC {role}")
    root = rc_input_root(role)
    supplied = root / relative
    candidate = supplied.resolve()
    if supplied.is_symlink() or not candidate.is_relative_to(root) or not candidate.is_file():
        fail(f"RC {role} input escapes its authoritative root")
    if sha256_bytes(bounded_bytes(candidate)) != expected:
        fail(f"stale or mutated RC input: {role}")
    return candidate


def _current_source() -> dict[str, str]:
    outputs: dict[str, str] = {}
    commands = (
        ("commit", ("/usr/bin/git", "rev-parse", "HEAD")),
        ("tree", ("/usr/bin/git", "rev-parse", "HEAD^{tree}")),
        ("dirty", ("/usr/bin/git", "status", "--porcelain")),
    )
    for label, argv in commands:
        result = subprocess.run(
            argv,
            cwd=ROOT,
            capture_output=True,
            check=False,
            env={"PATH": os.environ.get("PATH", "")},
            timeout=30,
        )
        if result.returncode != 0:
            fail(f"cannot resolve checked-out source {label}")
        outputs[label] = result.stdout.decode("ascii", "strict").strip()
    if outputs.pop("dirty"):
        fail("candidate source tree must be clean")
    if not all(re.fullmatch(r"[0-9a-f]{40}", outputs[label]) for label in ("commit", "tree")):
        fail("candidate source commit/tree identity is malformed")
    return outputs


def verify_role_signatures(
    statements: list[dict[str, Any]], signers: list[dict[str, Any]], keyring: Path
) -> None:
    """Verify exact detached role statements using only VALIDSIG fingerprints."""
    supplied_keyring = keyring
    if supplied_keyring.is_symlink() or not supplied_keyring.is_file():
        fail("signature keyring is unsafe or missing")
    keyring = supplied_keyring.resolve()
    expected = {item.get("role"): item for item in signers if isinstance(item, dict)}
    if set(expected) != {"owner", "E", "F"} or len(expected) != len(signers):
        fail("signature roles must be exact owner/E/F")
    fingerprints = {item.get("fingerprint") for item in expected.values()}
    if len(fingerprints) != 3 or not all(
        isinstance(value, str) and re.fullmatch(r"[0-9A-F]{40}", value)
        for value in fingerprints
    ):
        fail("signature role fingerprints must be distinct uppercase primary fingerprints")
    seen: set[str] = set()
    for statement in statements:
        role = statement.get("role")
        signer = expected.get(role)
        payload, signature = statement.get("payload"), statement.get("signature")
        if role in seen or signer is None or not isinstance(payload, Path) or not isinstance(signature, Path):
            fail("signature statement is malformed or duplicated")
        if payload.is_symlink() or signature.is_symlink() or not payload.is_file() or not signature.is_file():
            fail("signature statement inputs are unsafe")
        result = subprocess.run(
            ("gpgv", "--status-fd", "1", "--keyring", str(keyring), str(signature), str(payload)),
            capture_output=True,
            check=False,
            env={"PATH": os.environ.get("PATH", "")},
            timeout=30,
        )
        valid: list[str] = []
        for line in result.stdout.decode("utf-8", "replace").splitlines():
            parts = line.split()
            if len(parts) >= 12 and parts[:2] == ["[GNUPG:]", "VALIDSIG"]:
                valid.append(parts[-1])
        if result.returncode != 0 or valid != [signer["fingerprint"]]:
            fail("detached signature does not have exactly the required primary VALIDSIG fingerprint")
        seen.add(role)
    if seen != set(expected):
        fail("missing explicit owner/E/F signatures")
def _verify_approvals(rc: dict[str, Any]) -> None:
    characterization = resolve_rc_input(rc, "characterization")
    characterization_value = load_json(characterization)
    baseline = load_json(resolve_rc_input(rc, "baseline"))
    thresholds = load_json(resolve_rc_input(rc, "thresholds"))
    approvals = load_json(resolve_rc_input(rc, "approvals"))
    contract = load_json(resolve_rc_input(rc, "signer-contract"))
    profile_digest = sha256_file(resolve_rc_input(rc, "profile"))
    if not isinstance(characterization_value, dict) or characterization_value.get("schema") != "podway.g009.characterization/v1" or characterization_value.get("target") != rc["target"] or not isinstance(baseline, dict) or not isinstance(thresholds, dict):
        fail("RC performance inputs are malformed")
    from g009_performance import characterize, thresholds as derive_thresholds
    if characterize(characterization_value.get("workloads")) != baseline:
        fail("RC characterization baseline is not mechanically exact")
    if derive_thresholds(baseline) != thresholds:
        fail("RC thresholds are not mechanically derived")
    if not isinstance(contract, dict) or contract.get("schema") != "podway.g009.approval-signers/v1" or set(contract) != {"schema", "keyring", "signers"}:
        fail("approval signer contract is not exact")
    keyring_raw = os.environ.get("G009_QUALIFICATION_KEYRING")
    keyring_digest = os.environ.get("G009_QUALIFICATION_KEYRING_SHA256")
    fingerprints = {
        "owner": os.environ.get("G009_QUALIFICATION_OWNER_FINGERPRINT"),
        "E": os.environ.get("G009_QUALIFICATION_E_FINGERPRINT"),
        "F": os.environ.get("G009_QUALIFICATION_F_FINGERPRINT"),
    }
    if keyring_raw is None:
        fail("qualification approval trust root is unavailable")
    keyring = Path(keyring_raw)
    signers = contract.get("signers")
    if (
        keyring.is_symlink()
        or not keyring.is_file()
        or not isinstance(keyring_digest, str)
        or not re.fullmatch(r"[0-9a-f]{64}", keyring_digest)
        or sha256_file(keyring) != keyring_digest
        or any(not isinstance(value, str) or not re.fullmatch(r"[0-9A-F]{40}", value) for value in fingerprints.values())
        or len(set(fingerprints.values())) != 3
        or not isinstance(signers, list)
        or len(signers) != 3
    ):
        fail("qualification approval trust root is invalid")
    by_role = {item.get("role"): item for item in signers if isinstance(item, dict)}
    if set(by_role) != {"owner", "E", "F"} or any(
        set(item) != {"role", "signer", "fingerprint"}
        or not isinstance(item.get("signer"), str)
        or not item["signer"]
        or item.get("fingerprint") != fingerprints[item["role"]]
        for item in by_role.values()
    ):
        fail("approval signer roles differ from protected qualification identities")
    if not isinstance(approvals, dict) or approvals.get("schema") != "podway.g009.approvals/v1" or approvals.get("profile_sha256") != profile_digest or approvals.get("characterization_sha256") != sha256_file(characterization) or not isinstance(approvals.get("approvals"), list) or len(approvals["approvals"]) != 3:
        fail("approval bundle is stale or incomplete")
    baseline_digest = sha256_bytes(canonical_json(baseline))
    thresholds_digest = sha256_bytes(canonical_json(thresholds))
    statements: list[dict[str, Any]] = []
    signer_names: set[str] = set()
    for approval in approvals["approvals"]:
        if not isinstance(approval, dict) or set(approval) != {"role", "signer", "fingerprint", "profile_sha256", "characterization_sha256", "baseline_sha256", "thresholds_sha256", "payload", "signature"}:
            fail("approval has mutable or missing fields")
        role = approval["role"]
        expected = by_role.get(role)
        if expected is None or approval["signer"] != expected["signer"] or approval["fingerprint"] != expected["fingerprint"] or approval["profile_sha256"] != profile_digest or approval["characterization_sha256"] != sha256_file(characterization) or approval["baseline_sha256"] != baseline_digest or approval["thresholds_sha256"] != thresholds_digest:
            fail("approval binding or signer contract mismatch")
        immutable_root = rc_input_root("approvals")
        payload_raw, signature_raw = approval["payload"], approval["signature"]
        if not isinstance(payload_raw, str) or not isinstance(signature_raw, str):
            fail("approval detached signature paths are malformed")
        payload, signature = immutable_root / payload_raw, immutable_root / signature_raw
        if (
            Path(payload_raw).is_absolute()
            or Path(signature_raw).is_absolute()
            or payload.is_symlink()
            or signature.is_symlink()
            or not payload.resolve().is_relative_to(immutable_root)
            or not signature.resolve().is_relative_to(immutable_root)
        ):
            fail("approval detached signature paths escape immutable inputs")
        statement = canonical_json({"role": role, "signer": approval["signer"], "fingerprint": approval["fingerprint"], "profile_sha256": profile_digest, "characterization_sha256": approval["characterization_sha256"], "baseline_sha256": baseline_digest, "thresholds_sha256": thresholds_digest})
        if bounded_bytes(payload) != statement or approval["signer"] in signer_names:
            fail("approval payload or signer distinctness is invalid")
        signer_names.add(approval["signer"])
        statements.append({"role": role, "payload": payload, "signature": signature})
    verify_role_signatures(statements, signers, keyring)


def _verify_dependency_policy() -> None:
    policy = load_json(CONTROLLER_ROOT / "release/g009-release-policy-v1.json")
    exceptions = policy.get("dependency_exceptions") if isinstance(policy, dict) else None
    records = exceptions.get("records") if isinstance(exceptions, dict) else None
    if (
        not isinstance(records, list)
        or len(records) != 4
        or exceptions.get("require_exact_cargo_deny_skip_set") is not True
    ):
        fail("dependency exception policy is incomplete")
    expected: set[tuple[str, str]] = set()
    identifiers: set[str] = set()
    for record in records:
        if (
            not isinstance(record, dict)
            or set(record) != {"id", "crate", "owner", "reason", "expires_on"}
            or not all(isinstance(record.get(key), str) and record[key] for key in record)
            or record["id"] in identifiers
        ):
            fail("dependency exception record is malformed")
        try:
            expires = date.fromisoformat(record["expires_on"])
        except ValueError:
            fail("dependency exception expiry is malformed")
        if date.today() >= expires:
            fail(f"dependency exception expired: {record['id']}")
        identifiers.add(record["id"])
        expected.add((record["crate"], record["id"]))
    deny_path = ROOT / "deny.toml"
    if deny_path.is_symlink() or not deny_path.is_file():
        fail("candidate cargo-deny policy is unsafe or missing")
    try:
        deny = tomllib.loads(bounded_bytes(deny_path).decode("utf-8", "strict"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        fail(f"candidate cargo-deny policy is invalid: {exc}")
    skips = deny.get("bans", {}).get("skip")
    if not isinstance(skips, list) or {
        (item.get("crate"), item.get("reason"))
        for item in skips
        if isinstance(item, dict)
    } != expected:
        fail("candidate cargo-deny skips differ from exact unexpired controller policy")

def verify_rc_consumption(path: Path) -> dict[str, Any]:
    supplied = path
    if supplied.is_symlink():
        fail("RC must be a non-symlink immutable evidence artifact")
    raw = bounded_bytes(supplied)
    digest = sha256_bytes(raw)
    resolved = supplied.resolve()
    expected_digest = os.environ.get("G009_EXPECTED_RC_SHA256")
    if expected_digest is None or not re.fullmatch(r"[0-9a-f]{64}", expected_digest) or digest != expected_digest:
        fail("RC digest is not bound to the trusted workflow locator")
    if not resolved.is_relative_to(EVIDENCE_ROOT.resolve()) or resolved.name != f"{digest}.json":
        fail("RC must be an immutable digest-addressed evidence artifact")
    rc = load_rc(resolved, raw=raw)
    require_native_host(rc["target"])
    if _current_source() != {"commit": rc["source"]["commit"], "tree": rc["source"]["tree"]}:
        fail("checked-out source commit/tree differs from immutable RC")
    _verify_approvals(rc)
    _verify_dependency_policy()
    return rc

def load_rc(path: Path, *, raw: bytes | None = None) -> dict[str, Any]:
    rc = load_json(path) if raw is None else load_json_bytes(raw, str(path))
    required = {"schema", "target", "target_tuple", "minimum_macos", "rust", "source", "host", "inputs", "signing", "archive_root", "binaries"}
    if not isinstance(rc, dict) or rc.get("schema") != "podway.g009.rc-intent/v1":
        fail("not an exact G009 RC intent")
    if "target_tuple" not in rc:
        fail("RC target tuple is required")
    if set(rc) != required:
        fail("not an exact G009 RC intent")
    target = rc.get("target")
    require_native_host(target)
    expected_tuple = target_tuple(target)
    observed_tuple = rc["target_tuple"]
    if not isinstance(observed_tuple, dict):
        fail("RC target tuple is malformed")
    if set(observed_tuple) != set(expected_tuple):
        fail("RC target tuple must contain exactly four fields")
    if observed_tuple != expected_tuple:
        fail("RC target tuple differs from RC target")
    if rc.get("rust") != "1.85.0" or rc.get("archive_root") != archive_root(target):
        fail("RC identity drift")
    source, host = rc.get("source"), rc.get("host")
    if (
        not isinstance(source, dict)
        or set(source) != {"commit", "tree", "tools"}
        or not all(isinstance(source.get(key), str) and re.fullmatch(r"[0-9a-f]{40}", source[key]) for key in ("commit", "tree"))
    ):
        fail("RC source identity is malformed")
    tools = source.get("tools")
    if not isinstance(tools, list) or {item.get("id") for item in tools if isinstance(item, dict)} != {"cargo", "rustc"} or len(tools) != 2:
        fail("RC source tool provenance is incomplete")
    for tool in tools:
        expected_version = rf"^{re.escape(tool.get('id', ''))} 1\.85\.0 \(.+\)$"
        if (
            not isinstance(tool, dict)
            or set(tool) != {"id", "version", "path", "path_sha256"}
            or not isinstance(tool["version"], str)
            or not re.fullmatch(expected_version, tool["version"])
            or not isinstance(tool["path"], str)
            or not Path(tool["path"]).is_absolute()
        ):
            fail("RC source tool provenance schema is invalid")
        require_digest(tool["path_sha256"], f"RC source tool {tool['id']}")
        executable = Path(tool["path"])
        if executable.is_symlink() or not executable.is_file() or sha256_file(executable) != tool["path_sha256"]:
            fail("RC source tool provenance observation differs")
    if (
        not isinstance(host, dict)
        or set(host) != {"system", "machine", "platform"}
        or host["system"] != "Darwin"
        or host["machine"] != expected_tuple["host_arch"]
        or not isinstance(host["platform"], str)
        or not host["platform"]
    ):
        fail("RC host identity is malformed")
    signing = rc.get("signing")
    expected_signing = {"posture": "unsigned-internal", "codesign": "not_attempted_missing_credentials", "notarization": "not_attempted_missing_credentials", "stapling": "not_applicable_zip", "gatekeeper": "not_claimed"}
    if signing != expected_signing:
        fail("RC signing posture must be exact unsigned-internal")
    inputs = rc.get("inputs")
    repo = ROOT
    required_roles = {"profile", "characterization", "baseline", "thresholds", "approvals", "signer-contract", "lockfile", "interfaces", "handoffs", "crash-registry", "fuzz-policy", "observability-policy", "podway", "podwayd"}
    if not isinstance(inputs, list) or {item.get("role") for item in inputs if isinstance(item, dict)} != required_roles or len(inputs) != len(required_roles): fail("RC has incomplete input roles")
    for item in inputs:
        if not isinstance(item, dict) or set(item) != {"role", "path", "sha256"}: fail("RC has malformed input")
        resolve_rc_input(rc, item["role"])
        if item["role"] == "profile":
            profile = load_json(resolve_rc_input(rc, "profile"))
            if not isinstance(profile, dict) or profile_target_tuple(profile.get("target")) != expected_tuple:
                fail("RC profile target tuple differs from RC target")
    binaries = rc.get("binaries")
    if not isinstance(binaries, dict) or set(binaries) != {"podway", "podwayd"}:
        fail("RC lacks exact binary provenance")
    for name, bound in binaries.items():
        expected_input = next(item for item in inputs if item["role"] == name)
        if (
            not isinstance(bound, dict)
            or set(bound) != {"sha256", "provenance"}
            or bound["sha256"] != expected_input["sha256"]
            or bound["provenance"] != {"source": source, "target": rc["target"], "rust": rc["rust"]}
        ):
            fail(f"RC binary binding malformed: {name}")
        binary_path = (repo / expected_input["path"]).resolve()
        arch = subprocess.run(
            ("/usr/bin/lipo", "-archs", str(binary_path)),
            capture_output=True,
            check=False,
            env={"PATH": os.environ.get("PATH", "")},
            timeout=30,
        )
        if arch.returncode != 0 or arch.stdout.decode("ascii", "strict").strip() != expected_tuple["mach_o_arch"]:
            fail(f"RC binary is not {expected_tuple['mach_o_arch']}-only: {name}")
    return rc

def payload_manifest(members: dict[str, bytes], *, target: str) -> dict[str, Any]:
    expected_root = archive_root(target)
    return {"schema": "podway.g009.payload-digests/v1", "members": [
        {"path": name, "sha256": sha256_bytes(data), "size": len(data)}
        for name, data in sorted(members.items()) if name != f"{expected_root}/payload-digests-v1.json"]}

def inspect_archive(
    path: Path, declared_members: set[str] | None = None, *, target: str,
) -> dict[str, Any]:
    expected_root = archive_root(target)
    if path.is_symlink() or not path.is_file():
        fail(f"archive missing or unsafe: {path}")
    sidecar = path.with_name(path.name + ".sha256")
    if sidecar.is_symlink() or not sidecar.is_file():
        fail("archive requires detached final checksum")
    archive_bytes = bounded_bytes(path)
    archive_digest = sha256_bytes(archive_bytes)
    try:
        text = bounded_bytes(sidecar, 1024).decode("ascii", "strict").strip().split()
        if len(text) != 1 or text[0] != archive_digest:
            fail("detached final archive checksum mismatch")
        with zipfile.ZipFile(io.BytesIO(archive_bytes)) as archive:
            infos = archive.infolist()
            if not infos or len(infos) > MAX_ARCHIVE_MEMBERS:
                fail("archive member count invalid")
            if sum(info.file_size for info in infos) > MAX_ARCHIVE_UNCOMPRESSED:
                fail("archive exceeds uncompressed bound")
            names: set[str] = set()
            members: dict[str, bytes] = {}
            for info in infos:
                mode = info.external_attr >> 16
                if info.is_dir() or info.filename in names or (mode & 0o170000) != 0o100000:
                    fail("archive has duplicate, directory, or non-regular member")
                safe_extract_member(info.filename, target)
                if info.flag_bits & 0x1 or (mode & 0o777) not in (0o644, 0o755):
                    fail("unsafe archive member metadata")
                if info.filename.startswith(f"{expected_root}/bin/") != ((mode & 0o777) == 0o755):
                    fail("archive executable mode mismatch")
                names.add(info.filename)
                members[info.filename] = archive.read(info)
    except (OSError, zipfile.BadZipFile) as exc:
        fail(f"unsafe archive: {exc}")
    manifest_name = f"{expected_root}/payload-digests-v1.json"
    if manifest_name not in members:
        fail("archive missing internal payload manifest")
    manifest = load_json_from_bytes(members[manifest_name])
    expected = payload_manifest(members, target=target)
    if manifest != expected:
        fail("internal payload manifest mismatch or recursion")
    expected_names = {item["path"] for item in expected["members"]} | {manifest_name}
    if names != expected_names or (declared_members is not None and names != declared_members):
        fail("archive membership differs from declaration")
    return {"archive_sha256": archive_digest, "members": expected["members"]}

def load_json_from_bytes(data: bytes) -> Any:
    return load_json_bytes(data, "internal archive JSON")

def require_bound_file(rc: dict[str, Any], role: str, path: Path) -> None:
    inputs = rc.get("inputs")
    if not isinstance(inputs, list): fail("RC has no inputs")
    matching = [item for item in inputs if isinstance(item, dict) and item.get("role") == role]
    if len(matching) != 1: fail(f"RC requires exactly one bound {role}")
    expected = require_digest(matching[0].get("sha256"), f"RC {role}")
    if sha256_bytes(bounded_bytes(path)) != expected: fail(f"stale or mutated {role}")
