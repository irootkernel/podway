#!/usr/bin/env python3
"""Deterministic G009 policy and negative-sentinel verifier."""
from __future__ import annotations
import argparse
import io
import json
import os
import tempfile
import zipfile
from datetime import date
import tomllib
from fractions import Fraction
from pathlib import Path
import sys
from typing import Any
import re
import subprocess
from g009_common import QualificationError, TARGET, atomic_immutable_json, bounded_bytes, candidate_root, canonical_json, load_json, load_json_bytes, safe_extract_member, sha256_bytes, sha256_file
from g009_performance import characterize, nearest_rank
from g009_release import inspect_archive, load_rc, verify_rc_consumption
from run_g009_qualification import FUZZ_TARGETS, GATES

ARCHIVE_ROOT = "podway-0.1.0-aarch64-apple-darwin"
ROOT = Path(__file__).resolve().parents[1]

def reject(fn: Any, label: str, expected: str | None = None) -> None:
    try:
        fn()
    except QualificationError as exc:
        if expected is not None and expected not in str(exc):
            raise AssertionError(f"sentinel rejected {label} for the wrong reason: {exc}") from exc
        return
    raise AssertionError(f"sentinel did not reject {label}")
def validate_trust_policy(policy: dict[str, Any]) -> None:
    anchors, tools, stages = policy.get("trust_anchors"), policy.get("tool_policy"), policy.get("stage_contract")
    if not isinstance(anchors, dict) or not isinstance(tools, dict) or not isinstance(stages, dict):
        raise QualificationError("trust policy is incomplete")
    if anchors.get("repository") != {"github_context": "github.repository", "allow_dispatch_repository_override": False} or anchors.get("controller") != {"github_context": "github.workflow_sha", "require_full_commit_sha": True}:
        raise QualificationError("repository or controller trust anchor drift")
    if anchors.get("candidate_provenance") != {"required_fields": ["source.commit", "source.tree"], "commit_format": "40-lowercase-hex", "tree_format": "40-lowercase-hex", "require_exact_match": True}:
        raise QualificationError("candidate provenance anchor drift")
    keyring, fingerprints = anchors.get("reviewer_keyring"), anchors.get("reviewer_fingerprints")
    if not isinstance(keyring, dict) or keyring.get("source") != "GitHub Environment release-final-review secret G009_REVIEWER_KEYRING_B64" or keyring.get("materialized_path_argument") != "--reviewer-keyring" or keyring.get("require_sha256_argument") != "--reviewer-keyring-sha256" or keyring.get("sha256_format") != "64-lowercase-hex" or keyring.get("repository_controlled") is not True or keyring.get("fail_closed_when_unavailable") is not True:
        raise QualificationError("reviewer keyring trust anchor drift")
    if not isinstance(fingerprints, dict) or fingerprints.get("roles") != ["owner", "E", "F"] or fingerprints.get("format") != "40-uppercase-hex" or fingerprints.get("require_exact_validsig_primary_fingerprint") is not True or fingerprints.get("require_distinct") is not True or fingerprints.get("fail_closed_when_unavailable") is not True:
        raise QualificationError("reviewer fingerprint trust anchor drift")
    expected_tools = {"bash": None, "cargo": "1.85.0", "cargo-audit": None, "cargo-deny": None, "cargo-fuzz": None, "cargo-llvm-cov": None, "cargo-nightly": "nightly-2026-07-17", "git": None, "gpgv": None, "launchctl": None, "lipo": None, "ps": None, "python3": None, "rustc": "1.85.0", "rustc-nightly": "nightly-2026-07-17", "rustup": None, "sandbox-exec": None, "sysctl": None}
    actual = tools.get("required_tools")
    if (
        tools.get("identity_requirement") != "version-sha256-and-arm64-execution"
        or tools.get("require_exact_tool_set") is not True
        or not isinstance(actual, list)
        or len(actual) != len(expected_tools)
        or any(
            not isinstance(item, dict)
            or item.get("id") not in expected_tools
            or set(item) != ({"id"} if expected_tools[item["id"]] is None else {"id", "version"})
            for item in actual
        )
        or len({item["id"] for item in actual if isinstance(item, dict)}) != len(actual)
        or {item["id"]: item.get("version") for item in actual} != expected_tools
    ):
        raise QualificationError("tool identity policy drift")
    if stages != {"stage_1": {"name": "trusted-controller-qualification", "candidate_is_separate": True, "outputs": ["immutable-acceptance-index", "immutable-archive-bundle"]}, "stage_2": {"name": "independent-final-review", "requires_stage_1_index": True, "requires_independently_anchored_owner_E_F_signatures": True, "outputs": ["final-bundle"]}}:
        raise QualificationError("two-stage release contract drift")

def validate_release_policy(path: Path) -> None:
    value = load_json(path)
    expected_acceptance = [f"ACC-{number:02d}" for number in range(1, 12)] + ["FINAL-001"]
    expected_contracts = [f"G009-CTR-{number:02d}" for number in range(1, 21)]
    policy = value if isinstance(value, dict) else {}
    validate_trust_policy(policy)
    trace = policy.get("traceability")
    index = policy.get("acceptance_index")
    reviewers = policy.get("final_reviewer_attestation")
    exceptions = policy.get("dependency_exceptions")
    crash = policy.get("crash_coverage")
    if (
        policy.get("schema") != "podway.g009.release-policy/v1"
        or policy.get("version") != 1
        or not isinstance(trace, dict)
        or trace.get("required_acceptance_ids") != expected_acceptance
        or trace.get("required_contract_ids") != expected_contracts
        or trace.get("exact_row_count") != 32
        or not isinstance(index, dict)
        or index.get("required_upstream_gate_count") != 19
        or index.get("required_upstream_gate_ids") != ["G009-GATE-PREFLIGHT", "G009-GATE-FORMAT", "G009-GATE-CHECK", "G009-GATE-CLIPPY", "G009-GATE-NATIVE-TESTS", "G009-GATE-CONTRACTS", "G009-GATE-G005", "G009-GATE-G008", "G009-GATE-CRASH", "G009-GATE-OBS", "G009-GATE-FUZZ", "G009-GATE-AUDIT", "G009-GATE-DENY", "G009-GATE-COVERAGE", "G009-GATE-SECURITY", "G009-GATE-MIGRATION", "G009-GATE-PERFORMANCE", "G009-GATE-PACKAGE", "G009-GATE-LIFECYCLE"]
        or "G009-GATE-FINAL-001" in index.get("required_upstream_gate_ids", [])
        or index.get("final_001_is_output_only") is not True
        or not isinstance(reviewers, dict)
        or reviewers.get("required_roles") != ["owner", "E", "F"]
        or reviewers.get("signature_algorithm") != "openpgp-gpgv"
        or reviewers.get("required_signed_digest_fields") != ["qualification_archive_sha256", "acceptance_index_sha256", "rc_sha256", "traceability_sha256", "release_policy_sha256", "tool_manifest_sha256"]
        or reviewers.get("attestation_required_fields") != ["role", "fingerprint", "payload_sha256", "signature_sha256"]
        or crash != {"required_ids": ["C01", "C02", "C03", "C04", "C05", "C06", "C07", "C08", "C09", "C10", "C11", "C12", "C13", "C14", "D01", "D02", "S01", "S02", "S03"], "store_registry": "crates/podway-store/src/lib.rs::PHASE2_CRASH_BOUNDARY_REGISTRY_V1", "require_resolvable_source_locators": True, "require_resolvable_test_locators": True, "require_exact_windows": True}
        or not isinstance(exceptions, dict)
        or exceptions.get("require_exact_cargo_deny_skip_set") is not True
    ):
        raise QualificationError("release policy exact contract drift")
    records = exceptions.get("records")
    if not isinstance(records, list) or len(records) != 4:
        raise QualificationError("dependency exception policy is incomplete")
    exact_records = [
        {"id": "G009-DEP-EX-01", "crate": "foldhash@=0.1.5", "owner": "release-engineering", "reason": "rusqlite/hashlink 0.10 transition", "expires_on": "2026-10-18"},
        {"id": "G009-DEP-EX-02", "crate": "hashbrown@=0.15.5", "owner": "release-engineering", "reason": "rusqlite/hashlink 0.10 transition", "expires_on": "2026-10-18"},
        {"id": "G009-DEP-EX-03", "crate": "hashbrown@=0.16.1", "owner": "release-engineering", "reason": "yaml-rust2/hashlink 0.11 transition", "expires_on": "2026-10-18"},
        {"id": "G009-DEP-EX-04", "crate": "hashlink@=0.10.0", "owner": "release-engineering", "reason": "rusqlite 0.34 transition", "expires_on": "2026-10-18"},
    ]
    if records != exact_records:
        raise QualificationError("dependency exception allowlist replacement")
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

