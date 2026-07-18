#!/usr/bin/env python3
"""Deterministic G009 policy and negative-sentinel verifier."""
from __future__ import annotations
import argparse
import json
import tempfile
import zipfile
from fractions import Fraction
from pathlib import Path
from typing import Any
from g009_common import QualificationError, TARGET, canonical_json, load_json, safe_extract_member, sha256_file
from g009_performance import characterize, nearest_rank
from g009_release import inspect_archive, load_rc
from run_g009_qualification import FUZZ_TARGETS, GATES

ARCHIVE_ROOT = "podway-0.1.0-aarch64-apple-darwin"

def reject(fn: Any, label: str) -> None:
    try: fn()
    except QualificationError: return
    raise AssertionError(f"sentinel did not reject {label}")

def validate_protocol(path: Path) -> dict[str, Any]:
    value = load_json(path)
    if not isinstance(value, dict) or value.get("schema") != "podway.g009.qualification/v1" or value.get("version") != 1: raise QualificationError("invalid G009 qualification profile")
    target, rust, perf = value.get("target"), value.get("rust"), value.get("performance")
    if not isinstance(target, dict) or target.get("triple") != TARGET or target.get("arch") != "arm64" or target.get("host_arch") != "arm64" or target.get("x86_64_forbidden") is not True or target.get("universal_forbidden") is not True: raise QualificationError("profile is not arm64-only")
    if rust != {"channel": "1.85.0", "version": "1.85.0"}: raise QualificationError("profile Rust identity drift")
    if not isinstance(perf, dict) or perf.get("warmups") != 5 or perf.get("characterization_samples") != 30 or perf.get("holdout_samples") != 30 or perf.get("rounding_permitted") is not False: raise QualificationError("profile performance protocol drift")
    workloads = value.get("workloads")
    if not isinstance(workloads, list) or len(workloads) != 7 or len({item.get("id") for item in workloads if isinstance(item, dict)}) != 7: raise QualificationError("profile workload cardinality drift")
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
        "G009-GATE-PREFLIGHT": {"command": "preflight", "required_args": ["--profile", "--target"]},
        "G009-GATE-PERFORMANCE": {"command": "holdout", "required_args": ["--rc", "--warmups", "--samples", "--bin-dir"]},
        "G009-GATE-PACKAGE": {"command": "package", "required_args": ["--rc", "--archive", "--bin-dir"]},
        "G009-GATE-LIFECYCLE": {"command": "lifecycle", "required_args": ["--rc", "--archive", "--require-clean-user"]},
        "G009-GATE-FINAL-001": {"command": "final-review", "required_args": ["--rc", "--traceability", "--evidence-root", "--reviewer", "--require-final-001"]},
    }
    if not isinstance(checkpoints, list) or {item.get("id") for item in checkpoints if isinstance(item, dict)} != set(checkpoint_dispatches):
        raise QualificationError("workflow checkpoint replacements drift")
    for checkpoint in checkpoints:
        if not isinstance(checkpoint, dict) or checkpoint.get("dispatch") != checkpoint_dispatches[checkpoint["id"]]:
            raise QualificationError("workflow checkpoint replacement is incomplete")

def validate_traceability(path: Path) -> None:
    value = load_json(path)
    if not isinstance(value, dict) or value.get("schema") != "podway.g009.traceability/v1" or not isinstance(value.get("rows"), list): raise QualificationError("invalid traceability")
    rows = value["rows"]; ids = [row.get("id") for row in rows if isinstance(row, dict)]
    gates = [row.get("executable_gate") for row in rows if isinstance(row, dict) and row.get("executable_gate")]
    if len(ids) != len(set(ids)) or "FINAL-001" not in ids or not gates: raise QualificationError("traceability is incomplete")

def validate_crash_registry(path: Path) -> None:
    value = load_json(path)
    if not isinstance(value, dict): raise QualificationError("invalid crash registry")
    coverage = value.get("coverage")
    if isinstance(coverage, dict) and coverage.get("percent") not in (100, "100"):
        raise QualificationError("crash coverage is incomplete")

def validate_final(rc_path: Path, index_path: Path, review_path: Path, evidence_root: Path) -> None:
    rc = load_rc(rc_path)
    index, review = load_json(index_path), load_json(review_path)
    digest = sha256_file(rc_path)
    source = rc.get("source")
    if not isinstance(index, dict) or not isinstance(review, dict) or index.get("rc_sha256") != digest or review.get("rc_sha256") != digest or index.get("target") != TARGET or review.get("target") != TARGET or index.get("source") != source or review.get("source") != source or review.get("status") != "passed" or review.get("blockers") != []:
        raise QualificationError("final evidence is stale or incomplete")
    reviewers = review.get("reviewers")
    if not isinstance(reviewers, list) or len(reviewers) < 2 or len(set(reviewers)) != len(reviewers) or not all(isinstance(name, str) and name for name in reviewers):
        raise QualificationError("final review lacks distinct named reviewers")
    rows = index.get("evidence")
    if not isinstance(rows, list) or not rows: raise QualificationError("final index has no evidence")
    resolved_root = evidence_root.resolve()
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("path"), str) or not isinstance(row.get("sha256"), str):
            raise QualificationError("malformed indexed evidence")
        artifact = (resolved_root / row["path"]).resolve()
        if not artifact.is_relative_to(resolved_root) or not artifact.is_file() or sha256_file(artifact) != row["sha256"]:
            raise QualificationError("indexed evidence drift")
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
        {"id": "G009-GATE-PREFLIGHT", "dispatch": {"command": "preflight", "required_args": ["--profile", "--target"]}},
        {"id": "G009-GATE-PERFORMANCE", "dispatch": {"command": "holdout", "required_args": ["--rc", "--warmups", "--samples", "--bin-dir"]}},
        {"id": "G009-GATE-PACKAGE", "dispatch": {"command": "package", "required_args": ["--rc", "--archive", "--bin-dir"]}},
        {"id": "G009-GATE-LIFECYCLE", "dispatch": {"command": "lifecycle", "required_args": ["--rc", "--archive", "--require-clean-user"]}},
        {"id": "G009-GATE-FINAL-001", "dispatch": {"command": "final-review", "required_args": ["--rc", "--traceability", "--evidence-root", "--reviewer", "--require-final-001"]}},
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
