#!/usr/bin/env python3
"""Deterministic G009 policy and negative-sentinel verifier."""
from __future__ import annotations
import argparse
import json
import tempfile
import zipfile
from datetime import date
import tomllib
from fractions import Fraction
from pathlib import Path
from typing import Any
from g009_common import QualificationError, TARGET, canonical_json, load_json, safe_extract_member, sha256_file
from g009_performance import characterize, nearest_rank
from g009_release import inspect_archive, load_rc, verify_rc_consumption
from run_g009_qualification import FUZZ_TARGETS, GATES

ARCHIVE_ROOT = "podway-0.1.0-aarch64-apple-darwin"
ROOT = Path(__file__).resolve().parents[1]

def reject(fn: Any, label: str) -> None:
    try: fn()
    except QualificationError: return
    raise AssertionError(f"sentinel did not reject {label}")

def validate_release_policy(path: Path) -> None:
    value = load_json(path)
    expected_acceptance = [f"ACC-{number:02d}" for number in range(1, 12)] + ["FINAL-001"]
    expected_contracts = [f"G009-CTR-{number:02d}" for number in range(1, 21)]
    policy = value if isinstance(value, dict) else {}
    trace = policy.get("traceability")
    index = policy.get("acceptance_index")
    reviewers = policy.get("final_reviewer_attestation")
    exceptions = policy.get("dependency_exceptions")
    if (
        policy.get("schema") != "podway.g009.release-policy/v1"
        or policy.get("version") != 1
        or not isinstance(trace, dict)
        or trace.get("required_acceptance_ids") != expected_acceptance
        or trace.get("required_contract_ids") != expected_contracts
        or trace.get("exact_row_count") != 32
        or not isinstance(index, dict)
        or index.get("required_upstream_gate_count") != 19
        or len(index.get("required_upstream_gate_ids", [])) != 19
        or "G009-GATE-FINAL-001" in index.get("required_upstream_gate_ids", [])
        or index.get("final_001_is_output_only") is not True
        or not isinstance(reviewers, dict)
        or reviewers.get("required_roles") != ["owner", "E", "F"]
        or reviewers.get("signature_algorithm") != "openpgp-gpgv"
        or not isinstance(exceptions, dict)
        or exceptions.get("require_exact_cargo_deny_skip_set") is not True
    ):
        raise QualificationError("release policy exact contract drift")
    records = exceptions.get("records")
    if not isinstance(records, list) or len(records) != 4:
        raise QualificationError("dependency exception policy is incomplete")
    expected_skips: set[tuple[str, str]] = set()
    seen_ids: set[str] = set()
    today = date.today()
    for record in records:
        if (
            not isinstance(record, dict)
            or set(record) != {"id", "crate", "owner", "reason", "expires_on"}
            or not all(isinstance(record[field], str) and record[field] for field in record)
            or record["id"] in seen_ids
        ):
            raise QualificationError("dependency exception record is malformed")
        try:
            expires_on = date.fromisoformat(record["expires_on"])
        except ValueError as error:
            raise QualificationError("dependency exception expiry is malformed") from error
        if today >= expires_on:
            raise QualificationError(f"dependency exception expired: {record['id']}")
        seen_ids.add(record["id"])
        expected_skips.add((record["crate"], record["id"]))
    with (ROOT / "deny.toml").open("rb") as source:
        deny = tomllib.load(source)
    skips = deny.get("bans", {}).get("skip")
    if (
        not isinstance(skips, list)
        or {(item.get("crate"), item.get("reason")) for item in skips if isinstance(item, dict)}
        != expected_skips
    ):
        raise QualificationError("cargo-deny skips do not match release policy")