def workflow_run_surface(text: str) -> str:
    lines = text.splitlines()
    commands: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        if line.lstrip().startswith("#"):
            index += 1
            continue
        match = re.match(r"^(\s*)run:\s*(.*)$", line)
        if match is None:
            index += 1
            continue
        indent, inline = len(match.group(1)), match.group(2).strip()
        if inline and inline not in {"|", ">", "|-", ">-"}:
            commands.append(inline)
            index += 1
            continue
        index += 1
        while index < len(lines):
            candidate = lines[index]
            if candidate.strip() and len(candidate) - len(candidate.lstrip()) <= indent:
                break
            if not candidate.lstrip().startswith("#"):
                commands.append(candidate)
            index += 1
    return "\n".join(commands)


def validate_workflow_parity(path: Path, expected_gates: list[str]) -> None:
    stage1 = path.read_text(encoding="utf-8")
    stage2 = (ROOT / ".github/workflows/release-final-review.yml").read_text(encoding="utf-8")
    stage1_exec = workflow_run_surface(stage1)
    stage2_exec = workflow_run_surface(stage2)
    checkout = "actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683"
    download = "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093"
    upload = "actions/upload-artifact@65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08"
    if (
        stage1.count(checkout) != 2
        or stage1.count(download) != 1
        or stage1.count(upload) != 1
        or stage2.count(checkout) != 1
        or stage2.count(download) != 2
        or stage2.count(upload) != 1
    ):
        raise QualificationError("workflow action pin or stage cardinality drift")
    all_uses = re.findall(r"^\s*uses:\s*(\S+)\s*$", stage1 + "\n" + stage2, re.MULTILINE)
    if any(action not in {checkout, download, upload} for action in all_uses):
        raise QualificationError("workflow contains an unallowlisted or unpinned action")
    stage1_commands = (
        "tools/run_g009_qualification.py preflight",
        "tools/run_g009_qualification.py full-gates",
        "tools/run_g009_qualification.py holdout",
        "tools/run_g009_qualification.py package",
        "tools/run_g009_qualification.py lifecycle",
        "tools/run_g009_qualification.py acceptance-index",
        "tools/run_g009_qualification.py qualification-bundle",
    )
    if any(stage1_exec.count(token) != 1 for token in stage1_commands) or [stage1_exec.index(token) for token in stage1_commands] != sorted(stage1_exec.index(token) for token in stage1_commands):
        raise QualificationError("stage-1 command order drift")
    if f"--only {','.join(expected_gates)}" not in stage1_exec:
        raise QualificationError("stage-1 gate order drift")
    stage2_commands = (
        "tools/run_g009_qualification.py final-review",
        "tools/verify_g009_qualification.py --qualification-bundle",
        "tools/run_g009_qualification.py final-bundle",
    )
    if any(stage2_exec.count(token) != 1 for token in stage2_commands) or [stage2_exec.index(token) for token in stage2_commands] != sorted(stage2_exec.index(token) for token in stage2_commands):
        raise QualificationError("stage-2 command order drift")
    stage1_dispatch = stage1.split("workflow_dispatch:", 1)[1].split("permissions:", 1)[0]
    stage2_dispatch = stage2.split("workflow_dispatch:", 1)[1].split("permissions:", 1)[0]
    stage1_inputs = re.findall(r"^      ([a-z0-9_]+):\s*$", stage1_dispatch, re.MULTILINE)
    stage2_inputs = re.findall(r"^      ([a-z0-9_]+):\s*$", stage2_dispatch, re.MULTILINE)
    if stage1_inputs != ["candidate_commit", "candidate_tree", "rc_run_id", "rc_artifact_id", "rc_sha256"]:
        raise QualificationError("stage-1 dispatch input contract drift")
    if stage2_inputs != ["qualification_run_id", "qualification_artifact_id", "qualification_bundle_sha256", "post_index_review_run_id", "post_index_review_artifact_id"]:
        raise QualificationError("stage-2 dispatch input contract drift")
    if any(token in stage1 for token in ("final-review", "release-final-review", "G009_REVIEWER_KEYRING_B64", "G009_OWNER_FINGERPRINT", "G009_E_FINGERPRINT", "G009_F_FINGERPRINT")):
        raise QualificationError("stage-1 improperly contains final-review trust inputs")
    required_stage1 = (
        "ref: ${{ github.workflow_sha }}",
        "ref: ${{ inputs.candidate_commit }}",
        "path: candidate",
        "repository: ${{ github.repository }}",
        "EXPECTED_RC_SHA256",
        "G009_CANDIDATE_ROOT",
        "sysctl.proc_translated",
        "QUALIFICATION_DESCRIPTOR_SHA256",
    )
    required_stage2 = (
        "ref: ${{ github.workflow_sha }}",
        "repository: ${{ github.repository }}",
        "environment: release-final-review",
        "G009_REVIEWER_KEYRING_B64",
        "G009_OWNER_FINGERPRINT",
        "G009_E_FINGERPRINT",
        "G009_F_FINGERPRINT",
        "--reviewer-keyring-sha256",
        "--qualification-bundle",
        "--final-review",
        "--receipt-out",
        "FINAL_BUNDLE_SHA256",
        '"draft": True',
        "remote release asset differs",
        '"draft":false',
    )
    if (
        any(token not in stage1 for token in required_stage1)
        or any(token not in stage2 for token in required_stage2)
        or any(token not in stage2_exec for token in ('"draft": True', "remote release asset differs", '"draft":false'))
    ):
        raise QualificationError("stage trust-source token drift")
    if "G009_REVIEWER_KEYRING_BASE64" in stage2 or "inputs.repository" in stage1 + stage2 or "repository_override" in stage1 + stage2:
        raise QualificationError("workflow admits a mutable trust-source override")
def validate_protocol(path: Path) -> dict[str, Any]:
    value = load_json(path)
    if not isinstance(value, dict) or value.get("schema") != "podway.g009.qualification/v1" or value.get("version") != 1: raise QualificationError("invalid G009 qualification profile")
    target, rust, perf = value.get("target"), value.get("rust"), value.get("performance")
    if not isinstance(target, dict) or target.get("triple") != TARGET or target.get("arch") != "arm64" or target.get("host_arch") != "arm64" or target.get("x86_64_forbidden") is not True or target.get("universal_forbidden") is not True: raise QualificationError("profile is not arm64-only")
    if rust != {"channel": "1.85.0", "version": "1.85.0"}: raise QualificationError("profile Rust identity drift")
    if not isinstance(perf, dict) or perf.get("warmups") != 5 or perf.get("characterization_samples") != 30 or perf.get("holdout_samples") != 30 or perf.get("rounding_permitted") is not False: raise QualificationError("profile performance protocol drift")
    expected_prerequisites = [
        {"id": "G009-APPROVALS-OWNER-E-F", "kind": "protected-environment-detached-human-approvals", "environment": "release-qualification", "required_roles": ["owner", "E", "F"], "secret_sources": ["G009_QUALIFICATION_KEYRING_B64", "G009_QUALIFICATION_OWNER_FINGERPRINT", "G009_QUALIFICATION_E_FINGERPRINT", "G009_QUALIFICATION_F_FINGERPRINT"], "rc_input_roles": ["approvals", "signer-contract"], "workflow_behavior": "verify-against-protected-identities"},
        {"id": "G009-FINAL-REVIEW-OWNER-E-F", "kind": "protected-environment-detached-attestations", "environment": "release-final-review", "required_roles": ["owner", "E", "F"], "secret_sources": ["G009_REVIEWER_KEYRING_B64", "G009_OWNER_FINGERPRINT", "G009_E_FINGERPRINT", "G009_F_FINGERPRINT"], "workflow_behavior": "verify-after-qualification-index"},
    ]
    if value.get("external_prerequisites") != expected_prerequisites:
        raise QualificationError("protected reviewer prerequisite contract drift")
    workloads = value.get("workloads")
    if not isinstance(workloads, list) or len(workloads) != 7 or len({item.get("id") for item in workloads if isinstance(item, dict)}) != 7: raise QualificationError("profile workload cardinality drift")
    policy_path = value.get("release_policy")
    if policy_path != "release/g009-release-policy-v1.json":
        raise QualificationError("profile release policy reference drift")
    validate_release_policy(ROOT / policy_path)
    w07 = next((workload for workload in workloads if isinstance(workload, dict) and workload.get("id") == "G009-W07"), None)
    if not isinstance(w07, dict) or w07.get("input_generator_ref") != "release/g009-release-policy-v1.json#/w07_generator" or "input_generator" in w07 or w07.get("adapter_contract") != {"argument_index": 2, "argument_source": "generated_utf8_string", "argv_prefix": ["podway", "set", "target-audience"], "id": "G009-W07"}:
        raise QualificationError("W07 must reference the sole authoritative generator")
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
        "G009-GATE-FINAL-001": {"command": "final-review", "required_args": ["--qualification-bundle", "--reviewer-keyring", "--reviewer-keyring-sha256", "--reviewer-fingerprint", "--attestation"]},
    }
    if not isinstance(checkpoints, list) or {item.get("id") for item in checkpoints if isinstance(item, dict)} != set(checkpoint_dispatches):
        raise QualificationError("workflow checkpoint replacements drift")
    for checkpoint in checkpoints:
        if not isinstance(checkpoint, dict):
            raise QualificationError("workflow checkpoint replacement is incomplete")
        dispatch = checkpoint.get("dispatch")
        if dispatch != checkpoint_dispatches[checkpoint["id"]]:
            raise QualificationError("workflow checkpoint replacement is incomplete")