def validate_workflow_parity(path: Path, expected_gates: list[str]) -> None:
    text = path.read_text(encoding="utf-8")
    required_once = [
        "actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683",
        "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093",
        "tools/run_g009_qualification.py preflight",
        "tools/run_g009_qualification.py full-gates",
        "tools/run_g009_qualification.py holdout",
        "tools/run_g009_qualification.py package",
        "tools/run_g009_qualification.py lifecycle",
        "tools/run_g009_qualification.py acceptance-index",
        "tools/run_g009_qualification.py final-review",
        "tools/verify_g009_qualification.py",
    ]
    if any(text.count(token) != 1 for token in required_once):
        raise QualificationError("release workflow command or action parity drift")
    expected_only = ",".join(expected_gates)
    if f"--only {expected_only}" not in text:
        raise QualificationError("release workflow gate order drift")
    ordered = [
        "tools/run_g009_qualification.py preflight",
        "tools/run_g009_qualification.py full-gates",
        "tools/run_g009_qualification.py holdout",
        "tools/run_g009_qualification.py package",
        "tools/run_g009_qualification.py lifecycle",
        "tools/run_g009_qualification.py acceptance-index",
        "tools/run_g009_qualification.py final-review",
        "tools/verify_g009_qualification.py",
    ]
    offsets = [text.index(token) for token in ordered]
    if offsets != sorted(offsets):
        raise QualificationError("release workflow checkpoint order drift")
    for required in (
        '--reviewer owner',
        '--reviewer E',
        '--reviewer F',
        '--attestation "owner=$OWNER_ATTESTATION_PATH=$OWNER_SIGNATURE_PATH"',
        '--attestation "E=$E_ATTESTATION_PATH=$E_SIGNATURE_PATH"',
        '--attestation "F=$F_ATTESTATION_PATH=$F_SIGNATURE_PATH"',
    ):
        if required not in text:
            raise QualificationError("release workflow reviewer attestation parity drift")

def validate_protocol(path: Path) -> dict[str, Any]:
    value = load_json(path)
    if not isinstance(value, dict) or value.get("schema") != "podway.g009.qualification/v1" or value.get("version") != 1: raise QualificationError("invalid G009 qualification profile")
    target, rust, perf = value.get("target"), value.get("rust"), value.get("performance")
    if not isinstance(target, dict) or target.get("triple") != TARGET or target.get("arch") != "arm64" or target.get("host_arch") != "arm64" or target.get("x86_64_forbidden") is not True or target.get("universal_forbidden") is not True: raise QualificationError("profile is not arm64-only")
    if rust != {"channel": "1.85.0", "version": "1.85.0"}: raise QualificationError("profile Rust identity drift")
    if not isinstance(perf, dict) or perf.get("warmups") != 5 or perf.get("characterization_samples") != 30 or perf.get("holdout_samples") != 30 or perf.get("rounding_permitted") is not False: raise QualificationError("profile performance protocol drift")
    workloads = value.get("workloads")
    if not isinstance(workloads, list) or len(workloads) != 7 or len({item.get("id") for item in workloads if isinstance(item, dict)}) != 7: raise QualificationError("profile workload cardinality drift")
    policy_path = value.get("release_policy")
    if policy_path != "release/g009-release-policy-v1.json":
        raise QualificationError("profile release policy reference drift")
    validate_release_policy(ROOT / policy_path)
    validate_workflow_parity(
        ROOT / ".github/workflows/release.yml",
        [gate["id"] for gate in value["gates"]],
    )
    validate_gate_declarations(value)
    fuzz = value.get("fuzz")
    if not isinstance(fuzz, dict) or fuzz.get("corpus_root") != "artifacts/g009/fuzz/corpus" or fuzz.get("surfaces") != list(FUZZ_TARGETS):
        raise QualificationError("profile fuzz surfaces or corpus root drift")
    if fuzz.get("toolchain") != {"channel": "nightly-2026-07-17", "rustc": "1.99.0-nightly (3d50c25bc 2026-07-16)"}:
        raise QualificationError("profile fuzz toolchain drift")
    if fuzz.get("sanitizer_env") != {"ASAN_OPTIONS": "quarantine_size_mb=16:thread_local_quarantine_size_kb=64:detect_odr_violation=0"}:
        raise QualificationError("profile fuzz sanitizer environment drift")
    if fuzz.get("pre_rc") != {"seconds_per_target": 600} or fuzz.get("change_budget") != {"seconds_per_target": 60} or fuzz.get("rc") != {"rss_limit_mb": 512, "seconds_per_target": 3600, "timeout_seconds": 5}:
        raise QualificationError("profile fuzz bounds drift")
    return value