def rust_lexical_surface(text: str) -> str:
    """Erase Rust comments and literals without changing source offsets."""
    chars = list(text)
    index = 0
    while index < len(chars):
        if text.startswith("//", index):
            end = text.find("\n", index)
            end = len(chars) if end < 0 else end
            for cursor in range(index, end):
                chars[cursor] = " "
            index = end
        elif text.startswith("/*", index):
            depth, cursor = 1, index + 2
            while cursor < len(chars) and depth:
                if text.startswith("/*", cursor):
                    depth, cursor = depth + 1, cursor + 2
                elif text.startswith("*/", cursor):
                    depth, cursor = depth - 1, cursor + 2
                else:
                    cursor += 1
            if depth:
                raise QualificationError("unterminated Rust block comment")
            for offset in range(index, cursor):
                if chars[offset] != "\n":
                    chars[offset] = " "
            index = cursor
        else:
            raw = re.match(r"(?:br|r)(#{0,})\"", text[index:])
            if raw:
                terminator = '"' + raw.group(1)
                end = text.find(terminator, index + len(raw.group(0)))
                if end < 0:
                    raise QualificationError("unterminated Rust raw string")
                end += len(terminator)
            elif text[index] == '"':
                end = index + 1
                while end < len(chars):
                    if text[end] == "\\":
                        end += 2
                    elif text[end] == '"':
                        end += 1
                        break
                    else:
                        end += 1
                else:
                    raise QualificationError("unterminated Rust string")
            elif (character := re.match(r"'(?:\\.|[^\\'\n])'", text[index:])) is not None:
                end = index + len(character.group(0))
            else:
                index += 1
                continue
            for offset in range(index, end):
                if chars[offset] != "\n":
                    chars[offset] = " "
            index = end
    return "".join(chars)


def rust_matching_brace(surface: str, opening: int) -> int:
    depth = 0
    for index in range(opening, len(surface)):
        if surface[index] == "{":
            depth += 1
        elif surface[index] == "}":
            depth -= 1
            if depth == 0:
                return index
    raise QualificationError("unclosed Rust declaration body")


def rust_tokens(text: str) -> list[tuple[str, str, int]]:
    """Return Rust identifiers, string literals, and punctuation outside comments."""
    tokens: list[tuple[str, str, int]] = []
    index = 0
    while index < len(text):
        if text.startswith("//", index):
            index = text.find("\n", index)
            index = len(text) if index < 0 else index
        elif text.startswith("/*", index):
            depth, index = 1, index + 2
            while index < len(text) and depth:
                if text.startswith("/*", index):
                    depth, index = depth + 1, index + 2
                elif text.startswith("*/", index):
                    depth, index = depth - 1, index + 2
                else:
                    index += 1
            if depth:
                raise QualificationError("unterminated Rust block comment")
        elif text[index].isspace():
            index += 1
        elif raw := re.match(r'(?:br|r)(#{0,})"', text[index:]):
            start, terminator = index, '"' + raw.group(1)
            content_start = index + len(raw.group(0))
            end = text.find(terminator, content_start)
            if end < 0:
                raise QualificationError("unterminated Rust raw string")
            tokens.append(("string", text[content_start:end], start))
            index = end + len(terminator)
        elif text[index] == '"':
            start, index = index, index + 1
            value = []
            while index < len(text) and text[index] != '"':
                if text[index] == "\\":
                    if index + 1 >= len(text):
                        raise QualificationError("unterminated Rust string")
                    value.append(text[index + 1]); index += 2
                else:
                    value.append(text[index]); index += 1
            if index >= len(text):
                raise QualificationError("unterminated Rust string")
            tokens.append(("string", "".join(value), start)); index += 1
        elif character := re.match(r"'(?:\\.|[^\\'\n])'", text[index:]):
            index += len(character.group(0))
        elif match := re.match(r"[A-Za-z_][A-Za-z0-9_]*", text[index:]):
            tokens.append(("ident", match.group(0), index)); index += len(match.group(0))
        else:
            tokens.append(("punct", text[index], index)); index += 1
    return tokens


def rust_declarations(text: str) -> tuple[str, list[tuple[str, int, int, int, str | None]]]:
    surface, tokens = rust_lexical_surface(text), rust_tokens(text)
    results: list[tuple[str, int, int, int, str | None]] = []
    scopes: list[str | None] = [None]
    for index, token in enumerate(tokens):
        kind, value, offset = token
        if value == "{":
            header_start = max(
                (cursor for cursor in range(index - 1, -1, -1) if tokens[cursor][1] in "{};"),
                default=-1,
            ) + 1
            header = tokens[header_start:index]
            impl_positions = [
                position for position, item in enumerate(header) if item[1] == "impl"
            ]
            if impl_positions:
                header = header[impl_positions[-1]:]
                cursor, generic_depth = 1, 0
                if cursor < len(header) and header[cursor][1] == "<":
                    while cursor < len(header):
                        generic_depth += header[cursor][1] == "<"
                        generic_depth -= header[cursor][1] == ">"
                        cursor += 1
                        if generic_depth == 0:
                            break
                if any(item[1] == "for" for item in header[cursor + 1:]):
                    for_index = next(
                        position for position in range(cursor + 1, len(header))
                        if header[position][1] == "for"
                    )
                    owner = (
                        header[for_index + 1][1]
                        if for_index + 1 < len(header) and header[for_index + 1][0] == "ident"
                        else None
                    )
                else:
                    owner = header[cursor][1] if cursor < len(header) and header[cursor][0] == "ident" else None
                scopes.append(owner if owner is not None else "")
            else:
                scopes.append("")
        elif value == "}":
            if len(scopes) == 1:
                raise QualificationError("unbalanced Rust declaration body")
            scopes.pop()
        elif value == "fn" and scopes[-1] != "":
            if index + 2 >= len(tokens) or tokens[index + 1][0] != "ident" or tokens[index + 2][1] != "(":
                continue
            cursor, depth = index + 2, 0
            while cursor < len(tokens):
                if tokens[cursor][1] == "(":
                    depth += 1
                elif tokens[cursor][1] == ")":
                    depth -= 1
                    if depth == 0:
                        break
                cursor += 1
            if cursor >= len(tokens):
                raise QualificationError("unclosed Rust function signature")
            header_end = cursor + 1
            while header_end < len(tokens) and tokens[header_end][1] not in "{;}":
                header_end += 1
            if header_end >= len(tokens) or tokens[header_end][1] != "{":
                continue
            results.append((tokens[index + 1][1], offset, tokens[header_end][2], -1, scopes[-1]))
    return surface, results


def rust_outer_attributes(surface: str, declaration_start: int) -> list[str]:
    attributes: list[str] = []
    cursor = declaration_start
    while True:
        while cursor and surface[cursor - 1].isspace():
            cursor -= 1
        if not cursor or surface[cursor - 1] != "]":
            return attributes
        depth, end = 1, cursor - 2
        while end >= 0 and depth:
            if surface[end] == "]":
                depth += 1
            elif surface[end] == "[":
                depth -= 1
            end -= 1
        if depth or end < 0 or surface[end] != "#":
            return attributes
        attributes.append(surface[end:cursor])
        cursor = end




def validate_rust_function_locator(text: str, symbol: str, require_test: bool) -> None:
    leaf = symbol.rsplit("::", 1)[-1]
    surface, declarations = rust_declarations(text)
    matches = [item for item in declarations if item[0] == leaf]
    if require_test:
        matches = [
            item for item in matches
            if any(re.fullmatch(r"#\s*\[\s*test\s*\]", attribute.strip()) for attribute in rust_outer_attributes(surface, item[1]))
        ]
    elif "::" in symbol:
        owner = symbol.rsplit("::", 2)[-2]
        matches = [item for item in matches if item[4] == owner]
    if len(matches) != 1:
        kind = "test" if require_test else "source"
        raise QualificationError(f"Rust {kind} locator is absent or ambiguous")