def validate_gate_declarations(value: dict[str, Any]) -> None:
    gates = value.get("gates")
    if not isinstance(gates, list) or len(gates) != len(GATES):
        raise QualificationError("profile gate cardinality drift")
    ids = {item.get("id") for item in gates if isinstance(item, dict)}
    if ids != set(GATES):
        raise QualificationError("profile gate allowlist drift")
    for gate in gates:
        if not isinstance(gate, dict):
            raise QualificationError("malformed profile gate")
        if gate.get("dispatch") != {"command": "full-gates", "only": gate["id"], "required_args": ["--rc", "--only"]}:
            raise QualificationError("profile gate dispatch is not executable")
    checkpoints = value.get("workflow_checkpoints")
    checkpoint_dispatches = {
        "G009-GATE-PREFLIGHT": {"command": "preflight", "required_args": ["--rc"]},
        "G009-GATE-PERFORMANCE": {"command": "holdout", "required_args": ["--rc", "--warmups", "--samples", "--bin-dir"]},
        "G009-GATE-PACKAGE": {"command": "package", "required_args": ["--rc", "--archive", "--bin-dir"]},
        "G009-GATE-LIFECYCLE": {"command": "lifecycle", "required_args": ["--rc", "--archive", "--require-clean-user"]},
        "G009-GATE-FINAL-001": {"command": "final-review", "required_args": ["--rc", "--traceability", "--index", "--reviewer", "--reviewer-keyring", "--attestation"]},
    }
    if not isinstance(checkpoints, list) or {item.get("id") for item in checkpoints if isinstance(item, dict)} != set(checkpoint_dispatches):
        raise QualificationError("workflow checkpoint replacements drift")
    for checkpoint in checkpoints:
        if not isinstance(checkpoint, dict) or checkpoint.get("dispatch") != checkpoint_dispatches[checkpoint["id"]]:
            raise QualificationError("workflow checkpoint replacement is incomplete")

def validate_traceability(path: Path) -> None:
    value = load_json(path)
    if (
        not isinstance(value, dict)
        or value.get("schema") != "podway.g009.traceability/v1"
        or value.get("version") != 1
        or value.get("release_policy") != "release/g009-release-policy-v1.json"
        or not isinstance(value.get("rows"), list)
    ):
        raise QualificationError("invalid traceability")
    validate_release_policy(ROOT / value["release_policy"])
    rows = value["rows"]
    acceptance_ids = [row.get("id") for row in rows[:12] if isinstance(row, dict)]
    contract_ids = [row.get("id") for row in rows[12:] if isinstance(row, dict)]
    if (
        len(rows) != 32
        or acceptance_ids != [f"ACC-{number:02d}" for number in range(1, 12)] + ["FINAL-001"]
        or contract_ids != [f"G009-CTR-{number:02d}" for number in range(1, 21)]
        or any(
            not isinstance(row, dict) or row.get("exception_eligible") is not False
            for row in rows[:12]
        )
        or rows[11].get("executable_gate") != "G009-GATE-FINAL-001"
        or rows[31].get("executable_gate") != "G009-GATE-FINAL-001"
    ):
        raise QualificationError("traceability exact row contract drift")

def validate_crash_registry(path: Path) -> None:
    value = load_json(path)
    if not isinstance(value, dict) or not isinstance(value.get("coverage"), dict):
        raise QualificationError("crash registry has no machine-verifiable coverage")
    coverage = value["coverage"]
    required, covered = coverage.get("required"), coverage.get("covered")
    if not isinstance(required, list) or not isinstance(covered, list) or not required or any(not isinstance(item, str) or not item for item in required + covered):
        raise QualificationError("crash coverage is incomplete")
    if len(required) != len(set(required)) or len(covered) != len(set(covered)) or set(required) != set(covered):
        raise QualificationError("crash coverage does not exactly cover required surfaces")
    if coverage.get("percent") != 100 or set(coverage) != {"required", "covered", "percent"}:
        raise QualificationError("crash coverage is not exact")

def validate_final(rc_path: Path, index_path: Path, review_path: Path, evidence_root: Path) -> None:
    rc = verify_rc_consumption(rc_path)
    index, envelope = load_json(index_path), load_json(review_path)
    review = envelope.get("review") if isinstance(envelope, dict) else None
    digest = sha256_file(rc_path)
    source = rc.get("source")
    if not isinstance(index, dict) or not isinstance(review, dict) or index.get("rc_sha256") != digest or review.get("rc_sha256") != digest or review.get("acceptance_index_sha256") != sha256_file(index_path) or index.get("target") != TARGET or review.get("target") != TARGET or index.get("source") != source or review.get("source") != source or envelope.get("checkpoint_id") != "G009-GATE-FINAL-001" or review.get("status") != "passed" or review.get("blockers") != []:
        raise QualificationError("final evidence is stale or incomplete")
    reviewers = review.get("reviewers")
    if reviewers != ["owner", "E", "F"]:
        raise QualificationError("final review lacks exact ordered owner/E/F reviewers")
    attestations = review.get("attestations")
    if (
        not isinstance(attestations, list)
        or [item.get("reviewer") for item in attestations if isinstance(item, dict)]
        != reviewers
    ):
        raise QualificationError("final reviewer attestations are incomplete")
    rows = index.get("evidence")
    upstream = index.get("upstream_gate_ids")
    if not isinstance(rows, list) or not rows or not isinstance(upstream, list) or [row.get("gate_id") for row in rows if isinstance(row, dict)] != upstream:
        raise QualificationError("final index has no ordered upstream evidence")
    resolved_root = evidence_root.resolve()
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"gate_id", "path", "sha256", "rc_sha256", "target", "source", "blockers"}:
            raise QualificationError("malformed indexed evidence")
        if row["rc_sha256"] != digest or row["target"] != TARGET or row["source"] != source or row["blockers"] != []:
            raise QualificationError("indexed evidence is not bound to current RC")
        artifact = (resolved_root / row["path"]).resolve()
        if not artifact.is_relative_to(resolved_root) or not artifact.is_file() or artifact.is_symlink() or sha256_file(artifact) != row["sha256"]:
            raise QualificationError("indexed evidence drift")
        payload = load_json(artifact)
        if not isinstance(payload, dict) or payload.get("rc_sha256") != digest or payload.get("target") != TARGET or payload.get("source") != source or payload.get("blockers") != [] or payload.get("status") != "pass":
            raise QualificationError("indexed gate artifact is semantically unbound")