def traceability_coverage(rows: list[dict[str, Any]]) -> dict[str, Any]:
    acceptance_rows, contract_rows = rows[:12], rows[12:]
    return {
        "acceptance_mapping_cardinality": {row["id"]: len(row["contract_ids"]) for row in acceptance_rows},
        "acceptance_row_count": len(acceptance_rows),
        "contract_mapping_cardinality": {row["id"]: 1 for row in contract_rows},
        "contract_row_count": len(contract_rows),
        "exact_row_count": len(rows),
        "required_acceptance_families": len({row["acceptance_family"] for row in acceptance_rows}),
        "required_acceptance_ids": [row["id"] for row in acceptance_rows],
        "required_g009_contracts": len(contract_rows),
        "result": "intent-only",
        "status": "incomplete-until-current-rc-evidence",
    }


def validate_traceability(path: Path) -> None:
    value = load_json(path)
    if not isinstance(value, dict) or value.get("release_policy") != "release/g009-release-policy-v1.json":
        raise QualificationError("invalid traceability")
    validate_release_policy(ROOT / value["release_policy"])
    policy = load_json(ROOT / value["release_policy"])
    authority = policy.get("traceability")
    if not isinstance(authority, dict):
        raise QualificationError("traceability policy authority is malformed")
    required = {
        "semantic_rows", "execution_order", "invalidation_rule", "coverage", "evidence_root",
        "required_acceptance_ids", "required_contract_ids", "exact_row_count",
    }
    if not required <= set(authority) or set(value) != {
        "schema", "version", "release_policy", "evidence_root", "execution_order",
        "invalidation_rule", "coverage", "rows",
    }:
        raise QualificationError("traceability schema or policy authority drift")
    rows = value.get("rows")
    semantic_rows = authority["semantic_rows"]
    if (
        value.get("schema") != "podway.g009.traceability/v1"
        or value.get("version") != 1
        or not isinstance(rows, list)
        or not isinstance(semantic_rows, list)
        or rows != semantic_rows
        or len(rows) != authority["exact_row_count"]
        or [row.get("id") for row in rows[:12]] != authority["required_acceptance_ids"]
        or [row.get("id") for row in rows[12:]] != authority["required_contract_ids"]
        or value.get("execution_order") != authority["execution_order"]
        or value.get("invalidation_rule") != authority["invalidation_rule"]
        or value.get("evidence_root") != authority["evidence_root"]
    ):
        raise QualificationError("traceability canonical semantic mapping drift")
    computed_coverage = traceability_coverage(rows)
    if value.get("coverage") != computed_coverage or authority["coverage"] != computed_coverage:
        raise QualificationError("traceability coverage is not mechanically derived from policy rows")

def rust_balanced(tokens: list[tuple[str, str, int]], opening: int, left: str, right: str) -> int:
    depth = 0
    for index in range(opening, len(tokens)):
        if tokens[index][1] == left:
            depth += 1
        elif tokens[index][1] == right:
            depth -= 1
            if depth == 0:
                return index
    raise QualificationError("unclosed Rust token group")


def rust_store_crash_registry(text: str, symbol: str) -> tuple[list[str], dict[str, list[str]]]:
    tokens = rust_tokens(text)
    candidates: list[tuple[int, int]] = []
    depth = 0
    for index, (_, value, _) in enumerate(tokens):
        if value == "{":
            depth += 1
        elif value == "}":
            depth -= 1
        elif depth == 0 and value == "const" and index + 9 < len(tokens):
            header = [item[1] for item in tokens[index - 1:index + 9]]
            if header == ["pub", "const", symbol, ":", "&", "[", "StoreCrashBoundaryV1", "]", "=", "&"]:
                if tokens[index + 9][1] != "[":
                    continue
                candidates.append((index + 9, rust_balanced(tokens, index + 9, "[", "]")))
    if len(candidates) != 1:
        raise QualificationError("store crash registry constant is absent or ambiguous")
    opening, closing = candidates[0]
    if closing + 1 >= len(tokens) or tokens[closing + 1][1] != ";":
        raise QualificationError("store crash registry declaration is malformed")
    ids: list[str] = []
    failpoints: dict[str, list[str]] = {}
    cursor = opening + 1
    required_fields = {"id", "failpoints", "durability", "recovery_invariant", "requirements"}
    while cursor < closing:
        if tokens[cursor][1] == ",":
            cursor += 1
            continue
        if [item[1] for item in tokens[cursor:cursor + 2]] != ["StoreCrashBoundaryV1", "{"]:
            raise QualificationError("store crash registry entry is not a direct struct literal")
        entry_end = rust_balanced(tokens, cursor + 1, "{", "}")
        fields: dict[str, list[tuple[str, str, int]]] = {}
        field_cursor = cursor + 2
        while field_cursor < entry_end:
            if tokens[field_cursor][1] == ",":
                field_cursor += 1
                continue
            if (
                tokens[field_cursor][0] != "ident"
                or field_cursor + 1 >= entry_end
                or tokens[field_cursor + 1][1] != ":"
                or tokens[field_cursor][1] in fields
            ):
                raise QualificationError("store crash registry field is malformed or duplicated")
            name, value_start = tokens[field_cursor][1], field_cursor + 2
            value_end, nested = value_start, 0
            while value_end < entry_end:
                token = tokens[value_end][1]
                if token in "([{":
                    nested += 1
                elif token in ")]}":
                    nested -= 1
                elif token == "," and nested == 0:
                    break
                value_end += 1
            fields[name] = tokens[value_start:value_end]
            field_cursor = value_end + 1
        if set(fields) != required_fields or len(fields["id"]) != 1 or fields["id"][0][0] != "string":
            raise QualificationError("store crash registry fields are incomplete or malformed")
        crash_id = fields["id"][0][1]
        points = fields["failpoints"]
        if len(points) < 3 or [item[1] for item in points[:2]] != ["&", "["] or points[-1][1] != "]":
            raise QualificationError("store crash registry failpoint list is malformed")
        names: list[str] = []
        point_cursor = 2
        while point_cursor < len(points) - 1:
            if points[point_cursor][1] == ",":
                point_cursor += 1
                continue
            if [item[1] for item in points[point_cursor:point_cursor + 3:]] != ["StoreFailpointV1", ":", ":"]:
                raise QualificationError("store crash registry failpoint is not a direct enum variant")
            if point_cursor + 3 >= len(points) - 1 or points[point_cursor + 3][0] != "ident":
                raise QualificationError("store crash registry failpoint variant is malformed")
            names.append(points[point_cursor + 3][1])
            point_cursor += 4
        if crash_id in failpoints:
            raise QualificationError("store crash registry IDs are duplicated")
        ids.append(crash_id); failpoints[crash_id] = names
        cursor = entry_end + 1
    return ids, failpoints


def validate_crash_registry(path: Path) -> None:
    value = load_json(path)
    policy = load_json(ROOT / "release/g009-release-policy-v1.json")
    source_root = candidate_root()
    crash_policy = policy.get("crash_coverage") if isinstance(policy, dict) else None
    expected_ids = crash_policy.get("required_ids") if isinstance(crash_policy, dict) else None
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "version", "coverage", "windows"}
        or value.get("schema") != "podway.g009.crash-boundaries/v1"
        or value.get("version") != 1
        or not isinstance(expected_ids, list)
        or not expected_ids
    ):
        raise QualificationError("crash registry or controller crash policy is malformed")
    coverage = value["coverage"]
    if (
        not isinstance(coverage, dict)
        or coverage.get("required") != expected_ids
        or coverage.get("covered") != expected_ids
        or coverage.get("percent") != 100
        or set(coverage) != {"required", "covered", "percent"}
    ):
        raise QualificationError("crash coverage does not exactly match controller policy")
    registry_locator = crash_policy.get("store_registry")
    if not isinstance(registry_locator, str) or "::" not in registry_locator:
        raise QualificationError("store crash registry locator is malformed")
    registry_relative, registry_symbol = registry_locator.split("::", 1)
    registry_path = source_root / registry_relative
    if registry_path.is_symlink() or not registry_path.is_file() or not registry_path.resolve().is_relative_to(source_root.resolve()):
        raise QualificationError("store crash registry source is unsafe or absent")
    registry_text = bounded_bytes(registry_path).decode("utf-8", "strict")
    derived_store_ids, derived_failpoints = rust_store_crash_registry(registry_text, registry_symbol)
    if derived_store_ids != expected_ids[:14]:
        raise QualificationError("source-derived store crash IDs differ from policy")
    source_bound_failpoints = {
        "C05": ("crates/podway-daemon/src/execution.rs::DaemonExecutionEngineV1::execute_claimed", ["prepare boundary"]),
        "C06": ("crates/podway-daemon/src/native_execution.rs::NativeArtifactVerifierV1::hash_verified_local_artifact", ["artifact verification boundary"]),
        "D01": ("crates/podway-daemon/src/workspace.rs::ValidatedRuntimeDirectoryV1::publish_reset_marker", ["reset marker publish"]),
        "D02": ("crates/podway-daemon/src/registry.rs::persist_registry_v1", ["registry rename"]),
        "S01": ("crates/podway-service/src/lib.rs::StdServiceFilesystemV1::write_atomically", ["AfterTemporaryWrite", "AfterFileSyncAndMode", "BeforeRename", "AfterRename", "AfterParentDirectorySync"]),
        "S02": ("crates/podway-service/src/lib.rs::MacosServiceCommandRunnerV1::install_or_update", ["bootstrap side effect after plist publication"]),
        "S03": ("crates/podway-service/src/lib.rs::MacosServiceCommandRunnerV1::uninstall", ["after first declared remove_file"]),
    }
    for crash_id, (locator, failpoints) in source_bound_failpoints.items():
        if crash_id in derived_failpoints and derived_failpoints[crash_id]:
            raise QualificationError(f"source-bound failpoint mapping conflicts with store registry: {crash_id}")
        derived_failpoints[crash_id] = failpoints
    windows = value["windows"]
    if not isinstance(windows, list) or [item.get("id") for item in windows if isinstance(item, dict)] != expected_ids:
        raise QualificationError("crash windows are missing, duplicated, or reordered")
    proof_fields = {"failpoint", "test", "termination", "recovery", "invariant", "source_locator"}
    for window in windows:
        if not isinstance(window, dict) or set(window) != {"id", "proof"}:
            raise QualificationError("crash window schema is malformed")
        proof = window["proof"]
        if (
            not isinstance(proof, dict)
            or set(proof) != proof_fields
            or not all(isinstance(proof[field], str) and proof[field] for field in proof_fields)
        ):
            raise QualificationError("crash window proof is incomplete")
        for locator_field in ("source_locator", "test"):
            locator = proof[locator_field]
            if "::" not in locator:
                raise QualificationError(f"crash {locator_field} locator is malformed")
            relative, symbol = locator.split("::", 1)
            source_path = source_root / relative
            if source_path.is_symlink() or not source_path.is_file() or not source_path.resolve().is_relative_to(source_root.resolve()):
                raise QualificationError(f"crash {locator_field} path is unsafe or absent")
            locator_text = bounded_bytes(source_path).decode("utf-8", "strict")
            try:
                validate_rust_function_locator(locator_text, symbol, locator_field == "test")
            except QualificationError as error:
                raise QualificationError(
                    f"crash {window['id']} {locator_field} locator is not a real Rust declaration"
                ) from error
            if locator_field == "source_locator" and window["id"] in source_bound_failpoints:
                expected_locator, _ = source_bound_failpoints[window["id"]]
                if locator != expected_locator:
                    raise QualificationError(f"crash {window['id']} source-bound failpoint locator drift")
        declared_failpoints = [item.strip() for item in proof["failpoint"].split(",")]
        expected_failpoints = derived_failpoints.get(window["id"])
        if expected_failpoints is None or declared_failpoints != expected_failpoints:
            raise QualificationError(f"crash {window['id']} failpoints differ from controller-derived registry")