def self_test() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        duplicate = base / "duplicate.json"; duplicate.write_text('{"x":1,"x":2}', encoding="utf-8")
        reject(lambda: load_json(duplicate), "duplicate JSON key")
        reject(lambda: canonical_json({"measurement": 1.0}), "rounded metric")
        reject(lambda: safe_extract_member("../escape"), "unsafe extraction")
        reject(lambda: safe_extract_member("other/bin/podway"), "wrong archive root")
        malformed = {"w": {"kind":"latency", "warmups":[1]*5, "samples":[{"numerator":1,"denominator":0}]*30}}
        reject(lambda: characterize(malformed), "malformed rational")
        unstable = {"w": {"kind":"latency", "warmups":[1]*5, "samples":[{"numerator":1 if i < 15 else 100,"denominator":1} for i in range(30)]}}
        reject(lambda: characterize(unstable), "unstable samples")
        recursive = base / "recursive.zip"
        with zipfile.ZipFile(recursive, "w") as archive:
            archive.writestr(f"{ARCHIVE_ROOT}/payload-digests-v1.json", json.dumps({"schema":"podway.g009.payload-digests/v1", "members":[{"path":f"{ARCHIVE_ROOT}/payload-digests-v1.json","sha256":"0"*64,"size":1}]}))
        reject(lambda: inspect_archive(recursive), "recursive member checksum")
        escaping = base / "escaping.zip"
        with zipfile.ZipFile(escaping, "w") as archive: archive.writestr("../outside", b"x")
        reject(lambda: inspect_archive(escaping), "archive traversal")
    checkpoints = [
        {"id": "G009-GATE-PREFLIGHT", "dispatch": {"command": "preflight", "required_args": ["--rc"]}},
        {"id": "G009-GATE-PERFORMANCE", "dispatch": {"command": "holdout", "required_args": ["--rc", "--warmups", "--samples", "--bin-dir"]}},
        {"id": "G009-GATE-PACKAGE", "dispatch": {"command": "package", "required_args": ["--rc", "--archive", "--bin-dir"]}},
        {"id": "G009-GATE-LIFECYCLE", "dispatch": {"command": "lifecycle", "required_args": ["--rc", "--archive", "--require-clean-user"]}},
        {"id": "G009-GATE-FINAL-001", "dispatch": {"command": "final-review", "required_args": ["--rc", "--traceability", "--index", "--reviewer", "--reviewer-keyring", "--attestation"]}},
    ]
    declared = [{"id": gate, "dispatch": {"command": "full-gates", "only": gate, "required_args": ["--rc", "--only"]}} for gate in GATES]
    reject(lambda: validate_gate_declarations({"gates": declared[:-1], "workflow_checkpoints": checkpoints}), "missing allowlisted gate")
    drifted = [dict(gate) for gate in declared]
    drifted[0]["dispatch"] = {"command": "full-gates", "only": "unknown", "required_args": ["--rc", "--only"]}
    reject(lambda: validate_gate_declarations({"gates": drifted, "workflow_checkpoints": checkpoints}), "unknown logical gate dispatch")
    if nearest_rank([Fraction(number, 1) for number in range(1, 31)], 95, 100) != Fraction(29, 1): raise AssertionError("nearest rank rounded")
    print("G009 deterministic negative sentinels passed")

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="run credential-free deterministic negative sentinels")
    parser.add_argument("--protocol", help="G009 qualification profile to validate")
    parser.add_argument("--traceability", help="G009 traceability registry to validate")
    parser.add_argument("--crash-registry", help="crash registry to validate when it declares coverage")
    parser.add_argument("--rc", help="RC intent bound to the final evidence")
    parser.add_argument("--index", help="final acceptance index")
    parser.add_argument("--review", help="final review")
    parser.add_argument("--evidence-root", help="root containing indexed local evidence")
    args = parser.parse_args()
    try:
        final_requested = any((args.rc, args.index, args.review, args.evidence_root))
        if not any((args.self_test, args.protocol, args.traceability, args.crash_registry, final_requested)): parser.error("supply --self-test and/or validation inputs")
        if args.protocol: validate_protocol(Path(args.protocol))
        if args.traceability: validate_traceability(Path(args.traceability))
        if args.crash_registry: validate_crash_registry(Path(args.crash_registry))
        if final_requested:
            if not all((args.rc, args.index, args.review, args.evidence_root)): parser.error("final validation requires --rc --index --review --evidence-root")
            validate_final(Path(args.rc), Path(args.index), Path(args.review), Path(args.evidence_root))
        if args.self_test: self_test()
        print("G009 verifier completed")
        return 0
    except QualificationError as exc:
        print(f"G009 verification failed closed: {exc}", file=__import__("sys").stderr); return 2
if __name__ == "__main__": raise SystemExit(main())