def validate_fuzz_receipt(path: Path, evidence_root: Path, expected_source: dict[str, Any] | None = None) -> None:
    root = evidence_root.resolve()
    if path.is_symlink():
        raise QualificationError("fuzz receipt path is unsafe or absent")
    receipt_path = path.resolve()
    if not receipt_path.is_relative_to(root) or not receipt_path.is_file():
        raise QualificationError("fuzz receipt path is unsafe or absent")
    receipt = load_json(receipt_path)
    required = {"schema", "target", "corpus", "argv", "limits", "stdout", "stderr", "termination_reason",
                "exit_code", "signal", "timeout", "status", "provenance"}
    if not isinstance(receipt, dict) or set(receipt) != required or receipt["schema"] != "podway.g009.fuzz-receipt/v1":
        raise QualificationError("fuzz receipt schema is incomplete")
    if receipt["target"] not in FUZZ_TARGETS or not isinstance(receipt["argv"], list) or not receipt["argv"]:
        raise QualificationError("fuzz receipt target or argv is malformed")
    limits = receipt["limits"]
    if limits != {"stream_bytes": 1024 * 1024, "aggregate_bytes": 2 * 1024 * 1024,
                  "max_total_time": 3600, "timeout_seconds": 5, "rss_limit_mb": 512}:
        raise QualificationError("fuzz receipt limits drift")
    streams = [receipt["stdout"], receipt["stderr"]]
    if any(not isinstance(item, dict) or set(item) != {"path", "bytes", "sha256", "overflow"} for item in streams):
        raise QualificationError("fuzz receipt blob binding is incomplete")
    total = 0
    for stream in streams:
        relative, size, digest = stream["path"], stream["bytes"], stream["sha256"]
        if (not isinstance(relative, str) or not re.fullmatch(r"fuzz/blobs/[0-9a-f]{64}\.bin", relative)
                or not isinstance(size, int) or size < 0 or size > limits["stream_bytes"]
                or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
                or not isinstance(stream["overflow"], bool)):
            raise QualificationError("fuzz blob metadata is malformed")
        supplied_blob = root / relative
        if supplied_blob.is_symlink():
            raise QualificationError("fuzz blob is absent or hash/size binding differs")
        blob = supplied_blob.resolve()
        if not blob.is_relative_to(root) or not blob.is_file() or blob.stat().st_size != size or sha256_file(blob) != digest:
            raise QualificationError("fuzz blob is absent or hash/size binding differs")
        if relative != f"fuzz/blobs/{digest}.bin":
            raise QualificationError("fuzz blob path is not content addressed")
        total += size
    if total > limits["aggregate_bytes"]:
        raise QualificationError("fuzz blobs exceed aggregate capture limit")
    reason = receipt["termination_reason"]
    if reason not in {"completed", "timeout", "output_overflow"} or not isinstance(receipt["timeout"], bool):
        raise QualificationError("fuzz termination receipt is malformed")
    if (reason == "timeout") != receipt["timeout"]:
        raise QualificationError("fuzz timeout binding differs")
    if reason == "output_overflow" and not any(stream["overflow"] for stream in streams):
        raise QualificationError("fuzz overflow has no overflowing stream")
    if reason != "output_overflow" and any(stream["overflow"] for stream in streams):
        raise QualificationError("fuzz stream overflow reason differs")
    if not ((isinstance(receipt["exit_code"], int) and receipt["exit_code"] >= 0 and receipt["signal"] is None)
            or (receipt["exit_code"] is None and isinstance(receipt["signal"], int) and receipt["signal"] > 0)):
        raise QualificationError("fuzz exit status is malformed")
    expected_status = "pass" if reason == "completed" and receipt["exit_code"] == 0 else "fail"
    if receipt["status"] != expected_status:
        raise QualificationError("fuzz receipt status is incomplete")
    provenance = receipt["provenance"]
    if not isinstance(provenance, dict) or set(provenance) != {"source", "profile_sha256", "toolchain", "sources"}:
        raise QualificationError("fuzz receipt provenance is incomplete")
    source = provenance["source"]
    if (not isinstance(source, dict) or not isinstance(source.get("commit"), str) or not re.fullmatch(r"[0-9a-f]{40}", source["commit"])
            or not isinstance(source.get("tree"), str) or not re.fullmatch(r"[0-9a-f]{40}", source["tree"])):
        raise QualificationError("fuzz receipt lacks full commit/tree binding")
    if expected_source is not None and source != expected_source:
        raise QualificationError("fuzz receipt source differs from gate evidence")
    if not isinstance(provenance["profile_sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", provenance["profile_sha256"]):
        raise QualificationError("fuzz receipt profile digest is malformed")
    toolchain, sources = provenance["toolchain"], provenance["sources"]
    if not isinstance(toolchain, dict) or not isinstance(toolchain.get("tools"), list) or not toolchain["tools"] or not isinstance(sources, list) or not sources:
        raise QualificationError("fuzz receipt tool or source bindings are absent")
    for tool in toolchain["tools"]:
        if (not isinstance(tool, dict) or set(tool) != {"id", "path", "sha256"} or not isinstance(tool["id"], str)
                or not isinstance(tool["path"], str) or not Path(tool["path"]).is_file()
                or not isinstance(tool["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", tool["sha256"])
                or sha256_file(Path(tool["path"])) != tool["sha256"]):
            raise QualificationError("fuzz receipt tool digest binding is malformed")
    expected_sources = {"Cargo.lock", "fuzz/Cargo.toml", "tools/run_g009_qualification.py",
                        *(f"fuzz/fuzz_targets/{target}.rs" for target in FUZZ_TARGETS)}
    if {item.get("path") for item in sources if isinstance(item, dict)} != expected_sources:
        raise QualificationError("fuzz receipt source set is incomplete")
    for item in sources:
        if (not isinstance(item, dict) or set(item) != {"path", "sha256"} or not isinstance(item["path"], str)
                or not isinstance(item["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])):
            raise QualificationError("fuzz receipt source digest binding is malformed")
        source_path = (ROOT / item["path"]).resolve()
        if not source_path.is_relative_to(ROOT) or not source_path.is_file() or sha256_file(source_path) != item["sha256"]:
            raise QualificationError("fuzz receipt source binding differs")


def validate_fuzz_gate(payload: dict[str, Any], evidence_root: Path) -> None:
    commands = payload.get("commands")
    provenance = payload.get("provenance")
    if not isinstance(provenance, dict) or not isinstance(provenance.get("source"), dict):
        raise QualificationError("fuzz gate provenance is incomplete")
    if not isinstance(commands, list) or [item.get("target") for item in commands if isinstance(item, dict)] != list(FUZZ_TARGETS):
        raise QualificationError("fuzz gate lacks every target receipt")
    for command in commands:
        if not isinstance(command, dict) or set(command) != {"target", "corpus", "receipt", "status"}:
            raise QualificationError("fuzz gate command receipt is malformed")
        binding = command["receipt"]
        if not isinstance(binding, dict) or set(binding) != {"path", "sha256", "status"}:
            raise QualificationError("fuzz receipt reference is incomplete")
        if not isinstance(binding["path"], str) or not re.fullmatch(r"fuzz-receipts/[0-9a-f]{64}\.json", binding["path"]):
            raise QualificationError("fuzz receipt reference path is malformed")
        supplied_receipt = evidence_root.resolve() / binding["path"]
        if supplied_receipt.is_symlink():
            raise QualificationError("fuzz receipt reference is unbound")
        receipt_path = supplied_receipt.resolve()
        if not receipt_path.is_relative_to(evidence_root.resolve()) or not receipt_path.is_file() or sha256_file(receipt_path) != binding["sha256"]:
            raise QualificationError("fuzz receipt reference is unbound")
        validate_fuzz_receipt(receipt_path, evidence_root, provenance["source"])
        receipt = load_json(receipt_path)
        if binding["status"] != receipt["status"] or command["status"] != receipt["status"]:
            raise QualificationError("fuzz receipt status binding differs")
def validate_signatures(keyring: Path, keyring_sha256: str, bindings: list[str], fingerprints: list[str]) -> None:
    if keyring.is_symlink() or not keyring.is_file() or not re.fullmatch(r"[0-9a-f]{64}", keyring_sha256) or sha256_file(keyring) != keyring_sha256:
        raise QualificationError("reviewer keyring binding differs")
    expected: dict[str, str] = {}
    for binding in fingerprints:
        role, separator, fingerprint = binding.partition("=")
        if not separator or role in expected:
            raise QualificationError("reviewer fingerprint binding is malformed or duplicated")
        expected[role] = fingerprint
    if set(expected) != {"owner", "E", "F"} or any(not re.fullmatch(r"[0-9A-F]{40}", fingerprint) for fingerprint in expected.values()) or len(set(expected.values())) != 3:
        raise QualificationError("reviewer fingerprints are incomplete or non-distinct")
    actual = set()
    for binding in bindings:
        role, separator, remainder = binding.partition("=")
        payload_raw, second_separator, signature_raw = remainder.partition("=")
        payload, signature = Path(payload_raw), Path(signature_raw)
        if (
            not separator
            or not second_separator
            or role not in expected
            or role in actual
            or payload.is_symlink()
            or signature.is_symlink()
            or not payload.is_file()
            or not signature.is_file()
        ):
            raise QualificationError("reviewer attestation binding is malformed")
        result = subprocess.run(
            ("gpgv", "--status-fd", "1", "--keyring", str(keyring), str(signature), str(payload)),
            check=False,
            capture_output=True,
            text=True,
            env={"PATH": os.environ.get("PATH", "")},
            timeout=30,
        )
        valid = [line.split() for line in result.stdout.splitlines() if line.startswith("[GNUPG:] VALIDSIG ")]
        if result.returncode != 0 or len(valid) != 1 or len(valid[0]) < 12 or valid[0][-1] != expected[role]:
            raise QualificationError(f"reviewer signature is not an exact primary VALIDSIG: {role}")
        actual.add(role)
    if actual != set(expected):
        raise QualificationError("reviewer attestation set is incomplete")

def validate_final(qualification_path: Path, review_path: Path, evidence_root: Path, keyring: Path, keyring_sha256: str, signature_bindings: list[str], fingerprints: list[str]) -> dict[str, Any]:
    qualification = load_json(qualification_path)
    review = load_json(review_path)
    required = {"schema", "qualification_archive_sha256", "acceptance_index_sha256", "rc_sha256", "traceability_sha256", "release_policy_sha256", "tool_manifest_sha256", "source", "target"}
    if not isinstance(qualification, dict) or set(qualification) != required or qualification.get("schema") != "podway.g009.qualification-bundle/v1":
        raise QualificationError("qualification descriptor is malformed")
    if sha256_file(ROOT / "release/g009-release-policy-v1.json") != qualification["release_policy_sha256"]:
        raise QualificationError("qualification release policy differs from trusted controller")
    root = qualification_path.parent
    if qualification_path.is_symlink() or root.is_symlink():
        raise QualificationError("qualification descriptor path is unsafe")
    archive, index = root / "qualification-bundle.zip", root / "acceptance-index.json"
    archive_bytes = bounded_bytes(archive)
    if archive.is_symlink() or not archive.resolve().is_relative_to(root.resolve()) or sha256_bytes(archive_bytes) != qualification["qualification_archive_sha256"]:
        raise QualificationError("qualification archive binding differs")
    if index.is_symlink() or not index.resolve().is_relative_to(root.resolve()) or not index.is_file() or sha256_file(index) != qualification["acceptance_index_sha256"]:
        raise QualificationError("qualification index binding differs")
    index_value = load_json(index)
    evidence_rows = index_value.get("evidence") if isinstance(index_value, dict) else None
    if (
        not isinstance(index_value, dict)
        or index_value.get("rc_sha256") != qualification["rc_sha256"]
        or index_value.get("target") != qualification["target"]
        or index_value.get("status") != "pass"
        or index_value.get("blockers") != []
        or not isinstance(evidence_rows, list)
        or not evidence_rows
    ):
        raise QualificationError("qualification acceptance index is stale or malformed")
    policy_gate_ids = load_json(ROOT / "release/g009-release-policy-v1.json").get("acceptance_index", {}).get("required_upstream_gate_ids")
    if (
        not isinstance(policy_gate_ids, list)
        or index_value.get("checkpoint_id") != "G009-GATE-ACCEPTANCE-INDEX"
        or index_value.get("upstream_gate_ids") != policy_gate_ids
        or index_value.get("acceptance_ids") != [f"ACC-{number:02d}" for number in range(1, 12)]
        or len(evidence_rows) != len(policy_gate_ids)
        or [row.get("gate_id") for row in evidence_rows if isinstance(row, dict)] != policy_gate_ids
    ):
        raise QualificationError("qualification acceptance index does not reconstruct the exact gate contract")
    expected_names = {
        "rc.json",
        "traceability.json",
        "release-policy.json",
        "acceptance-index.json",
        "archive.zip",
        "archive.zip.sha256",
        "tool-manifest.json",
        "receipt.json",
    }
    for row in evidence_rows:
        relative = row.get("path") if isinstance(row, dict) else None
        digest = row.get("sha256") if isinstance(row, dict) else None
        row_source = row.get("source") if isinstance(row, dict) else None
        public_row_source = (
            {
                "commit": row_source.get("commit"),
                "tree": row_source.get("tree"),
                "tools": [
                    {"id": tool.get("id"), "version": tool.get("version"), "path_sha256": tool.get("path_sha256")}
                    for tool in row_source.get("tools", [])
                    if isinstance(tool, dict)
                ],
            }
            if isinstance(row_source, dict)
            else None
        )
        if (
            not isinstance(row, dict)
            or set(row) != {"gate_id", "path", "sha256", "rc_sha256", "target", "source", "blockers"}
            or row["rc_sha256"] != qualification["rc_sha256"]
            or row["target"] != qualification["target"]
            or public_row_source != qualification["source"]
            or row["blockers"] != []
            or not isinstance(relative, str)
            or Path(relative).is_absolute()
            or ".." in Path(relative).parts
            or not isinstance(digest, str)
            or not re.fullmatch(r"[0-9a-f]{64}", digest)
        ):
            raise QualificationError("qualification evidence envelope binding differs")
        expected_names.add(f"evidence/{relative}")
    try:
        with zipfile.ZipFile(io.BytesIO(archive_bytes)) as bundle:
            names = bundle.namelist()
            if len(names) != len(set(names)) or set(names) != expected_names:
                raise QualificationError("qualification bundle membership is not the exact acceptance set")
            for row in evidence_rows:
                raw_evidence = bundle.read(f"evidence/{row['path']}")
                if sha256_bytes(raw_evidence) != row["sha256"]:
                    raise QualificationError("qualification evidence member digest differs")
                payload = load_json_bytes(raw_evidence, f"evidence/{row['path']}")
                if (
                    not isinstance(payload, dict)
                    or payload.get("status") != "pass"
                    or payload.get("rc_sha256") != qualification["rc_sha256"]
                    or payload.get("target") != qualification["target"]
                    or payload.get("source") != row["source"]
                    or payload.get("blockers") != []
                ):
                    raise QualificationError("qualification evidence payload is not a current pass envelope")
                if row["gate_id"] in GATES:
                    results = payload.get("results")
                    if not isinstance(results, list):
                        raise QualificationError("qualification aggregate gate evidence is incomplete")
                    matched = [
                        item for item in results
                        if isinstance(item, dict)
                        and item.get("gate_id") == row["gate_id"]
                        and item.get("status") == "pass"
                    ]
                    if payload.get("checkpoint_id") != "G009-GATE-GATES" or len(matched) != 1:
                        raise QualificationError("qualification aggregate gate evidence is incomplete")
                elif payload.get("checkpoint_id") != row["gate_id"]:
                    raise QualificationError("qualification checkpoint identity differs")
            if sha256_bytes(bundle.read("tool-manifest.json")) != qualification["tool_manifest_sha256"]:
                raise QualificationError("qualification tool manifest binding differs")
            if sha256_bytes(bundle.read("release-policy.json")) != qualification["release_policy_sha256"]:
                raise QualificationError("qualification release policy binding differs")
            tool_manifest = load_json_bytes(bundle.read("tool-manifest.json"), "qualification tool manifest")
            expected_tool_ids = {
                "bash", "cargo", "cargo-audit", "cargo-deny", "cargo-fuzz",
                "cargo-llvm-cov", "cargo-nightly", "git", "gpgv", "launchctl",
                "lipo", "ps", "python3", "rustc", "rustc-nightly", "rustup",
                "sandbox-exec", "sysctl",
            }
            tools = tool_manifest.get("tools") if isinstance(tool_manifest, dict) else None
            controller_sources = tool_manifest.get("controller_sources") if isinstance(tool_manifest, dict) else None
            expected_controller_sources = [
                {"id": source_id, "path_sha256": sha256_file(ROOT / source_id)}
                for source_id in (
                    ".github/workflows/release.yml",
                    ".github/workflows/release-final-review.yml",
                    "tools/g009_common.py",
                    "tools/g009_performance.py",
                    "tools/g009_release.py",
                    "tools/run_g009_qualification.py",
                    "tools/run_verification.py",
                    "tools/run_g005_vertical.py",
                    "tools/run_g008_dogfood.py",
                    "tools/verify_g009_qualification.py",
                )
            ]
            if (
                not isinstance(tool_manifest, dict)
                or set(tool_manifest) != {"schema", "source", "tools", "controller_sources"}
                or tool_manifest.get("schema") != "podway.g009.release-tool-manifest/v1"
                or tool_manifest.get("source") != qualification["source"]
                or not isinstance(tools, list)
                or {item.get("id") for item in tools if isinstance(item, dict)} != expected_tool_ids
                or len(tools) != len(expected_tool_ids)
                or any(
                    not isinstance(item, dict)
                    or set(item) != {"id", "version", "path_sha256", "architecture"}
                    or item.get("architecture") != "arm64"
                    or not isinstance(item.get("version"), str)
                    or not item["version"]
                    or not isinstance(item.get("path_sha256"), str)
                    or not re.fullmatch(r"[0-9a-f]{64}", item["path_sha256"])
                    for item in tools
                )
                or controller_sources != expected_controller_sources
            ):
                raise QualificationError("qualification tool manifest schema or exact set differs")
    except (KeyError, zipfile.BadZipFile) as exc:
        raise QualificationError(f"qualification archive is invalid: {exc}") from exc
    if not isinstance(review, dict) or review.get("schema") != "podway.g009.final-review/v2" or review.get("status") != "passed" or review.get("blockers") != []:
        raise QualificationError("final review is malformed")
    if review.get("qualification_bundle_sha256") != sha256_file(qualification_path) or any(review.get(field) != qualification.get(field) for field in ("qualification_archive_sha256", "acceptance_index_sha256", "rc_sha256", "traceability_sha256", "release_policy_sha256", "tool_manifest_sha256", "source", "target")):
        raise QualificationError("final review is not bound to qualification identities")
    expected_statement = canonical_json({
        "qualification_archive_sha256": qualification["qualification_archive_sha256"],
        "acceptance_index_sha256": qualification["acceptance_index_sha256"],
        "rc_sha256": qualification["rc_sha256"],
        "traceability_sha256": qualification["traceability_sha256"],
        "release_policy_sha256": qualification["release_policy_sha256"],
        "tool_manifest_sha256": qualification["tool_manifest_sha256"],
    })
    validate_signatures(keyring, keyring_sha256, signature_bindings, fingerprints)
    expected = {role: fingerprint for role, _, fingerprint in (binding.partition("=") for binding in fingerprints)}
    attestations = review.get("attestations")
    if review.get("reviewers") != ["owner", "E", "F"] or not isinstance(attestations, list) or [item.get("role") for item in attestations if isinstance(item, dict)] != ["owner", "E", "F"]:
        raise QualificationError("final review lacks ordered owner/E/F attestations")
    receipt_attestations: list[dict[str, str]] = []
    for item, binding in zip(attestations, signature_bindings):
        role, _, remainder = binding.partition("=")
        payload_raw, _, signature_raw = remainder.partition("=")
        payload, signature = Path(payload_raw), Path(signature_raw)
        if bounded_bytes(payload) != expected_statement:
            raise QualificationError("final reviewer payload is not the exact qualification-bound statement")
        if item.get("fingerprint") != expected[role] or item.get("payload_sha256") != sha256_file(payload) or item.get("signature_sha256") != sha256_file(signature):
            raise QualificationError("final review attestation digest binding differs")
        receipt_attestations.append({
            "role": role,
            "fingerprint": expected[role],
            "payload_sha256": sha256_file(payload),
            "signature_sha256": sha256_file(signature),
        })
    if review_path.is_symlink() or evidence_root.is_symlink() or not review_path.resolve().is_relative_to(evidence_root.resolve()):
        raise QualificationError("final review is outside controller evidence root")
    return {
        "schema": "podway.g009.strict-verifier-receipt/v1",
        "status": "passed",
        "qualification_bundle_sha256": sha256_file(qualification_path),
        "qualification_archive_sha256": qualification["qualification_archive_sha256"],
        "acceptance_index_sha256": qualification["acceptance_index_sha256"],
        "rc_sha256": qualification["rc_sha256"],
        "traceability_sha256": qualification["traceability_sha256"],
        "release_policy_sha256": qualification["release_policy_sha256"],
        "tool_manifest_sha256": qualification["tool_manifest_sha256"],
        "final_review_sha256": sha256_file(review_path),
        "reviewer_keyring_sha256": keyring_sha256,
        "source": qualification["source"],
        "target": qualification["target"],
        "attestations": receipt_attestations,
    }
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
        reject(
            lambda: validate_rust_function_locator(
                "// #[test]\nfn commented_test() {}\n", "commented_test", True
            ),
            "commented #[test] attribute",
            "Rust test locator is absent or ambiguous",
        )
        reject(
            lambda: validate_rust_function_locator(
                "/* impl Owner { fn target() {} } */\nimpl Other { fn target() {} }\n",
                "Owner::target",
                False,
            ),
            "false impl ownership",
            "Rust source locator is absent or ambiguous",
        )
        traceability = load_json(ROOT / "release/g009-traceability-v1.json")
        for field, mutate in (
            ("rows", lambda value: value["rows"][0].__setitem__("owner", "mutated")),
            ("execution order", lambda value: value["execution_order"].pop()),
            ("invalidation rule", lambda value: value["invalidation_rule"].__setitem__("reject_stale_evidence", False)),
            ("coverage", lambda value: value["coverage"].__setitem__("exact_row_count", 31)),
            ("evidence root", lambda value: value.__setitem__("evidence_root", "artifacts/other")),
        ):
            candidate = base / f"traceability-{field.replace(' ', '-')}.json"
            mutated = json.loads(json.dumps(traceability))
            mutate(mutated)
            candidate.write_text(json.dumps(mutated), encoding="utf-8")
            reject(lambda candidate=candidate: validate_traceability(candidate), f"traceability {field} mutation")
        registry = (
            'pub const PHASE2_CRASH_BOUNDARY_REGISTRY_V1: &[StoreCrashBoundaryV1] = &['
            'StoreCrashBoundaryV1 { id: "C01", failpoints: &[], durability: D, '
            'recovery_invariant: "x", requirements: &[] },];'
        )
        if rust_store_crash_registry(registry, "PHASE2_CRASH_BOUNDARY_REGISTRY_V1")[0] != ["C01"]:
            raise AssertionError("registry parser sentinel failed")
        for label, source in (
            ("comment registry", "// " + registry),
            ("string registry", f'const X: &str = "{registry}";'),
            ("raw string registry", f'const X: &str = r#"{registry}"#;'),
            ("macro registry", f"macro_rules! x {{ () => {{ {registry} }}; }}"),
            ("nested registry", f"fn x() {{ {registry} }}"),
            ("duplicate registry", registry + registry),
        ):
            reject(
                lambda source=source: rust_store_crash_registry(source, "PHASE2_CRASH_BOUNDARY_REGISTRY_V1"),
                label,
            )
        for label, source in (
            ("macro function", "macro_rules! x { () => { fn target() {} }; }"),
            ("nested function", "fn outer() { fn target() {} }"),
            ("where impl spoof", "impl Other where Owner: Sized { fn target() {} }"),
        ):
            reject(lambda source=source: validate_rust_function_locator(source, "Owner::target", False), label)
        recursive = base / "recursive.zip"
        with zipfile.ZipFile(recursive, "w") as archive:
            info = zipfile.ZipInfo(f"{ARCHIVE_ROOT}/payload-digests-v1.json")
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            archive.writestr(info, json.dumps({"schema":"podway.g009.payload-digests/v1", "members":[{"path":f"{ARCHIVE_ROOT}/payload-digests-v1.json","sha256":"0"*64,"size":1}]}))
        recursive.with_name(recursive.name + ".sha256").write_text(sha256_file(recursive) + "\n", encoding="ascii")
        reject(lambda: inspect_archive(recursive), "recursive member checksum", "internal payload manifest mismatch")
        escaping = base / "escaping.zip"
        with zipfile.ZipFile(escaping, "w") as archive:
            info = zipfile.ZipInfo("../outside")
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            archive.writestr(info, b"x")
        escaping.with_name(escaping.name + ".sha256").write_text(sha256_file(escaping) + "\n", encoding="ascii")
        reject(lambda: inspect_archive(escaping), "archive traversal", "unsafe relative path")
    from run_g009_qualification import _stream_fuzz_command
    fixtures = (
        ((sys.executable, "-c", "import sys;sys.stdout.buffer.write(b'ok')"), 1.0, "completed", 0),
        ((sys.executable, "-c", "import sys;sys.exit(7)"), 1.0, "completed", 7),
        ((sys.executable, "-c", "import time;time.sleep(60)"), 0.01, "timeout", None),
        ((sys.executable, "-c", "import sys;sys.stdout.buffer.write(b'x'*(1024*1024+1))"), 1.0, "output_overflow", None),
    )
    for argv, timeout, reason, exit_code in fixtures:
        captured = _stream_fuzz_command(argv, cwd=ROOT, env=dict(os.environ), timeout=timeout)
        if captured["termination_reason"] != reason or (exit_code is not None and captured["exit_code"] != exit_code):
            raise AssertionError(f"fuzz streaming sentinel failed: {reason}")
    checkpoints = [
        {"id": "G009-GATE-PREFLIGHT", "dispatch": {"command": "preflight", "required_args": ["--rc"]}},
        {"id": "G009-GATE-PERFORMANCE", "dispatch": {"command": "holdout", "required_args": ["--rc", "--warmups", "--samples", "--bin-dir"]}},
        {"id": "G009-GATE-PACKAGE", "dispatch": {"command": "package", "required_args": ["--rc", "--archive", "--bin-dir"]}},
        {"id": "G009-GATE-LIFECYCLE", "dispatch": {"command": "lifecycle", "required_args": ["--rc", "--archive", "--require-clean-user"]}},
        {"id": "G009-GATE-FINAL-001", "dispatch": {"command": "final-review", "required_args": ["--qualification-bundle", "--reviewer-keyring", "--reviewer-keyring-sha256", "--reviewer-fingerprint", "--attestation"]}},
    ]
    declared = [{"id": gate, "dispatch": {"command": "full-gates", "only": gate, "required_args": ["--rc", "--only"]}} for gate in GATES]
    reject(lambda: validate_gate_declarations({"gates": declared[:-1], "workflow_checkpoints": checkpoints}), "missing allowlisted gate")
    drifted = [dict(gate) for gate in declared]
    drifted[0]["dispatch"] = {"command": "full-gates", "only": "unknown", "required_args": ["--rc", "--only"]}
    reject(lambda: validate_gate_declarations({"gates": drifted, "workflow_checkpoints": checkpoints}), "unknown logical gate dispatch")
    legacy = [dict(checkpoint) for checkpoint in checkpoints]
    legacy[-1] = {"id": "G009-GATE-FINAL-001", "dispatch": {"command": "final-review", "required_args": ["--rc", "--traceability", "--index", "--reviewer", "--reviewer-keyring", "--attestation"]}}
    reject(lambda: validate_gate_declarations({"gates": declared, "workflow_checkpoints": legacy}), "legacy final-review dispatch")
    if nearest_rank([Fraction(number, 1) for number in range(1, 31)], 95, 100) != Fraction(29, 1): raise AssertionError("nearest rank rounded")
    print("G009 deterministic negative sentinels passed")

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="run credential-free deterministic negative sentinels")
    parser.add_argument("--protocol", help="G009 qualification profile to validate")
    parser.add_argument("--traceability", help="G009 traceability registry to validate")
    parser.add_argument("--crash-registry", help="crash registry to validate when it declares coverage")
    parser.add_argument("--qualification-bundle", help="stage-1 qualification-bundle.json descriptor")
    parser.add_argument("--final-review", help="canonical stage-2 final review")
    parser.add_argument("--evidence-root", help="root containing controller evidence")
    parser.add_argument("--fuzz-receipt", help="fuzz receipt to validate with --evidence-root")
    parser.add_argument("--reviewer-keyring", help="repository-controlled final-review keyring")
    parser.add_argument("--reviewer-keyring-sha256", help="SHA-256 of --reviewer-keyring")
    parser.add_argument("--attestation", action="append", default=[], help="ROLE=PAYLOAD=SIGNATURE; repeat owner/E/F")
    parser.add_argument("--reviewer-fingerprint", action="append", default=[], help="ROLE=40-UPPERCASE-HEX primary fingerprint; repeat owner/E/F")
    parser.add_argument("--receipt-out", help="immutable strict-verifier receipt path under --evidence-root")
    args = parser.parse_args()
    try:
        final_requested = any((args.qualification_bundle, args.final_review))
        if not any((args.self_test, args.protocol, args.traceability, args.crash_registry, args.fuzz_receipt, final_requested)): parser.error("supply --self-test and/or validation inputs")
        if args.protocol: validate_protocol(Path(args.protocol))
        if args.traceability: validate_traceability(Path(args.traceability))
        if args.crash_registry: validate_crash_registry(Path(args.crash_registry))
        if args.fuzz_receipt:
            if not args.evidence_root: parser.error("--fuzz-receipt requires --evidence-root")
            validate_fuzz_receipt(Path(args.fuzz_receipt), Path(args.evidence_root))
        receipt: dict[str, Any] | None = None
        if final_requested:
            if not all((args.qualification_bundle, args.final_review, args.evidence_root, args.reviewer_keyring, args.reviewer_keyring_sha256)) or len(args.attestation) != 3 or len(args.reviewer_fingerprint) != 3:
                parser.error("final validation requires --qualification-bundle --final-review --evidence-root --reviewer-keyring --reviewer-keyring-sha256 and three --attestation/--reviewer-fingerprint bindings")
            receipt = validate_final(Path(args.qualification_bundle), Path(args.final_review), Path(args.evidence_root), Path(args.reviewer_keyring), args.reviewer_keyring_sha256, args.attestation, args.reviewer_fingerprint)
            if args.receipt_out:
                receipt_path = Path(args.receipt_out)
                root = Path(args.evidence_root)
                if receipt_path.is_symlink() or not receipt_path.parent.resolve().is_relative_to(root.resolve()):
                    raise QualificationError("strict-verifier receipt path escapes evidence root")
                digest = atomic_immutable_json(receipt_path, receipt)
                print(f"{receipt_path.resolve()} {digest}")
        elif args.receipt_out:
            parser.error("--receipt-out requires final validation")
        if args.self_test:
            self_test()
        if receipt is None or not args.receipt_out:
            print("G009 verifier completed")
        return 0
    except QualificationError as exc:
        print(f"G009 verification failed closed: {exc}", file=__import__("sys").stderr); return 2
if __name__ == "__main__": raise SystemExit(main())
