#!/usr/bin/env python3
"""Deterministic G009 policy and negative-sentinel verifier."""
from __future__ import annotations
import base64
import ast
import argparse
import io
import json
import hashlib
import os
import tempfile
import shutil
import tarfile
import stat
import zipfile
from datetime import date
import tomllib
from fractions import Fraction
from pathlib import Path
import sys
from typing import Any
import platform
import re
import shlex
import subprocess
from g009_publication import self_test as publication_controller_self_test
from g009_common import QualificationError, TARGET, archive_root, atomic_immutable_json, bounded_bytes, bounded_process, bounded_process_self_test, bounded_regular_tree, candidate_root, canonical_json, host_manifest, load_json, load_json_bytes, require_native_host, safe_extract_member, sha256_bytes, sha256_file, target_tuple
from g009_performance import characterize, nearest_rank
from g009_release import inspect_archive, load_rc, verify_rc_consumption
from run_g009_qualification import (
    FUZZ_POLICY_MODES, FUZZ_TARGETS, GATES, LABEL, LAUNCHCTL, _active_fuzz_lockfile,
    _candidate_source_manifest, _fuzz_executable_sha256,
    _require_fuzz_executable_unchanged, _fuzz_limits,
    _validate_candidate_build_surface, _validate_final_archive_binding, fuzz_seeds,
    profile as validate_profile, sandboxed_candidate_argv,
)

ROOT = Path(__file__).resolve().parents[1]
def _reconstruct_lifecycle_generation(
    staged: Path, identity: str, metadata: dict[str, Any], log_path: Path,
) -> tuple[str, bytes]:
    def xml_escape(value: str) -> str:
        return value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;").replace("'", "&apos;")

    plist_without_generation = (
        '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0">\n<dict>\n'
        f"  <key>Label</key>\n  <string>{LABEL}</string>\n  <key>PodwayDaemonSha256</key>\n  <string>{identity}</string>\n"
        f"\n  <key>ProgramArguments</key>\n  <array>\n    <string>{xml_escape(str(staged))}</string>\n    <string>--service</string>\n  </array>\n\n  <key>RunAtLoad</key>\n  <true/>\n\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n\n  <key>ThrottleInterval</key>\n  <integer>5</integer>\n\n  <key>ProcessType</key>\n  <string>Background</string>\n\n  <key>StandardOutPath</key>\n  <string>{xml_escape(str(log_path))}</string>\n\n  <key>StandardErrorPath</key>\n  <string>{xml_escape(str(log_path))}</string>\n</dict>\n</plist>\n"
    ).encode()
    preimage = {
        "version": metadata["version"], "label": metadata["label"],
        "daemon_binary": metadata["daemon_binary"], "daemon_identity": metadata["daemon_identity"],
        "installed_at": metadata["installed_at"], "updated_at": metadata["updated_at"],
    }
    generation = sha256_bytes(
        plist_without_generation + b"\n"
        + json.dumps(preimage, separators=(",", ":"), ensure_ascii=False).encode()
    )
    label = f"  <string>{LABEL}</string>".encode()
    return generation, plist_without_generation.replace(
        label,
        label + f"\n  <key>PodwayGeneration</key>\n  <string>{generation}</string>\n".encode(),
        1,
    )
def _reconstruct_service_metadata(metadata: dict[str, Any]) -> bytes:
    fields = (
        "version", "label", "daemon_binary", "daemon_identity", "installed_at",
        "updated_at", "publication_state", "generation",
    )
    if set(metadata) != set(fields):
        raise QualificationError("lifecycle service metadata keys are incomplete")
    return json.dumps(
        {field: metadata[field] for field in fields},
        separators=(",", ":"), ensure_ascii=False,
    ).encode("utf-8") + b"\n"


QUALIFICATION_PROFILE_PATHS = {
    TARGET: ROOT / "release/g009-qualification-v1.json",
}
ARCHIVE_ROOT = archive_root(TARGET)
QUALIFICATION_ZIP_MEMBER_MAX_BYTES = 64 * 1024 * 1024
QUALIFICATION_ZIP_AGGREGATE_MAX_BYTES = 256 * 1024 * 1024
QUALIFICATION_ZIP_MEMBER_COUNT_MAX = 4096
def trusted_qualification_profile(target: str) -> dict[str, Any]:
    path = QUALIFICATION_PROFILE_PATHS.get(target)
    if path is None:
        raise QualificationError("qualification target is not an approved native tuple")
    profile_data = validate_protocol(path)
    if profile_data["target"]["triple"] != target:
        raise QualificationError("trusted qualification profile target binding differs")
    return profile_data
def trusted_fuzz_profile(native_target: Any) -> dict[str, Any]:
    if not isinstance(native_target, str):
        raise QualificationError("fuzz evidence native target is missing or ambiguous")
    return trusted_qualification_profile(native_target)




def _preflight_qualification_bundle(bundle: zipfile.ZipFile) -> None:
    """Reject decompression bombs before reading any bundle member."""
    infos = bundle.infolist()
    if len(infos) > QUALIFICATION_ZIP_MEMBER_COUNT_MAX:
        raise QualificationError("qualification bundle member count exceeds frozen limit")
    aggregate = 0
    for info in infos:
        if info.is_dir() or info.file_size < 0 or info.file_size > QUALIFICATION_ZIP_MEMBER_MAX_BYTES:
            raise QualificationError("qualification bundle member exceeds uncompressed size limit")
        aggregate += info.file_size
        if aggregate > QUALIFICATION_ZIP_AGGREGATE_MAX_BYTES:
            raise QualificationError("qualification bundle aggregate exceeds uncompressed size limit")


def _bounded_corpus_members(
    corpus_path: Path, limits: dict[str, Any],
) -> list[str]:
    return [
        relative
        for relative, _, _ in bounded_regular_tree(
            corpus_path,
            member_limit=limits["corpus_member_count"],
            path_depth=limits["corpus_path_depth"],
            path_length=limits["corpus_path_length"],
            label="current fuzz corpus",
        )
    ]

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
    expected_native_platform = {
        "system": "Darwin",
        "tuples": [
            {"triple": "aarch64-apple-darwin", "arch": "arm64", "host_arch": "arm64", "mach_o_arch": "arm64"},
        ],
        "require_exactly_one_tuple_per_rc": True,
        "require_exactly_one_complete_bundle_per_tuple": True,
        "require_native_execution": True,
        "forbid": ["cross-build", "translated-process", "target-relabel", "universal-binary", "fat-binary"],
    }
    if (
        tools.get("identity_requirement") != "version-sha256-and-exact-native-tuple-execution"
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
        or policy.get("native_platform") != expected_native_platform
    ):
        raise QualificationError("tool identity or native platform policy drift")
    if stages != {"stage_1": {"name": "trusted-controller-qualification", "candidate_is_separate": True, "outputs": ["immutable-acceptance-index", "immutable-archive-bundle"]}, "stage_2": {"name": "independent-final-review", "requires_stage_1_index": True, "requires_independently_anchored_owner_E_F_signatures": True, "outputs": ["final-bundle"]}, "stage_3": {"name": "verified-publication", "requires_immutable_stage_2_handoff": True, "reauthenticates_controller_policy_and_manifest_bindings": True, "outputs": ["published-product-archive-and-declared-sidecars"]}}:
        raise QualificationError("three-stage release contract drift")
    expected_signing = {
        "current_public_package": {
            "posture": "unsigned-not-notarized",
            "codesign": "not_attempted_missing_credentials",
            "notarization": "not_attempted_missing_credentials",
            "stapling": "not_applicable_zip",
            "gatekeeper": "not_claimed",
            "release_notes_asset": "RELEASE_NOTES.md",
            "release_notes_must_document_status": True,
            "status_frozen_for_current_release": True,
        },
        "developer_id_and_notarization": {
            "recommendation": "should_be_completed_when_infrastructure_allows",
            "qualification_requirement": False,
            "detached_human_release_step": True,
        },
        "publication_receipt": {
            "generated_only_after_successful_publication": True,
            "must_bind_release_tag_and_exact_pre_publication_asset_digests": True,
            "is_not_a_pre_publication_asset": True,
        },
    }
    if policy.get("signing_evidence") != expected_signing:
        raise QualificationError("unsigned/not-notarized public-release policy drift")

def validate_release_policy(path: Path) -> None:
    value = load_json(path)
    matrix_contract = {
        "path": "release/product-acceptance-matrix-v1.json",
        "schema": "podway.product-acceptance-matrix/v1",
        "sha256": G036_MATRIX_SHA256,
        "criterion_count": G036_CRITERION_COUNT,
        "require_exact_sot_bullet_order": True,
        "require_bound_proof_file_sha256": True,
    }
    report_contract = {
        "path": "artifacts/g036/g036-test-report.json",
        "kind": "api-package-test-report",
        "schema_version": 6,
        "story_id": "G036",
        "criterion_count": G036_CRITERION_COUNT,
        "exact_command_count": G036_EXACT_COMMAND_COUNT,
        "canonical_matrix_path": G036_MATRIX_PATH.relative_to(ROOT).as_posix(),
        "frozen_matrix_sha256": G036_MATRIX_SHA256,
        "require_current_source_identity": True,
        "require_product_source_tree_digest": True,
        "require_non_ignored_executed_receipts": True,
        "require_trusted_verifier_replay": True,
        "require_complete_product_input_closure": True,
        "require_passed_result": True,
        "require_ambient_network_denial": True,
        "require_invocation_owned_write_roots": True,
        "require_immutable_vendor_manifest": True,
        "require_frozen_canonical_daemon_receipt": True,
        "require_exact_named_test_receipts": True,
        "require_authenticated_exact_read_execute_closure": True,
    }
    expected_acceptance = [f"ACC-{number:02d}" for number in range(1, 12)] + ["FINAL-001"]
    expected_contracts = [f"G009-CTR-{number:02d}" for number in range(1, 21)]
    policy = value if isinstance(value, dict) else {}
    validate_trust_policy(policy)
    if policy.get("product_acceptance_matrix") != matrix_contract:
        raise QualificationError("product acceptance matrix policy contract drift")
    if policy.get("g036_test_report") != report_contract:
        raise QualificationError("G036 test report policy contract drift")
    if policy.get("g036_trusted_environment") != G036_TRUSTED_ENVIRONMENT:
        raise QualificationError("G036 trusted environment policy contract drift")
    matrix_path = ROOT / matrix_contract["path"]
    if sha256_file(matrix_path) != matrix_contract["sha256"]:
        raise QualificationError("product acceptance matrix policy digest is stale")
    validate_product_acceptance_matrix(matrix_path)
    validate_g036_test_report(ROOT / report_contract["path"], matrix_path, path)
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
        or crash != {"required_ids": ["C01", "C02", "C03", "C04", "C05", "C06", "C07", "C08", "C09", "C10", "C11", "C12", "C13", "C14", "C15", "C16", "P01", "D01", "D02", "S01", "S02", "S03"], "store_registry": "crates/podway-store/src/lib.rs::PHASE2_CRASH_BOUNDARY_REGISTRY_V1", "require_resolvable_source_locators": True, "require_resolvable_test_locators": True, "require_exact_windows": True}
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
def _publication_calls(node: ast.AST, receiver: str | None = None) -> list[ast.Call]:
    return [
        child for child in ast.walk(node)
        if isinstance(child, ast.Call)
        and isinstance(child.func, ast.Attribute)
        and (receiver is None or isinstance(child.func.value, ast.Name) and child.func.value.id == receiver)
    ]


def _ast_node(source: str, *, expression: bool = False) -> ast.AST:
    tree = ast.parse(source, mode="eval" if expression else "exec")
    return tree.body if expression else tree.body[0]


def _same_ast(left: ast.AST, right: ast.AST) -> bool:
    return ast.dump(left, annotate_fields=True, include_attributes=False) == ast.dump(
        right, annotate_fields=True, include_attributes=False
    )


def _require_exact_call(function: ast.FunctionDef, expected: str, label: str) -> None:
    target = _ast_node(expected, expression=True)
    calls = [node for node in ast.walk(function) if isinstance(node, ast.Call) and _same_ast(node, target)]
    if len(calls) != 1:
        raise QualificationError(f"publication {label} binding differs")


def _require_publication_skeleton(module: ast.Module, functions: dict[str, ast.FunctionDef], adapter: ast.ClassDef) -> None:
    """Pin runtime bindings, controller graph, entry point, and every REST adapter method."""
    names = (
        "_validate_snapshot",
        "_release_identity",
        "_require_same_release",
        "_verify_assets",
        "_fresh_verified_snapshot",
        "publish_release",
        "main",
    )
    adapter_methods = ("__init__", "_json", "snapshot", "read_asset", "create_draft", "upload", "publish")
    if any(name not in functions for name in names):
        raise QualificationError("publication controller proof surface is incomplete")
    if [node.name for node in adapter.body if isinstance(node, ast.FunctionDef)] != list(adapter_methods):
        raise QualificationError("GitHub transport runtime method bindings differ")
    normalized = "\n".join(
        ast.dump(node, annotate_fields=True, include_attributes=False)
        for node in module.body
    ).encode("utf-8")
    if sha256_bytes(normalized) != "c42bfaf51ce73c10eb10916c23d13b9d39faaa365c0a0ea862ef36d47d8b4779":
        raise QualificationError("publication controller normalized runtime binding drift")


def _require_publication_bindings(module: ast.Module, functions: dict[str, ast.FunctionDef], adapter: ast.ClassDef) -> None:
    protected = {
        "_validate_snapshot", "_release_identity", "_require_same_release",
        "_verify_assets", "_fresh_verified_snapshot", "publish_release", "GitHubTransport",
    }
    if any(alias.asname for node in module.body if isinstance(node, (ast.Import, ast.ImportFrom)) for alias in node.names):
        raise QualificationError("publication runtime imports may not introduce aliases")
    if any(
        isinstance(node, (ast.Assign, ast.AnnAssign, ast.AugAssign, ast.NamedExpr))
        and any(isinstance(target, ast.Name) and target.id in protected for target in ast.walk(node))
        for node in module.body
    ):
        raise QualificationError("publication protected runtime binding is rebound")

    controller = functions["publish_release"]
    if any(isinstance(node, ast.Global) and protected & set(node.names) for node in ast.walk(controller)):
        raise QualificationError("publication controller may not rebind protected helpers")
    for call in ast.walk(controller):
        if not isinstance(call, ast.Call):
            continue
        if isinstance(call.func, ast.Name) and call.func.id in protected - {"GitHubTransport"}:
            if call.func.id == "_fresh_verified_snapshot":
                valid = (
                    len(call.args) == 4
                    and isinstance(call.args[0], ast.Name) and call.args[0].id == "transport"
                    and all(isinstance(arg, ast.Name) and arg.id == name for arg, name in zip(call.args[1:], ("tag", "target", "desired")))
                    and len(call.keywords) == 1 and call.keywords[0].arg == "complete"
                    and isinstance(call.keywords[0].value, ast.Constant) and isinstance(call.keywords[0].value.value, bool)
                )
            elif call.func.id == "_validate_snapshot":
                valid = len(call.args) == 3 and not call.keywords and all(
                    isinstance(arg, ast.Name) and arg.id == name
                    for arg, name in zip(call.args, ("release", "tag", "target"))
                )
            elif call.func.id == "_require_same_release":
                valid = len(call.args) == 2 and not call.keywords and all(isinstance(arg, ast.Name) for arg in call.args)
            else:
                valid = False
            if not valid:
                raise QualificationError("publication controller helper call graph differs")
        if isinstance(call.func, ast.Attribute) and call.func.attr in {"snapshot", "create_draft", "upload", "publish"}:
            expected = {
                "snapshot": ("tag",),
                "create_draft": ("tag", "target"),
                "upload": ("release", "name", "data"),
                "publish": ("publication",),
            }[call.func.attr]
            if (
                not isinstance(call.func.value, ast.Name) or call.func.value.id != "transport"
                or len(call.args) != len(expected) or call.keywords
                or any(not isinstance(arg, ast.Name) or arg.id != name for arg, name in zip(call.args, expected))
            ):
                raise QualificationError("publication controller transport call graph differs")

    main = functions["main"]
    expected_main = _ast_node(
        'publish_release(GitHubTransport(os.environ["GITHUB_API_URL"], os.environ["GITHUB_REPOSITORY"], os.environ["RELEASE_TOKEN"]), tag, target, desired)',
        expression=True,
    )
    main_calls = [node for node in ast.walk(main) if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "publish_release"]
    if len(main_calls) != 1 or not _same_ast(main_calls[0], expected_main):
        raise QualificationError("publication main does not bind the controller to GitHub runtime inputs")
    required_main_statements = (
        'names = os.environ["RELEASE_ASSETS"].split(",")',
        'desired = {name: (Path("release-assets") / name).read_bytes() for name in names}',
        'tag, target = os.environ["RELEASE_TAG"], os.environ["CANDIDATE_COMMIT"]',
        'Path("publication-receipt.json").write_bytes(_publication_receipt(tag, target, desired))',
    )
    for expected in required_main_statements:
        expected_node = _ast_node(expected)
        if len([node for node in main.body if _same_ast(node, expected_node)]) != 1:
            raise QualificationError("publication main runtime input or receipt binding differs")
    guard = module.body[-1] if module.body else None
    if not isinstance(guard, ast.If) or not _same_ast(guard, _ast_node('if __name__ == "__main__":\n    raise SystemExit(main())')):
        raise QualificationError("publication module entry point bypasses main")

    create = next(node for node in adapter.body if isinstance(node, ast.FunctionDef) and node.name == "create_draft")
    publish = next(node for node in adapter.body if isinstance(node, ast.FunctionDef) and node.name == "publish")
    expected_body = _ast_node(
        'body = json.dumps({"tag_name": tag, "target_commitish": target, "name": f"Podway Apple Silicon {tag}", "draft": True, "prerelease": False, "make_latest": "false"}, separators=(",", ":")).encode()'
    )
    if len([node for node in ast.walk(create) if isinstance(node, ast.Assign) and _same_ast(node, expected_body)]) != 1:
        raise QualificationError("publication create payload binding differs")
    _require_exact_call(
        create,
        'self._json(f"{self.api}/repos/{self.repo}/releases", data=body, method="POST")',
        "create POST URL, body, or method",
    )
    _require_exact_call(
        publish,
        """self._json(url, data=b'{"draft":false,"prerelease":false,"make_latest":"false"}', method="PATCH")""",
        "publish PATCH URL, body, or method",
    )


def validate_release_publication_state_machine(stage3_exec: str, *, controller_source: str | None = None) -> None:
    """AST-prove the security-sensitive, point-in-time publication controller."""
    workflow = (ROOT / ".github/workflows/release-publish.yml").read_text(encoding="utf-8")
    if stage3_exec.count("python3 tools/g009_publication.py") != 1 or "method=" in stage3_exec or "concurrency:" not in workflow or "release-publish-${{ github.repository }}-${{ inputs.handoff_sha256 }}" not in workflow or "cancel-in-progress: false" not in workflow:
        raise QualificationError("release workflow must invoke only the pinned serialized controller")
    default_source = controller_source is None
    source = (ROOT / "tools/g009_publication.py").read_text(encoding="utf-8") if default_source else controller_source
    try:
        module = ast.parse(source, filename="tools/g009_publication.py")
    except SyntaxError as exc:
        raise QualificationError("publication controller syntax is invalid") from exc
    function_names = [node.name for node in module.body if isinstance(node, ast.FunctionDef)]
    class_names = [node.name for node in module.body if isinstance(node, ast.ClassDef)]
    unique_functions = {
        "_validate_snapshot", "_release_identity", "_require_same_release", "_verify_assets",
        "_fresh_verified_snapshot", "publish_release", "main", "self_test",
    }
    unique_classes = {"Transport", "GitHubTransport"}
    if any(function_names.count(name) != 1 for name in unique_functions) or any(
        class_names.count(name) != 1 for name in unique_classes
    ):
        raise QualificationError("publication protected runtime declarations are not unique")
    functions = {node.name: node for node in module.body if isinstance(node, ast.FunctionDef)}
    classes = {node.name: node for node in module.body if isinstance(node, ast.ClassDef)}
    required = {"_validate_snapshot", "_release_identity", "_require_same_release", "_verify_assets", "_fresh_verified_snapshot", "publish_release", "main"}
    if not required <= set(functions) or not {"Transport", "GitHubTransport"} <= set(classes):
        raise QualificationError("publication controller surface is incomplete")
    release = next((node for node in module.body if isinstance(node, ast.ClassDef) and node.name == "Release"), None)
    fields = [item.target.id for item in release.body if isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name)] if release else []
    if fields != ["id", "tag", "target", "title", "draft", "prerelease", "immutable", "make_latest", "assets"]:
        raise QualificationError("release metadata identity surface is incomplete")
    identity = ast.unparse(functions["_release_identity"])
    if "release.make_latest" in identity or not all(f"release.{field}" in identity for field in ("id", "tag", "target", "title", "prerelease", "immutable")):
        raise QualificationError("observable release identity boundary drift")
    snapshot_rendered = ast.unparse(functions["_validate_snapshot"])
    if any(check not in snapshot_rendered for check in ("release.tag != tag", "release.target != target", "release.title != f'Podway Apple Silicon {tag}'", "release.prerelease is not False", "release.immutable is not False", "release.make_latest not in (False, None)")):
        raise QualificationError("release policy metadata checks are incomplete")
    protocol_methods = [node.name for node in classes["Transport"].body if isinstance(node, ast.FunctionDef)]
    if protocol_methods != ["snapshot", "read_asset", "create_draft", "upload", "publish"]:
        raise QualificationError("transport methods are not uniquely constrained")
    adapter = classes["GitHubTransport"]
    _require_publication_skeleton(module, functions, adapter)
    _require_publication_bindings(module, functions, adapter)
    all_attributes = [call.func.attr.lower() for call in _publication_calls(module)]
    if {"delete", "overwrite", "replace", "put"} & set(all_attributes):
        raise QualificationError("publication controller permits destructive mutation helpers")
    verbs = {node.value.upper() for node in ast.walk(module) if isinstance(node, ast.Constant) and isinstance(node.value, str)}
    if {"DELETE", "PUT"} & verbs:
        raise QualificationError("publication controller permits destructive HTTP mutation")
    if "per_page=100&page={page}" not in source or "len(page_raw) < 100" not in source:
        raise QualificationError("publication controller does not exhaustively paginate assets")
    if default_source:
        sentinels = (
            ("controller receiver", "transport.upload(release, name, data)", "other.upload(release, name, data)"),
            ("controller helper rebinding", "    release = transport.snapshot(tag)", "    _fresh_verified_snapshot = lambda *args, **kwargs: release\n    release = transport.snapshot(tag)"),
            ("module helper rebinding", "def main() -> int:", "_fresh_verified_snapshot = lambda *args, **kwargs: None\n\ndef main() -> int:"),
            ("import alias bypass", "import json", "import json as json"),
            ("main bypass", "    publish_release(GitHubTransport(", "    other_publish_release(GitHubTransport("),
            ("create POST receiver", "            self._json(f\"{self.api}/repos/{self.repo}/releases\", data=body, method=\"POST\")", "            other._json(f\"{self.api}/repos/{self.repo}/releases\", data=body, method=\"POST\")"),
            ("create POST URL", 'f"{self.api}/repos/{self.repo}/releases", data=body, method="POST"', 'f"{self.api}/repos/{self.repo}/wrong", data=body, method="POST"'),
            ("create POST body", '"tag_name": tag, "target_commitish": target', '"target_commitish": target, "tag_name": tag'),
            ("create POST method", 'method="POST"', 'method="PATCH"'),
            ("publish PATCH receiver", 'self._json(url, data=b\'{"draft":false,"prerelease":false,"make_latest":"false"}\', method="PATCH")', 'other._json(url, data=b\'{"draft":false,"prerelease":false,"make_latest":"false"}\', method="PATCH")'),
            ("publish PATCH URL", 'self._json(url, data=b\'{"draft":false,"prerelease":false,"make_latest":"false"}\', method="PATCH")', 'self._json(other_url, data=b\'{"draft":false,"prerelease":false,"make_latest":"false"}\', method="PATCH")'),
            ("publish PATCH body", 'b\'{"draft":false,"prerelease":false,"make_latest":"false"}\'', 'b\'{"prerelease":false,"draft":false,"make_latest":"false"}\''),
            ("publish PATCH method", 'method="PATCH"', 'method="POST"'),
            ("self-test default rebinding", "def self_test() -> None:", 'def self_test(_=(globals().__setitem__("publish_release", lambda *_a, **_k: None) if __name__ == "__main__" else None)) -> None:'),
            ("duplicate self-test", "def main() -> int:", "def self_test() -> None:\n    pass\n\ndef main() -> int:"),
            ("hidden destructive helper", "def main() -> int:", "def hidden(transport):\n    transport.delete()\n\ndef main() -> int:"),
        )
        for label, before, after in sentinels:
            mutated = source.replace(before, after, 1)
            if mutated == source:
                raise AssertionError(f"publication AST sentinel fixture missing: {label}")
            try:
                validate_release_publication_state_machine(stage3_exec, controller_source=mutated)
            except QualificationError:
                continue
            raise AssertionError(f"publication AST sentinel accepted: {label}")

def validate_workflow_parity(path: Path, expected_gates: list[str]) -> None:
    stage1 = path.read_text(encoding="utf-8")
    stage2 = (ROOT / ".github/workflows/release-final-review.yml").read_text(encoding="utf-8")
    stage3 = (ROOT / ".github/workflows/release-publish.yml").read_text(encoding="utf-8")
    stage1_exec = workflow_run_surface(stage1)
    stage2_exec = workflow_run_surface(stage2)
    stage3_exec = workflow_run_surface(stage3)
    validate_release_publication_state_machine(stage3_exec)
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
        or stage3.count(checkout) != 1
        or stage3.count(download) != 1
        or stage3.count(upload) != 1
    ):
        raise QualificationError("workflow action pin or stage cardinality drift")
    all_uses = re.findall(r"^\s*uses:\s*(\S+)\s*$", stage1 + "\n" + stage2 + "\n" + stage3, re.MULTILINE)
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
    build_token = 'cargo", "+1.85.0", "build", "--locked", "--release"'
    preflight_token = "tools/run_g009_qualification.py preflight"
    if (
        build_token not in stage1_exec
        or stage1_exec.index(preflight_token) >= stage1_exec.index(build_token)
        or "_validate_candidate_build_surface()" not in stage1_exec
        or "sandboxed_candidate_argv" not in stage1_exec
        or "CARGO_TARGET_DIR" not in stage1_exec
        or "candidate source bytes changed during Cargo build" not in stage1_exec
    ):
        raise QualificationError("candidate Cargo validation or containment ordering drift")
    runner_source = (ROOT / "tools/run_g009_qualification.py").read_text(encoding="utf-8")
    containment_required = (
        "def qualification_scratch_root()",
        'G009_SCRATCH_ROOT',
        'f\'(deny file-write* (subpath "{candidate_root}"))\'',
        "def sandboxed_fuzz_execution_argv(",
        "fuzz execution writable path overlaps an immutable root",
        ".g009-transient-create-delete-sentinel",
        "'new-root-child'",
        "'existing-file-mutation'",
        "'rename'",
        "'symlink'",
        "'workload-write'",
        "dir=qualification_scratch_root()",
        'root = qualification_scratch_root() / "fuzz" / "corpus"',
        '"(version 1)(deny default)"',
        'f\'(allow file-write* (subpath "{rendered_scratch}"))\'',
        "def _native_scratch_source(",
        "def _verify_materialized_candidate_source(",
        'sandboxed_candidate_argv(("/bin/cp", "-p", str(source), str(target)))',
        "def _lifecycle_sandbox(",
        "def _lifecycle_candidate_completed(",
        'arguments != [str(expected_staged), "--service"]',
        "controller_lifecycle_cleanup",
        "lifecycle sandbox mutation sentinel failed",
        "LAUNCHCTL = Path(\"/bin/launchctl\")",
        "def _validate_lifecycle_install(",
        "def _lifecycle_generation(",
        "expected_staged = support / \".podway-daemons-v1\" / wrapper_digest",
        'f"{shlex.quote(str(podwayd))} \\"$@\\"\\n"',
        "bounded_bytes(plist) != expected_plist",
        "launchctl state is not bound to the exact target, plist, staged executable, and running process",
        "def _wait_for_launchctl_absence(",
        "def _lifecycle_protected_snapshot(",
        "(allow process-fork)",
        '"route": "install-qualification-wrapper"',
        '"--wrapper-path", qualification_install["wrapper_path"]',
        '"--wrapper-sha256", qualification_install["wrapper_sha256"]',
        '"--sandbox-profile-path", qualification_install["sandbox_profile_path"]',
        '"--sandbox-profile-sha256", qualification_install["sandbox_profile_sha256"]',
        '"--archived-daemon-path", qualification_install["archived_daemon_path"]',
        '"--archived-daemon-sha256", qualification_install["archived_daemon_sha256"]',
    )
    if (
        "protect_candidate_tree" in runner_source
        or "protect_candidate_tree" in stage1_exec
        or any(token not in runner_source for token in containment_required)
        or "G009_SCRATCH_ROOT" not in stage1_exec
        or 'ARCHIVE_PATH=%s' not in stage1_exec
        or '$GITHUB_WORKSPACE/candidate/target' in stage1_exec
    ):
        raise QualificationError("candidate-root containment or scratch allowlist drift")
    stage2_commands = (
        "tools/run_g009_qualification.py final-review",
        "tools/verify_g009_qualification.py --qualification-bundle",
        "tools/run_g009_qualification.py final-bundle",
    )
    if any(stage2_exec.count(token) != 1 for token in stage2_commands) or [stage2_exec.index(token) for token in stage2_commands] != sorted(stage2_exec.index(token) for token in stage2_commands):
        raise QualificationError("stage-2 command order drift")
    stage1_dispatch = stage1.split("workflow_dispatch:", 1)[1].split("permissions:", 1)[0]
    stage2_dispatch = stage2.split("workflow_dispatch:", 1)[1].split("permissions:", 1)[0]
    stage1_inputs = re.findall(r"^      ([a-z0-9_]+):(?:\s*\{.*\})?\s*$", stage1_dispatch, re.MULTILINE)
    stage2_inputs = re.findall(r"^      ([a-z0-9_]+):(?:\s*\{.*\})?\s*$", stage2_dispatch, re.MULTILINE)
    if stage1_inputs != ["candidate_commit", "candidate_tree", "arm64_rc_run_id", "arm64_rc_artifact_id", "arm64_rc_sha256"]:
        raise QualificationError("stage-1 dispatch input contract drift")
    if stage2_inputs != ["arm64_qualification_run_id", "arm64_qualification_artifact_id", "arm64_qualification_bundle_sha256", "arm64_post_index_review_run_id", "arm64_post_index_review_artifact_id"]:
        raise QualificationError("stage-2 dispatch input contract drift")
    if any(token in stage1 for token in ("final-review", "release-final-review", "G009_REVIEWER_KEYRING_B64", "G009_OWNER_FINGERPRINT", "G009_E_FINGERPRINT", "G009_F_FINGERPRINT")):
        raise QualificationError("stage-1 improperly contains final-review trust inputs")
    required_stage2 = (
        "contents: read",
        "environment: release-final-review",
        "G009_REVIEWER_KEYRING_B64",
        "--reviewer-keyring-sha256",
        "--qualification-bundle",
        "--final-review",
        "--receipt-out",
        "--output artifacts/g009/final-bundle/",
        "manifest.json",
        "final-bundle-manifest.json",
        "controller-manifest.json",
        "release-policy.json",
        "controller_sha",
        "producing_workflow_sha",
        "checked_out_workflow_sha",
        "controller_manifest_sha256",
        "release_policy_sha256",
        "product_archive_sha256",
        "final_review_sha256",
        "fingerprint",
        "SHA256SUMS",
        "attestations':review['attestations']",
        "g019-publication-handoff-arm64",
    )
    required_stage3 = (
        "workflow_dispatch:",
        "workflow_call:",
        "contents: write",
        "handoff_run_id",
        "handoff_artifact_id",
        "handoff_sha256",
        ".github/workflows/release-final-review.yml",
        "FINAL_WORKFLOW_SHA",
        "publication-handoff.json",
        "handoff member set is not exact",
        "handoff checksum exact-set failed",
        "handoff workflow SHA binding failed",
        "handoff policy or controller manifest digest differs",
        "final bundle manifest is not exact",
        "controller manifest does not bind checked-out controller",
        "SHA256SUMS",
        "RELEASE_NOTES.md",
        "checked-out release notes are unsafe",
        "declared_assets",
        "CANDIDATE_COMMIT",
        "release-assets",
    )
    if any(token not in stage2 for token in required_stage2) or any(token not in stage3 for token in required_stage3):
        raise QualificationError("stage trust-source token drift")
    if any(
        token in stage3
        for token in (
            "signing_run_id",
            "signing_artifact_id",
            "signing_sha256",
            "SIGNING_RUN",
            "SIGNING_ARTIFACT",
            "SIGNING_SHA256",
            ".github/workflows/release-signing.yml",
            "g009-signing-handoff-arm64",
            ".developer-id.cms",
        )
    ):
        raise QualificationError("unsigned publication workflow retains a signing producer boundary")
    if "declared_assets=[*final_files,'RELEASE_NOTES.md']" not in stage3_exec:
        raise QualificationError("unsigned publication asset set drift")
    exact_stage2 = (
        "G009_TARGET: arm64",
        "G009_TRIPLE: aarch64-apple-darwin",
        "G009_ARCH: arm64",
        "G009_RUNNER_ARCH: ARM64",
        'test "$RUNNER_ARCH" = "$G009_RUNNER_ARCH"',
        'test "$(uname -m)" = "$G009_ARCH"',
        "target_tuple(os.environ['G009_TRIPLE'])",
    )
    if any(token not in stage2 for token in exact_stage2) or any(
        token not in stage3
        for token in (
            "runs-on: [self-hosted, macOS, ARM64, podway-release]",
            'test "$(uname -s)" = Darwin',
            'test "$(uname -m)" = arm64',
            'test "$(sysctl -in sysctl.proc_translated)" = 0',
        )
    ):
        raise QualificationError("workflow native tuple enforcement drift")
    if "contents: write" in stage2 or '"draft":false' in stage2_exec:
        raise QualificationError("final review is not read-only")
    if (
        "G009_REVIEWER_KEYRING_BASE64" in stage2
        or "inputs.repository" in stage1 + stage2 + stage3
        or "repository_override" in stage1 + stage2 + stage3
        or "--clobber" in stage2 + stage3
        or "gh release " in stage2_exec + stage3_exec
    ):
        raise QualificationError("workflow admits a mutable trust-source override")
MIGRATION_EVIDENCE_PATH = ROOT / "release/migration-evidence-v1.json"
MIGRATION_EVIDENCE_SOURCES = (
    "crates/podway-store/src/schema.rs",
    "crates/podway-store/src/sqlite_store.rs",
    "crates/podway-store/tests/phase2_schema_codec.rs",
    "spec/sqlite-v1.sql",
)
def validate_migration_evidence(path: Path = MIGRATION_EVIDENCE_PATH) -> None:
    evidence = load_json(path)
    expected_sources = [{"path": source, "sha256": sha256_file(ROOT / source)} for source in MIGRATION_EVIDENCE_SOURCES]
    expected_fixture = {"path": "tests/fixtures/phase0/schema-0-uninitialized.json", "sha256": sha256_file(ROOT / "tests/fixtures/phase0/schema-0-uninitialized.json")}
    runtime_tests = [
        {"criterionId": criterion_id, "path": member["path"], "function": member["function"], "command": member["command"], "argv": shlex.split(member["command"])}
        for criterion_id in ("PAC-040", "PAC-041")
        for member in G040_PROOF_MEMBERSHIP[criterion_id]
    ]
    expected_preimage = {"schema": "podway.g009.migration-evidence/v3", "version": 3, "fixture": expected_fixture, "provenanceSources": expected_sources, "runtimeTests": runtime_tests}
    expected = {**expected_preimage, "evidenceIdentity": {"kind": "trusted-verifier-replay-required", "sha256": sha256_bytes(canonical_json(expected_preimage))}}
    if evidence != expected:
        raise QualificationError("migration evidence is not an exact runtime-test replay contract")
    toolchain = _g036_toolchain()
    with tempfile.TemporaryDirectory(prefix="podway-g036-migration-") as raw:
        target_dir = Path(raw) / "target"
        for test in runtime_tests:
            argv = shlex.split(test["command"])
            separator = argv.index("--")
            input_tree = _g036_product_source_tree()
            environment = _g036_sanitized_environment(toolchain, target_dir)
            completed = _g036_sandboxed_candidate_run(
                [
                    toolchain["cargo"]["path"],
                    *argv[1:separator],
                    "--target",
                    G036_TARGET["triple"],
                    *argv[separator:],
                ],
                environment,
                target_dir,
            )
            _validate_g036_post_command_identity(input_tree, toolchain)
            if completed.returncode != 0:
                raise QualificationError("migration trusted replay command failed under the hermetic sandbox")
            test_count, ignored_count = _validate_cargo_receipt_output(completed.stdout + completed.stderr, {"path": test["path"], "function": test["function"]})
            if (test_count, ignored_count) != (1, 0):
                raise QualificationError("migration trusted replay did not execute the exact passing PAC runtime test")
def validate_protocol(path: Path) -> dict[str, Any]:
    validate_profile(path)
    value = load_json(path)
    if not isinstance(value, dict) or value.get("schema") != "podway.g009.qualification/v1" or value.get("version") != 1: raise QualificationError("invalid G009 qualification profile")
    target, rust, perf = value.get("target"), value.get("rust"), value.get("performance")
    if target != {**target_tuple(target.get("triple") if isinstance(target, dict) else ""), "native_required": True, "universal_forbidden": True}:
        raise QualificationError("profile is not an exact native target tuple")
    if value.get("archive", {}).get("root") != archive_root(target["triple"]) + "/":
        raise QualificationError("profile archive root does not bind its native target tuple")
    if rust != {"channel": "1.85.0", "version": "1.85.0"}: raise QualificationError("profile Rust identity drift")
    if not isinstance(perf, dict) or perf.get("warmups") != 5 or perf.get("characterization_samples") != 30 or perf.get("holdout_samples") != 30 or perf.get("rounding_permitted") is not False: raise QualificationError("profile performance protocol drift")
    expected_prerequisites = [
        {"id": "G009-APPROVALS-OWNER-E-F", "kind": "protected-environment-detached-human-approvals", "environment": "release-qualification", "required_roles": ["owner", "E", "F"], "secret_sources": ["G009_QUALIFICATION_KEYRING_B64", "G009_QUALIFICATION_OWNER_FINGERPRINT", "G009_QUALIFICATION_E_FINGERPRINT", "G009_QUALIFICATION_F_FINGERPRINT"], "rc_input_roles": ["approvals", "signer-contract"], "workflow_behavior": "verify-against-protected-identities", "approval_keyring_sha256_required": True},
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
    validate_migration_evidence()
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
    expected_fuzz_policies = {
        "rc": {"rss_limit_mb": 512, "seconds_per_target": 3600, "timeout_seconds": 5},
        "local_smoke": {"rss_limit_mb": 512, "seconds_per_target": 5, "timeout_seconds": 5},
    }
    if fuzz.get("pre_rc") != {"seconds_per_target": 600} or fuzz.get("change_budget") != {"seconds_per_target": 60} or any(
        fuzz.get(mode) != limits for mode, limits in expected_fuzz_policies.items()
    ):
        raise QualificationError("profile fuzz bounds drift")
    fuzz_seeds(value)
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
G040_SEMANTIC_CRITERION_IDS = ('PAC-005', 'PAC-010', 'PAC-017', 'PAC-018', 'PAC-022', 'PAC-024', 'PAC-026', 'PAC-027', 'PAC-030', 'PAC-036', 'PAC-037', 'PAC-040', 'PAC-041', 'PAC-044', 'PAC-045', 'PAC-048', 'PAC-050', 'PAC-053', 'PAC-062', 'PAC-063', 'PAC-064')
G040_OBLIGATIONS = {'PAC-005': (('reject-missing-required-item', 'Completion rejects when any required item is unsatisfied.'), ('reject-changed-required-local-artifact', 'Completion rejects when required local-artifact evidence no longer matches the current artifact.'), ('reject-open-blocker', 'Completion rejects while the active attempt has an open blocker.'), ('rejection-is-nonmutating', 'Every rejected completion preserves the prior aggregate and revision.')), 'PAC-010': (('allow-explicit-skip', 'A stage whose policy explicitly permits skip can be skipped.'), ('reject-unpermitted-skip', 'A stage without explicit skip permission rejects skip.'), ('rejected-skip-is-nonmutating', 'A rejected skip preserves the prior aggregate and revision.')), 'PAC-017': (('mutation-enters-daemon-ipc', 'Every normal public mutation route enters framed daemon IPC.'), ('daemon-owns-store-write-lifecycle', 'The daemon runtime opens and executes the writable workspace store lifecycle.'), ('normal-nondaemon-writer-path-absent', 'A closed-world crate, dependency, and public-API inventory exposes no normal non-daemon workspace-state writer.')), 'PAC-018': (('ack-after-commit', 'A successful admission acknowledgement is returned only after the admission transaction commits.'), ('receipt-replays-after-independent-reopen', 'An independent reopen replays the exact acknowledged idempotency receipt.'), ('queued-mutation-visible-after-independent-reopen', 'An independent reopen observes the admitted mutation in durable queue state.')), 'PAC-022': (('cursor-precondition-transmitted', 'The mutation transmits the caller-selected active-attempt cursor precondition.'), ('active-stage-drift-rejected', 'Authoritative active-stage drift rejects the cursor mutation.'), ('drift-not-retargeted-or-mutated', 'Drift neither retargets the request to the new cursor nor changes session state.')), 'PAC-024': (('queued-jobs-persist-across-close-reopen', 'Queued jobs persist across daemon-equivalent store close and reopen.'), ('workspace-identity-remains-discoverable', 'Reopen resolves the same worktree identity and can enumerate its jobs.'), ('reopened-queue-remains-claimable', 'Persisted queued jobs remain claimable in workspace FIFO order.')), 'PAC-026': (('defined-crash-registry-exact', 'The executed registry exactly equals every controller-defined crash-injection point, with no omissions or extras.'), ('every-defined-point-executed', 'Every registered point is exercised by a bound crash scenario.'), ('crash-process-termination-observed', 'Every crash scenario observes forced child-process termination at its point.'), ('deterministic-valid-recovery-observed', 'Reopen at every point produces its declared deterministic valid recovery outcome.')), 'PAC-027': (('transition-and-terminal-commit-together', 'A successful state transition and terminal job result become durable in one commit.'), ('precommit-failure-rolls-back-transition', 'A terminal pre-commit failure leaves the state transition unapplied.'), ('precommit-failure-leaves-no-terminal', 'A terminal pre-commit failure leaves no terminal job result.'), ('postcommit-reopen-observes-both', 'Reopen after commit observes both the transition and terminal result.')), 'PAC-030': (('all-interrupted-marker-boundaries-executed', 'Every reset-all marker/publication interruption boundary is exercised.'), ('recovery-is-valid-old-or-new-state', 'Recovery yields only a complete prior state or complete replacement state.'), ('retry-converges-idempotently', 'Retrying the same reset-all identity converges without duplicate mutation.'), ('owned-temporary-marker-state-cleaned', 'Recovery removes Store-owned interrupted marker/publication temporary state.')), 'PAC-036': (('runtime-confined-to-dot-podway-runtime', 'Runtime state is created only below `.podway/runtime/`.'), ('exact-runtime-ignore-rule', 'The workspace-local Git ignore bytes exactly ignore `runtime/`.'), ('replay-does-not-broaden-or-duplicate-ignore', 'Initialization replay neither broadens nor duplicates the ignore rule.')), 'PAC-037': (('seed-task-state-and-queued-job', 'The deleted-worktree scenario first persists task state and at least one queued job.'), ('delete-worktree-removes-local-database', 'Deleting the worktree removes its workspace-local SQLite state and side files.'), ('deleted-identity-is-unavailable-not-adopted', 'Recovery classifies the registered identity as worktree-gone and does not adopt a copy.'), ('deleted-task-and-job-cannot-be-recovered', 'The deleted task state and queued job cannot be enumerated or executed after recovery.')), 'PAC-040': (('exact-schema0-predecessor-and-v1-pragmas', 'Conformance starts from the exact schema-0-uninitialized predecessor and verifies the required schema-v1 pragmas and objects.'), ('transactional-initialization-no-partial-installation', 'An injected initialization failure rolls back to schema 0 with no partial v1 installation.'), ('retained-predecessor-state-not-lost', 'All state admitted in the schema-0 fixture is preserved except the explicitly declared v1 installation changes.'), ('reopen-does-not-duplicate-migration-or-mutation', 'Reopen records one migration and does not duplicate task state or a user mutation.')), 'PAC-041': (('migration-transaction-rolls-back-on-failure', 'Migration failure rolls back the whole migration transaction.'), ('failed-migration-leaves-schema0', 'The failed migration leaves the predecessor structurally schema 0.'), ('successful-migration-records-exact-checksum', 'Successful migration records the exact approved v1 migration checksum.'), ('tampered-or-missing-checksum-fails-closed', 'A tampered or missing migration checksum fails closed.')), 'PAC-044': (('reset-all-destroys-existing-session-history', 'Public reset-all removes seeded session-local task history.'), ('reset-all-recreates-valid-workspace', 'Reset-all recreates a valid schema and workspace identity.'), ('post-reset-start-and-mutation-succeed', 'A fresh session start and mutation succeed in the recreated workspace.'), ('reset-all-replay-is-idempotent', 'Replaying the acknowledged reset-all identity does not reset or mutate again.')), 'PAC-045': (('terminal-job-retention-policy', 'Terminal jobs obey the frozen minimum, age, and maximum retention policy.'), ('idempotency-retention-and-replay-policy', 'Idempotency records obey their scope-specific retention policy and retained terminal receipts still replay exactly after job pruning.'), ('journal-retention-policy', 'Operational journal rows obey the frozen age, minimum-newest, and maximum-row policy.'), ('automatic-pruning-boundary', 'Successful mutation or idle maintenance invokes pruning without allowing unbounded growth.')), 'PAC-048': (('exact-public-command-set-enumerated', 'The proof enumerates the complete frozen public-command set, not a sample.'), ('successful-json-is-versioned-envelope', 'Every public command success emits one valid versioned JSON envelope.'), ('json-command-and-result-match-route', 'Every JSON envelope identifies the invoked route and its typed result.'), ('json-errors-are-versioned-envelopes', 'Every exercised public-command failure emits a valid versioned JSON error envelope.')), 'PAC-050': (('same-success-scenario-in-both-modes', 'Text and JSON modes execute the same successful status scenario.'), ('semantic-state-projections-equal', 'Active stage, attempt, item state, blockers, and pending-job semantics project equally in both modes.'), ('same-daemon-contract-in-both-modes', 'Both modes send the identical typed daemon request contract.')), 'PAC-053': (('explicit-session-revision-reaches-wire', 'An explicit session revision reaches the exact protocol precondition field.'), ('explicit-attempt-reaches-wire', 'An explicit attempt ID reaches the exact protocol precondition field.'), ('explicit-item-revision-reaches-wire', 'An explicit item revision reaches the exact protocol precondition field.'), ('explicit-idempotency-key-reaches-wire', 'An explicit idempotency key reaches the exact protocol field unchanged.')), 'PAC-062': (('workspace-commands-preserve-git-metadata', 'Representative public workspace mutations preserve complete Git administrative metadata bytes.'), ('no-public-git-mutator-api', 'A closed-world public-API inventory exposes no Git mutation operation.'), ('no-mutating-git-dependency-or-process-path', 'The complete product dependency/process boundary admits no mutating Git library or Git subprocess path.')), 'PAC-063': (('only-private-unix-endpoint', 'Runtime binding exposes only the user-private Unix-domain endpoint.'), ('unix-endpoint-permissions-private', 'The daemon socket and runtime permissions exclude group and other access.'), ('no-network-api-in-complete-daemon-source-set', 'A closed-world daemon source inventory admits no network listener or network I/O API.'), ('no-network-client-or-server-dependency', 'The exact daemon dependency graph contains no network client or server dependency.')), 'PAC-064': (('real-sentinel-enters-artifact-boundary', 'Unique sentinel bytes are written to a real local artifact and enter the actual hash/attach verification boundary.'), ('only-artifact-metadata-persists', 'Only reference/path, digest, size, and media type persist for the artifact.'), ('sentinel-absent-from-all-durable-fields', 'The sentinel is absent from every durable request, session, item, job, idempotency, and journal field.'), ('absence-survives-sqlite-reopen', 'The sentinel remains absent after closing and reopening the real SQLite store.'))}
G040_PROOF_MEMBERSHIP = {'PAC-005': ({'criterion_id': 'PAC-005', 'path': 'crates/podway-core/tests/phase1_transitions.rs', 'function': 'completion_rechecks_required_local_artifact_metadata_field_by_field', 'command': 'cargo test -p podway-core --test phase1_transitions completion_rechecks_required_local_artifact_metadata_field_by_field --locked -- --exact', 'obligation_ids': ('reject-missing-required-item', 'reject-changed-required-local-artifact', 'rejection-is-nonmutating')}, {'criterion_id': 'PAC-005', 'path': 'crates/podway-core/tests/phase1_transitions.rs', 'function': 'complete_skip_retry_return_block_unblock_and_cancel_preserve_attempt_history', 'command': 'cargo test -p podway-core --test phase1_transitions complete_skip_retry_return_block_unblock_and_cancel_preserve_attempt_history --locked -- --exact', 'obligation_ids': ('reject-open-blocker',)}), 'PAC-010': ({'criterion_id': 'PAC-010', 'path': 'crates/podway-core/tests/phase1_transitions.rs', 'function': 'complete_skip_retry_return_block_unblock_and_cancel_preserve_attempt_history', 'command': 'cargo test -p podway-core --test phase1_transitions complete_skip_retry_return_block_unblock_and_cancel_preserve_attempt_history --locked -- --exact', 'obligation_ids': ('allow-explicit-skip', 'reject-unpermitted-skip', 'rejected-skip-is-nonmutating')},), 'PAC-017': ({'criterion_id': 'PAC-017', 'path': 'crates/podway-daemon/tests/phase4_daemon_runtime.rs', 'function': 'pac017_daemon_is_the_sole_normal_store_writer', 'command': 'cargo test -p podway-daemon --test phase4_daemon_runtime pac017_daemon_is_the_sole_normal_store_writer --locked -- --exact', 'obligation_ids': ('mutation-enters-daemon-ipc', 'daemon-owns-store-write-lifecycle', 'normal-nondaemon-writer-path-absent')},), 'PAC-018': ({'criterion_id': 'PAC-018', 'path': 'crates/podway-store/tests/phase4_store_transactions.rs', 'function': 'pac_018_admission_acknowledgement_survives_independent_reopen_with_receipt_and_fifo_state', 'command': 'cargo test -p podway-store --test phase4_store_transactions pac_018_admission_acknowledgement_survives_independent_reopen_with_receipt_and_fifo_state --locked -- --exact', 'obligation_ids': ('ack-after-commit', 'receipt-replays-after-independent-reopen', 'queued-mutation-visible-after-independent-reopen')},), 'PAC-022': ({'criterion_id': 'PAC-022', 'path': 'crates/podway-cli/tests/phase4_commands.rs', 'function': 'pac_022_cursor_mutation_is_rejected_after_authoritative_active_stage_drift', 'command': 'cargo test -p podway-cli --test phase4_commands pac_022_cursor_mutation_is_rejected_after_authoritative_active_stage_drift --locked -- --exact', 'obligation_ids': ('cursor-precondition-transmitted',)}, {'criterion_id': 'PAC-022', 'path': 'crates/podway-daemon/tests/phase5_execution.rs', 'function': 'g006_stale_uncheck_is_terminal_and_does_not_partially_mutate', 'command': 'cargo test -p podway-daemon --test phase5_execution g006_stale_uncheck_is_terminal_and_does_not_partially_mutate --locked -- --exact', 'obligation_ids': ('active-stage-drift-rejected', 'drift-not-retargeted-or-mutated')}), 'PAC-024': ({'criterion_id': 'PAC-024', 'path': 'crates/podway-store/tests/phase4_store_transactions.rs', 'function': 'pac_024_daemon_equivalent_reopen_recovers_running_and_keeps_queued_jobs_discoverable', 'command': 'cargo test -p podway-store --test phase4_store_transactions pac_024_daemon_equivalent_reopen_recovers_running_and_keeps_queued_jobs_discoverable --locked -- --exact', 'obligation_ids': ('queued-jobs-persist-across-close-reopen', 'workspace-identity-remains-discoverable', 'reopened-queue-remains-claimable')},), 'PAC-026': ({'criterion_id': 'PAC-026', 'path': 'crates/podway-store/tests/phase2_crash_matrix.rs', 'function': 'crash_registry_has_exact_unique_store_owned_failpoint_coverage', 'command': 'cargo test -p podway-store --test phase2_crash_matrix crash_registry_has_exact_unique_store_owned_failpoint_coverage --locked -- --exact', 'obligation_ids': ('defined-crash-registry-exact', 'every-defined-point-executed')}, {'criterion_id': 'PAC-026', 'path': 'crates/podway-store/tests/phase2_crash_matrix.rs', 'function': 'store_owned_crash_matrix_aborts_children_then_recovers_exactly_once', 'command': 'cargo test -p podway-store --test phase2_crash_matrix store_owned_crash_matrix_aborts_children_then_recovers_exactly_once --locked -- --exact', 'obligation_ids': ('crash-process-termination-observed', 'deterministic-valid-recovery-observed')}), 'PAC-027': ({'criterion_id': 'PAC-027', 'path': 'crates/podway-store/tests/phase4_store_transactions.rs', 'function': 'pac_027_terminal_state_and_result_roll_back_or_commit_together', 'command': 'cargo test -p podway-store --test phase4_store_transactions pac_027_terminal_state_and_result_roll_back_or_commit_together --locked -- --exact', 'obligation_ids': ('transition-and-terminal-commit-together', 'precommit-failure-rolls-back-transition', 'precommit-failure-leaves-no-terminal', 'postcommit-reopen-observes-both')},), 'PAC-030': ({'criterion_id': 'PAC-030', 'path': 'crates/podway-store/tests/phase2_reset_lifecycle.rs', 'function': 'pac_030_interrupted_reset_all_publication_recovers_and_retries_idempotently', 'command': 'cargo test -p podway-store --test phase2_reset_lifecycle pac_030_interrupted_reset_all_publication_recovers_and_retries_idempotently --locked -- --exact', 'obligation_ids': ('all-interrupted-marker-boundaries-executed', 'recovery-is-valid-old-or-new-state', 'retry-converges-idempotently', 'owned-temporary-marker-state-cleaned')},), 'PAC-036': ({'criterion_id': 'PAC-036', 'path': 'crates/podway-git/tests/phase4_init_layout.rs', 'function': 'pac036_runtime_is_confined_to_podway_and_ignored_by_the_exact_rule', 'command': 'cargo test -p podway-git --test phase4_init_layout pac036_runtime_is_confined_to_podway_and_ignored_by_the_exact_rule --locked -- --exact', 'obligation_ids': ('runtime-confined-to-dot-podway-runtime', 'exact-runtime-ignore-rule', 'replay-does-not-broaden-or-duplicate-ignore')},), 'PAC-037': ({'criterion_id': 'PAC-037', 'path': 'crates/podway-daemon/tests/phase4_daemon_runtime.rs', 'function': 'pac037_deleted_registered_worktree_is_unavailable_without_adoption', 'command': 'cargo test -p podway-daemon --test phase4_daemon_runtime pac037_deleted_registered_worktree_is_unavailable_without_adoption --locked -- --exact', 'obligation_ids': ('seed-task-state-and-queued-job', 'delete-worktree-removes-local-database', 'deleted-identity-is-unavailable-not-adopted', 'deleted-task-and-job-cannot-be-recovered')},), 'PAC-040': ({'criterion_id': 'PAC-040', 'path': 'crates/podway-store/tests/phase2_schema_codec.rs', 'function': 'pac_040_schema0_pragmas_transactional_initialization_preserves_task_state_without_duplicate_mutation', 'command': 'cargo test -p podway-store --test phase2_schema_codec pac_040_schema0_pragmas_transactional_initialization_preserves_task_state_without_duplicate_mutation --locked -- --exact', 'obligation_ids': ('exact-schema0-predecessor-and-v1-pragmas', 'transactional-initialization-no-partial-installation', 'retained-predecessor-state-not-lost', 'reopen-does-not-duplicate-migration-or-mutation')},), 'PAC-041': ({'criterion_id': 'PAC-041', 'path': 'crates/podway-store/tests/phase2_schema_codec.rs', 'function': 'pac_041_migration_checksum_validation_and_transactional_rollback_fail_closed', 'command': 'cargo test -p podway-store --test phase2_schema_codec pac_041_migration_checksum_validation_and_transactional_rollback_fail_closed --locked -- --exact', 'obligation_ids': ('migration-transaction-rolls-back-on-failure', 'failed-migration-leaves-schema0', 'successful-migration-records-exact-checksum', 'tampered-or-missing-checksum-fails-closed')},), 'PAC-044': ({'criterion_id': 'PAC-044', 'path': 'crates/podway-daemon/tests/phase5_reset_runtime.rs', 'function': 'pac_044_reset_all_destroys_history_recreates_a_mutable_workspace_and_replays_idempotently', 'command': 'cargo test -p podway-daemon --test phase5_reset_runtime pac_044_reset_all_destroys_history_recreates_a_mutable_workspace_and_replays_idempotently --locked -- --exact', 'obligation_ids': ('reset-all-destroys-existing-session-history', 'reset-all-recreates-valid-workspace', 'post-reset-start-and-mutation-succeed', 'reset-all-replay-is-idempotent')},), 'PAC-045': ({'criterion_id': 'PAC-045', 'path': 'crates/podway-store/tests/phase4_store_transactions.rs', 'function': 'pac_045_pruning_bounds_terminal_and_idempotency_retention_with_oldest_replay', 'command': 'cargo test -p podway-store --test phase4_store_transactions pac_045_pruning_bounds_terminal_and_idempotency_retention_with_oldest_replay --locked -- --exact', 'obligation_ids': ('terminal-job-retention-policy', 'idempotency-retention-and-replay-policy', 'journal-retention-policy', 'automatic-pruning-boundary')},), 'PAC-048': ({'criterion_id': 'PAC-048', 'path': 'crates/podway-cli/tests/phase5_cli.rs', 'function': 'pac_048_recording_daemon_contract_table_validates_successful_versioned_json_output_for_every_route', 'command': 'cargo test -p podway-cli --test phase5_cli pac_048_recording_daemon_contract_table_validates_successful_versioned_json_output_for_every_route --locked -- --exact', 'obligation_ids': ('exact-public-command-set-enumerated', 'successful-json-is-versioned-envelope', 'json-command-and-result-match-route', 'json-errors-are-versioned-envelopes')},), 'PAC-050': ({'criterion_id': 'PAC-050', 'path': 'crates/podway-cli/tests/phase5_cli.rs', 'function': 'pac_050_status_text_and_json_render_the_same_typed_state_semantics', 'command': 'cargo test -p podway-cli --test phase5_cli pac_050_status_text_and_json_render_the_same_typed_state_semantics --locked -- --exact', 'obligation_ids': ('same-success-scenario-in-both-modes', 'semantic-state-projections-equal', 'same-daemon-contract-in-both-modes')},), 'PAC-053': ({'criterion_id': 'PAC-053', 'path': 'crates/podway-cli/tests/phase4_commands.rs', 'function': 'pac_053_explicit_revision_attempt_item_revision_and_idempotency_reach_exact_wire_fields', 'command': 'cargo test -p podway-cli --test phase4_commands pac_053_explicit_revision_attempt_item_revision_and_idempotency_reach_exact_wire_fields --locked -- --exact', 'obligation_ids': ('explicit-session-revision-reaches-wire', 'explicit-attempt-reaches-wire', 'explicit-item-revision-reaches-wire', 'explicit-idempotency-key-reaches-wire')},), 'PAC-062': ({'criterion_id': 'PAC-062', 'path': 'crates/podway-git/tests/phase4_init_layout.rs', 'function': 'pac062_layout_api_preserves_real_main_and_linked_worktree_metadata', 'command': 'cargo test -p podway-git --test phase4_init_layout pac062_layout_api_preserves_real_main_and_linked_worktree_metadata --locked -- --exact', 'obligation_ids': ('workspace-commands-preserve-git-metadata', 'no-public-git-mutator-api', 'no-mutating-git-dependency-or-process-path')},), 'PAC-063': ({'criterion_id': 'PAC-063', 'path': 'crates/podway-daemon/tests/phase4_endpoint.rs', 'function': 'pac063_daemon_exposes_only_a_private_unix_endpoint_and_no_network_surface', 'command': 'cargo test -p podway-daemon --test phase4_endpoint pac063_daemon_exposes_only_a_private_unix_endpoint_and_no_network_surface --locked -- --exact', 'obligation_ids': ('only-private-unix-endpoint', 'unix-endpoint-permissions-private', 'no-network-api-in-complete-daemon-source-set', 'no-network-client-or-server-dependency')},), 'PAC-064': ({'criterion_id': 'PAC-064', 'path': 'crates/podway-daemon/tests/phase5_execution.rs', 'function': 'pac064_local_artifact_content_never_enters_durable_request_session_or_event_data', 'command': 'cargo test -p podway-daemon --test phase5_execution pac064_local_artifact_content_never_enters_durable_request_session_or_event_data --locked -- --exact', 'obligation_ids': ('real-sentinel-enters-artifact-boundary', 'only-artifact-metadata-persists', 'sentinel-absent-from-all-durable-fields', 'absence-survives-sqlite-reopen')},)}
G036_CRITERION_COUNT = 71
G036_EXACT_COMMAND_COUNT = 50
G036_MATRIX_PATH = ROOT / "release/product-acceptance-matrix-v1.json"
G036_MATRIX_SHA256 = "fbea960e462011fd192389fb9c47cbe40068b603f38f5241a37bacb85bdb091f"
G036_REPORT_PATH = ROOT / "artifacts/g036/g036-test-report.json"
G036_PRODUCT_SOURCE_TREE_GLOBS = (
    "Cargo.lock",
    "Cargo.toml",
    ".cargo/config*",
    "rust-toolchain*",
    "build.rs",
    "*.rs",
    "crates/**/*",
    "presets/**/*.yaml",
    "schemas/**/*.json",
    "spec/**",
    "tests/fixtures/**",
    "release/migration-evidence-v1.json",
    "RELEASE_NOTES.md",
)
G036_TRUSTED_ENVIRONMENT = {
    "cargo": {
        "path": "/opt/homebrew/Cellar/rust/1.97.0/bin/cargo",
        "sha256": "41435cf3cb8134188d32e245098c47feb56f1ecbf72a31b9afd53ab177751234",
        "version": "cargo 1.97.0 (c980f4866 2026-06-30) (Homebrew)",
    },
    "rustc": {
        "path": "/opt/homebrew/Cellar/rust/1.97.0/bin/rustc",
        "sha256": "d0d2e341c4a90cd02a8c444e5184be2381e205045a467cd77cc249a36852b549",
        "version": "rustc 1.97.0 (2d8144b78 2026-07-07) (Homebrew)",
    },
    "environment": {
        "allow": ["CARGO_HOME", "CARGO_NET_OFFLINE", "CARGO_TARGET_DIR", "HOME", "LANG", "LC_ALL", "PATH", "RUSTC", "TERM", "TMPDIR"],
        "cargoConfig": "invocation-scoped directory source derived from Cargo.lock checksum-verified crate archives",
        "cargoHome": "invocation-scoped below replay temporary root",
        "home": "invocation-scoped below replay temporary root",
        "injectedTestVariables": ["PODWAYD_BUILD_RECEIPT", "PODWAYD_TEST_BINARY"],
        "offline": True,
        "rejectAncestorCargoConfigOutsideProductRoot": True,
        "rejectInheritedPrefixes": ["PODWAYD_TEST_"],
        "rejectInheritedVariables": [
            "CARGO_BUILD_TARGET",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "RUSTFLAGS",
            "RUSTDOCFLAGS",
            "RUSTUP_TOOLCHAIN",
            "RUST_TARGET_PATH",
        ],
        "targetOverrideVariables": ["CARGO_BUILD_TARGET", "RUSTFLAGS", "RUST_TARGET_PATH"],
        "network": "IP networking denied; Unix-domain socket connect and bind only",
        "readRoots": "authenticated exact product files and traversal-only ancestors; immutable vendor manifest files; exact pinned cargo/rustc and digest-bound dynamic-library closure; narrowly enumerated macOS platform files",
        "writeRoots": "invocation-owned home cargo-home target tmp only; anonymous pipe data permitted",
        "vendor": "fresh checksum-verified immutable manifest, checked before and after every command",
    },
}
def _validate_pinned_file_digest(path: Path, digest: str, label: str) -> None:
    if not path.is_file() or sha256_file(path) != digest:
        raise QualificationError(f"{label} digest differs")
def _g036_product_source_tree() -> dict[str, Any]:
    for relative in (".cargo", "crates", "presets", "schemas", "spec", "tests/fixtures"):
        root = ROOT / relative
        if root.exists() and (root.is_symlink() or not root.is_dir()):
            raise QualificationError("G036 product input closure root is a symlink or non-directory")
    candidates: set[Path] = set()
    for pattern in G036_PRODUCT_SOURCE_TREE_GLOBS:
        for candidate in ROOT.glob(pattern):
            if candidate.is_symlink():
                raise QualificationError("G036 product input closure contains a symlink")
            if candidate.is_file():
                candidates.add(candidate)
            elif candidate.exists() and not candidate.is_dir():
                raise QualificationError("G036 product input closure contains a non-regular file")
    paths = sorted(candidate.relative_to(ROOT).as_posix() for candidate in candidates)
    files = [{"path": relative, "sha256": sha256_file(ROOT / relative)} for relative in paths]
    return {
        "paths": paths,
        "sha256": sha256_bytes(canonical_json({"files": files})),
    }
def _g036_toolchain() -> dict[str, Any]:
    toolchain: dict[str, Any] = {}
    for tool_id in ("cargo", "rustc"):
        expected = G036_TRUSTED_ENVIRONMENT[tool_id]
        path = Path(expected["path"])
        if not path.is_absolute() or path.is_symlink() or not path.is_file():
            raise QualificationError(f"G036 trusted {tool_id} executable is not a regular non-symlink path")
        if sha256_file(path) != expected["sha256"]:
            raise QualificationError(f"G036 trusted {tool_id} executable digest differs")
        completed = subprocess.run([str(path), "--version"], cwd=ROOT, capture_output=True, check=False)
        if completed.returncode or completed.stdout.decode("utf-8", "replace").strip() != expected["version"]:
            raise QualificationError(f"G036 trusted {tool_id} executable version differs")
        toolchain[tool_id] = dict(expected)
    return toolchain
def _validate_g036_workspace_cargo_configuration() -> None:
    for config in ROOT.glob(".cargo/config*"):
        if config.is_symlink() or not config.is_file():
            raise QualificationError("G036 Cargo configuration is not a regular file")
        try:
            value = tomllib.loads(config.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
            raise QualificationError("G036 Cargo configuration is unreadable") from exc
        forbidden = {"target", "rustc", "rustc-wrapper", "rustc-workspace-wrapper", "rustflags"}
        def walk(item: Any) -> None:
            if isinstance(item, dict):
                for key, child in item.items():
                    if str(key).lower().replace("_", "-") in forbidden:
                        raise QualificationError("G036 Cargo configuration declares a wrapper or target override")
                    walk(child)
        walk(value)
        if value.get("env"):
            raise QualificationError("G036 Cargo configuration declares test environment variables")
    ancestor = ROOT.parent
    while ancestor != ancestor.parent:
        if any(ancestor.glob(".cargo/config*")):
            raise QualificationError("G036 repository-ancestor Cargo configuration is forbidden")
        ancestor = ancestor.parent


def _g036_locked_registry_packages() -> list[dict[str, str]]:
    try:
        lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise QualificationError("G036 Cargo.lock is unreadable") from exc
    packages = lock.get("package")
    if not isinstance(packages, list):
        raise QualificationError("G036 Cargo.lock package list is malformed")
    locked: list[dict[str, str]] = []
    for package in packages:
        if not isinstance(package, dict):
            raise QualificationError("G036 Cargo.lock package record is malformed")
        source = package.get("source")
        if source is None:
            continue
        name, version, checksum = package.get("name"), package.get("version"), package.get("checksum")
        if (
            not isinstance(source, str)
            or not source.startswith("registry+")
            or not isinstance(name, str)
            or not isinstance(version, str)
            or not isinstance(checksum, str)
            or not re.fullmatch(r"[0-9a-f]{64}", checksum)
        ):
            raise QualificationError("G036 Cargo.lock contains an unsupported or unchecksummed dependency")
        locked.append({"name": name, "version": version, "checksum": checksum})
    if not locked:
        raise QualificationError("G036 Cargo.lock has no registry dependencies")
    return locked


def _g036_vendor_manifest(vendor: Path) -> dict[str, Any]:
    if vendor.is_symlink() or not vendor.is_dir():
        raise QualificationError("G036 invocation vendor root is unsafe")
    files: list[dict[str, str]] = []
    for candidate in sorted(vendor.rglob("*")):
        if candidate.is_symlink() or (candidate.exists() and not candidate.is_file() and not candidate.is_dir()):
            raise QualificationError("G036 invocation vendor contains an unsafe member")
        if candidate.is_file():
            files.append({"path": candidate.relative_to(vendor).as_posix(), "sha256": sha256_file(candidate)})
    return {"files": files, "sha256": sha256_bytes(canonical_json({"files": files}))}


def _g036_materialize_verified_vendor(replay_root: Path) -> tuple[Path, dict[str, Any]]:
    vendor = replay_root / "inputs" / "vendor"
    if vendor.exists():
        manifest = _g036_vendor_manifest(vendor)
        if any(candidate.is_file() and candidate.stat().st_mode & 0o222 for candidate in vendor.rglob("*")):
            raise QualificationError("G036 invocation vendor root is mutable")
        return vendor, manifest
    cache = Path.home() / ".cargo" / "registry" / "cache"
    if cache.is_symlink() or not cache.is_dir():
        raise QualificationError("G036 checksum-verified crate archive cache is unavailable")
    vendor.mkdir(parents=True)
    for package in _g036_locked_registry_packages():
        archive_name = f"{package['name']}-{package['version']}.crate"
        archives = [
            path for path in cache.glob(f"*/{archive_name}")
            if not path.is_symlink() and path.is_file() and sha256_file(path) == package["checksum"]
        ]
        if len(archives) != 1:
            raise QualificationError("G036 locked crate archive is missing, ambiguous, or has a checksum mismatch")
        destination = vendor / archive_name.removesuffix(".crate")
        destination.mkdir()
        files: dict[str, str] = {}
        try:
            with tarfile.open(archives[0], "r:gz") as archive:
                for member in archive.getmembers():
                    relative = Path(member.name)
                    if (
                        relative.is_absolute()
                        or ".." in relative.parts
                        or not relative.parts
                        or relative.parts[0] != destination.name
                    ):
                        raise QualificationError("G036 crate archive member escapes its package root")
                    output = vendor.joinpath(*relative.parts)
                    if member.isdir():
                        output.mkdir(parents=True, exist_ok=True)
                    elif member.isfile():
                        if output.exists() or output.is_symlink():
                            raise QualificationError("G036 crate archive contains duplicate or unsafe members")
                        payload = archive.extractfile(member)
                        if payload is None:
                            raise QualificationError("G036 crate archive member is unreadable")
                        contents = payload.read()
                        output.parent.mkdir(parents=True, exist_ok=True)
                        output.write_bytes(contents)
                        output.chmod(member.mode & 0o777)
                        files[output.relative_to(destination).as_posix()] = sha256_bytes(contents)
                    else:
                        raise QualificationError("G036 crate archive contains a non-regular member")
        except (OSError, tarfile.TarError) as exc:
            raise QualificationError("G036 locked crate archive cannot be safely materialized") from exc
        (destination / ".cargo-checksum.json").write_bytes(
            canonical_json({"files": files, "package": package["checksum"]})
        )
    manifest = _g036_vendor_manifest(vendor)
    for candidate in sorted(vendor.rglob("*"), reverse=True):
        if candidate.is_dir():
            candidate.chmod(0o555)
        else:
            candidate.chmod(0o444)
    vendor.chmod(0o555)
    return vendor, manifest


def _g036_sanitized_environment(toolchain: dict[str, Any], target_dir: Path, daemon: dict[str, str] | None = None) -> dict[str, str]:
    contract = G036_TRUSTED_ENVIRONMENT["environment"]
    forbidden_variables = set(contract["rejectInheritedVariables"])
    forbidden_prefixes = tuple(contract["rejectInheritedPrefixes"])
    if any(name in forbidden_variables or name.startswith(forbidden_prefixes) for name in os.environ):
        raise QualificationError("G036 inherited compiler, target, or test override is forbidden")
    if any(token == "--target" or token.startswith("--target=") for token in sys.argv):
        raise QualificationError("G036 target override is forbidden")
    replay_root = target_dir.parent
    if not target_dir.is_absolute() or replay_root == target_dir or replay_root.is_symlink():
        raise QualificationError("G036 replay target directory is not invocation-scoped")
    _validate_g036_workspace_cargo_configuration()
    vendor, vendor_manifest = _g036_materialize_verified_vendor(replay_root)
    if _g036_vendor_manifest(vendor) != vendor_manifest:
        raise QualificationError("G036 invocation vendor manifest drifted during materialization")
    sdk_root = Path(
        "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk"
    ).resolve(strict=True)
    home, cargo_home, temp_dir = (
        replay_root / "home",
        replay_root / "cargo-home",
        replay_root / "tmp",
    )
    target_dir.mkdir(parents=True, exist_ok=True)
    tool_sources = {
        target_dir / "tool-bin/clang": Path(
            "/Library/Developer/CommandLineTools/usr/bin/clang"
        ),
        target_dir / "tool-bin/ld": Path(
            "/Library/Developer/CommandLineTools/usr/bin/ld-classic"
        ),
        target_dir / "tool-bin/ar": Path(
            "/Library/Developer/CommandLineTools/usr/bin/ar"
        ),
        target_dir / "tool-bin/ranlib": Path(
            "/Library/Developer/CommandLineTools/usr/bin/ranlib"
        ).resolve(strict=True),
        target_dir / "tool-bin/git": Path(
            "/Library/Developer/CommandLineTools/usr/bin/git"
        ),
        target_dir / "lib/libtapi.dylib": Path(
            "/Library/Developer/CommandLineTools/usr/lib/libtapi.dylib"
        ),
        target_dir / "lib/libLTO.dylib": Path(
            "/Library/Developer/CommandLineTools/usr/lib/libLTO.dylib"
        ),
        target_dir / "lib/libswiftDemangle.dylib": Path(
            "/Library/Developer/CommandLineTools/usr/lib/libswiftDemangle.dylib"
        ),
    }
    for copied, source in tool_sources.items():
        copied.parent.mkdir(exist_ok=True)
        if not copied.exists():
            shutil.copyfile(source, copied)
            copied.chmod(0o555)
        if (
            copied.is_symlink()
            or not copied.is_file()
            or _g036_runtime_digest(copied)
            != _g036_runtime_digest(source)
        ):
            raise QualificationError(
                "G036 invocation tool copy differs from the pinned native tool"
            )
    linker_dir = target_dir / "tool-bin"
    git_executable = linker_dir / "git"
    git_helpers = Path(
        "/Library/Developer/CommandLineTools/usr/libexec/git-core"
    )
    for helper in sorted(git_helpers.iterdir()):
        if (
            helper.is_symlink()
            and helper.resolve(strict=True)
            == tool_sources[git_executable]
        ):
            copied_helper = linker_dir / helper.name
            if not copied_helper.exists():
                os.link(git_executable, copied_helper)
            git_identity = git_executable.stat()
            helper_identity = copied_helper.stat()
            if (
                copied_helper.is_symlink()
                or not copied_helper.is_file()
                or (helper_identity.st_dev, helper_identity.st_ino)
                != (git_identity.st_dev, git_identity.st_ino)
            ):
                raise QualificationError(
                    "G036 invocation Git helper closure is unsafe"
                )
    home.mkdir(parents=True, exist_ok=True)
    cargo_home.mkdir(parents=True, exist_ok=True)
    temp_dir.mkdir(parents=True, exist_ok=True)
    (cargo_home / "config.toml").write_text(
        "[net]\noffline = true\n\n[source.crates-io]\nreplace-with = \"g036-locked-vendor\"\n\n[source.g036-locked-vendor]\ndirectory = "
        + json.dumps(str(vendor))
        + "\n\n[target.aarch64-apple-darwin]\nlinker = "
        + json.dumps(str(linker_dir / "clang"))
        + "\n",
        encoding="utf-8",
    )
    environment = {
        "HOME": str(home),
        "CARGO_HOME": str(cargo_home),
        "CARGO_NET_OFFLINE": "true",
        "LANG": "C",
        "LC_ALL": "C",
        "TERM": "dumb",
        "PATH": f"{linker_dir}:/usr/bin:/bin",
        "DEVELOPER_DIR": "/Library/Developer/CommandLineTools",
        "SDKROOT": str(sdk_root),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_SYSTEM": "/dev/null",
        "GIT_TEMPLATE_DIR": (
            "/Library/Developer/CommandLineTools/usr/share/git-core/templates"
        ),
        "GIT_EXEC_PATH": str(linker_dir),
        "CC": str(linker_dir / "clang"),
        "AR": str(linker_dir / "ar"),
        "CFLAGS": (
            f"-isysroot {sdk_root} "
            "-resource-dir /Library/Developer/CommandLineTools/usr/lib/clang/21"
        ),
        "RUSTC": toolchain["rustc"]["path"],
        "CARGO_TARGET_DIR": str(target_dir),
        "TMPDIR": str(temp_dir),
    }
    if daemon is not None:
        if set(daemon) != set(G036_TRUSTED_ENVIRONMENT["environment"]["injectedTestVariables"]):
            raise QualificationError("G036 daemon replay declares an undeclared test environment variable")
        environment.update(daemon)
    return environment
def _g036_runtime_digest(path: Path) -> str:
    if path.is_symlink():
        raise QualificationError("G036 native runtime dependency is a symlink")
    with path.open("rb") as source:
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size > 512 * 1024 * 1024:
            raise QualificationError("G036 native runtime dependency is unsafe or oversized")
        digest = hashlib.sha256()
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
        after = os.fstat(source.fileno())
    before_identity = (
        before.st_dev,
        before.st_ino,
        before.st_size,
        before.st_mtime_ns,
    )
    after_identity = (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
    )
    if before_identity != after_identity:
        raise QualificationError("G036 native runtime dependency changed while hashing")
    return digest.hexdigest()


def _g036_runtime_closure(toolchain: dict[str, Any]) -> list[dict[str, Any]]:
    """Return digest-authenticated, recursively resolved native loader inputs."""
    platform_tools = tuple(
        Path(path)
        for path in (
            "/usr/bin/cc",
            "/usr/bin/ld",
            "/usr/bin/ar",
            "/usr/bin/ranlib",
            "/usr/bin/dsymutil",
            "/usr/bin/strip",
            "/usr/bin/xcrun",
            "/Library/Developer/CommandLineTools/usr/bin/clang",
        )
    )
    pending = [
        Path(toolchain[name]["path"]) for name in ("cargo", "rustc")
    ] + list(platform_tools)
    approved_tools = {str(path.resolve(strict=True)) for path in platform_tools}
    closure: dict[Path, dict[str, Any]] = {}
    while pending:
        declared = pending.pop()
        candidate = declared.resolve(strict=True)
        member = closure.get(candidate)
        if member is not None:
            if declared != candidate:
                member["aliases"].add(str(declared))
                physical_alias = declared.parent.resolve(strict=True) / declared.name
                if physical_alias != candidate:
                    member["aliases"].add(str(physical_alias))
            continue
        if candidate.is_symlink() or not candidate.is_file():
            raise QualificationError("G036 native runtime dependency is unsafe")
        allowed = (
            "/System/",
            "/usr/lib/",
            "/opt/homebrew/Cellar/",
            "/Library/Developer/CommandLineTools/usr/bin/",
        )
        if (
            str(candidate)
            not in (
                {toolchain["cargo"]["path"], toolchain["rustc"]["path"]}
                | approved_tools
            )
            and not str(candidate).startswith(allowed)
        ):
            raise QualificationError(
                "G036 native runtime dependency escapes the pinned platform closure"
            )
        member = {
            "path": str(candidate),
            "sha256": _g036_runtime_digest(candidate),
            "aliases": set(),
        }
        if declared != candidate:
            member["aliases"].add(str(declared))
            physical_alias = declared.parent.resolve(strict=True) / declared.name
            if physical_alias != candidate:
                member["aliases"].add(str(physical_alias))
        closure[candidate] = member
        observed = subprocess.run(
            ["/usr/bin/otool", "-L", str(candidate)],
            capture_output=True,
            check=False,
        )
        if observed.returncode:
            raise QualificationError("G036 cannot resolve native runtime dependencies")
        dependencies = [
            line.strip().split(" (", 1)[0]
            for line in observed.stdout.decode("utf-8", "strict").splitlines()[1:]
        ]
        loader = subprocess.run(
            ["/usr/bin/otool", "-l", str(candidate)],
            capture_output=True,
            check=False,
        )
        if loader.returncode:
            raise QualificationError("G036 cannot resolve native runtime rpaths")
        rpaths = re.findall(
            r"(?ms)cmd LC_RPATH\s+cmdsize \d+\s+path ([^\n]+?) \(offset",
            loader.stdout.decode("utf-8", "strict"),
        )
        for dependency in dependencies:
            if dependency.startswith("/"):
                dependency_path = Path(dependency)
                if (
                    not dependency_path.exists()
                    and dependency.startswith(("/System/", "/usr/lib/"))
                ):
                    continue
                pending.append(dependency_path)
            elif dependency.startswith("@rpath/"):
                resolved = [
                    Path(path.replace("@loader_path", str(candidate.parent)))
                    / dependency.removeprefix("@rpath/")
                    for path in rpaths
                ]
                matches = [path for path in resolved if path.is_file()]
                if len(matches) != 1:
                    raise QualificationError(
                        "G036 native runtime rpath dependency is absent or ambiguous"
                    )
                pending.append(matches[0])
            elif dependency.startswith("@"):
                raise QualificationError(
                    "G036 native runtime contains an unresolved dynamic dependency"
                )
    return [
        {
            "path": member["path"],
            "sha256": member["sha256"],
            "aliases": sorted(member["aliases"]),
        }
        for _, member in sorted(closure.items())
    ]


def _g036_external_read_roots(
    toolchain: dict[str, Any],
) -> tuple[Path, ...]:
    rust_root = Path(toolchain["rustc"]["path"]).parents[1]
    roots = (
        rust_root / "lib/rustlib/aarch64-apple-darwin/lib",
        Path("/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk").resolve(
            strict=True
        ),
        Path("/Library/Developer/CommandLineTools/usr/lib/clang/21"),
        Path("/Library/Developer/CommandLineTools/usr/lib"),
        Path("/Library/Developer/CommandLineTools/usr/share/git-core"),
    )
    rust_prefix = f"{rust_root}/"
    for root in roots:
        resolved = root.resolve(strict=True)
        if (
            root.is_symlink()
            or not resolved.is_dir()
            or (
                not str(resolved).startswith(rust_prefix)
                and not str(resolved).startswith(
                    "/Library/Developer/CommandLineTools/"
                )
            )
        ):
            raise QualificationError(
                "G036 authenticated compiler resource root is unsafe"
            )
        if str(resolved).startswith(
            "/Library/Developer/CommandLineTools/"
        ):
            metadata = resolved.stat()
            if metadata.st_uid != 0 or metadata.st_mode & 0o022:
                raise QualificationError(
                    "G036 platform compiler resource root is mutable"
                )
    return tuple(root.resolve(strict=True) for root in roots)


G036_PLATFORM_UTILITIES = (
    Path("/bin/sh"),
    Path("/bin/bash"),
    Path("/bin/rm"),
    Path("/bin/chmod"),
    Path("/bin/kill"),
    Path("/usr/bin/touch"),
)
G036_LIFECYCLE_LOCK_DIRECTORY = Path("/private/var/tmp")


def _g036_sandbox_filter_any(filters: list[str]) -> str:
    if not filters:
        raise QualificationError("G036 sandbox filter set is empty")
    if len(filters) == 1:
        return filters[0]
    return "(require-any " + " ".join(filters) + ")"


def _g036_sandbox_profile(
    writable_roots: tuple[Path, ...],
    readable_files: tuple[Path, ...],
    executable_files: tuple[Path, ...],
    authenticated_file_aliases: tuple[Path, ...] = (),
    authenticated_read_roots: tuple[Path, ...] = (),
    authenticated_external_read_roots: tuple[Path, ...] = (),
) -> str:
    sandbox = Path("/usr/bin/sandbox-exec")
    if sandbox.is_symlink() or not sandbox.is_file():
        raise QualificationError("G036 trusted replay requires /usr/bin/sandbox-exec")
    rendered_writes = []
    for root in writable_roots:
        resolved = root.resolve(strict=True)
        if not resolved.is_absolute() or resolved == ROOT or ROOT in resolved.parents or any(c in str(resolved) for c in ('"', "\\", "\n", "\r")):
            raise QualificationError("G036 sandbox writable root is unsafe")
        rendered_writes.append(str(resolved))
    if len(set(rendered_writes)) != len(rendered_writes):
        raise QualificationError("G036 sandbox writable roots are ambiguous")
    invocation_root = Path(os.path.commonpath(rendered_writes))
    rendered_read_roots: list[str] = []
    for root in authenticated_read_roots:
        resolved = root.resolve(strict=True)
        if (
            resolved.is_symlink()
            or not resolved.is_dir()
            or resolved == invocation_root
            or invocation_root not in resolved.parents
            or any(c in str(resolved) for c in ('"', "\\", "\n", "\r"))
        ):
            raise QualificationError("G036 authenticated sandbox read root is unsafe")
        rendered_read_roots.append(str(resolved))
    if len(set(rendered_read_roots)) != len(rendered_read_roots):
        raise QualificationError("G036 authenticated sandbox read roots are ambiguous")
    rendered_external_read_roots: list[str] = []
    for root in authenticated_external_read_roots:
        resolved = root.resolve(strict=True)
        if (
            root.is_symlink()
            or not resolved.is_dir()
            or not str(resolved).startswith(
                (
                    "/opt/homebrew/Cellar/rust/",
                    "/Library/Developer/CommandLineTools/",
                )
            )
        ):
            raise QualificationError(
                "G036 external sandbox read root is unsafe"
            )
        rendered_external_read_roots.append(str(resolved))
    if len(set(rendered_external_read_roots)) != len(
        rendered_external_read_roots
    ):
        raise QualificationError(
            "G036 external sandbox read roots are ambiguous"
        )
    files: set[str] = set()
    ancestors: set[str] = set()
    for file in readable_files:
        resolved = file.resolve(strict=True)
        if file.is_symlink() or not resolved.is_file() or any(c in str(resolved) for c in ('"', "\\", "\n", "\r")):
            raise QualificationError("G036 sandbox read closure contains an unsafe member")
        files.add(str(resolved))
        ancestors.update(str(parent) for parent in resolved.parents)
    alias_files: set[str] = set()
    for alias in authenticated_file_aliases:
        if (
            not alias.is_absolute()
            or not alias.is_file()
            or any(c in str(alias) for c in ('"', "\\", "\n", "\r"))
            or str(alias.resolve(strict=True)) not in files
        ):
            raise QualificationError("G036 authenticated runtime alias is unsafe")
        alias_files.add(str(alias))
        ancestors.update(str(parent) for parent in alias.parents)
    literal_reads = [
        f'(literal "{path}")'
        for path in sorted(ancestors | files | alias_files)
    ]
    for special_path in (Path("/dev/null"),):
        metadata = special_path.stat()
        if not stat.S_ISCHR(metadata.st_mode):
            raise QualificationError(
                "G036 sandbox special read path is unsafe"
            )
        literal_reads.append(f'(literal "{special_path}")')
    literal_maps = [
        f'(literal "{path}")'
        for path in sorted(files | alias_files)
    ]
    executable_filters = [
        f'(literal "{path}")'
        for path in sorted(
            {
                str(path.resolve(strict=True))
                for path in executable_files
            }
        )
    ]
    executable_filters.append(f'(subpath "{rendered_writes[2]}")')
    # Lifecycle-qualification tests execute invocation-scoped fixture scripts
    # (a fake launchctl) from the scratch root; tests already execute arbitrary
    # code from the target root, so this does not widen the trust boundary.
    executable_filters.append(f'(subpath "{rendered_writes[3]}")')
    # Those fixture scripts and test helpers rely on a fixed set of immutable,
    # root-owned platform utilities executed in place (copies of arm64e
    # platform binaries are killed by the platform loader, so they cannot be
    # provisioned into the invocation root like the Developer tools above).
    for utility in G036_PLATFORM_UTILITIES:
        if utility.is_symlink() or not utility.is_file():
            raise QualificationError("G036 platform utility is absent or unsafe")
        metadata = utility.stat()
        if metadata.st_uid != 0 or metadata.st_mode & 0o022:
            raise QualificationError("G036 platform utility is mutable")
        executable_filters.append(f'(literal "{utility}")')
        literal_reads.append(f'(literal "{utility}")')
        literal_maps.append(f'(literal "{utility}")')
    # The production service runner serializes lifecycle transactions by
    # flocking the root-owned sticky /private/var/tmp directory itself; the
    # lifecycle-qualification tests exercise that real runner, so the sandbox
    # permits read and lock (never write) of exactly that verified directory.
    lock_directory = G036_LIFECYCLE_LOCK_DIRECTORY
    lock_metadata = lock_directory.stat()
    if (
        lock_directory.is_symlink()
        or not lock_directory.is_dir()
        or lock_metadata.st_uid != 0
        or lock_metadata.st_mode & 0o1000 == 0
    ):
        raise QualificationError(
            "G036 lifecycle lock directory is not a root-owned sticky directory"
        )
    literal_reads.append(f'(literal "{lock_directory}")')
    literal_reads.extend(
        f'(literal "{parent}")' for parent in lock_directory.parents
    )
    for path in rendered_external_read_roots:
        literal_reads.append(f'(literal "{path}")')
        literal_reads.extend(
            f'(literal "{parent}")'
            for parent in Path(path).parents
        )
    read_filters = literal_reads + [
        f'(subpath "{path}")'
        for path in (
            rendered_read_roots
            + rendered_external_read_roots
            + rendered_writes
        )
    ]
    map_filters = literal_maps + [
        f'(subpath "{path}")'
        for path in rendered_writes
    ]
    write_filters = [
        f'(subpath "{path}")'
        for path in rendered_writes
    ]
    # Lifecycle and IPC tests pin short `/tmp/pw*-…` fixture roots so their
    # Unix-domain socket paths stay inside the kernel sun_path bound; permit
    # exactly that reserved prefix (the `/tmp` symlink resolves to
    # `/private/tmp` before sandbox evaluation).
    fixture_prefix_filter = '(regex #"^/private/tmp/pw[0-9a-z]+-")'
    write_filters.append(fixture_prefix_filter)
    read_filters.append(fixture_prefix_filter)
    map_filters.append(fixture_prefix_filter)
    executable_filters.append(fixture_prefix_filter)
    lock_filters = write_filters + [f'(literal "{lock_directory}")']
    return (
        "(version 1)(deny default)"
        "(allow file-read-metadata)"
        "(allow process-info*)(allow process-fork)(allow signal)"
        "(allow sysctl-read)(allow mach-lookup)(allow ipc-posix-shm)"
        "(allow file-write-data (require-not (vnode-type REGULAR-FILE)))"
        "(allow network-outbound (remote unix-socket))"
        "(allow network-bind (local unix-socket))"
        f"(allow process-exec {_g036_sandbox_filter_any(executable_filters)})"
        f"(allow file-read* {_g036_sandbox_filter_any(read_filters)})"
        f"(allow file-map-executable {_g036_sandbox_filter_any(map_filters)})"
        f"(allow file-write* {_g036_sandbox_filter_any(write_filters)})"
        f"(allow file-link {_g036_sandbox_filter_any(write_filters)})"
        f"(allow file-lock {_g036_sandbox_filter_any(lock_filters)})"
    )


def _g036_sandboxed_candidate_run(argv: list[str], environment: dict[str, str], target_dir: Path) -> subprocess.CompletedProcess[bytes]:
    target_dir.mkdir(parents=True, exist_ok=True)
    replay_root = target_dir.parent.resolve(strict=True)
    writable_roots = (Path(environment["HOME"]), Path(environment["CARGO_HOME"]), target_dir, Path(environment["TMPDIR"]))
    vendor = replay_root / "inputs" / "vendor"
    vendor_manifest = _g036_vendor_manifest(vendor)
    toolchain = _g036_toolchain()
    runtime = _g036_runtime_closure(toolchain)
    platform_read_files = (Path("/private/etc/ssl/openssl.cnf"),)
    platform_read_digests = {
        path: _g036_runtime_digest(path)
        for path in platform_read_files
    }
    for dependency in runtime:
        if _g036_runtime_digest(Path(dependency["path"])) != dependency["sha256"]:
            raise QualificationError("G036 native runtime dependency digest drifted")
    external_read_roots = _g036_external_read_roots(toolchain)
    product = _g036_product_source_tree()
    readable = [ROOT / relative for relative in product["paths"]]
    readable.extend(Path(item["path"]) for item in runtime)
    readable.extend(platform_read_files)
    runtime_aliases = tuple(
        Path(alias)
        for dependency in runtime
        for alias in dependency["aliases"]
    )
    readable.extend(Path(environment[name]) for name in G036_TRUSTED_ENVIRONMENT["environment"]["injectedTestVariables"] if name in environment)
    executable = tuple(
        [Path(item["path"]) for item in runtime]
        + list(runtime_aliases)
        + (
            [Path(environment["PODWAYD_TEST_BINARY"])]
            if "PODWAYD_TEST_BINARY" in environment
            else []
        )
    )
    profile = _g036_sandbox_profile(
        writable_roots,
        tuple(readable),
        executable,
        runtime_aliases,
        (vendor,),
        external_read_roots,
    )
    profile_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            prefix="sandbox-",
            suffix=".sb",
            dir=target_dir,
            delete=False,
        ) as profile_file:
            profile_file.write(profile)
            profile_file.flush()
            os.fsync(profile_file.fileno())
            profile_path = Path(profile_file.name)
        completed = subprocess.run(
            ["/usr/bin/sandbox-exec", "-f", str(profile_path), *argv],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            check=False,
        )
    finally:
        if profile_path is not None:
            profile_path.unlink(missing_ok=True)
    if (
        _g036_product_source_tree() != product
        or _g036_vendor_manifest(vendor) != vendor_manifest
    ):
        raise QualificationError(
            "G036 authenticated read closure drifted during candidate command"
        )
    for path, digest in platform_read_digests.items():
        if _g036_runtime_digest(path) != digest:
            raise QualificationError(
                "G036 authenticated platform read dependency drifted during candidate command"
            )
    for dependency in runtime:
        if _g036_runtime_digest(Path(dependency["path"])) != dependency["sha256"]:
            raise QualificationError(
                "G036 native runtime dependency drifted during candidate command"
            )
    return completed


def _validate_g036_post_command_identity(
    input_tree: dict[str, Any], toolchain: dict[str, Any], daemon_path: Path | None = None,
    receipt_path: Path | None = None, daemon_sha256: str | None = None, vendor: Path | None = None,
    vendor_manifest: dict[str, Any] | None = None,
) -> None:
    if (
        _g036_product_source_tree() != input_tree
        or _g036_toolchain() != toolchain
        or (vendor is not None and _g036_vendor_manifest(vendor) != vendor_manifest)
    ):
        raise QualificationError("G036 trusted replay product input tree or toolchain drifted after candidate command")
    if daemon_path is None or receipt_path is None or daemon_sha256 is None:
        return
    if daemon_path.is_symlink() or not daemon_path.is_file() or receipt_path.is_symlink():
        raise QualificationError("G036 trusted replay canonical daemon or receipt became unsafe")
    daemon = daemon_path.resolve(strict=True)
    _validate_g036_thin_arm64_mach_o(daemon, "canonical daemon")
    receipt = load_json(receipt_path)
    if (
        not isinstance(receipt, dict)
        or set(receipt) != {"schema", "binary", "binary_sha256", "inputs", "toolchain"}
        or receipt.get("schema") != "podway.daemon-build-receipt/v1"
        or receipt.get("binary") != str(daemon)
        or receipt.get("binary_sha256") != daemon_sha256
        or sha256_file(daemon) != daemon_sha256
        or receipt.get("inputs") != {
            relative: sha256_file(ROOT / relative) for relative in input_tree["paths"]
        }
        or receipt.get("toolchain") != toolchain
    ):
        raise QualificationError("G036 trusted replay canonical daemon receipt identity drifted after candidate command")

def _g036_daemon_build_receipt(
    toolchain: dict[str, Any], target_dir: Path, receipt_dir: Path,
) -> tuple[Path, Path]:
    input_tree = _g036_product_source_tree()
    environment = _g036_sanitized_environment(toolchain, target_dir)
    completed = _g036_sandboxed_candidate_run(
        [toolchain["cargo"]["path"], "test", "--workspace", "--locked", "--target", G036_TARGET["triple"], "--no-run"],
        environment,
        target_dir,
    )
    _validate_g036_post_command_identity(input_tree, toolchain)
    if completed.returncode:
        raise QualificationError("G036 trusted replay could not build the canonical daemon")
    built_binary_path = target_dir / G036_TARGET["triple"] / "debug/podwayd"
    if built_binary_path.is_symlink() or not built_binary_path.is_file():
        raise QualificationError("G036 trusted replay canonical daemon is absent or unsafe")
    built_binary = built_binary_path.resolve(strict=True)
    _validate_g036_thin_arm64_mach_o(built_binary, "canonical daemon")
    receipt_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
    frozen_binary = receipt_dir / "podwayd"
    with built_binary.open("rb") as source, frozen_binary.open("xb") as destination:
        shutil.copyfileobj(source, destination)
    frozen_binary.chmod(0o555)
    binary = frozen_binary.resolve(strict=True)
    if sha256_file(binary) != sha256_file(built_binary):
        raise QualificationError("G036 trusted replay canonical daemon snapshot differs")
    receipt = {
        "schema": "podway.daemon-build-receipt/v1",
        "binary": str(binary),
        "binary_sha256": sha256_file(binary),
        "inputs": {
            item["path"]: item["sha256"]
            for item in [
                {"path": relative, "sha256": sha256_file(ROOT / relative)}
                for relative in _g036_product_source_tree()["paths"]
            ]
        },
        "toolchain": toolchain,
    }
    receipt_path = receipt_dir / "g036-daemon-build-receipt.json"
    receipt_path.write_bytes(canonical_json(receipt) + b"\n")
    _validate_g036_post_command_identity(
        input_tree, toolchain, binary, receipt_path, receipt["binary_sha256"],
    )
    daemon_environment = {
        "PODWAYD_TEST_BINARY": str(binary),
        "PODWAYD_BUILD_RECEIPT": str(receipt_path),
    }
    stabilized = _g036_sandboxed_candidate_run(
        [toolchain["cargo"]["path"], "test", "--workspace", "--locked", "--target", G036_TARGET["triple"], "--no-run"],
        _g036_sanitized_environment(toolchain, target_dir, daemon_environment),
        target_dir,
    )
    _validate_g036_post_command_identity(
        input_tree, toolchain, binary, receipt_path, receipt["binary_sha256"],
    )
    if stabilized.returncode:
        raise QualificationError("G036 trusted replay could not stabilize the canonical daemon build")
    return binary, receipt_path

G036_TARGET = {
    "triple": "aarch64-apple-darwin",
    "arch": "arm64",
    "host_arch": "arm64",
    "mach_o_arch": "arm64",
}
def _relative_digest_reference(value: Any, label: str) -> tuple[Path, str]:
    if (
        not isinstance(value, dict)
        or set(value) != {"path", "sha256"}
        or not isinstance(value["path"], str)
        or Path(value["path"]).is_absolute()
        or ".." in Path(value["path"]).parts
        or not re.fullmatch(r"[0-9a-f]{64}", str(value["sha256"]))
    ):
        raise QualificationError(f"{label} is malformed")
    candidate = ROOT / value["path"]
    if not candidate.is_file() or sha256_file(candidate) != value["sha256"]:
        raise QualificationError(f"{label} is stale")
    return candidate, value["sha256"]
def _matrix_cargo_commands(matrix: dict[str, Any]) -> dict[str, dict[str, Any]]:
    commands: dict[str, dict[str, Any]] = {}
    for criterion in matrix["criteria"]:
        proof = criterion["proof"]
        members = proof["members"] if proof["kind"] == "cargo-test-set" else [proof] if proof["kind"] == "cargo-test" else []
        for member in members:
            command = member["command"]
            descriptor = commands.setdefault(command, {"path": member["path"], "function": member["function"], "semanticBindings": []})
            if (descriptor["path"], descriptor["function"]) != (member["path"], member["function"]):
                raise QualificationError("product acceptance matrix command is ambiguously bound")
            if proof["kind"] == "cargo-test-set":
                descriptor["semanticBindings"].append({"criterionId": criterion["id"], "path": member["path"], "function": member["function"], "obligationIds": member["obligation_ids"]})
    for descriptor in commands.values():
        descriptor["semanticBindings"].sort(key=lambda binding: (binding["criterionId"], binding["path"], binding["function"], binding["obligationIds"]))
    return commands

def _validate_g036_thin_arm64_mach_o(path: Path, label: str) -> None:
    if path.is_symlink() or not path.is_file():
        raise QualificationError(f"G036 {label} executable is absent or unsafe")
    with path.open("rb") as executable:
        header = executable.read(16)
    if len(header) != 16:
        raise QualificationError(f"G036 {label} executable is not a thin arm64 Mach-O")
    if header[:8] == b"\xcf\xfa\xed\xfe\x0c\x00\x00\x01":
        file_type = int.from_bytes(header[12:16], "little")
    elif header[:8] == b"\xfe\xed\xfa\xcf\x01\x00\x00\x0c":
        file_type = int.from_bytes(header[12:16], "big")
    else:
        raise QualificationError(f"G036 {label} executable is not a thin arm64 Mach-O")
    if file_type != 0x2:
        raise QualificationError(f"G036 {label} executable is not an MH_EXECUTE Mach-O")


def _validate_cargo_receipt_output(output: bytes, descriptor: dict[str, Any]) -> tuple[int, int]:
    try:
        text = output.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise QualificationError("G036 command replay emitted non-UTF-8 Cargo output") from exc
    test_file = Path(descriptor["path"]).name
    test_binary = Path(test_file).stem
    test_markers = re.findall(r"(?m)^test ([^\s]+) \.\.\. ok$", text)
    binary_markers = re.findall(rf"(?m)^\s+Running tests/{re.escape(test_file)} \(([^()\r\n]*/debug/deps/{re.escape(test_binary)}-[0-9a-f]+)\)$", text)
    summaries = re.findall(r"(?m)^test result: ok\. (\d+) passed; 0 failed; (\d+) ignored;", text)
    if not re.findall(r"(?m)^running 1 test$", text) or test_markers != [descriptor["function"]] or len(binary_markers) != 1 or len(summaries) != 1:
        raise QualificationError("G036 command replay lacks an authentic exact test binary/function marker")
    _validate_g036_thin_arm64_mach_o(Path(binary_markers[0]), "exact test")
    return int(summaries[0][0]), int(summaries[0][1])


def _g036_replay_identity(
    toolchain: dict[str, Any], target_dir: Path, receipt_dir: Path,
) -> dict[str, Any]:
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise QualificationError("G036 trusted replay requires the native Apple-Silicon host")
    daemon_binary, daemon_receipt = _g036_daemon_build_receipt(toolchain, target_dir, receipt_dir)
    return {
        "invocationId": sha256_bytes(os.urandom(32)),
        "host": {"system": platform.system(), "machine": platform.machine(), "target": G036_TARGET},
        "toolchain": toolchain,
        "inputTree": _g036_product_source_tree(),
        "environment": {
            "PODWAYD_TEST_BINARY": str(daemon_binary),
            "PODWAYD_BUILD_RECEIPT": str(daemon_receipt),
        },
    }
def _trusted_replay_g036_command(receipt: dict[str, Any], descriptor: dict[str, Any], identity: dict[str, Any], target_dir: Path) -> None:
    argv = shlex.split(receipt["command"])
    host_toolchain_sha256 = sha256_bytes(canonical_json({"host": identity["host"], "toolchain": identity["toolchain"]}))
    if (
        receipt["argv"] != argv
        or receipt["inputTreeSha256"] != identity["inputTree"]["sha256"]
        or receipt["hostToolchainSha256"] != host_toolchain_sha256
    ):
        raise QualificationError("G036 command receipt exact argv, host/toolchain, or input closure differs")
    if any(token == "--target" or token.startswith("--target=") for token in argv):
        raise QualificationError("G036 command receipt target override is forbidden")
    replay_environment = _g036_sanitized_environment(
        identity["toolchain"], target_dir, identity["environment"],
    )
    vendor = target_dir.parent / "inputs" / "vendor"
    vendor_manifest = _g036_vendor_manifest(vendor)
    separator = argv.index("--")
    daemon_path = Path(identity["environment"]["PODWAYD_TEST_BINARY"])
    receipt_path = Path(identity["environment"]["PODWAYD_BUILD_RECEIPT"])
    if daemon_path.is_symlink() or not daemon_path.is_file() or receipt_path.is_symlink():
        raise QualificationError("G036 trusted replay daemon identity became unsafe")
    daemon_path = daemon_path.resolve(strict=True)
    daemon_receipt = load_json(receipt_path)
    prior_daemon_sha256 = sha256_file(daemon_path)
    if (
        not isinstance(daemon_receipt, dict)
        or set(daemon_receipt) != {"schema", "binary", "binary_sha256", "inputs", "toolchain"}
        or daemon_receipt.get("schema") != "podway.daemon-build-receipt/v1"
        or not isinstance(daemon_receipt.get("binary_sha256"), str)
        or not re.fullmatch(r"[0-9a-f]{64}", daemon_receipt["binary_sha256"])
        or daemon_receipt["binary_sha256"] != prior_daemon_sha256
        or daemon_receipt.get("binary") != str(daemon_path)
        or daemon_receipt.get("inputs")
        != {
            relative: sha256_file(ROOT / relative)
            for relative in identity["inputTree"]["paths"]
        }
        or daemon_receipt.get("toolchain") != identity["toolchain"]
    ):
        raise QualificationError("G036 trusted replay prior daemon receipt identity drifted")
    _validate_g036_thin_arm64_mach_o(daemon_path, "prior canonical daemon")

    prebuild_argv = [
        identity["toolchain"]["cargo"]["path"],
        *argv[1:separator],
        "--target",
        G036_TARGET["triple"],
        "--no-run",
    ]
    prebuilt = _g036_sandboxed_candidate_run(prebuild_argv, replay_environment, target_dir)
    _validate_g036_post_command_identity(
        identity["inputTree"], identity["toolchain"], vendor=vendor, vendor_manifest=vendor_manifest,
    )
    if prebuilt.returncode:
        raise QualificationError("G036 trusted replay could not prebuild an exact test command")
    _validate_g036_thin_arm64_mach_o(daemon_path, "prebuilt canonical daemon")
    if sha256_file(daemon_path) != prior_daemon_sha256:
        raise QualificationError("G036 trusted replay canonical daemon drifted during exact-command prebuild")
    _validate_g036_post_command_identity(
        identity["inputTree"], identity["toolchain"], daemon_path, receipt_path, prior_daemon_sha256,
        vendor, vendor_manifest,
    )
    completed = _g036_sandboxed_candidate_run(
        [
            identity["toolchain"]["cargo"]["path"],
            *argv[1:separator],
            "--target",
            G036_TARGET["triple"],
            *argv[separator:],
        ],
        replay_environment,
        target_dir,
    )
    _validate_g036_post_command_identity(
        identity["inputTree"], identity["toolchain"], daemon_path, receipt_path, prior_daemon_sha256,
        vendor, vendor_manifest,
    )
    combined = completed.stdout + completed.stderr
    if completed.returncode != 0:
        tail = combined[-800:].decode("utf-8", "replace")
        raise QualificationError(
            f"G036 exact command failed under the hermetic sandbox: {tail}"
        )
    test_count, ignored_count = _validate_cargo_receipt_output(combined, descriptor)
    observed = {
        "exitCode": completed.returncode,
        "testCount": test_count,
        "ignoredCount": ignored_count,
    }
    if observed != {field: receipt[field] for field in observed}:
        raise QualificationError("G036 trusted replay result or exact test identity differs")

def validate_g036_test_report(
    path: Path = G036_REPORT_PATH,
    matrix_path: Path = G036_MATRIX_PATH,
    policy_path: Path = ROOT / "release/g009-release-policy-v1.json",
) -> dict[str, Any]:
    if matrix_path != G036_MATRIX_PATH:
        raise QualificationError("G036 test report matrix path is not canonical")
    if sha256_file(G036_MATRIX_PATH) != G036_MATRIX_SHA256:
        raise QualificationError("G036 canonical matrix digest is stale")
    matrix = validate_product_acceptance_matrix(matrix_path)
    report = load_json(path)
    required = {
        "schemaVersion", "kind", "storyId", "generatedAt", "target", "source",
        "scope", "criteria", "commands", "artifactProofs", "replay", "result",
    }
    if not isinstance(report, dict) or set(report) != required:
        raise QualificationError("G036 test report schema is malformed")
    if (
        report["schemaVersion"] != 6
        or report["kind"] != "api-package-test-report"
        or report["storyId"] != "G036"
        or not isinstance(report["generatedAt"], str)
        or not re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?\+00:00", report["generatedAt"])
        or report["target"] != G036_TARGET
        or report["replay"] != {"kind": "trusted-verifier-replay", "requireSingleCurrentInvocation": True}
        or report["result"] != {"status": "passed"}
    ):
        raise QualificationError("G036 test report identity, native target, replay contract, or result drift")
    source = report["source"]
    expected_source_keys = {
        "cargoLock", "testSources", "matrix", "policy", "verifier", "productSourceTree",
    }
    if not isinstance(source, dict) or set(source) != expected_source_keys:
        raise QualificationError("G036 test report source binding is malformed")
    cargo_lock, _ = _relative_digest_reference(source["cargoLock"], "G036 Cargo.lock binding")
    if cargo_lock != ROOT / "Cargo.lock":
        raise QualificationError("G036 test report Cargo.lock binding differs")
    test_sources = source["testSources"]
    if not isinstance(test_sources, dict) or test_sources != matrix["source_files"]:
        raise QualificationError("G036 test report test-source bindings differ from matrix")
    for source_path, digest in test_sources.items():
        _relative_digest_reference({"path": source_path, "sha256": digest}, "G036 test-source binding")
    if source["productSourceTree"] != _g036_product_source_tree():
        raise QualificationError("G036 product source-tree digest or coverage differs")
    expected_bindings = (
        ("matrix", G036_MATRIX_PATH.relative_to(ROOT).as_posix(), G036_MATRIX_SHA256),
        ("policy", policy_path.relative_to(ROOT).as_posix(), None),
        ("verifier", "tools/verify_g009_qualification.py", None),
    )
    for field, expected_path, expected_digest in expected_bindings:
        candidate, digest = _relative_digest_reference(source[field], f"G036 {field} binding")
        if candidate != ROOT / expected_path or (
            expected_digest is not None and digest != expected_digest
        ):
            raise QualificationError(f"G036 {field} binding differs")
    expected_scope = {
        "criterionCount": G036_CRITERION_COUNT,
        "cargoCriterionCount": sum(item["proof"]["kind"] in {"cargo-test", "cargo-test-set"} for item in matrix["criteria"]),
        "artifactCriterionCount": sum(item["proof"]["kind"] == "artifact" for item in matrix["criteria"]),
        "exactCommandCount": G036_EXACT_COMMAND_COUNT,
    }
    if report["scope"] != expected_scope:
        raise QualificationError("G036 test report scope differs from frozen contract")
    expected_criteria = {
        criterion["id"]: criterion["proof"] for criterion in matrix["criteria"]
    }
    criteria = report["criteria"]
    if not isinstance(criteria, list) or len(criteria) != G036_CRITERION_COUNT:
        raise QualificationError("G036 test report criterion coverage is incomplete")
    actual_criteria: dict[str, Any] = {}
    for row in criteria:
        if not isinstance(row, dict) or set(row) != {"id", "proof"} or row["id"] in actual_criteria:
            raise QualificationError("G036 test report criteria are malformed or duplicated")
        actual_criteria[row["id"]] = row["proof"]
    if actual_criteria != expected_criteria:
        raise QualificationError("G036 test report criteria are stale, relabelled, or unbound")
    commands = report["commands"]
    expected_commands = _matrix_cargo_commands(matrix)
    if (
        len(expected_commands) != G036_EXACT_COMMAND_COUNT
        or not isinstance(commands, list)
        or len(commands) != G036_EXACT_COMMAND_COUNT
    ):
        raise QualificationError("G036 test report exact command coverage is incomplete")
    actual_commands: dict[str, dict[str, Any]] = {}
    for receipt in commands:
        required_receipt = {"command", "argv", "semanticBindings", "inputTreeSha256", "hostToolchainSha256", "exitCode", "testCount", "ignoredCount"}
        if not isinstance(receipt, dict) or set(receipt) != required_receipt:
            raise QualificationError("G036 command receipt is malformed")
        command = receipt["command"]
        if not isinstance(command, str) or command in actual_commands or receipt["argv"] != shlex.split(command):
            raise QualificationError("G036 command receipt is duplicate or malformed")
        expected_descriptor = expected_commands.get(command)
        if (
            expected_descriptor is None
            or receipt["exitCode"] != 0
            or not isinstance(receipt["testCount"], int)
            or isinstance(receipt["testCount"], bool)
            or receipt["testCount"] != 1
            or receipt["ignoredCount"] != 0
            or receipt["inputTreeSha256"] != source["productSourceTree"]["sha256"]
            or not re.fullmatch(r"[0-9a-f]{64}", str(receipt["hostToolchainSha256"]))
        ):
            raise QualificationError("G036 command receipt is not a passing non-ignored replay contract")
        if receipt["semanticBindings"] != expected_descriptor["semanticBindings"]:
            raise QualificationError("G036 command semantic binding differs")
        actual_commands[command] = receipt
    if set(actual_commands) != set(expected_commands):
        raise QualificationError("G036 command receipt is absent, stale, or relabelled")
    toolchain = _g036_toolchain()
    with tempfile.TemporaryDirectory(prefix="p36-", dir="/private/tmp") as raw:
        replay_root = Path(raw)
        replay_identity = _g036_replay_identity(
            toolchain, replay_root / "target", replay_root / "receipts",
        )
        for command, receipt in actual_commands.items():
            _trusted_replay_g036_command(
                receipt, expected_commands[command], replay_identity, replay_root / "target",
            )
    if report["artifactProofs"] != []:
        raise QualificationError("G036 direct-evidence report retains obsolete artifact proofs")
    return report
def validate_product_acceptance_matrix(path: Path = ROOT / "release/product-acceptance-matrix-v1.json") -> dict[str, Any]:
    value = load_json(path)
    if not isinstance(value, dict) or set(value) != {"schema", "version", "source", "source_files", "input_closure", "semantic_contracts", "criteria"}:
        raise QualificationError("product acceptance matrix schema is malformed")
    if value["schema"] != "podway.product-acceptance-matrix/v1" or value["version"] != 3:
        raise QualificationError("product acceptance matrix version drift")
    expected_contracts = [{"criterion_id": cid, "obligations": [{"id": oid, "statement": statement} for oid, statement in obligations]} for cid, obligations in G040_OBLIGATIONS.items()]
    if value["semantic_contracts"] != expected_contracts:
        actual = value["semantic_contracts"]
        criterion = next((row.get("criterion_id") for row in actual if isinstance(row, dict) and row.get("criterion_id") in G040_OBLIGATIONS), "PAC-005") if isinstance(actual, list) else "PAC-005"
        raise QualificationError(f"product acceptance semantic contract differs: {criterion}")
    source, source_files, input_closure, criteria = value["source"], value["source_files"], value["input_closure"], value["criteria"]
    if not isinstance(source, dict) or source.get("path") != "sot/docs/60-quality/61-product-acceptance.md" or not isinstance(source.get("excluded_lines"), list): raise QualificationError("product acceptance matrix source binding is malformed")
    if set(source["excluded_lines"]) != {108,109,110,111,112,113,114,118,120,121,122,123}: raise QualificationError("product acceptance matrix exclusion contract drift")
    sot=(ROOT/source["path"]).read_text(encoding="utf-8").splitlines(); expected=[(number,line[2:]) for number,line in enumerate(sot,1) if line.startswith("- ") and number not in set(source["excluded_lines"])]
    if not isinstance(criteria,list) or len(criteria)!=len(expected) or len(criteria)!=G036_CRITERION_COUNT: raise QualificationError("product acceptance matrix criterion count differs from SOT")
    if input_closure != {"globs": list(G036_PRODUCT_SOURCE_TREE_GLOBS), "requireCompleteFileDigests": True}:
        raise QualificationError("product acceptance matrix product input closure drift")
    if not isinstance(source_files,dict) or not source_files: raise QualificationError("product acceptance matrix proof-file binding is missing")
    for proof_path,digest in source_files.items():
        if not isinstance(proof_path,str) or Path(proof_path).is_absolute() or ".." in Path(proof_path).parts or not re.fullmatch(r"[0-9a-f]{64}",str(digest)): raise QualificationError("product acceptance matrix proof-file binding is unsafe")
        if not (ROOT/proof_path).is_file() or sha256_file(ROOT/proof_path)!=digest: raise QualificationError("product acceptance matrix proof-file digest is stale")
    for index,(criterion,(line,text)) in enumerate(zip(criteria,expected),1):
        if not isinstance(criterion,dict) or set(criterion)!={"id","line","text","proof","status"}: raise QualificationError("product acceptance matrix criterion fields are malformed")
        if criterion["id"]!=f"PAC-{index:03d}" or criterion["line"]!=line or criterion["text"]!=text or criterion["status"]!="automated": raise QualificationError("product acceptance matrix criterion ordering or source text drift")
        proof=criterion["proof"]; cid=criterion["id"]
        if not isinstance(proof,dict) or proof.get("kind") not in {"cargo-test","cargo-test-set"}: raise QualificationError("product acceptance matrix proof kind is malformed")
        if cid in G040_SEMANTIC_CRITERION_IDS:
            if proof.get("kind")!="cargo-test-set" or proof.get("criterion_id")!=cid: raise QualificationError(f"product acceptance semantic criterion identity differs: {cid}")
            members=proof.get("members")
            if not isinstance(members,list) or not members: raise QualificationError(f"product acceptance semantic proof membership differs: {cid}")
            seen=set()
            for member in members:
                if not isinstance(member,dict) or set(member)!={"criterion_id","path","function","command","obligation_ids"} or member["criterion_id"]!=cid or member["path"] not in source_files: raise QualificationError(f"product acceptance semantic proof membership differs: {cid}")
                identity=(member["path"],member["function"],member["command"])
                if identity in seen: raise QualificationError(f"product acceptance semantic proof membership differs: {cid}")
                seen.add(identity)
                command=shlex.split(member["command"]); expected_command=["cargo","test","-p",Path(member["path"]).parts[1],"--test",Path(member["path"]).stem,member["function"],"--locked","--","--exact"]
                if command!=expected_command: raise QualificationError(f"product acceptance semantic proof membership differs: {cid}")
                validate_rust_function_locator((ROOT/member["path"]).read_text(encoding="utf-8"),member["function"],True)
                allowed={oid for oid,_ in G040_OBLIGATIONS[cid]}; ids=member["obligation_ids"]
                if not isinstance(ids,list) or not ids or len(ids)!=len(set(ids)) or any(oid not in allowed for oid in ids): raise QualificationError(f"product acceptance semantic obligation coverage differs: {cid}")
            expected_ids=[oid for oid,_ in G040_OBLIGATIONS[cid]]; owned=[oid for member in members for oid in member["obligation_ids"]]
            if len(owned)!=len(expected_ids) or set(owned)!=set(expected_ids): raise QualificationError(f"product acceptance semantic obligation coverage differs: {cid}")
            expected_members=[{**member,"obligation_ids":list(member["obligation_ids"])} for member in G040_PROOF_MEMBERSHIP[cid]]
            if members!=expected_members: raise QualificationError(f"product acceptance semantic proof membership differs: {cid}")
        elif proof["kind"]=="cargo-test-set": raise QualificationError("product acceptance matrix semantic proof is not allowed for this criterion")
        elif proof["kind"]=="cargo-test":
            if set(proof)!={"kind","command","path","function"} or proof["path"] not in source_files: raise QualificationError("product acceptance matrix Cargo proof is malformed")
            command=shlex.split(proof["command"]); expected_command=["cargo","test","-p",Path(proof["path"]).parts[1],"--test",Path(proof["path"]).stem,proof["function"],"--locked","--","--exact"]
            if cid in {"PAC-001","PAC-003","PAC-004","PAC-006"}: expected_command.append("--ignored")
            if command!=expected_command: raise QualificationError("product acceptance matrix command does not target its exact test binary")
            validate_rust_function_locator((ROOT/proof["path"]).read_text(encoding="utf-8"),proof["function"],True)
    if len(_matrix_cargo_commands(value))!=G036_EXACT_COMMAND_COUNT: raise QualificationError("product acceptance matrix exact Cargo command count drift")
    return value

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


def reset_crash_boundary_mappings(source_root: Path) -> dict[str, str]:
    runtime_path = source_root / "crates/podway-daemon/src/runtime_workspace.rs"
    test_path = source_root / "crates/podway-daemon/tests/phase5_reset_runtime.rs"
    for candidate in (runtime_path, test_path):
        if candidate.is_symlink() or not candidate.is_file() or not candidate.resolve().is_relative_to(source_root.resolve()):
            raise QualificationError("reset crash boundary source or test is unsafe or absent")
    runtime = bounded_bytes(runtime_path).decode("utf-8", "strict")
    test = bounded_bytes(test_path).decode("utf-8", "strict")
    enum_match = re.search(
        r"pub\s+enum\s+ResetAllCrashBoundaryV1\s*\{(?P<body>.*?)\}",
        runtime,
        re.DOTALL,
    )
    if enum_match is None:
        raise QualificationError("reset crash boundary enum is absent")
    variants = re.findall(r"\b([A-Z][A-Za-z0-9_]*)\s*,", enum_match.group("body"))
    expected_variants = ["MarkerCreated", "OldDatabaseDeleted", "NewTargetDatabaseCreated"]
    if variants != expected_variants:
        raise QualificationError("reset crash boundary enum variants drift")
    if "PODWAY_RESET_ALL_FAILPOINT" in runtime or "std::env::var" in runtime[:enum_match.end() + 1200]:
        raise QualificationError("reset crash injection must not use ambient production environment")
    if not re.search(
        r"reset_crash_injection:\s*ResetAllCrashInjectionV1::default\(\)",
        runtime,
    ) or not re.search(
        r"with_reset_crash_boundary_for_tests.*?reset_crash_injection\s*=\s*"
        r"ResetAllCrashInjectionV1\(Some\(boundary\)\)",
        runtime,
        re.DOTALL,
    ):
        raise QualificationError("reset crash boundary injection is not explicitly test-configured")
    calls = re.findall(
        r"reset_crash_injection\s*\.\s*abort_at\(\s*ResetAllCrashBoundaryV1::([A-Za-z0-9_]+)\s*\)",
        runtime,
    )
    if calls != ["MarkerCreated", "OldDatabaseDeleted", "NewTargetDatabaseCreated", "OldDatabaseDeleted", "NewTargetDatabaseCreated"]:
        raise QualificationError("reset crash boundary calls drift from durable reset transitions")
    mappings = dict(re.findall(
        r'\("(?P<id>C1[456])",\s*ResetAllCrashBoundaryV1::(?P<boundary>[A-Za-z0-9_]+),\s*(?:true|false)\)',
        test,
    ))
    expected_mappings = {
        "C14": "MarkerCreated",
        "C15": "OldDatabaseDeleted",
        "C16": "NewTargetDatabaseCreated",
    }
    if mappings != expected_mappings:
        raise QualificationError("reset crash test-to-boundary mapping drift")
    if (
        "status.signal()" not in test
        or "nix::libc::SIGABRT" not in test
        or "ResetMarkerV1::decode_canonical" not in test
        or "database_must_exist" not in test
        or "source_database_identity" not in test
    ):
        raise QualificationError("reset crash test lacks SIGABRT or durable boundary-state proof")
    return mappings


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
    if derived_store_ids != [f"C{number:02d}" for number in range(1, 14)] + ["P01"]:
        raise QualificationError("source-derived store crash IDs differ from policy")
    source_bound_failpoints = {
        "C05": ("crates/podway-daemon/src/execution.rs::DaemonExecutionEngineV1::execute_claimed", ["prepare boundary"]),
        "C06": ("crates/podway-daemon/src/native_execution.rs::NativeArtifactVerifierV1::hash_verified_local_artifact", ["artifact verification boundary"]),
        "C14": ("crates/podway-daemon/src/runtime_workspace.rs::WorkspaceRuntimeManagerV1::complete_reset_all_authorized", ["MarkerCreated"]),
        "C15": ("crates/podway-daemon/src/runtime_workspace.rs::WorkspaceRuntimeManagerV1::complete_reset_all_authorized", ["OldDatabaseDeleted"]),
        "C16": ("crates/podway-daemon/src/runtime_workspace.rs::WorkspaceRuntimeManagerV1::complete_reset_all_authorized", ["NewTargetDatabaseCreated"]),
        "D01": ("crates/podway-daemon/src/workspace.rs::ValidatedRuntimeDirectoryV1::publish_reset_marker", ["reset marker publish"]),
        "D02": ("crates/podway-daemon/src/registry.rs::persist_registry_v1", ["registry rename"]),
        "S01": ("crates/podway-service/src/lib.rs::StdServiceFilesystemV1::write_atomically", ["AfterTemporaryWrite", "AfterFileSyncAndMode", "BeforeRename", "AfterRename", "AfterParentDirectorySync"]),
        "S02": ("crates/podway-service/src/lib.rs::MacosServiceCommandRunnerV1::install_or_update", ["bootstrap side effect after plist publication"]),
        "S03": ("crates/podway-service/src/lib.rs::MacosServiceCommandRunnerV1::uninstall", ["after first declared remove_file"]),
    }
    source_bound_tests = {
        crash_id: "crates/podway-daemon/tests/phase5_reset_runtime.rs::reset_all_crash_boundaries_resume_once_without_duplicate_effects"
        for crash_id in ("C14", "C15", "C16")
    }
    reset_mappings = reset_crash_boundary_mappings(source_root)
    for crash_id, boundary in reset_mappings.items():
        if source_bound_failpoints[crash_id][1] != [boundary]:
            raise QualificationError("reset crash source and test boundary mappings disagree")
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
            if locator_field == "test" and window["id"] in source_bound_tests and locator != source_bound_tests[window["id"]]:
                raise QualificationError(f"crash {window['id']} test locator drift")
        declared_failpoints = [item.strip() for item in proof["failpoint"].split(",")]
        expected_failpoints = derived_failpoints.get(window["id"])
        if expected_failpoints is None or declared_failpoints != expected_failpoints:
            raise QualificationError(f"crash {window['id']} failpoints differ from controller-derived registry")

def _fuzz_relative_path(value: Any, pattern: str, label: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(pattern, value):
        raise QualificationError(f"{label} path is malformed")
    relative = Path(value)
    if relative.is_absolute() or "\\" in value or ".." in relative.parts or any(part in {"", "."} for part in relative.parts):
        raise QualificationError(f"{label} path is unsafe")
    return value


def _fuzz_regular(root: Path, relative: str, label: str) -> Path:
    supplied = root / relative
    cursor = supplied
    while cursor != root:
        if cursor.is_symlink():
            raise QualificationError(f"{label} is unsafe or absent")
        cursor = cursor.parent
    resolved = supplied.resolve()
    if not resolved.is_relative_to(root) or not resolved.is_file() or resolved.is_symlink():
        raise QualificationError(f"{label} is unsafe or absent")
    return resolved


def validate_fuzz_policy_binding(policy_mode: Any, profile_sha256: Any, limits: Any, profile_data: dict[str, Any]) -> None:
    fuzz_profile = profile_data.get("fuzz") if isinstance(profile_data, dict) else None
    if not isinstance(fuzz_profile, dict) or not isinstance(policy_mode, str) or policy_mode not in {"rc", "local_smoke"}:
        raise QualificationError("fuzz receipt policy mode is malformed")
    expected_digest = sha256_bytes(canonical_json(profile_data))
    if profile_sha256 != expected_digest:
        raise QualificationError("fuzz receipt profile digest differs from trusted profile")
    policy = fuzz_profile.get(policy_mode)
    if not isinstance(policy, dict):
        raise QualificationError("trusted fuzz policy is malformed")
    expected_limits = _fuzz_limits(profile_data, policy)
    if limits != expected_limits:
        raise QualificationError("fuzz receipt limits drift")
def validate_qualification_source(source: Any, *, include_paths: bool) -> dict[str, Any]:
    tool_fields = {"id", "version", "path_sha256", "path"} if include_paths else {"id", "version", "path_sha256"}
    if (
        not isinstance(source, dict)
        or set(source) != {"commit", "tree", "tools"}
        or not all(isinstance(source.get(key), str) and re.fullmatch(r"[0-9a-f]{40}", source[key]) for key in ("commit", "tree"))
        or not isinstance(source.get("tools"), list)
        or [item.get("id") if isinstance(item, dict) else None for item in source["tools"]] != ["cargo", "rustc"]
        or any(
            not isinstance(item, dict)
            or set(item) != tool_fields
            or not isinstance(item.get("version"), str)
            or not item["version"]
            or not isinstance(item.get("path_sha256"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", item["path_sha256"])
            or (include_paths and (not isinstance(item.get("path"), str) or not Path(item["path"]).is_absolute()))
            for item in source["tools"]
        )
    ):
        raise QualificationError("qualification source provenance is malformed")
    return {
        "commit": source["commit"],
        "tree": source["tree"],
        "tools": [
            {"id": item["id"], "version": item["version"], "path_sha256": item["path_sha256"]}
            for item in source["tools"]
        ],
    }

FUZZ_TOOL_IDS = ("rustc", "cargo", "cargo-fuzz")

def _validate_fuzz_toolchain_records(toolchain: Any, profile_toolchain: dict[str, Any]) -> list[dict[str, Any]]:
    if (
        not isinstance(toolchain, dict)
        or set(toolchain) != {"channel", "rustc", "tools"}
        or toolchain.get("channel") != profile_toolchain.get("channel")
        or toolchain.get("rustc") != profile_toolchain.get("rustc")
        or not isinstance(toolchain.get("tools"), list)
        or len(toolchain["tools"]) != len(FUZZ_TOOL_IDS)
        or any(
            not isinstance(tool, dict)
            or set(tool) != {"id", "path", "sha256"}
            or not isinstance(tool.get("path"), str)
            or not Path(tool["path"]).is_absolute()
            or not isinstance(tool.get("sha256"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", tool["sha256"])
            for tool in toolchain["tools"]
        )
        or tuple(tool["id"] for tool in toolchain["tools"]) != FUZZ_TOOL_IDS
    ):
        raise QualificationError("fuzz receipt toolchain binding differs")
    return toolchain["tools"]

def _validate_fuzz_manifest_binding(
    manifest: Any, receipt: dict[str, Any], target: str, corpus: str, phase: str,
) -> list[dict[str, Any]]:
    if not isinstance(manifest, dict) or set(manifest) != {
        "schema", "host", "target", "phase", "corpus", "files", "run_identity",
    }:
        raise QualificationError("fuzz corpus manifest schema is incomplete")
    files = manifest["files"]
    if (
        manifest["schema"] != "podway.g009.fuzz-corpus-manifest/v1"
        or manifest["target"] != target
        or manifest["phase"] != phase
        or manifest["corpus"] != corpus
        or manifest["host"] != receipt["host"]
        or manifest["run_identity"] != receipt["run_identity"]
        or not isinstance(files, list)
    ):
        raise QualificationError("fuzz corpus manifest binding differs")
    return files
def _require_unique_bundled_fuzz_corpus(corpus: Any, corpora: set[str]) -> str:
    canonical = _fuzz_relative_path(
        corpus, r"artifacts/g009/fuzz/corpus/[^/]+", "bundled fuzz corpus",
    )
    if canonical in corpora:
        raise QualificationError("bundled fuzz corpus is reused")
    corpora.add(canonical)
    return canonical


def _validate_fuzz_receipt_core(receipt: Any, target: str, native_target: str) -> dict[str, Any]:
    """Content-source-independent receipt semantics shared by filesystem and ZIP proofs."""
    required = {
        "schema", "host", "policy_mode", "profile_sha256", "native_target", "target", "corpus", "argv", "limits", "execution",
        "stdout", "stderr", "corpus_manifests", "seed_manifest", "initialization", "terminal_mode", "termination_reason",
        "exit_code", "signal", "timeout", "status", "provenance", "run_identity",
    }
    if not isinstance(receipt, dict) or set(receipt) != required or receipt.get("schema") != "podway.g009.fuzz-receipt/v2":
        raise QualificationError("fuzz receipt schema is incomplete")
    if (
        receipt.get("host") != host_manifest()
        or receipt.get("target") != target
        or receipt.get("native_target") != native_target
    ):
        raise QualificationError("fuzz receipt host, surface, or native-target binding differs")
    profile_data = trusted_fuzz_profile(native_target)
    validate_fuzz_policy_binding(receipt.get("policy_mode"), receipt.get("profile_sha256"), receipt.get("limits"), profile_data)
    seed = fuzz_seeds(profile_data)[FUZZ_TARGETS.index(target)]
    expected_seed = {"profile_sha256": receipt["profile_sha256"], "seeds": [{"name": seed["name"], "target": target, "path": f"00-{seed['name']}.seed", "sha256": seed["sha256"], "bytes": len(seed["bytes"])}]}
    if receipt.get("seed_manifest") != expected_seed or receipt.get("initialization") != {"seed_corpus_files": 1, "requires_captured_output": True}:
        raise QualificationError("fuzz receipt seed/initialization proof differs")
    limits = receipt["limits"]
    execution = receipt.get("execution")
    if (
        not isinstance(execution, dict)
        or set(execution) != {"budget_seconds", "elapsed_ns", "completed", "binary_sha256", "argv"}
        or execution.get("budget_seconds") != limits["max_total_time"]
        or not isinstance(execution.get("elapsed_ns"), int)
        or isinstance(execution["elapsed_ns"], bool)
        or execution["elapsed_ns"] < limits["max_total_time"] * 1_000_000_000
        or execution.get("completed") is not True
        or not isinstance(execution.get("binary_sha256"), str)
        or not re.fullmatch(r"[0-9a-f]{64}", execution["binary_sha256"])
        or execution.get("argv") != [
            f"sha256:{execution['binary_sha256']}",
            f"controller/{receipt.get('corpus')}",
            f"-max_total_time={limits['max_total_time']}",
            f"-timeout={limits['timeout_seconds']}",
            f"-rss_limit_mb={limits['rss_limit_mb']}",
        ]
    ):
        raise QualificationError("fuzz receipt execution-budget proof differs")
    argv = receipt.get("argv")
    if not isinstance(argv, list) or argv[:4] != ["cargo", "fuzz", "run", target] or argv[-3:] != [f"-max_total_time={limits['max_total_time']}", f"-timeout={limits['timeout_seconds']}", f"-rss_limit_mb={limits['rss_limit_mb']}"]:
        raise QualificationError("fuzz receipt argv binding differs")
    provenance = receipt.get("provenance")
    if (
        not isinstance(provenance, dict)
        or set(provenance)
        != {
            "source", "profile_sha256", "toolchain", "sources",
            "candidate_source_manifest", "active_lockfile",
        }
        or provenance.get("profile_sha256") != receipt["profile_sha256"]
    ):
        raise QualificationError("fuzz receipt provenance is incomplete")
    corpus = _fuzz_relative_path(receipt["corpus"], r"artifacts/g009/fuzz/corpus/[^/]+", "fuzz corpus")
    if receipt.get("argv") != [
        "cargo", "fuzz", "run", target, f"controller/{corpus}", "--",
        f"-max_total_time={receipt['limits']['max_total_time']}",
        f"-timeout={receipt['limits']['timeout_seconds']}",
        f"-rss_limit_mb={receipt['limits']['rss_limit_mb']}",
    ]:
        raise QualificationError("fuzz receipt argv binding differs")
    fuzz_profile = profile_data.get("fuzz")
    if not isinstance(fuzz_profile, dict) or not isinstance(fuzz_profile.get("toolchain"), dict):
        raise QualificationError("trusted fuzz profile is malformed")
    _validate_fuzz_toolchain_records(provenance["toolchain"], fuzz_profile["toolchain"])
    return receipt
def validate_fuzz_receipt(path: Path, evidence_root: Path, expected_source: dict[str, Any] | None = None, expected_native_target: str | None = None) -> None:
    root = evidence_root.resolve()
    if evidence_root.is_symlink() or not root.is_dir() or path.is_symlink():
        raise QualificationError("fuzz receipt path is unsafe or absent")
    receipt_path = path.resolve()
    if not receipt_path.is_relative_to(root) or not receipt_path.is_file():
        raise QualificationError("fuzz receipt path is unsafe or absent")
    receipt = load_json(receipt_path)
    required = {
        "schema", "host", "policy_mode", "profile_sha256", "native_target", "target", "corpus", "argv", "limits", "execution", "stdout", "stderr",
        "corpus_manifests", "seed_manifest", "initialization", "terminal_mode", "termination_reason", "exit_code",
        "signal", "timeout", "status", "provenance", "run_identity",
    }
    if not isinstance(receipt, dict) or set(receipt) != required:
        raise QualificationError("legacy fuzz receipt without an explicit native target is unsupported")
    if receipt["schema"] != "podway.g009.fuzz-receipt/v2":
        raise QualificationError("unsupported fuzz receipt version")
    if receipt["host"] != host_manifest():
        raise QualificationError("fuzz receipt host binding differs")
    if not isinstance(receipt["run_identity"], str) or not re.fullmatch(
        r"[0-9a-f]{64}", receipt["run_identity"]
    ):
        raise QualificationError("fuzz receipt run identity is malformed")
    target = receipt["target"]
    if target not in FUZZ_TARGETS:
        raise QualificationError("fuzz receipt target is malformed")
    native_target = receipt.get("native_target")
    profile_data = trusted_fuzz_profile(native_target)
    _validate_fuzz_receipt_core(receipt, target, native_target)
    if expected_native_target is not None and native_target != expected_native_target:
        raise QualificationError("fuzz receipt native target differs from gate evidence")
    fuzz_profile = profile_data.get("fuzz") if isinstance(profile_data, dict) else None
    if not isinstance(fuzz_profile, dict) or not isinstance(fuzz_profile.get("toolchain"), dict):
        raise QualificationError("trusted fuzz profile is malformed")
    policy_mode, profile_toolchain = receipt["policy_mode"], fuzz_profile["toolchain"]
    validate_fuzz_policy_binding(policy_mode, receipt["profile_sha256"], receipt["limits"], profile_data)
    profile_seeds = fuzz_seeds(profile_data)
    seed = profile_seeds[FUZZ_TARGETS.index(target)]
    expected_seed = {
        "profile_sha256": receipt["profile_sha256"],
        "seeds": [{"name": seed["name"], "target": target, "path": f"00-{seed['name']}.seed", "sha256": seed["sha256"], "bytes": len(seed["bytes"])}],
    }
    if receipt.get("seed_manifest") != expected_seed:
        raise QualificationError("fuzz seed manifest/profile binding differs")
    if receipt.get("initialization") != {"seed_corpus_files": 1, "requires_captured_output": True}:
        raise QualificationError("fuzz initialization proof declaration is malformed")
    expected_limits = receipt["limits"]
    corpus = receipt["corpus"]
    candidate = candidate_root()
    assert candidate is not None
    corpus_path = candidate / corpus
    if receipt["limits"] != expected_limits:
        raise QualificationError("fuzz receipt limits drift")
    execution = receipt["execution"]
    budget_ns = expected_limits["max_total_time"] * 1_000_000_000
    if (
        not isinstance(execution, dict)
        or set(execution) != {"budget_seconds", "elapsed_ns", "completed", "binary_sha256", "argv"}
        or execution["budget_seconds"] != expected_limits["max_total_time"]
        or not isinstance(execution["elapsed_ns"], int)
        or isinstance(execution["elapsed_ns"], bool)
        or execution["elapsed_ns"] < budget_ns
        or execution["completed"] is not True
        or not isinstance(execution["binary_sha256"], str)
        or not re.fullmatch(r"[0-9a-f]{64}", execution["binary_sha256"])
        or execution.get("argv") != [
            f"sha256:{execution['binary_sha256']}",
            f"controller/{corpus}",
            f"-max_total_time={expected_limits['max_total_time']}",
            f"-timeout={expected_limits['timeout_seconds']}",
            f"-rss_limit_mb={expected_limits['rss_limit_mb']}",
        ]
    ):
        raise QualificationError("fuzz receipt lacks exact completed execution-budget proof")
    cursor = corpus_path
    while cursor != candidate:
        if cursor.is_symlink():
            raise QualificationError("fuzz corpus binding differs")
        cursor = cursor.parent
    if not corpus_path.is_dir() or not corpus_path.resolve().is_relative_to(candidate) or corpus_path.parent.resolve() != (candidate / fuzz_profile.get("corpus_root", "")).resolve():
        raise QualificationError("fuzz corpus binding differs")
    streams = [receipt["stdout"], receipt["stderr"]]
    if any(not isinstance(item, dict) or set(item) != {"path", "bytes", "sha256", "overflow"} for item in streams):
        raise QualificationError("fuzz receipt blob binding is incomplete")
    total = 0
    captured_output = bytearray()
    for stream in streams:
        relative, size, digest = stream["path"], stream["bytes"], stream["sha256"]
        _fuzz_relative_path(relative, r"fuzz/blobs/[0-9a-f]{64}\.bin", "fuzz blob")
        if (not isinstance(size, int) or isinstance(size, bool) or size < 0 or size > expected_limits["stream_bytes"]
                or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
                or not isinstance(stream["overflow"], bool) or relative != f"fuzz/blobs/{digest}.bin"):
            raise QualificationError("fuzz blob metadata is malformed")
        blob = _fuzz_regular(root, relative, "fuzz blob")
        if blob.stat().st_size != size or sha256_file(blob) != digest:
            raise QualificationError("fuzz blob hash/size binding differs")
        captured_output.extend(bounded_bytes(blob, expected_limits["stream_bytes"]))
        total += size
    if total > expected_limits["aggregate_bytes"]:
        raise QualificationError("fuzz blobs exceed aggregate capture limit")
    manifests = receipt["corpus_manifests"]
    if not isinstance(manifests, dict) or set(manifests) != {"before", "after"}:
        raise QualificationError("fuzz corpus manifest binding is incomplete")
    seen_manifest_digests: set[str] = set()
    manifest_files: dict[str, list[dict[str, Any]]] = {}
    for phase in ("before", "after"):
        binding = manifests[phase]
        if not isinstance(binding, dict) or set(binding) != {"path", "sha256", "bytes"}:
            raise QualificationError("fuzz corpus manifest reference is incomplete")
        relative, digest, manifest_size = binding.get("path"), binding.get("sha256"), binding.get("bytes")
        _fuzz_relative_path(relative, r"fuzz-corpus-manifests/[0-9a-f]{64}\.json", "fuzz corpus manifest")
        if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest) or relative != f"fuzz-corpus-manifests/{digest}.json":
            raise QualificationError("fuzz corpus manifest reference is malformed")
        if (not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
                or not isinstance(manifest_size, int) or isinstance(manifest_size, bool)
                or manifest_size < 0 or manifest_size > expected_limits["manifest_bytes"]
                or relative != f"fuzz-corpus-manifests/{digest}.json"):
            raise QualificationError("fuzz corpus manifest reference is malformed")
        manifest_path = _fuzz_regular(root, relative, "fuzz corpus manifest")
        if manifest_path.stat().st_size != manifest_size or sha256_file(manifest_path) != digest or digest in seen_manifest_digests:
            raise QualificationError("fuzz corpus manifest digest binding differs")
        seen_manifest_digests.add(digest)
        manifest = load_json_bytes(bounded_bytes(manifest_path, expected_limits["manifest_bytes"]), relative)
        files = _validate_fuzz_manifest_binding(manifest, receipt, target, corpus, phase)
        members: list[str] = []
        aggregate_members = 0
        for member in files:
            if not isinstance(member, dict) or set(member) != {"path", "sha256", "bytes"}:
                raise QualificationError("fuzz corpus manifest member is malformed")
            member_path, member_digest, member_size = member["path"], member["sha256"], member["bytes"]
            _fuzz_relative_path(member_path, r"[^/]+(?:/[^/]+)*", "fuzz corpus member")
            if (not isinstance(member_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", member_digest)
                    or not isinstance(member_size, int) or isinstance(member_size, bool) or member_size < 0
                    or member_size > expected_limits["corpus_member_bytes"]
                    or len(member_path) > expected_limits["corpus_path_length"]
                    or len(Path(member_path).parts) > expected_limits["corpus_path_depth"]):
                raise QualificationError("fuzz corpus manifest member is malformed")
            members.append(member_path)
            if len(members) > expected_limits["corpus_member_count"]:
                raise QualificationError("fuzz corpus member count exceeds frozen limit")
            aggregate_members += member_size
            if aggregate_members > expected_limits["corpus_aggregate_bytes"]:
                raise QualificationError("fuzz corpus aggregate exceeds frozen limit")
            if phase == "after":
                current = _fuzz_regular(corpus_path, member_path, "current fuzz corpus member")
                if current.stat().st_size != member_size or sha256_file(current) != member_digest:
                    raise QualificationError("current fuzz corpus member differs from manifest")
        if members != sorted(members) or len(members) != len(set(members)):
            raise QualificationError("fuzz corpus manifest members are not unique and sorted")
        manifest_files[phase] = files
        if phase == "after":
            current_members = _bounded_corpus_members(corpus_path, expected_limits)
            if current_members != members:
                raise QualificationError("current fuzz corpus membership differs from manifest")
    expected_before_files = [{"path": expected_seed["seeds"][0]["path"], "sha256": seed["sha256"], "bytes": len(seed["bytes"])}]
    if manifest_files.get("before") != expected_before_files:
        raise QualificationError("fuzz before manifest is empty or differs from exact frozen seeds")
    before_members = {(member["path"], member["sha256"], member["bytes"]) for member in manifest_files["before"]}
    after_members = {(member["path"], member["sha256"], member["bytes"]) for member in manifest_files.get("after", [])}
    if not before_members.issubset(after_members):
        raise QualificationError("fuzz corpus membership is not monotonic from seeded before manifest")
    if (
        any(stream["overflow"] for stream in streams)
        or not re.search(rb"INFO:.*seed corpus: files: 1\b", bytes(captured_output))
        or not re.search(rb"#\d+\s+INITED\b", bytes(captured_output))
        or not re.search(rb"#\d+\s+DONE\b", bytes(captured_output))
    ):
        raise QualificationError(
            "fuzz captured output lacks grounded seed initialization and completed execution proof"
        )
    reason, terminal = receipt["termination_reason"], receipt["terminal_mode"]
    if reason not in {"completed", "timeout", "output_overflow", "post_kill_drain_timeout"} or terminal not in {
        "success", "nonzero_exit", "signal", "timeout", "output_overflow", "post_kill_drain_timeout",
    } or not isinstance(receipt["timeout"], bool):
        raise QualificationError("fuzz terminal receipt is malformed")
    if (reason == "timeout") != receipt["timeout"]:
        raise QualificationError("fuzz timeout binding differs")
    exit_code, signal = receipt["exit_code"], receipt["signal"]
    if not ((isinstance(exit_code, int) and not isinstance(exit_code, bool) and exit_code >= 0 and signal is None)
            or (exit_code is None and isinstance(signal, int) and not isinstance(signal, bool) and signal > 0)):
        raise QualificationError("fuzz exit status is malformed")
    derived_terminal = (
        "success" if reason == "completed" and exit_code == 0 else
        "nonzero_exit" if reason == "completed" and isinstance(exit_code, int) and exit_code > 0 else
        "signal" if reason == "completed" and signal is not None else reason
    )
    if terminal != derived_terminal:
        raise QualificationError("fuzz terminal mode binding differs")
    if (reason == "output_overflow") != any(stream["overflow"] for stream in streams):
        raise QualificationError("fuzz stream overflow reason differs")
    if receipt["status"] != ("pass" if terminal == "success" else "fail"):
        raise QualificationError("fuzz receipt status is incomplete")
    if receipt["status"] == "pass" and receipt["execution"]["completed"] is not True:
        raise QualificationError("passing fuzz receipt did not complete execution")
    provenance = receipt["provenance"]
    if (
        not isinstance(provenance, dict)
        or set(provenance)
        != {
            "source", "profile_sha256", "toolchain", "sources",
            "candidate_source_manifest", "active_lockfile",
        }
    ):
        raise QualificationError("fuzz receipt provenance is incomplete")
    source = provenance["source"]
    validate_qualification_source(source, include_paths=True)
    for tool in source["tools"]:
        tool_path = Path(tool["path"])
        if tool_path.is_symlink() or not tool_path.is_file() or sha256_file(tool_path) != tool["path_sha256"]:
            raise QualificationError("fuzz receipt source tool binding differs")
    if expected_source is not None and source != expected_source:
        raise QualificationError("fuzz receipt source differs from gate evidence")
    if provenance["profile_sha256"] != receipt["profile_sha256"]:
        raise QualificationError("fuzz receipt provenance/profile digest differs")
    source_manifest = provenance["candidate_source_manifest"]
    if (
        not isinstance(source_manifest, dict)
        or set(source_manifest) != {"sha256", "entries"}
        or not isinstance(source_manifest["sha256"], str)
        or not re.fullmatch(r"[0-9a-f]{64}", source_manifest["sha256"])
        or not isinstance(source_manifest["entries"], int)
        or isinstance(source_manifest["entries"], bool)
        or source_manifest["entries"] < 1
        or source_manifest != _candidate_source_manifest()
    ):
        raise QualificationError("fuzz receipt candidate source manifest differs")
    if provenance["active_lockfile"] != _active_fuzz_lockfile():
        raise QualificationError("fuzz receipt active lockfile binding differs")
    toolchain, sources = provenance["toolchain"], provenance["sources"]
    for tool in _validate_fuzz_toolchain_records(toolchain, profile_toolchain):
        tool_path = Path(tool["path"])
        if tool_path.is_symlink() or not tool_path.is_file() or sha256_file(tool_path) != tool["sha256"]:
            raise QualificationError("fuzz receipt tool digest binding is malformed")
    expected_sources = {
        *({("candidate", "Cargo.lock"), ("candidate", "fuzz/Cargo.lock"), ("candidate", "fuzz/Cargo.toml")}),
        *(("candidate", f"fuzz/fuzz_targets/{name}.rs") for name in FUZZ_TARGETS),
        ("controller", "tools/run_g009_qualification.py"), ("controller", "tools/g009_common.py"),
    }
    if not isinstance(sources, list) or len(sources) != len(expected_sources):
        raise QualificationError("fuzz receipt source set is incomplete")
    actual_sources: set[tuple[str, str]] = set()
    for item in sources:
        if (not isinstance(item, dict) or set(item) != {"root", "path", "sha256"} or item.get("root") not in {"candidate", "controller"}
                or not isinstance(item.get("path"), str) or not isinstance(item.get("sha256"), str) or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])):
            raise QualificationError("fuzz receipt source digest binding is malformed")
        source_root = candidate if item["root"] == "candidate" else ROOT
        source_path = _fuzz_regular(source_root, item["path"], "fuzz source")
        if sha256_file(source_path) != item["sha256"]:
            raise QualificationError("fuzz receipt source binding differs")
        actual_sources.add((item["root"], item["path"]))
    if actual_sources != expected_sources:
        raise QualificationError("fuzz receipt source set is incomplete")
    active_lockfile = provenance["active_lockfile"]
    active_lock_sources = [
        item for item in sources
        if item["root"] == "candidate"
        and item["path"] == active_lockfile["path"]
    ]
    if (
        len(active_lock_sources) != 1
        or active_lock_sources[0]["sha256"] != active_lockfile["sha256"]
    ):
        raise QualificationError("active fuzz lockfile is not bound by provenance sources")


def _validate_fuzz_gate_envelope(payload: Any) -> dict[str, Any]:
    """Shared semantic gate contract for filesystem and immutable ZIP evidence."""
    if (
        not isinstance(payload, dict)
        or set(payload) != {
            "schema", "gate_id", "host", "policy_mode", "profile_sha256", "native_target",
            "provenance", "commands", "run_identity", "status",
        }
        or payload.get("gate_id") != "G009-GATE-FUZZ"
        or payload.get("schema") != "podway.g009.checkpoint/v1"
        or payload.get("host") != host_manifest()
        or payload.get("policy_mode") not in FUZZ_POLICY_MODES
        or payload.get("status") != "pass"
        or not isinstance(payload.get("run_identity"), str)
        or not re.fullmatch(r"[0-9a-f]{64}", payload["run_identity"])
        or not isinstance(payload.get("commands"), list)
        or [item.get("target") if isinstance(item, dict) else None for item in payload["commands"]] != list(FUZZ_TARGETS)
        or not isinstance(payload.get("provenance"), dict)
        or payload["provenance"].get("profile_sha256") != payload["profile_sha256"]
    ):
        raise QualificationError("fuzz gate payload is malformed")
    profile_data = trusted_fuzz_profile(payload.get("native_target"))
    policy = profile_data["fuzz"][payload["policy_mode"]]
    validate_fuzz_policy_binding(
        payload["policy_mode"], payload["profile_sha256"],
        _fuzz_limits(profile_data, policy), profile_data,
    )
    return payload
def validate_fuzz_gate(payload: dict[str, Any], evidence_root: Path) -> None:
    payload = _validate_fuzz_gate_envelope(payload)
    native_target = payload["native_target"]
    profile_data = trusted_fuzz_profile(native_target)
    commands, provenance = payload["commands"], payload["provenance"]
    if (not isinstance(provenance, dict) or not isinstance(provenance.get("source"), dict)
            or provenance.get("profile_sha256") != payload["profile_sha256"]):
        raise QualificationError("fuzz gate provenance is incomplete")
    if not isinstance(commands, list) or len(commands) != len(FUZZ_TARGETS) or [item.get("target") for item in commands if isinstance(item, dict)] != list(FUZZ_TARGETS):
        raise QualificationError("fuzz gate lacks every target receipt")
    seen_receipts: set[str] = set()
    seen_corpora: set[str] = set()
    for command in commands:
        if not isinstance(command, dict) or set(command) != {"target", "corpus", "receipt", "status"}:
            raise QualificationError("fuzz gate command receipt is malformed")
        binding = command["receipt"]
        if not isinstance(binding, dict) or set(binding) != {"path", "sha256", "status", "target"}:
            raise QualificationError("fuzz receipt reference is incomplete")
        relative = _fuzz_relative_path(binding.get("path"), r"fuzz-receipts/[0-9a-f]{64}\.json", "fuzz receipt reference")
        if (not isinstance(binding.get("sha256"), str) or not re.fullmatch(r"[0-9a-f]{64}", binding["sha256"])
                or relative != f"fuzz-receipts/{binding['sha256']}.json" or binding["target"] != command["target"]):
            raise QualificationError("fuzz receipt reference is malformed")
        receipt_path = _fuzz_regular(evidence_root.resolve(), relative, "fuzz receipt reference")
        if sha256_file(receipt_path) != binding["sha256"] or binding["sha256"] in seen_receipts:
            raise QualificationError("fuzz receipt reference is unbound or duplicated")
        seen_receipts.add(binding["sha256"])
        validate_fuzz_receipt(receipt_path, evidence_root, provenance["source"], native_target)
        receipt = load_json(receipt_path)
        if receipt["corpus"] in seen_corpora:
            raise QualificationError("fuzz receipt corpus is reused")
        seen_corpora.add(receipt["corpus"])
        if (receipt["policy_mode"] != payload["policy_mode"] or receipt["profile_sha256"] != payload["profile_sha256"]
                or receipt["native_target"] != native_target or receipt["provenance"] != provenance
                or receipt["run_identity"] != payload["run_identity"]
                or command["corpus"] != receipt["corpus"] or command["target"] != receipt["target"]
                or binding["target"] != receipt["target"] or binding["status"] != receipt["status"]
                or command["status"] != receipt["status"]):
            raise QualificationError("fuzz receipt command binding differs")
        if (
            command["status"] != "pass"
            or binding["status"] != "pass"
            or receipt["status"] != "pass"
            or receipt["terminal_mode"] != "success"
            or receipt["termination_reason"] != "completed"
            or receipt["exit_code"] != 0
            or receipt["signal"] is not None
            or receipt["timeout"] is not False
            or receipt["execution"]["completed"] is not True
        ):
            raise QualificationError("live fuzz gate contains a non-passing receipt")
def _require_bundled_fuzz_pass_status(
    command: dict[str, Any], receipt_ref: dict[str, Any], receipt: dict[str, Any],
) -> None:
    if (
        command.get("status") != "pass"
        or receipt_ref.get("status") != "pass"
        or receipt.get("status") != "pass"
    ):
        raise QualificationError("bundled fuzz receipt statuses must all be pass")

def validate_bundled_fuzz_dependencies(payload: dict[str, Any], bundle: zipfile.ZipFile, expected_run_identity: str) -> set[str]:
    """Validate the immutable fuzz proof with the same gate contract as live evidence."""
    _preflight_qualification_bundle(bundle)
    payload = _validate_fuzz_gate_envelope(payload)
    if payload.get("run_identity") != expected_run_identity:
        raise QualificationError("bundled fuzz gate uses a different invocation identity")
    native_target = payload["native_target"]
    required_receipt = {
        "schema", "host", "policy_mode", "profile_sha256", "native_target", "target", "corpus", "argv", "limits", "execution",
        "stdout", "stderr", "corpus_manifests", "seed_manifest", "initialization", "terminal_mode",
        "termination_reason", "exit_code", "signal", "timeout", "status", "provenance", "run_identity",
    }
    expected: set[str] = set()
    identities: set[str] = set()
    corpora: set[str] = set()
    for command in payload["commands"]:
        receipt_ref = command.get("receipt") if isinstance(command, dict) else None
        receipt_path = receipt_ref.get("path") if isinstance(receipt_ref, dict) else None
        receipt_digest = receipt_ref.get("sha256") if isinstance(receipt_ref, dict) else None
        if (
            not isinstance(command, dict) or set(command) != {"target", "corpus", "receipt", "status"}
            or not isinstance(receipt_ref, dict) or set(receipt_ref) != {"path", "sha256", "status", "target"}
            or receipt_ref.get("target") != command.get("target") or receipt_ref.get("status") != command.get("status")
            or not isinstance(receipt_path, str) or not isinstance(receipt_digest, str)
            or not re.fullmatch(r"fuzz-receipts/[0-9a-f]{64}\.json", receipt_path)
            or receipt_path != f"fuzz-receipts/{receipt_digest}.json"
        ):
            raise QualificationError("bundled fuzz receipt reference is malformed")
        raw = bundle.read(f"evidence/{receipt_path}")
        if sha256_bytes(raw) != receipt_digest:
            raise QualificationError("bundled fuzz receipt digest differs")
        expected.add(f"evidence/{receipt_path}")
        receipt = load_json_bytes(raw, receipt_path)
        if not isinstance(receipt, dict) or set(receipt) != required_receipt or receipt.get("schema") != "podway.g009.fuzz-receipt/v2":
            raise QualificationError("bundled fuzz receipt schema is incomplete")
        if (
            receipt.get("target") != command["target"] or receipt.get("status") != "pass"
            or receipt.get("native_target") != native_target
            or receipt.get("policy_mode") != payload["policy_mode"]
            or receipt.get("profile_sha256") != payload["profile_sha256"]
            or receipt.get("provenance") != payload["provenance"]
            or receipt.get("run_identity") != payload["run_identity"]
            or receipt.get("corpus") != command["corpus"]
        ):
            raise QualificationError("bundled fuzz receipt binding differs")
        _require_bundled_fuzz_pass_status(command, receipt_ref, receipt)
        _validate_fuzz_receipt_core(receipt, command["target"], native_target)
        corpus = _require_unique_bundled_fuzz_corpus(receipt["corpus"], corpora)
        source = receipt["provenance"]["source"]
        validate_qualification_source(source, include_paths=True)
        provenance_sources = receipt["provenance"].get("sources")
        expected_sources = {
            ("candidate", "Cargo.lock"), ("candidate", "fuzz/Cargo.lock"), ("candidate", "fuzz/Cargo.toml"),
            *(("candidate", f"fuzz/fuzz_targets/{name}.rs") for name in FUZZ_TARGETS),
            ("controller", "tools/run_g009_qualification.py"), ("controller", "tools/g009_common.py"),
        }
        if not isinstance(provenance_sources, list) or len(provenance_sources) != len(expected_sources):
            raise QualificationError("bundled fuzz source set is incomplete")
        actual_sources: set[tuple[str, str]] = set()
        for item in provenance_sources:
            if (
                not isinstance(item, dict) or set(item) != {"root", "path", "sha256"}
                or item.get("root") not in {"candidate", "controller"}
                or not isinstance(item.get("path"), str) or not isinstance(item.get("sha256"), str)
                or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])
            ):
                raise QualificationError("bundled fuzz source digest binding is malformed")
            relative = _fuzz_relative_path(item["path"], r"[^/]+(?:/[^/]+)*", "bundled fuzz source")
            source_name = f"fuzz-sources/{item['root']}/{relative}"
            data = bundle.read(source_name)
            if sha256_bytes(data) != item["sha256"]:
                raise QualificationError("bundled fuzz source binding differs")
            expected.add(source_name)
            actual_sources.add((item["root"], item["path"]))
        if actual_sources != expected_sources:
            raise QualificationError("bundled fuzz source set is incomplete")
        active_lockfile = receipt["provenance"].get("active_lockfile")
        active_lock_sources = [
            item for item in provenance_sources
            if isinstance(item, dict) and item.get("root") == "candidate"
            and item.get("path") == "fuzz/Cargo.lock"
        ]
        if (
            not isinstance(active_lockfile, dict)
            or set(active_lockfile) != {"path", "sha256"}
            or active_lockfile.get("path") != "fuzz/Cargo.lock"
            or not isinstance(active_lockfile.get("sha256"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", active_lockfile["sha256"])
            or len(active_lock_sources) != 1
            or active_lock_sources[0].get("sha256") != active_lockfile["sha256"]
        ):
            raise QualificationError("bundled fuzz active lockfile binding differs")
        toolchain = receipt["provenance"]["toolchain"]
        for tool in [*source["tools"], *toolchain["tools"]]:
            if not isinstance(tool, dict):
                raise QualificationError("bundled fuzz tool digest binding is malformed")
            digest_key = "path_sha256" if "version" in tool else "sha256"
            required_tool_fields = {"id", "version", "path", "path_sha256"} if digest_key == "path_sha256" else {"id", "path", "sha256"}
            if (
                set(tool) != required_tool_fields
                or not isinstance(tool.get("path"), str) or not Path(tool["path"]).is_absolute()
                or not isinstance(tool.get(digest_key), str) or not re.fullmatch(r"[0-9a-f]{64}", tool[digest_key])
            ):
                raise QualificationError("bundled fuzz tool digest binding is malformed")
        identity = receipt.get("run_identity")
        if not isinstance(identity, str) or not re.fullmatch(r"[0-9a-f]{64}", identity):
            raise QualificationError("bundled fuzz receipt run identity is malformed")
        identities.add(identity)
        captured_output = bytearray()
        for stream_name in ("stdout", "stderr"):
            stream = receipt.get(stream_name)
            if not isinstance(stream, dict) or set(stream) != {"path", "bytes", "sha256", "overflow"}:
                raise QualificationError("bundled fuzz blob reference is malformed")
            name, digest = stream.get("path"), stream.get("sha256")
            if (
                not isinstance(name, str) or not isinstance(digest, str)
                or not isinstance(stream.get("bytes"), int) or isinstance(stream["bytes"], bool)
                or stream["bytes"] < 0 or stream["bytes"] > receipt["limits"]["stream_bytes"]
                or not isinstance(stream.get("overflow"), bool)
                or not re.fullmatch(r"fuzz/blobs/[0-9a-f]{64}\.bin", name)
                or name != f"fuzz/blobs/{digest}.bin" or not re.fullmatch(r"[0-9a-f]{64}", digest)
            ):
                raise QualificationError("bundled fuzz blob metadata is malformed")
            data = bundle.read(name)
            if len(data) != stream["bytes"] or sha256_bytes(data) != digest:
                raise QualificationError("bundled fuzz blob binding differs")
            expected.add(name)
            captured_output.extend(data)
        if sum(receipt[name]["bytes"] for name in ("stdout", "stderr")) > receipt["limits"]["aggregate_bytes"]:
            raise QualificationError("bundled fuzz blobs exceed aggregate capture limit")
        manifests = receipt.get("corpus_manifests")
        if not isinstance(manifests, dict) or set(manifests) != {"before", "after"}:
            raise QualificationError("bundled fuzz manifests are malformed")
        manifest_files: dict[str, list[dict[str, Any]]] = {}
        for phase in ("before", "after"):
            binding = manifests[phase]
            if not isinstance(binding, dict) or set(binding) != {"path", "sha256", "bytes"}:
                raise QualificationError("bundled fuzz manifest reference is malformed")
            name, digest, manifest_size = binding.get("path"), binding.get("sha256"), binding.get("bytes")
            if (
                not isinstance(name, str) or not isinstance(digest, str)
                or not re.fullmatch(r"fuzz-corpus-manifests/[0-9a-f]{64}\.json", name)
                or name != f"fuzz-corpus-manifests/{digest}.json" or not re.fullmatch(r"[0-9a-f]{64}", digest)
            ):
                raise QualificationError("bundled fuzz manifest identity is malformed")
            raw_manifest = bundle.read(name)
            if (not isinstance(manifest_size, int) or isinstance(manifest_size, bool) or manifest_size < 0
                    or manifest_size > receipt["limits"]["manifest_bytes"]
                    or len(raw_manifest) != manifest_size or sha256_bytes(raw_manifest) != digest):
                raise QualificationError("bundled fuzz manifest digest differs")
            manifest = load_json_bytes(raw_manifest, name)
            files = _validate_fuzz_manifest_binding(
                manifest, receipt, receipt["target"], corpus, phase,
            )
            expected.add(name)
            aggregate_members = 0
            for member in files:
                if not isinstance(member, dict) or set(member) != {"path", "sha256", "bytes"}:
                    raise QualificationError("bundled fuzz corpus member is malformed")
                relative, digest = member.get("path"), member.get("sha256")
                if (
                    not isinstance(relative, str) or not re.fullmatch(r"[^/]+(?:/[^/]+)*", relative)
                    or Path(relative).is_absolute() or ".." in Path(relative).parts
                    or not isinstance(member.get("bytes"), int) or isinstance(member["bytes"], bool) or member["bytes"] < 0
                    or member["bytes"] > receipt["limits"]["corpus_member_bytes"]
                    or len(relative) > receipt["limits"]["corpus_path_length"]
                    or len(Path(relative).parts) > receipt["limits"]["corpus_path_depth"]
                    or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
                ):
                    raise QualificationError("bundled fuzz corpus member identity is malformed")
                corpus_name = f"fuzz-corpus/{corpus}/{relative}"
                data = bundle.read(corpus_name)
                if len(data) != member.get("bytes") or sha256_bytes(data) != digest:
                    raise QualificationError("bundled fuzz corpus member differs from manifest")
                expected.add(corpus_name)
                if len(files) > receipt["limits"]["corpus_member_count"]:
                    raise QualificationError("bundled fuzz corpus exceeds member-count limit")
                aggregate_members += member["bytes"]
                if aggregate_members > receipt["limits"]["corpus_aggregate_bytes"]:
                    raise QualificationError("bundled fuzz corpus exceeds aggregate limit")
            member_paths = [member["path"] for member in files]
            if member_paths != sorted(member_paths) or len(member_paths) != len(set(member_paths)):
                raise QualificationError("bundled fuzz corpus manifest members are not unique and sorted")
            manifest_files[phase] = files
        profile_data = trusted_fuzz_profile(native_target)
        seed = fuzz_seeds(profile_data)[FUZZ_TARGETS.index(receipt["target"])]
        expected_before = [{"path": f"00-{seed['name']}.seed", "sha256": seed["sha256"], "bytes": len(seed["bytes"])}]
        if manifest_files.get("before") != expected_before:
            raise QualificationError("bundled fuzz before corpus differs from the exact frozen seed")
        before = {(item["path"], item["sha256"], item["bytes"]) for item in manifest_files["before"]}
        after = {(item["path"], item["sha256"], item["bytes"]) for item in manifest_files.get("after", [])}
        if not before.issubset(after):
            raise QualificationError("bundled fuzz corpus is not monotonic from the seeded before manifest")
        if (
            any(item.get("overflow") for item in (receipt["stdout"], receipt["stderr"]))
            or not re.search(rb"INFO:.*seed corpus: files: 1\b", bytes(captured_output))
            or not re.search(rb"#\d+\s+INITED\b", bytes(captured_output))
            or not re.search(rb"#\d+\s+DONE\b", bytes(captured_output))
            or receipt.get("termination_reason") != "completed"
            or receipt.get("terminal_mode") != "success"
            or receipt.get("exit_code") != 0 or receipt.get("signal") is not None
            or receipt.get("timeout") is not False or receipt.get("status") != "pass"
        ):
            raise QualificationError("bundled fuzz receipt lacks completed INITED/DONE terminal proof")
    if identities != {payload.get("run_identity")}:
        raise QualificationError("bundled fuzz receipts/manifests do not share one run identity")
    return expected
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
        captured = bounded_process(
            ("gpgv", "--status-fd", "1", "--keyring", str(keyring), str(signature), str(payload)),
            cwd=ROOT, env={"PATH": os.environ.get("PATH", "")}, timeout=30,
            stream_limit=1024 * 1024, aggregate_limit=2 * 1024 * 1024,
        )
        if captured["terminal_mode"] == "output_overflow":
            raise QualificationError("reviewer signature output exceeded bounded capture")
        stdout = captured["stdout"].decode("utf-8", "strict")
        valid = [line.split() for line in stdout.splitlines() if line.startswith("[GNUPG:] VALIDSIG ")]
        if captured["terminal_mode"] != "success" or len(valid) != 1 or len(valid[0]) < 12 or valid[0][-1] != expected[role]:
            raise QualificationError(f"reviewer signature is not an exact primary VALIDSIG: {role}")
        actual.add(role)
    if actual != set(expected):
        raise QualificationError("reviewer attestation set is incomplete")

def _is_canonical_absolute_path(value: Any) -> bool:
    return (
        isinstance(value, str)
        and Path(value).is_absolute()
        and str(Path(value)) == value
    )

def _is_isolated_lifecycle_runtime(value: Any) -> bool:
    if not _is_canonical_absolute_path(value):
        return False
    runtime = Path(value)
    return runtime.name == "runtime" and runtime.parent.name.startswith("g009-lifecycle-")


def _validate_lifecycle_identity(identity: Any) -> dict[str, Any]:
    fields = {"uid", "target", "runtime_path", "socket_path"}
    if (
        not isinstance(identity, dict) or set(identity) != fields
        or not isinstance(identity.get("uid"), int) or isinstance(identity["uid"], bool) or identity["uid"] < 0
        or not _is_isolated_lifecycle_runtime(identity.get("runtime_path"))
        or identity.get("target") != f"gui/{identity['uid']}/{LABEL}"
        or identity.get("socket_path") != str(
            Path(identity["runtime_path"]) / f"podway-{identity['uid']}" / "podwayd.sock"
        )
    ):
        raise QualificationError("lifecycle identity envelope is not the exact isolated TMPDIR socket")
    return identity



def _validate_lifecycle_snapshot(snapshot: Any, identity_fields: set[str]) -> None:
    if not isinstance(snapshot, dict) or set(snapshot) != {"artifacts", "canonical_ancestors"}:
        raise QualificationError("lifecycle protected identity snapshot is malformed")
    artifacts, ancestors = snapshot["artifacts"], snapshot["canonical_ancestors"]
    if not isinstance(artifacts, dict) or not artifacts or not isinstance(ancestors, list) or not ancestors:
        raise QualificationError("lifecycle protected identity snapshot is incomplete")
    if any(
        not isinstance(value, dict) or role != value.get("role")
        or set(value) != identity_fields or not isinstance(value.get("role"), str)
        or not _is_canonical_absolute_path(value.get("path"))
        or not isinstance(value.get("sha256"), str) or not re.fullmatch(r"[0-9a-f]{64}", value["sha256"])
        or not isinstance(value.get("mode"), int) or value["mode"] < 0
        or not isinstance(value.get("bytes"), int) or value["bytes"] < 0
        for role, value in artifacts.items()
    ):
        raise QualificationError("lifecycle protected artifact identity is malformed")
    expected_pairs: set[tuple[str, str]] = set()
    for role, identity in artifacts.items():
        cursor = Path(identity["path"]).parent
        while True:
            expected_pairs.add((role, str(cursor)))
            if cursor == cursor.parent:
                break
            cursor = cursor.parent
    actual_pairs = [(item.get("role"), item.get("path")) for item in ancestors if isinstance(item, dict)]
    if (
        len(actual_pairs) != len(set(actual_pairs))
        or set(actual_pairs) != expected_pairs
        or any(not isinstance(item, dict) or set(item) != {"role", "path", "mode", "device", "inode"} or item["role"] not in artifacts
               or not _is_canonical_absolute_path(item.get("path"))
               or not all(isinstance(item[field], int) and item[field] >= 0 for field in ("mode", "device", "inode"))
               for item in ancestors)
    ):
        raise QualificationError("lifecycle canonical ancestor identity is malformed")
    if ancestors != sorted(ancestors, key=lambda item: (item["path"], item["role"])):
        raise QualificationError("lifecycle canonical ancestor identities are not canonical")
def _validate_launchctl_absence(observation: Any, expected_target: str) -> None:
    if not isinstance(observation, dict) or set(observation) != {
        "target", "exit_code", "stdout_sha256", "stderr_sha256", "stderr",
    }:
        raise QualificationError("lifecycle launchctl absence observation is malformed")
    target, exit_code, stdout, stderr = (
        observation["target"], observation["exit_code"], observation["stdout_sha256"], observation["stderr"],
    )
    if (
        not isinstance(target, str) or target != expected_target
        or not isinstance(exit_code, int) or exit_code not in {3, 113}
        or not isinstance(stdout, str) or stdout != sha256_bytes(b"")
        or not isinstance(stderr, str)
        or not isinstance(observation["stderr_sha256"], str)
        or observation["stderr_sha256"] != sha256_bytes(stderr.encode("utf-8"))
    ):
        raise QualificationError("lifecycle launchctl absence tuple is malformed")
    uid = target.split("/")[1]
    accepted = {
        (113, f'Bad request.\nCould not find service "{LABEL}" in domain for user gui: {uid}'),
        (113, f'Bad request.\nCould not find service "{LABEL}" in domain for user gui: {uid}\n'),
        (3, f'Could not find service "{LABEL}" in domain for gui/{uid}'),
        (3, f'Could not find service "{LABEL}" in domain for gui/{uid}\n'),
    }
    if (exit_code, stderr) not in accepted:
        raise QualificationError("lifecycle launchctl absence tuple is not an accepted exact platform observation")
def _reconstruct_lifecycle_command_policy(inputs: dict[str, list[str]]) -> str:
    writable = inputs["writable"]
    protected = inputs["protected"]
    if any(
        any(character in value for character in ('"', "\\", "\n", "\r"))
        for value in (*writable, *protected)
    ):
        raise QualificationError("lifecycle sandbox path cannot be represented safely")
    ancestor_denials = sorted(
        {str(Path(path).parent) for path in protected} - set(writable)
    )
    return (
        "(version 1)(deny default)(allow process-info*)(allow process-fork)(allow file-read*)(allow sysctl-read)"
        + "".join(f'(allow file-write* (subpath "{path}"))' for path in writable)
        + "".join(
            f'(deny file-write* (literal "{path}"))(deny file-write* (subpath "{path}"))'
            f'(deny file-link (literal "{path}"))(deny file-link (subpath "{path}"))'
            for path in protected
        )
        + "".join(
            f'(deny file-write* (literal "{path}"))(deny file-link (literal "{path}"))'
            for path in ancestor_denials
        )
    )

def _validate_lifecycle_policy_probes(probes: Any, controller_profile_sha256: Any) -> None:
    expected_rows = {
        (target, operation)
        for target in ("candidate_root", "staged_publication")
        for operation in ("transient_create_then_delete", "existing_overwrite", "rename", "symlink")
    }
    if (
        not isinstance(probes, dict) or set(probes) != {"service_profile", "command_policy"}
        or not isinstance(controller_profile_sha256, str)
        or probes.get("service_profile", {}).get("policy_sha256") != controller_profile_sha256
    ):
        raise QualificationError("lifecycle policy probe is malformed")
    boundaries = {"service_profile": "staged_service", "command_policy": "command_policy"}
    for name, probe in probes.items():
        expected_fields = {"policy_sha256", "operations"} | ({"inputs"} if name == "command_policy" else set())
        if (
            not isinstance(probe, dict) or set(probe) != expected_fields
            or not isinstance(probe.get("policy_sha256"), str) or not re.fullmatch(r"[0-9a-f]{64}", probe["policy_sha256"])
            or not isinstance(probe.get("operations"), list) or len(probe["operations"]) != len(expected_rows)
            or {(row.get("target_class"), row.get("operation")) for row in probe["operations"] if isinstance(row, dict)} != expected_rows
            or any(
                not isinstance(row, dict) or set(row) != {"boundary", "target_class", "operation", "denied", "errno", "diagnostic"}
                or row["boundary"] != boundaries[name] or row["denied"] is not True
                or not isinstance(row["errno"], int) or row["errno"] not in {1, 13, 30}
                or not isinstance(row["diagnostic"], str) or not row["diagnostic"]
                for row in probe["operations"]
            )
        ):
            raise QualificationError("lifecycle policy probe does not bind its exact boundary matrix")
    command = probes["command_policy"]
    inputs = command["inputs"]
    if (
        not isinstance(inputs, dict) or set(inputs) != {"writable", "protected"}
        or any(not isinstance(inputs[name], list) or not inputs[name] or any(not _is_canonical_absolute_path(path) for path in inputs[name]) for name in ("writable", "protected"))
        or len(set(inputs["writable"])) != len(inputs["writable"])
        or len(set(inputs["protected"])) != len(inputs["protected"])
        or command["policy_sha256"] != sha256_bytes(
            _reconstruct_lifecycle_command_policy(inputs).encode("utf-8")
        )
        or command["policy_sha256"] == probes["service_profile"]["policy_sha256"]
    ):
        raise QualificationError("lifecycle command policy digest is not independently reconstructed")
def _validate_lifecycle_runtime_socket(observation: Any, target_uid: int, expected_runtime: str) -> None:
    fields = {"socket_path", "socket_owner_uid", "socket_mode", "runtime_path", "runtime_owner_uid", "runtime_mode", "target_uid"}
    if (
        not isinstance(observation, dict) or set(observation) != fields
        or observation.get("target_uid") != target_uid
        or observation.get("runtime_path") != expected_runtime
        or not _is_canonical_absolute_path(observation.get("runtime_path"))
        or observation.get("socket_path") != str(Path(expected_runtime) / f"podway-{target_uid}" / "podwayd.sock")
        or any(not isinstance(observation[field], int) or observation[field] < 0 for field in ("socket_owner_uid", "socket_mode", "runtime_owner_uid", "runtime_mode"))
        or observation["socket_owner_uid"] != target_uid
        or observation["runtime_owner_uid"] != target_uid
        or observation["socket_mode"] & 0o077 or observation["runtime_mode"] & 0o077
    ):
        raise QualificationError("lifecycle runtime socket is not bound to the launchctl UID-specific runtime directory")
def _validate_launchctl_binding(observation: Any, plist_path: str, staged_path: str, expected_target: str) -> None:
    fields = {"target", "plist_path", "program", "arguments", "state", "pid", "stdout_sha256", "stderr_sha256", "stdout_base64", "stderr_base64"}
    if (
        not isinstance(observation, dict) or set(observation) != fields
        or observation.get("target") != expected_target
        or observation["plist_path"] != plist_path or observation["program"] != staged_path
        or observation["arguments"] != [staged_path, "--service"] or observation["state"] != "running"
        or not isinstance(observation["pid"], int) or observation["pid"] <= 0
        or not all(isinstance(observation[field], str) and re.fullmatch(r"[0-9a-f]{64}", observation[field]) for field in ("stdout_sha256", "stderr_sha256"))
        or not all(isinstance(observation[field], str) for field in ("stdout_base64", "stderr_base64"))
    ):
        raise QualificationError("lifecycle launchctl binding is malformed")
    try:
        stdout = base64.b64decode(observation["stdout_base64"], validate=True)
        stderr = base64.b64decode(observation["stderr_base64"], validate=True)
        text = stdout.decode("utf-8", "strict")
    except (ValueError, TypeError, UnicodeDecodeError) as exc:
        raise QualificationError("lifecycle launchctl output bytes are malformed") from exc
    if (
        base64.b64encode(stdout).decode("ascii") != observation["stdout_base64"]
        or base64.b64encode(stderr).decode("ascii") != observation["stderr_base64"]
        or sha256_bytes(stdout) != observation["stdout_sha256"]
        or sha256_bytes(stderr) != observation["stderr_sha256"]
        or stderr != b""
        or not all(re.search(pattern, text) for pattern in (
            rf"(?m)^{re.escape(observation['target'])} = \{{$",
            rf"(?m)^\s*path = {re.escape(plist_path)}$",
            rf"(?m)^\s*program = {re.escape(staged_path)}$",
            rf"(?ms)^\s*arguments = \{{\s*{re.escape(staged_path)}\s*--service\s*\}}$",
            r"(?m)^\s*state = running$",
            rf"(?m)^\s*pid = {observation['pid']}$",
        ))
    ):
        raise QualificationError("lifecycle launchctl binding does not reconstruct exact running state")
def _require_lifecycle_digest(actual: Any, expected: str, label: str) -> None:
    if actual != expected:
        raise QualificationError(f"lifecycle {label} digest differs from reconstructed bytes")
def _reconstruct_controller_wrapper(profile_path: Path, podwayd: Path) -> bytes:
    return (
        "#!/bin/sh\n"
        f"exec /usr/bin/sandbox-exec -f {shlex.quote(str(profile_path))} "
        f"{shlex.quote(str(podwayd))} \"$@\"\n"
    ).encode("utf-8")
def _qualification_install_argv(cli_path: str, binding: dict[str, str]) -> list[str]:
    return [
        cli_path, "daemon", binding["route"],
        "--wrapper-path", binding["wrapper_path"],
        "--wrapper-sha256", binding["wrapper_sha256"],
        "--sandbox-profile-path", binding["sandbox_profile_path"],
        "--sandbox-profile-sha256", binding["sandbox_profile_sha256"],
        "--archived-daemon-path", binding["archived_daemon_path"],
        "--archived-daemon-sha256", binding["archived_daemon_sha256"],
    ]


def validate_checkpoint_payload(payload: Any, gate_id: str) -> None:
    base = {"schema", "host", "run_identity", "checkpoint_id", "status", "rc_sha256", "source", "target", "blockers"}
    if (
        not isinstance(payload, dict)
        or payload.get("schema") != "podway.g009.checkpoint/v1"
        or payload.get("host") != host_manifest()
        or not isinstance(payload.get("run_identity"), str)
        or not re.fullmatch(r"[0-9a-f]{64}", payload["run_identity"])
        or payload.get("target") != TARGET
        or payload.get("status") != "pass"
        or payload.get("blockers") != []
    ):
        raise QualificationError("checkpoint is not a current exact evidence envelope")
    validate_qualification_source(payload.get("source"), include_paths=True)
    if gate_id not in GATES:
        expected_fields = {
            "G009-GATE-PREFLIGHT": base | {"profile_sha256", "source_manifest"},
            "G009-GATE-PERFORMANCE": base | {"holdout"},
            "G009-GATE-PACKAGE": base | {"archive", "signing"},
            "G009-GATE-LIFECYCLE": base | {"archive_sha256", "archive_members_sha256", "binaries", "home_isolated", "source_materialization_preserved", "lifecycle_identity", "commands", "lifecycle_sandbox"},
        }.get(gate_id)
        if expected_fields is None or set(payload) != expected_fields or payload.get("checkpoint_id") != gate_id:
            raise QualificationError("checkpoint envelope schema is incomplete")
        if gate_id == "G009-GATE-PREFLIGHT":
            trusted_qualification_profile(payload["target"])
            if payload["profile_sha256"] != sha256_file(QUALIFICATION_PROFILE_PATHS[payload["target"]]):
                raise QualificationError("preflight does not bind the exact qualification profile")
            validate_qualification_source(payload["source_manifest"], include_paths=True)
        elif gate_id == "G009-GATE-PERFORMANCE":
            holdout = payload["holdout"]
            decision = holdout.get("decision") if isinstance(holdout, dict) else None
            workloads = decision.get("workloads") if isinstance(decision, dict) else None
            if (
                not isinstance(holdout, dict)
                or holdout.get("schema") != "podway.g009.characterization/v1"
                or holdout.get("phase") != "holdout"
                or holdout.get("target") != payload.get("target")
                or holdout.get("warmups") != 5 or holdout.get("samples") != 30
                or not isinstance(decision, dict) or set(decision) != {"passed", "workloads"}
                or decision["passed"] is not True
                or not isinstance(workloads, dict) or not workloads
                or any(not isinstance(value, dict) or value.get("passed") is not True for value in workloads.values())
            ):
                raise QualificationError("performance checkpoint is not mechanically derived from a passing holdout decision")
        elif gate_id == "G009-GATE-PACKAGE":
            archive = payload["archive"]
            if not isinstance(archive, dict) or set(archive) != {"archive_sha256", "members"} or not isinstance(archive.get("members"), list) or not isinstance(archive.get("archive_sha256"), str) or not re.fullmatch(r"[0-9a-f]{64}", archive["archive_sha256"]):
                raise QualificationError("package checkpoint archive proof is malformed")
        else:
            binaries, commands, sandbox = payload["binaries"], payload["commands"], payload["lifecycle_sandbox"]
            expected_actions = [["install-qualification-wrapper"], ["stop"], ["start"], ["status"], ["restart"], ["logs", "--lines", "1"], ["uninstall"], ["uninstall"]]
            identity_fields = {"role", "path", "sha256", "mode", "bytes"}
            publication = sandbox.get("publication") if isinstance(sandbox, dict) else None
            lifecycle_identity = _validate_lifecycle_identity(payload.get("lifecycle_identity"))
            if (
                not isinstance(binaries, dict) or set(binaries) != {"podway", "podwayd"}
                or any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value) for value in binaries.values())
                or not isinstance(payload["archive_sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", payload["archive_sha256"])
                or not isinstance(payload["archive_members_sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", payload["archive_members_sha256"])
                or payload["home_isolated"] is not True or payload["source_materialization_preserved"] is not True
                or not isinstance(sandbox, dict) or set(sandbox) != {"controller_identities", "program_arguments", "qualification_install", "policy_probe", "cleanup", "publication"}
                or not isinstance(sandbox["program_arguments"], list) or len(sandbox["program_arguments"]) != 2 or not _is_canonical_absolute_path(sandbox["program_arguments"][0]) or sandbox["program_arguments"][1] != "--service"
                or not isinstance(sandbox["qualification_install"], dict) or set(sandbox["qualification_install"]) != {"route", "wrapper_path", "wrapper_sha256", "sandbox_profile_path", "sandbox_profile_sha256", "archived_daemon_path", "archived_daemon_sha256"}
                or sandbox["qualification_install"].get("route") != "install-qualification-wrapper"
                or any(not _is_canonical_absolute_path(sandbox["qualification_install"].get(field)) for field in ("wrapper_path", "sandbox_profile_path", "archived_daemon_path"))
                or any(not isinstance(sandbox["qualification_install"].get(field), str) or not re.fullmatch(r"[0-9a-f]{64}", sandbox["qualification_install"][field]) for field in ("wrapper_sha256", "sandbox_profile_sha256", "archived_daemon_sha256"))
                or not isinstance(sandbox["controller_identities"], dict) or set(sandbox["controller_identities"]) != {"controller_wrapper", "controller_profile", "extracted_archived_daemon", "extracted_archived_cli"}
                or any(not isinstance(value, dict) or set(value) != identity_fields or not _is_canonical_absolute_path(value.get("path")) or not isinstance(value["bytes"], int) or value["bytes"] < 0 or not isinstance(value["mode"], int) or not isinstance(value["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", value["sha256"]) for value in sandbox["controller_identities"].values())
                or any(sandbox["controller_identities"][key].get("role") != key for key in ("controller_wrapper", "controller_profile", "extracted_archived_daemon", "extracted_archived_cli"))
                or not isinstance(publication, dict) or set(publication) != {"staged_path", "daemon_identity", "metadata", "metadata_identity", "log_path", "launchctl_path"}
                or publication["staged_path"] != sandbox["program_arguments"][0] or publication["daemon_identity"] != sandbox["controller_identities"]["controller_wrapper"]["sha256"] or publication["launchctl_path"] != str(LAUNCHCTL)
                or Path(publication["staged_path"]).name != publication["daemon_identity"] or Path(publication["staged_path"]).parent.name != ".podway-daemons-v1"
                or not isinstance(publication["metadata"], dict) or set(publication["metadata"]) != {"version", "label", "daemon_binary", "daemon_identity", "installed_at", "updated_at", "publication_state", "generation"}
                or publication["metadata"].get("version") != 1 or publication["metadata"].get("label") != LABEL or publication["metadata"].get("daemon_binary") != publication["staged_path"] or publication["metadata"].get("daemon_identity") != publication["daemon_identity"] or publication["metadata"].get("publication_state") != "receipt_durable"
                or not all(isinstance(publication["metadata"].get(field), int) and publication["metadata"][field] >= 0 for field in ("installed_at", "updated_at")) or publication["metadata"]["updated_at"] < publication["metadata"]["installed_at"]
                or publication["metadata"].get("generation") != _reconstruct_lifecycle_generation(Path(publication["staged_path"]), publication["daemon_identity"], publication["metadata"], Path(publication["log_path"]))[0]
                or not isinstance(sandbox["policy_probe"], dict) or set(sandbox["policy_probe"]) != {"service_profile", "command_policy"}
                or not isinstance(sandbox["cleanup"], dict) or set(sandbox["cleanup"]) != {"launchctl_path", "bootout_exit_code", "observed_state", "absence", "removed_paths"} or sandbox["cleanup"]["launchctl_path"] != str(LAUNCHCTL) or sandbox["cleanup"]["bootout_exit_code"] not in {0, 3, 113} or sandbox["cleanup"]["removed_paths"] != [commands[0]["plist"]["path"], commands[0]["metadata"]["path"], sandbox["program_arguments"][0]]
                or not isinstance(commands, list) or len(commands) != len(expected_actions)
            ):
                raise QualificationError("lifecycle checkpoint lacks mechanically reconstructed staging, policy, or cleanup proof")
            _validate_lifecycle_policy_probes(
                sandbox["policy_probe"], sandbox["controller_identities"]["controller_profile"]["sha256"],
            )
            expected_target = lifecycle_identity["target"]
            expected_uid = lifecycle_identity["uid"]
            expected_runtime = lifecycle_identity["runtime_path"]
            expected_socket = lifecycle_identity["socket_path"]
            wrapper = sandbox["controller_identities"]["controller_wrapper"]
            daemon = sandbox["controller_identities"]["extracted_archived_daemon"]
            profile_identity = sandbox["controller_identities"]["controller_profile"]
            _require_lifecycle_digest(
                wrapper["sha256"],
                sha256_bytes(_reconstruct_controller_wrapper(Path(profile_identity["path"]), Path(daemon["path"]))),
                "controller wrapper",
            )
            if (
                daemon["sha256"] != binaries["podwayd"]
                or Path(daemon["path"]).name != "podwayd"
                or Path(sandbox["controller_identities"]["extracted_archived_cli"]["path"]).name != "podway"
                or Path(sandbox["controller_identities"]["extracted_archived_cli"]["path"]).parent != Path(daemon["path"]).parent
                or sandbox["qualification_install"]["wrapper_path"] != wrapper["path"]
                or sandbox["qualification_install"]["wrapper_sha256"] != wrapper["sha256"]
                or sandbox["qualification_install"]["sandbox_profile_path"] != profile_identity["path"]
                or sandbox["qualification_install"]["sandbox_profile_sha256"] != profile_identity["sha256"]
                or sandbox["qualification_install"]["archived_daemon_path"] != daemon["path"]
                or sandbox["qualification_install"]["archived_daemon_sha256"] != daemon["sha256"]
            ):
                raise QualificationError("lifecycle qualification wrapper binding does not reconstruct the archived production daemon")
            for index, (command, action) in enumerate(zip(commands, expected_actions)):
                required = {"argv", "exit_code", "stdout_sha256", "stderr_sha256", "protected_before", "protected_after"}
                if index == 0:
                    required |= {"plist", "staged_wrapper", "metadata", "program_arguments", "runtime_socket", "login_load", "idempotent_install", "unexpected_exit_relaunch"}
                if index == 1:
                    required |= {"launchctl_path", "absence", "keepalive_defeated"}
                if index == 6:
                    required |= {"absence", "worktree_state"}
                if index in {0, 2, 4}:
                    required |= {"launchctl", "running", "staged_path", "pid", "runtime_socket"}
                if index == len(commands) - 1: required |= {"candidate_uninstall", "absence"}
                argv = command.get("argv") if isinstance(command, dict) else None
                expected_install_argv = (
                    _qualification_install_argv(
                        sandbox["controller_identities"]["extracted_archived_cli"]["path"],
                        sandbox["qualification_install"],
                    )
                    if index == 0
                    else None
                )
                if (
                    not isinstance(command, dict) or set(command) != required or not isinstance(argv, list) or len(argv) != (len(expected_install_argv) if expected_install_argv is not None else len(action) + 2)
                    or not isinstance(argv[0], str) or not Path(argv[0]).is_absolute() or argv[1] != "daemon" or argv[2] != action[0]
                    or (index == 0 and (argv != expected_install_argv or command["program_arguments"] != sandbox["program_arguments"] or any(not isinstance(command[key], dict) or set(command[key]) != identity_fields for key in {"plist", "staged_wrapper", "metadata"}) or command["staged_wrapper"]["path"] != sandbox["program_arguments"][0]))
                    or (index != 0 and argv[3:] != action[1:]) or command.get("exit_code") != 0
                    or not all(isinstance(command.get(field), str) and re.fullmatch(r"[0-9a-f]{64}", command[field]) for field in {"stdout_sha256", "stderr_sha256"})
                    or (index == 1 and (command.get("launchctl_path") != str(LAUNCHCTL)))
                    or (index in {0, 2, 4} and (command.get("running") is not True or command.get("staged_path") != sandbox["program_arguments"][0] or not isinstance(command.get("pid"), int) or command["pid"] <= 0))
                    or (index == len(commands) - 1 and command.get("candidate_uninstall") is not True)
                ):
                    raise QualificationError("lifecycle checkpoint command order or result proof differs")
                if index in {0, 2, 4}:
                    if command["launchctl"].get("target") != expected_target:
                        raise QualificationError("lifecycle launchctl target differs from signed lifecycle identity")
                    if command["runtime_socket"].get("socket_path") != expected_socket:
                        raise QualificationError("lifecycle runtime socket differs from signed lifecycle identity")
                    _validate_launchctl_binding(command["launchctl"], commands[0]["plist"]["path"], sandbox["program_arguments"][0], expected_target)
                    if command["launchctl"]["pid"] != command["pid"]:
                        raise QualificationError("lifecycle launchctl PID differs from command PID")
                    _validate_lifecycle_runtime_socket(command["runtime_socket"], expected_uid, expected_runtime)
                if index in {1, 6, 7}:
                    _validate_launchctl_absence(command["absence"], expected_target)
                if index == 0:
                    login = command["login_load"]
                    relaunch = command["unexpected_exit_relaunch"]
                    if (
                        login != {"plist_run_at_load": "true", "bootstrap_target": command["launchctl"]["target"], "bootstrap_pid": command["pid"]}
                        or not isinstance(relaunch, dict) or set(relaunch) != {"signal", "before_pid", "after_pid", "launchctl", "runtime_socket", "stale_socket_recovery"}
                        or relaunch["signal"] != 9 or not isinstance(relaunch["before_pid"], int) or not isinstance(relaunch["after_pid"], int)
                        or relaunch["before_pid"] <= 0 or relaunch["after_pid"] <= 0 or relaunch["before_pid"] == relaunch["after_pid"]
                        or relaunch["before_pid"] != command["pid"] or not isinstance(relaunch["launchctl"], dict) or relaunch["launchctl"].get("pid") != relaunch["after_pid"]
                        or not isinstance(relaunch["stale_socket_recovery"], dict) or set(relaunch["stale_socket_recovery"]) != {"stale_socket_path", "stale_socket_owner_uid", "recovered_socket_path", "recovered"}
                    ):
                        raise QualificationError("lifecycle login bootstrap or unexpected-exit relaunch proof is malformed")
                    target_uid = expected_uid
                    _validate_launchctl_binding(relaunch["launchctl"], commands[0]["plist"]["path"], sandbox["program_arguments"][0], expected_target)
                    if relaunch["launchctl"]["target"] != expected_target:
                        raise QualificationError("lifecycle unexpected-exit relaunch target drift")
                    _validate_lifecycle_runtime_socket(relaunch["runtime_socket"], target_uid, expected_runtime)
                    stale = relaunch["stale_socket_recovery"]
                    if (
                        stale["stale_socket_path"] != command["runtime_socket"]["socket_path"]
                        or stale["recovered_socket_path"] != command["runtime_socket"]["socket_path"]
                        or stale["stale_socket_owner_uid"] != target_uid or stale["recovered"] is not True
                    ):
                        raise QualificationError("lifecycle stale-socket recovery proof is malformed")
                    idempotent = command["idempotent_install"]
                    if (
                        not isinstance(idempotent, dict)
                        or set(idempotent) != {"argv", "exit_code", "stdout_sha256", "stderr_sha256", "launchctl", "running", "pid", "runtime_socket"}
                        or idempotent["argv"] != command["argv"] or idempotent["exit_code"] != 0
                        or idempotent["running"] is not True or not isinstance(idempotent["launchctl"], dict) or idempotent["launchctl"].get("pid") != idempotent["pid"]
                        or not all(isinstance(idempotent[field], str) and re.fullmatch(r"[0-9a-f]{64}", idempotent[field]) for field in ("stdout_sha256", "stderr_sha256"))
                    ):
                        raise QualificationError("lifecycle idempotent install proof is malformed")
                    _validate_launchctl_binding(idempotent["launchctl"], commands[0]["plist"]["path"], sandbox["program_arguments"][0], expected_target)
                    _validate_lifecycle_runtime_socket(idempotent["runtime_socket"], target_uid, expected_runtime)
                if index == 1:
                    keepalive = command["keepalive_defeated"]
                    if not isinstance(keepalive, dict) or set(keepalive) != {"first_absence", "post_throttle_absence", "throttle_interval_seconds"} or keepalive["throttle_interval_seconds"] != 5:
                        raise QualificationError("lifecycle explicit-stop keepalive proof is malformed")
                    _validate_launchctl_absence(keepalive["first_absence"], expected_target)
                    _validate_launchctl_absence(keepalive["post_throttle_absence"], expected_target)
                if index == 6:
                    worktree = command["worktree_state"]
                    if not isinstance(worktree, dict) or set(worktree) != {"marker_path", "marker_sha256", "marker_bytes"} or not _is_canonical_absolute_path(worktree.get("marker_path")) or worktree["marker_bytes"] != len(b"preserve\n") or worktree["marker_sha256"] != sha256_bytes(b"preserve\n"):
                        raise QualificationError("lifecycle uninstall worktree preservation proof is malformed")
                if index == 0:
                    post_install = command["protected_after"]["artifacts"]
                    expected_plist_path = (
                        Path(commands[0]["plist"]["path"]).parents[2]
                        / "Library" / "LaunchAgents" / f"{LABEL}.plist"
                    )
                    expected_metadata_path = (
                        Path(commands[0]["plist"]["path"]).parents[2]
                        / "Library" / "Application Support" / "Podway" / "service.json"
                    )
                    expected_staged_path = expected_metadata_path.parent / ".podway-daemons-v1" / publication["daemon_identity"]
                    generation, authenticated_plist = _reconstruct_lifecycle_generation(
                        Path(publication["staged_path"]), publication["daemon_identity"],
                        publication["metadata"], Path(publication["log_path"]),
                    )
                    if (
                        argv[4] != sandbox["qualification_install"]["wrapper_path"]
                        or command["plist"] != post_install.get("plist")
                        or command["metadata"] != post_install.get("metadata")
                        or command["staged_wrapper"] != post_install.get("staged_wrapper")
                        or command["plist"]["path"] != str(expected_plist_path)
                        or command["metadata"]["path"] != str(expected_metadata_path)
                        or command["staged_wrapper"]["path"] != str(expected_staged_path)
                        or publication["staged_path"] != str(expected_staged_path)
                        or publication["log_path"] != str(expected_plist_path.parents[2] / "Library" / "Logs" / "Podway" / "podwayd.log")
                        or command["plist"]["sha256"] != sha256_bytes(authenticated_plist)
                        or publication["metadata_identity"] != command["metadata"]
                        or command["metadata"]["bytes"] != len(_reconstruct_service_metadata(publication["metadata"]))
                        or command["metadata"]["sha256"] != sha256_bytes(_reconstruct_service_metadata(publication["metadata"]))
                        or publication["metadata"]["generation"] != generation
                        or command["staged_wrapper"]["sha256"] != sandbox["controller_identities"]["controller_wrapper"]["sha256"]
                    ):
                        raise QualificationError("lifecycle publication identities or exact paths are not independently bound")
                    _require_lifecycle_digest(command["plist"]["sha256"], sha256_bytes(authenticated_plist), "plist publication")
                _validate_lifecycle_snapshot(command["protected_before"], identity_fields)
                _validate_lifecycle_snapshot(command["protected_after"], identity_fields)
                before_snapshot, after_snapshot = command["protected_before"], command["protected_after"]
                before_artifacts, after_artifacts = before_snapshot["artifacts"], after_snapshot["artifacts"]
                control_roles = {"controller_wrapper", "controller_profile", "extracted_archived_daemon", "extracted_archived_cli"}
                service_roles = {"plist", "metadata", "staged_wrapper"}
                if any(
                    before_artifacts.get(role) != sandbox["controller_identities"][role]
                    or after_artifacts.get(role) != sandbox["controller_identities"][role]
                    for role in control_roles
                    if role in before_artifacts or role in after_artifacts
                ):
                    raise QualificationError("lifecycle immutable packaged artifact snapshot differs from authenticated identity")
                common_roles = set(before_artifacts) & set(after_artifacts)
                before_common_ancestors = {
                    (item["role"], item["path"]): (item["device"], item["inode"], item["mode"])
                    for item in before_snapshot["canonical_ancestors"] if item["role"] in common_roles
                }
                after_common_ancestors = {
                    (item["role"], item["path"]): (item["device"], item["inode"], item["mode"])
                    for item in after_snapshot["canonical_ancestors"] if item["role"] in common_roles
                }
                transition_differs = before_common_ancestors != after_common_ancestors
                transition_differs = transition_differs or (
                    index == 0
                    and (
                        set(before_artifacts) != control_roles
                        or set(after_artifacts) != control_roles | service_roles
                    )
                )
                transition_differs = transition_differs or (
                    index in {1, 2, 3, 4, 5} and before_snapshot != after_snapshot
                )
                transition_differs = transition_differs or (
                    index == 6
                    and (
                        set(before_artifacts) != control_roles | service_roles
                        or set(after_artifacts) != control_roles
                    )
                )
                transition_differs = transition_differs or (
                    index == 7
                    and (set(before_artifacts) != control_roles or before_snapshot != after_snapshot)
                )
                if transition_differs:
                    raise QualificationError("lifecycle protected identity or ancestor transition differs")
            if expected_target is None or expected_uid is None or expected_runtime is None:
                raise QualificationError("lifecycle identity envelope is missing")
            cleanup = sandbox["cleanup"]
            if cleanup["observed_state"] is not None:
                _validate_launchctl_binding(cleanup["observed_state"], commands[0]["plist"]["path"], sandbox["program_arguments"][0], expected_target)
            _validate_launchctl_absence(cleanup["absence"], expected_target)
        return
    if set(payload) != base | {"results"} or payload.get("checkpoint_id") != "G009-GATE-GATES":
        raise QualificationError("aggregate checkpoint envelope schema is incomplete")
    results = payload.get("results")
    if not isinstance(results, list):
        raise QualificationError("aggregate checkpoint results are malformed")
    matched = [item for item in results if isinstance(item, dict) and item.get("gate_id") == gate_id]
    if len(matched) != 1:
        raise QualificationError("aggregate checkpoint gate result is missing or duplicated")
    result = matched[0]
    if result.get("status") != "pass":
        raise QualificationError("aggregate checkpoint gate did not pass")
    if gate_id == "G009-GATE-FUZZ":
        if set(result) != {"gate_id", "policy_mode", "profile_sha256", "native_target", "provenance", "commands", "run_identity", "status"}:
            raise QualificationError("fuzz gate result schema is incomplete")
        if result.get("native_target") != payload.get("target"):
            raise QualificationError("fuzz gate native target differs from qualification target")
        return
    expected_argv = [[payload["target"] if item == TARGET else item for item in argv] for argv in GATES[gate_id]]
    if gate_id == "G009-GATE-COVERAGE":
        expected_argv.insert(0, ["cargo", "+1.85.0", "llvm-cov", "--workspace", "--all-targets", "--target", payload["target"]])
    if set(result) != {"gate_id", "commands", "status"} or not isinstance(result.get("commands"), list) or len(result["commands"]) != len(expected_argv):
        raise QualificationError("gate result schema is incomplete")
    for index, command in enumerate(result["commands"]):
        allowed = {"argv", "exit_code", "stdout_sha256", "stderr_sha256", "status"}
        if gate_id == "G009-GATE-COVERAGE":
            allowed.add("phase")
        if (
            not isinstance(command, dict) or set(command) != allowed
            or command.get("argv") != expected_argv[index]
            or command.get("exit_code") != 0 or command.get("status") != "pass"
            or not all(isinstance(command.get(field), str) and re.fullmatch(r"[0-9a-f]{64}", command[field]) for field in ("stdout_sha256", "stderr_sha256"))
            or (gate_id == "G009-GATE-COVERAGE" and command.get("phase") != ("isolated-current-run-collection" if index == 0 else "current-run-report"))
        ):
            raise QualificationError("gate command result does not prove the exact successful gate outcome")
def validate_final(qualification_path: Path, review_path: Path, evidence_root: Path, keyring: Path, keyring_sha256: str, signature_bindings: list[str], fingerprints: list[str], invocation_nonce: str) -> dict[str, Any]:
    qualification_target = load_json(qualification_path).get("target") if isinstance(load_json(qualification_path), dict) else None
    require_native_host(qualification_target)
    qualification = load_json(qualification_path)
    review = load_json(review_path)
    if not isinstance(invocation_nonce, str) or not re.fullmatch(r"[0-9a-f]{64}", invocation_nonce):
        raise QualificationError("strict verification requires a trusted 64-hex invocation nonce")
    validate_qualification_source(qualification.get("source") if isinstance(qualification, dict) else None, include_paths=False)
    required = {"schema", "qualification_archive_sha256", "acceptance_index_sha256", "rc_sha256", "traceability_sha256", "release_policy_sha256", "tool_manifest_sha256", "source", "target", "target_tuple"}
    if not isinstance(qualification, dict) or set(qualification) != required or qualification.get("schema") != "podway.g009.qualification-bundle/v1" or qualification.get("target_tuple") != target_tuple(qualification.get("target")):
        raise QualificationError("qualification descriptor is malformed")
    trusted_qualification_profile(qualification["target"])
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
        or index_value.get("traceability_sha256") != qualification["traceability_sha256"]
        or index_value.get("run_identity") != invocation_nonce
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
        "manifest.json",
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
        try:
            public_row_source = validate_qualification_source(row_source, include_paths=True)
        except QualificationError:
            raise QualificationError("qualification evidence source provenance is malformed") from None
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
            if len(names) != len(set(names)):
                raise QualificationError("qualification bundle has duplicate members")
            _preflight_qualification_bundle(bundle)
            core_members = {
                "rc.json": qualification["rc_sha256"],
                "traceability.json": qualification["traceability_sha256"],
                "acceptance-index.json": qualification["acceptance_index_sha256"],
                "release-policy.json": qualification["release_policy_sha256"],
                "tool-manifest.json": qualification["tool_manifest_sha256"],
            }
            if any(sha256_bytes(bundle.read(name)) != digest for name, digest in core_members.items()):
                raise QualificationError("qualification core member digest differs from descriptor")
            if bundle.read("acceptance-index.json") != bounded_bytes(index):
                raise QualificationError("bundled acceptance index differs from adjacent index")
            bundled_traceability = bundle.read("traceability.json")
            if bundled_traceability != bounded_bytes(ROOT / "release/g009-traceability-v1.json"):
                raise QualificationError("bundled traceability is not the exact trusted semantic registry")
            validate_traceability(ROOT / "release/g009-traceability-v1.json")
            archive_digest = sha256_bytes(bundle.read("archive.zip"))
            if bundle.read("archive.zip.sha256") != (archive_digest + "\n").encode("ascii"):
                raise QualificationError("bundled package archive checksum differs")
            receipt = load_json_bytes(bundle.read("receipt.json"), "qualification receipt")
            if (
                not isinstance(receipt, dict)
                or receipt.get("schema") != "podway.g009.qualification-bundle-receipt/v1"
                or receipt.get("rc_sha256") != qualification["rc_sha256"]
                or receipt.get("archive_sha256") != archive_digest
                or receipt.get("index_sha256") != qualification["acceptance_index_sha256"]
                or receipt.get("traceability_sha256") != qualification["traceability_sha256"]
                or receipt.get("release_policy_sha256") != qualification["release_policy_sha256"]
                or receipt.get("tool_manifest_sha256") != qualification["tool_manifest_sha256"]
                or receipt.get("source") != qualification["source"]
                or receipt.get("target") != qualification["target"]
                or receipt.get("target_tuple") != qualification["target_tuple"]
            ):
                raise QualificationError("qualification receipt does not cross-bind core members")
            package_archive_sha256: str | None = None
            lifecycle_archive_sha256: str | None = None
            with tempfile.TemporaryDirectory(prefix="g009-offline-archive-") as raw_archive:
                archive_path = Path(raw_archive) / "archive.zip"
                archive_path.write_bytes(bundle.read("archive.zip"))
                archive_path.with_name("archive.zip.sha256").write_text(archive_digest + "\n", encoding="ascii")
                bundled_archive_report = inspect_archive(archive_path, target=qualification["target"])
            for row in evidence_rows:
                raw_evidence = bundle.read(f"evidence/{row['path']}")
                if sha256_bytes(raw_evidence) != row["sha256"]:
                    raise QualificationError("qualification evidence member digest differs")
                payload = load_json_bytes(raw_evidence, f"evidence/{row['path']}")
                if (
                    not isinstance(payload, dict)
                    or payload.get("status") != "pass"
                    or payload.get("run_identity") != index_value.get("run_identity")
                    or not isinstance(index_value.get("run_identity"), str)
                    or not re.fullmatch(r"[0-9a-f]{64}", index_value["run_identity"])
                    or payload.get("rc_sha256") != qualification["rc_sha256"]
                    or payload.get("target") != qualification["target"]
                    or payload.get("source") != row["source"]
                    or payload.get("blockers") != []
                ):
                    raise QualificationError("qualification evidence payload is not a current pass envelope")
                validate_checkpoint_payload(payload, row["gate_id"])
                if row["gate_id"] == "G009-GATE-PACKAGE":
                    archive = payload.get("archive")
                    if archive != bundled_archive_report:
                        raise QualificationError("package checkpoint members are not mechanically derived from the bundled archive")
                    package_archive_sha256 = archive.get("archive_sha256") if isinstance(archive, dict) else None
                if row["gate_id"] == "G009-GATE-LIFECYCLE":
                    if payload.get("archive_members_sha256") != sha256_bytes(canonical_json(bundled_archive_report["members"])):
                        raise QualificationError("lifecycle checkpoint members are not mechanically derived from the bundled archive")
                    lifecycle_archive_sha256 = payload.get("archive_sha256")
                    expected_binaries = {
                        Path(member["path"]).name: member["sha256"]
                        for member in bundled_archive_report["members"]
                        if member["path"] in {f"{archive_root(qualification['target'])}/bin/podway", f"{archive_root(qualification['target'])}/bin/podwayd"}
                    }
                    if payload.get("binaries") != expected_binaries:
                        raise QualificationError("lifecycle checkpoint binaries do not bind the bundled archive")
                if (
                    row["gate_id"] in {"G009-GATE-PACKAGE", "G009-GATE-LIFECYCLE"}
                    and (not isinstance(payload.get("archive_sha256") if row["gate_id"] == "G009-GATE-LIFECYCLE" else package_archive_sha256, str)
                         or (payload.get("archive_sha256") if row["gate_id"] == "G009-GATE-LIFECYCLE" else package_archive_sha256) != archive_digest)
                ):
                    raise QualificationError("package or lifecycle evidence does not bind bundled archive")
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
                    if row["gate_id"] == "G009-GATE-FUZZ":
                        expected_names.update(validate_bundled_fuzz_dependencies(matched[0], bundle, invocation_nonce))
                elif payload.get("checkpoint_id") != row["gate_id"]:
                    raise QualificationError("qualification checkpoint identity differs")
            if package_archive_sha256 != lifecycle_archive_sha256:
                raise QualificationError("package and lifecycle evidence bind different archives")
            if set(names) != expected_names:
                raise QualificationError("qualification bundle membership is not the exact acceptance set")
            manifest = load_json_bytes(bundle.read("manifest.json"), "qualification bundle manifest")
            if (
                not isinstance(manifest, dict)
                or set(manifest) != {"schema", "members"}
                or manifest.get("schema") != "podway.g009.bundle-manifest/v1"
                or manifest.get("members") != [
                    {"path": member, "size": len(bundle.read(member)), "sha256": sha256_bytes(bundle.read(member))}
                    for member in sorted(set(names) - {"manifest.json"})
                ]
            ):
                raise QualificationError("qualification bundle manifest is not the exact immutable member set")
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
            validate_qualification_source(tool_manifest.get("source") if isinstance(tool_manifest, dict) else None, include_paths=False)
            controller_sources = tool_manifest.get("controller_sources") if isinstance(tool_manifest, dict) else None
            expected_controller_sources = [
                {"id": source_id, "path_sha256": sha256_file(ROOT / source_id)}
                for source_id in (
                    ".github/workflows/release.yml",
                    ".github/workflows/release-final-review.yml",
                    ".github/workflows/release-publish.yml",
                    "tools/g009_common.py",
                    "tools/g009_performance.py",
                    "tools/g009_release.py",
                    "tools/run_g009_qualification.py",
                    "tools/run_verification.py",
                    "tools/run_g005_vertical.py",
                    "tools/run_g008_dogfood.py",
                    "tools/verify_g009_qualification.py",
                    "tools/g009_publication.py",
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
                    or item.get("architecture") != target_tuple(qualification["target"])["mach_o_arch"]
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
    expected_review_keys = {
        "schema", "status", "qualification_bundle_sha256", "qualification_archive_sha256",
        "acceptance_index_sha256", "rc_sha256", "traceability_sha256",
        "release_policy_sha256", "tool_manifest_sha256", "source", "target",
        "target_tuple", "reviewers", "reviewer_keyring_sha256", "attestations", "blockers",
    }
    if (
        not isinstance(review, dict)
        or set(review) != expected_review_keys
        or review.get("schema") != "podway.g009.final-review/v2"
        or review.get("status") != "passed"
        or review.get("blockers") != []
    ):
        raise QualificationError("final review is malformed")
    identity_fields = (
        "qualification_archive_sha256", "acceptance_index_sha256", "rc_sha256",
        "traceability_sha256", "release_policy_sha256", "tool_manifest_sha256",
        "source", "target", "target_tuple",
    )
    if (
        review.get("qualification_bundle_sha256") != sha256_file(qualification_path)
        or any(review.get(field) != qualification.get(field) for field in identity_fields)
    ):
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
        "target_tuple": qualification["target_tuple"],
        "attestations": receipt_attestations,
        "invocation_nonce": invocation_nonce,
    }
def _lifecycle_checkpoint_fixture(uid: int = 501, runtime_name: str = "runtime") -> dict[str, Any]:
    root = Path("/g009-lifecycle-fixture")
    plist, metadata_path = root / "home/Library/LaunchAgents" / f"{LABEL}.plist", root / "home/Library/Application Support/Podway/service.json"
    wrapper, profile, daemon, cli = root / "wrapper", root / "profile", root / "extract/bin/podwayd", root / "extract/bin/podway"
    wrapper_digest = sha256_bytes(_reconstruct_controller_wrapper(profile, daemon))
    staged = metadata_path.parent / ".podway-daemons-v1" / wrapper_digest
    metadata = {"version": 1, "label": LABEL, "daemon_binary": str(staged), "daemon_identity": wrapper_digest, "installed_at": 1, "updated_at": 1, "publication_state": "receipt_durable", "generation": ""}
    log_path = root / "home/Library/Logs/Podway/podwayd.log"
    metadata["generation"] = _reconstruct_lifecycle_generation(staged, wrapper_digest, metadata, log_path)[0]
    def item(role: str, path: Path, digest: str, mode: int) -> dict[str, Any]:
        return {"role": role, "path": str(path), "sha256": digest, "mode": mode, "bytes": 1}
    controls = {
        "controller_wrapper": item("controller_wrapper", wrapper, wrapper_digest, 0o700),
        "controller_profile": item("controller_profile", profile, "f" * 64, 0o600),
        "extracted_archived_daemon": item("extracted_archived_daemon", daemon, "d" * 64, 0o700),
        "extracted_archived_cli": item("extracted_archived_cli", cli, "c" * 64, 0o700),
    }
    qualification_install = {
        "route": "install-qualification-wrapper",
        "wrapper_path": str(wrapper),
        "wrapper_sha256": wrapper_digest,
        "sandbox_profile_path": str(profile),
        "sandbox_profile_sha256": controls["controller_profile"]["sha256"],
        "archived_daemon_path": str(daemon),
        "archived_daemon_sha256": controls["extracted_archived_daemon"]["sha256"],
    }
    service = {
        "plist": item("plist", plist, sha256_bytes(_reconstruct_lifecycle_generation(staged, wrapper_digest, metadata, log_path)[1]), 0o600),
        "metadata": item("metadata", metadata_path, sha256_bytes(_reconstruct_service_metadata(metadata)), 0o600),
        "staged_wrapper": item("staged_wrapper", staged, wrapper_digest, 0o700),
    }
    service["metadata"]["bytes"] = len(_reconstruct_service_metadata(metadata))
    def snap(artifacts: dict[str, Any]) -> dict[str, Any]:
        ancestors = []
        for role, value in artifacts.items():
            cursor = Path(value["path"]).parent
            while True:
                ancestors.append({"role": role, "path": str(cursor), "mode": 0o700, "device": 1, "inode": 1})
                if cursor == cursor.parent: break
                cursor = cursor.parent
        return {"artifacts": artifacts, "canonical_ancestors": sorted(ancestors, key=lambda row: (row["path"], row["role"]))}
    before, installed = snap(controls), snap(controls | service)
    absence = {"target": f"gui/{uid}/{LABEL}", "exit_code": 113, "stdout_sha256": sha256_bytes(b""), "stderr": f'Bad request.\nCould not find service "{LABEL}" in domain for user gui: {uid}'}
    absence["stderr_sha256"] = sha256_bytes(absence["stderr"].encode())
    launch_stdout = (f"{absence['target']} = {{\n\tpath = {plist}\n\tprogram = {staged}\n\targuments = {{\n\t\t{staged}\n\t\t--service\n\t}}\n\tstate = running\n\tpid = 1\n}}\n").encode()
    launch = {"target": absence["target"], "plist_path": str(plist), "program": str(staged), "arguments": [str(staged), "--service"], "state": "running", "pid": 1, "stdout_sha256": sha256_bytes(launch_stdout), "stderr_sha256": sha256_bytes(b""), "stdout_base64": base64.b64encode(launch_stdout).decode("ascii"), "stderr_base64": ""}
    runtime = root / runtime_name
    inputs = {"writable": [str(runtime)], "protected": [str(path) for path in (wrapper, profile, daemon, cli, plist, metadata_path, staged)]}
    rows = [{"boundary": "staged_service", "target_class": target, "operation": operation, "denied": True, "errno": 1, "diagnostic": "denied"} for target in ("candidate_root", "staged_publication") for operation in ("transient_create_then_delete", "existing_overwrite", "rename", "symlink")]
    policies = {"service_profile": {"policy_sha256": controls["controller_profile"]["sha256"], "operations": rows}, "command_policy": {"policy_sha256": sha256_bytes(_reconstruct_lifecycle_command_policy(inputs).encode()), "inputs": inputs, "operations": [{**row, "boundary": "command_policy"} for row in rows]}}
    runtime_socket = {"socket_path": str(runtime / f"podway-{uid}/podwayd.sock"), "socket_owner_uid": uid, "socket_mode": 0o600, "runtime_path": str(runtime), "runtime_owner_uid": uid, "runtime_mode": 0o700, "target_uid": uid}
    relaunch_stdout = launch_stdout.replace(b"pid = 1", b"pid = 2")
    relaunch_launch = {**launch, "pid": 2, "stdout_sha256": sha256_bytes(relaunch_stdout), "stdout_base64": base64.b64encode(relaunch_stdout).decode("ascii")}
    actions = (["install-qualification-wrapper"], ["stop"], ["start"], ["status"], ["restart"], ["logs", "--lines", "1"], ["uninstall"], ["uninstall"])
    commands = []
    for index, action in enumerate(actions):
        argv = _qualification_install_argv(str(cli), qualification_install) if index == 0 else [str(cli), "daemon", *action]
        command = {"argv": argv, "exit_code": 0, "stdout_sha256": "0" * 64, "stderr_sha256": "1" * 64, "protected_before": before if index == 0 or index == 7 else installed, "protected_after": installed if index < 6 else before}
        if index == 0:
            command.update({**service, "program_arguments": [str(staged), "--service"], "launchctl": launch, "running": True, "staged_path": str(staged), "pid": 1, "runtime_socket": runtime_socket, "login_load": {"plist_run_at_load": "true", "bootstrap_target": launch["target"], "bootstrap_pid": 1}, "idempotent_install": {"argv": argv, "exit_code": 0, "stdout_sha256": "0" * 64, "stderr_sha256": "1" * 64, "launchctl": launch, "running": True, "pid": 1, "runtime_socket": runtime_socket}, "unexpected_exit_relaunch": {"signal": 9, "before_pid": 1, "after_pid": 2, "launchctl": relaunch_launch, "runtime_socket": runtime_socket, "stale_socket_recovery": {"stale_socket_path": runtime_socket["socket_path"], "stale_socket_owner_uid": uid, "recovered_socket_path": runtime_socket["socket_path"], "recovered": True}}})
        if index == 1:
            command.update({"launchctl_path": str(LAUNCHCTL), "absence": absence, "keepalive_defeated": {"first_absence": absence, "post_throttle_absence": absence, "throttle_interval_seconds": 5}})
        if index in {2, 4}:
            command.update({"launchctl": launch, "running": True, "staged_path": str(staged), "pid": 1, "runtime_socket": runtime_socket})
        if index == 6:
            command.update({"absence": absence, "worktree_state": {"marker_path": str(root / "workspace/.g009-preserve"), "marker_sha256": sha256_bytes(b"preserve\n"), "marker_bytes": len(b"preserve\n")}})
        if index == 7:
            command.update({"candidate_uninstall": True, "absence": absence})
        commands.append(command)
    return {"schema": "podway.g009.checkpoint/v1", "host": host_manifest(), "run_identity": "a" * 64, "checkpoint_id": "G009-GATE-LIFECYCLE", "status": "pass", "rc_sha256": "a" * 64, "source": {"commit": "a" * 40, "tree": "b" * 40, "tools": [{"id": "cargo", "version": "fixture", "path_sha256": "c" * 64, "path": str(root / "cargo")}, {"id": "rustc", "version": "fixture", "path_sha256": "d" * 64, "path": str(root / "rustc")}]}, "target": TARGET, "blockers": [], "archive_sha256": "e" * 64, "archive_members_sha256": "f" * 64, "binaries": {"podway": "c" * 64, "podwayd": "d" * 64}, "home_isolated": True, "source_materialization_preserved": True, "lifecycle_identity": {"uid": uid, "target": absence["target"], "runtime_path": str(runtime), "socket_path": runtime_socket["socket_path"]}, "commands": commands, "lifecycle_sandbox": {"controller_identities": controls, "program_arguments": [str(staged), "--service"], "qualification_install": qualification_install, "policy_probe": policies, "cleanup": {"launchctl_path": str(LAUNCHCTL), "bootout_exit_code": 0, "observed_state": launch, "absence": absence, "removed_paths": [str(plist), str(metadata_path), str(staged)]}, "publication": {"staged_path": str(staged), "daemon_identity": wrapper_digest, "metadata": metadata, "metadata_identity": service["metadata"], "log_path": str(log_path), "launchctl_path": str(LAUNCHCTL)}}}

def _g036_sandbox_mutation_sentinel() -> None:
    before = _g036_product_source_tree()
    sentinel = ROOT / f".g036-replay-sandbox-sentinel-{sha256_bytes(os.urandom(32))}"
    if sentinel.exists():
        raise AssertionError("G036 sandbox mutation sentinel path is unexpectedly occupied")
    with tempfile.TemporaryDirectory(prefix="podway-g036-sandbox-sentinel-") as raw:
        replay_root = Path(raw)
        target_dir = replay_root / "target"
        environment = {
            "HOME": str(replay_root / "home"),
            "CARGO_HOME": str(replay_root / "cargo-home"),
            "TMPDIR": str(replay_root / "tmp"),
        }
        for path in (*map(Path, environment.values()), target_dir, replay_root / "inputs" / "vendor"):
            path.mkdir(parents=True)
        result = _g036_sandboxed_candidate_run(
            [
                sys.executable,
                "-c",
                "from pathlib import Path; import os, sys; Path(sys.argv[1]).read_bytes(); os.execv('/usr/bin/true', ['true'])",
                "/etc/passwd",
            ],
            environment,
            target_dir,
        )
    if result.returncode == 0 or sentinel.exists() or _g036_product_source_tree() != before:
        raise AssertionError("G036 sandbox unbound read/execute candidate was not denied or product source changed")


def self_test() -> None:
    bounded_process_self_test()
    publication_controller_self_test()
    _g036_sandbox_mutation_sentinel()
    validate_migration_evidence()
    matrix = validate_product_acceptance_matrix()
    with tempfile.TemporaryDirectory() as raw:
        candidate = Path(raw) / "product-acceptance-matrix.json"
        for field, replacement in (("schema", "wrong"), ("version", 1), ("source", {}), ("source_files", {}), ("semantic_contracts", []), ("criteria", [])):
            altered = json.loads(json.dumps(matrix))
            altered[field] = replacement
            candidate.write_bytes(canonical_json(altered))
            reject(lambda candidate=candidate: validate_product_acceptance_matrix(candidate), f"product matrix {field} mutation")
        representative_mutations = (
            ("source path", ("source", "path"), "wrong.md"),
            ("excluded lines", ("source", "excluded_lines"), []),
            ("proof source digest", ("source_files", next(iter(matrix["source_files"]))), "0" * 64),
            ("criterion id", ("criteria", 0, "id"), "PAC-999"),
            ("criterion line", ("criteria", 0, "line"), 999),
            ("criterion text", ("criteria", 0, "text"), "drift"),
            ("criterion status", ("criteria", 0, "status"), "manual"),
            ("Cargo proof kind", ("criteria", 0, "proof", "kind"), "artifact"),
            ("Cargo proof command", ("criteria", 0, "proof", "command"), "cargo test"),
            ("Cargo proof path", ("criteria", 0, "proof", "path"), "../escape.rs"),
            ("Cargo proof function", ("criteria", 0, "proof", "function"), "missing"),
            ("current PAC-039 proof path", ("criteria", 38, "proof", "path"), "../escape.rs"),
            ("current PAC-039 proof function", ("criteria", 38, "proof", "function"), "forged"),
            ("current PAC-054 proof command", ("criteria", 53, "proof", "command"), "cargo test"),
        )
        for label, path, replacement in representative_mutations:
            altered = json.loads(json.dumps(matrix))
            cursor = altered
            for key in path[:-1]:
                cursor = cursor[key]
            cursor[path[-1]] = replacement
            candidate.write_bytes(canonical_json(altered))
            reject(lambda candidate=candidate: validate_product_acceptance_matrix(candidate), f"product matrix {label} mutation")
        for label, mutate in (
            ("missing criterion", lambda value: value["criteria"].pop()),
            ("duplicate criterion", lambda value: value["criteria"].append(value["criteria"][0])),
            ("relabeled criterion", lambda value: value["criteria"][0].__setitem__("id", "PAC-999")),
            ("criterion text", lambda value: value["criteria"][0].__setitem__("text", "drift")),
            ("proof path traversal", lambda value: value["criteria"][0]["proof"].__setitem__("path", "../escape.rs")),
            (
                "exact Cargo command count",
                lambda value: value["criteria"][66]["proof"].update({
                    "command": "cargo test -p podway-cli --test phase5_cli pac_003_help_states_the_same_user_local_socket_trust_boundary --locked -- --exact",
                    "path": "crates/podway-cli/tests/phase5_cli.rs",
                    "function": "pac_003_help_states_the_same_user_local_socket_trust_boundary",
                }),
            ),
        ):
            altered = json.loads(json.dumps(matrix))
            mutate(altered)
            candidate.write_bytes(canonical_json(altered))
            reject(lambda candidate=candidate: validate_product_acceptance_matrix(candidate), f"product matrix {label} mutation")
        altered = json.loads(json.dumps(matrix))
        pac005 = altered["criteria"][4]["proof"]
        pac010 = altered["criteria"][9]["proof"]
        pac005["members"] = json.loads(json.dumps(pac010["members"]))
        pac005["members"][0]["criterion_id"] = "PAC-005"
        pac005["members"][0]["obligation_ids"] = [row[0] for row in G040_OBLIGATIONS["PAC-005"]]
        candidate.write_bytes(canonical_json(altered))
        reject(lambda candidate=candidate: validate_product_acceptance_matrix(candidate), "product matrix repaired valid proof swap", "product acceptance semantic proof membership differs: PAC-005")
        altered = json.loads(json.dumps(matrix))
        altered["semantic_contracts"][0]["obligations"][0]["statement"] = "drift"
        candidate.write_bytes(canonical_json(altered))
        reject(lambda candidate=candidate: validate_product_acceptance_matrix(candidate), "product matrix obligation text drift", "product acceptance semantic contract differs: PAC-005")
        altered = json.loads(json.dumps(matrix))
        altered["criteria"][4]["proof"]["members"][0]["obligation_ids"].pop()
        candidate.write_bytes(canonical_json(altered))
        reject(lambda candidate=candidate: validate_product_acceptance_matrix(candidate), "product matrix obligation coverage removal", "product acceptance semantic obligation coverage differs: PAC-005")
        for label, obligation_id in (("duplicate", "reject-missing-required-item"), ("unknown", "unknown-obligation")):
            altered = json.loads(json.dumps(matrix))
            altered["criteria"][4]["proof"]["members"][0]["obligation_ids"].append(obligation_id)
            candidate.write_bytes(canonical_json(altered))
            reject(lambda candidate=candidate: validate_product_acceptance_matrix(candidate), f"product matrix {label} semantic obligation", "product acceptance semantic obligation coverage differs: PAC-005")
        altered = json.loads(json.dumps(matrix))
        members = altered["criteria"][25]["proof"]["members"]
        members[0]["obligation_ids"].extend(members[1]["obligation_ids"])
        members.pop()
        candidate.write_bytes(canonical_json(altered))
        reject(lambda candidate=candidate: validate_product_acceptance_matrix(candidate), "product matrix semantic member omission", "product acceptance semantic proof membership differs: PAC-026")
    with tempfile.TemporaryDirectory() as raw:
        candidate = Path(raw) / "g036-test-report.json"
        report = load_json(G036_REPORT_PATH)
        validate_g036_test_report(G036_REPORT_PATH)
        copied_matrix = Path(raw) / "matrix-copy.json"
        copied_matrix.write_bytes(G036_MATRIX_PATH.read_bytes())
        reject(
            lambda: validate_g036_test_report(G036_REPORT_PATH, copied_matrix),
            "G036 copied matrix path substitution",
        )
        mutations = (
            ("schemaVersion", lambda value: value.__setitem__("schemaVersion", 1)),
            ("kind", lambda value: value.__setitem__("kind", "wrong")),
            ("storyId", lambda value: value.__setitem__("storyId", "G999")),
            ("generatedAt", lambda value: value.__setitem__("generatedAt", "now")),
            ("target", lambda value: value["target"].__setitem__("arch", "x86_64")),
            ("cargoLock", lambda value: value["source"]["cargoLock"].__setitem__("sha256", "0" * 64)),
            ("testSources", lambda value: value["source"]["testSources"].clear()),
            ("matrix binding", lambda value: value["source"]["matrix"].__setitem__("sha256", "0" * 64)),
            ("policy binding", lambda value: value["source"]["policy"].__setitem__("sha256", "0" * 64)),
            ("verifier binding", lambda value: value["source"]["verifier"].__setitem__("sha256", "0" * 64)),
            ("product source tree", lambda value: value["source"]["productSourceTree"].__setitem__("sha256", "0" * 64)),
            ("matrix path", lambda value: value["source"]["matrix"].__setitem__("path", "release/substitute-matrix.json")),
            ("scope", lambda value: value["scope"].__setitem__("exactCommandCount", 0)),
            ("scope criterion count", lambda value: value["scope"].__setitem__("criterionCount", 0)),
            ("criterion duplicate", lambda value: value["criteria"].__setitem__(1, value["criteria"][0])),
            ("criterion proof", lambda value: value["criteria"][0].__setitem__("proof", {})),
            ("command duplicate", lambda value: value["commands"].__setitem__(1, value["commands"][0])),
            ("command exit", lambda value: value["commands"][0].__setitem__("exitCode", 1)),
            ("command zero tests", lambda value: value["commands"][0].__setitem__("testCount", 0)),
            ("command ignored", lambda value: value["commands"][0].__setitem__("ignoredCount", 1)),
            ("command exact argv", lambda value: value["commands"][0]["argv"].__setitem__(0, "forged-cargo")),
            ("command input closure", lambda value: value["commands"][0].__setitem__("inputTreeSha256", "0" * 64)),
            ("command host toolchain", lambda value: value["commands"][0].__setitem__("hostToolchainSha256", "0" * 64)),
            ("current PAC-039 receipt binding", lambda value: next(row for row in value["criteria"] if row["id"] == "PAC-039")["proof"].__setitem__("function", "forged")),
            ("result", lambda value: value.__setitem__("result", {"status": "blocked"})),
        )
        for label, mutate in mutations:
            altered = json.loads(json.dumps(report))
            mutate(altered)
            candidate.write_bytes(canonical_json(altered))
            reject(lambda candidate=candidate: validate_g036_test_report(candidate), f"G036 test report {label} mutation")
        semantic_receipts = [receipt for receipt in report["commands"] if receipt["semanticBindings"]]
        if len(semantic_receipts) < 2:
            raise AssertionError("G036 semantic binding swap sentinel requires two semantic receipts")
        altered = json.loads(json.dumps(report))
        first, second = next(receipt for receipt in altered["commands"] if receipt["command"] == semantic_receipts[0]["command"]), next(receipt for receipt in altered["commands"] if receipt["command"] == semantic_receipts[1]["command"])
        first["semanticBindings"], second["semanticBindings"] = second["semanticBindings"], first["semanticBindings"]
        if first["semanticBindings"] == semantic_receipts[0]["semanticBindings"]:
            raise AssertionError("G036 semantic binding swap sentinel did not change fixture")
        candidate.write_bytes(canonical_json(altered))
        reject(lambda candidate=candidate: validate_g036_test_report(candidate), "G036 receipt semantic-binding swap", "G036 command semantic binding differs")
        binding_receipt = next(receipt for receipt in report["commands"] if receipt["semanticBindings"])
        for label, mutate in (("omission", lambda bindings: bindings.pop()), ("duplication", lambda bindings: bindings.append(bindings[0]))):
            altered = json.loads(json.dumps(report))
            receipt = next(receipt for receipt in altered["commands"] if receipt["command"] == binding_receipt["command"])
            mutate(receipt["semanticBindings"])
            if receipt["semanticBindings"] == binding_receipt["semanticBindings"]:
                raise AssertionError(f"G036 semantic binding {label} sentinel did not change fixture")
            candidate.write_bytes(canonical_json(altered))
            reject(lambda candidate=candidate: validate_g036_test_report(candidate), f"G036 receipt semantic-binding {label}", "G036 command semantic binding differs")
        altered = json.loads(json.dumps(matrix))
        altered["criteria"][38]["proof"]["function"] = "forged"
        candidate_matrix = Path(raw) / "matrix.json"
        candidate_matrix.write_bytes(canonical_json(altered))
        reject(lambda: validate_product_acceptance_matrix(candidate_matrix), "current PAC-039 exact proof relabel")
    with tempfile.TemporaryDirectory() as raw:
        candidate = Path(raw) / "migration-evidence.json"
        original = load_json(MIGRATION_EVIDENCE_PATH)
        def migration_leaves(value: Any, prefix: tuple[str | int, ...] = ()) -> list[tuple[str | int, ...]]:
            if isinstance(value, dict):
                return [path for key, child in value.items() for path in migration_leaves(child, (*prefix, key))]
            if isinstance(value, list):
                return [prefix]
            return [prefix]
        for path in migration_leaves(original):
            altered = json.loads(json.dumps(original))
            cursor = altered
            for key in path[:-1]:
                cursor = cursor[key]
            current = cursor[path[-1]]
            cursor[path[-1]] = (
                not current if isinstance(current, bool) else current + 1 if isinstance(current, int)
                else current + "x" if isinstance(current, str) else [*current, None]
            )
            candidate.write_bytes(canonical_json(altered))
            reject(lambda: validate_migration_evidence(candidate), f"migration evidence {'/'.join(map(str, path))} mutation")
    signing_policy = load_json(ROOT / "release/g009-release-policy-v1.json")
    signing_policy["signing_evidence"]["current_public_package"]["posture"] = "signed-public"
    reject(lambda: validate_trust_policy(signing_policy), "unsigned public-release posture replacement")
    fixture = _lifecycle_checkpoint_fixture()
    validate_checkpoint_payload(fixture, "G009-GATE-LIFECYCLE")
    for field, replacement in {
        "route": "install",
        "wrapper_path": fixture["lifecycle_sandbox"]["qualification_install"]["archived_daemon_path"],
        "wrapper_sha256": "0" * 64,
        "sandbox_profile_path": fixture["lifecycle_sandbox"]["qualification_install"]["wrapper_path"],
        "sandbox_profile_sha256": "0" * 64,
        "archived_daemon_path": fixture["lifecycle_sandbox"]["qualification_install"]["wrapper_path"],
        "archived_daemon_sha256": "0" * 64,
    }.items():
        altered = json.loads(json.dumps(fixture))
        altered["lifecycle_sandbox"]["qualification_install"][field] = replacement
        reject(
            lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
            f"qualification install binding {field} mutation",
        )
    for label, mutate in (
        ("public install substitution", lambda argv: argv.__setitem__(2, "install")),
        ("missing flag", lambda argv: argv.pop(7)),
        ("extra flag", lambda argv: argv.append("--daemon-path")),
        ("reordered flags", lambda argv: (argv.__setitem__(3, "--wrapper-sha256"), argv.__setitem__(5, "--wrapper-path"))),
        ("wrapper and daemon path swap", lambda argv: (argv.__setitem__(4, argv[12]), argv.__setitem__(12, argv[4]))),
        ("wrapper and daemon digest swap", lambda argv: (argv.__setitem__(6, argv[14]), argv.__setitem__(14, argv[6]))),
        ("stale sandbox profile digest", lambda argv: argv.__setitem__(10, "0" * 64)),
        ("stale archived digest", lambda argv: argv.__setitem__(14, "0" * 64)),
    ):
        altered = json.loads(json.dumps(fixture))
        mutate(altered["commands"][0]["argv"])
        reject(
            lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
            f"qualification install argv {label}",
        )
    altered = json.loads(json.dumps(fixture))
    altered["binaries"]["podwayd"] = altered["lifecycle_sandbox"]["controller_identities"]["controller_wrapper"]["sha256"]
    reject(
        lambda: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
        "qualification wrapper as production daemon",
    )
    altered = json.loads(json.dumps(fixture))
    altered["lifecycle_sandbox"]["controller_identities"]["extracted_archived_daemon"]["role"] = "controller_wrapper"
    reject(
        lambda: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
        "qualification archived daemon role relabel",
    )
    for role, field, replacement in (
        ("controller_wrapper", "sha256", "0" * 64),
        ("controller_profile", "sha256", "0" * 64),
        ("extracted_archived_daemon", "mode", 0o600),
        ("extracted_archived_cli", "bytes", 0),
    ):
        altered = json.loads(json.dumps(fixture))
        altered["commands"][0]["protected_after"]["artifacts"][role][field] = replacement
        reject(
            lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
            f"immutable packaged snapshot {role} {field} mutation",
            "lifecycle immutable packaged artifact snapshot differs from authenticated identity",
        )
    isolated_identity = json.loads(json.dumps(fixture["lifecycle_identity"]))
    isolated_identity["runtime_path"] = "/tmp/runtime"
    isolated_identity["socket_path"] = "/tmp/runtime/podway-501/podwayd.sock"
    reject(
        lambda: _validate_lifecycle_identity(isolated_identity),
        "shared TMPDIR lifecycle socket substitution",
        "lifecycle identity envelope is not the exact isolated TMPDIR socket",
    )
    for field, replacement in {
        "uid": 502,
        "target": f"gui/502/{LABEL}",
        "runtime_path": "/g009-fixture/other-runtime",
        "socket_path": "/g009-fixture/other-runtime/podway-502/podwayd.sock",
    }.items():
        altered = json.loads(json.dumps(fixture))
        altered["lifecycle_identity"][field] = replacement
        reject(
            lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
            f"lifecycle identity {field} mutation",
        )
    coordinated = _lifecycle_checkpoint_fixture(uid=502, runtime_name="other-runtime")
    coordinated["lifecycle_identity"] = fixture["lifecycle_identity"]
    reject(
        lambda: validate_checkpoint_payload(coordinated, "G009-GATE-LIFECYCLE"),
        "lifecycle coordinated nested UID target runtime rewrite",
    )
    mutations = (
        ("publication staged path", ("lifecycle_sandbox", "publication", "staged_path"), "/alias"),
        ("publication metadata bytes", ("commands", 0, "metadata", "bytes"), 2),
        ("publication metadata digest", ("commands", 0, "metadata", "sha256"), "0" * 64),
        ("service policy name", ("lifecycle_sandbox", "policy_probe", "service_profile"), None),
        ("command policy name", ("lifecycle_sandbox", "policy_probe", "command_policy"), None),
        ("service policy digest", ("lifecycle_sandbox", "policy_probe", "service_profile", "policy_sha256"), "0" * 64),
        ("command policy digest", ("lifecycle_sandbox", "policy_probe", "command_policy", "policy_sha256"), "0" * 64),
        ("ancestor role", ("commands", 0, "protected_after", "canonical_ancestors", 0, "role"), "wrong"),
        ("ancestor removal", ("commands", 0, "protected_after", "canonical_ancestors"), []),
        ("runtime socket owner", ("commands", 0, "runtime_socket", "socket_owner_uid"), 0),
        ("login RunAtLoad", ("commands", 0, "login_load", "plist_run_at_load"), "false"),
        ("unexpected exit replacement pid", ("commands", 0, "unexpected_exit_relaunch", "after_pid"), 1),
        ("stop keepalive throttle", ("commands", 1, "keepalive_defeated", "throttle_interval_seconds"), 0),
        ("uninstall worktree digest", ("commands", 6, "worktree_state", "marker_sha256"), "0" * 64),
    )
    for label, path, replacement in mutations:
        altered = json.loads(json.dumps(fixture))
        cursor = altered
        for key in path[:-1]:
            cursor = cursor[key]
        if replacement is None:
            del cursor[path[-1]]
        else:
            cursor[path[-1]] = replacement
        reject(
            lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
            f"lifecycle checkpoint {label} mutation",
        )
    launchctl_mutations = {
        "target": f"gui/502/{LABEL}", "plist_path": "/alias.plist", "program": "/alias",
        "arguments": ["/alias", "--service"], "state": "stopped", "pid": 9,
        "stdout_sha256": "0" * 64, "stderr_sha256": "1" * 64,
        "stdout_base64": "", "stderr_base64": "AA==",
    }
    for owner, path in (
        ("install", ("commands", 0, "launchctl")),
        ("start", ("commands", 2, "launchctl")),
        ("restart", ("commands", 4, "launchctl")),
        ("idempotent install", ("commands", 0, "idempotent_install", "launchctl")),
        ("unexpected-exit relaunch", ("commands", 0, "unexpected_exit_relaunch", "launchctl")),
    ):
        for field, replacement in launchctl_mutations.items():
            altered = json.loads(json.dumps(fixture))
            cursor = altered
            for key in path:
                cursor = cursor[key]
            cursor[field] = replacement
            reject(
                lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
                f"lifecycle {owner} launchctl {field} mutation",
            )
    for owner, path in (
        ("install", ("commands", 0, "runtime_socket")),
        ("start", ("commands", 2, "runtime_socket")),
        ("restart", ("commands", 4, "runtime_socket")),
        ("idempotent install", ("commands", 0, "idempotent_install", "runtime_socket")),
        ("unexpected-exit relaunch", ("commands", 0, "unexpected_exit_relaunch", "runtime_socket")),
    ):
        for field, replacement in {
            "socket_path": "/tmp/unrelated/podwayd.sock", "socket_owner_uid": 502,
            "socket_mode": 0o644, "runtime_path": "/tmp/unrelated", "runtime_owner_uid": 502,
            "runtime_mode": 0o755, "target_uid": 502,
        }.items():
            altered = json.loads(json.dumps(fixture))
            cursor = altered
            for key in path:
                cursor = cursor[key]
            cursor[field] = replacement
            reject(
                lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
                f"lifecycle {owner} runtime socket {field} mutation",
            )
    for field, replacement in {
        "stale_socket_path": "/tmp/unrelated/podwayd.sock", "stale_socket_owner_uid": 502,
        "recovered_socket_path": "/tmp/unrelated/podwayd.sock", "recovered": False,
    }.items():
        altered = json.loads(json.dumps(fixture))
        altered["commands"][0]["unexpected_exit_relaunch"]["stale_socket_recovery"][field] = replacement
        reject(
            lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
            f"lifecycle stale-socket recovery {field} mutation",
        )
    for tuple_owner, tuple_path in (
        ("stop", ("commands", 1, "absence")),
        ("uninstall", ("commands", 6, "absence")),
        ("cleanup", ("lifecycle_sandbox", "cleanup", "absence")),
    ):
        for field, replacement in (("target", "gui/0/nope"), ("exit_code", "113"), ("stdout_sha256", 1), ("stderr_sha256", 1), ("stderr", 1)):
            altered = json.loads(json.dumps(fixture))
            cursor = altered
            for key in tuple_path:
                cursor = cursor[key]
            cursor[field] = replacement
            reject(
                lambda altered=altered: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
                f"lifecycle {tuple_owner} absence {field} mutation",
            )
    altered = json.loads(json.dumps(fixture))
    launch = altered["commands"][2]["launchctl"]
    launch["target"] = f"gui/502/{LABEL}"
    output = base64.b64decode(launch["stdout_base64"]).replace(
        f"gui/501/{LABEL}".encode("utf-8"), f"gui/502/{LABEL}".encode("utf-8")
    )
    launch["stdout_base64"] = base64.b64encode(output).decode("ascii")
    launch["stdout_sha256"] = sha256_bytes(output)
    reject(
        lambda: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
        "lifecycle coordinated target and launchctl output mutation",
    )
    altered = json.loads(json.dumps(fixture))
    runtime_socket = altered["commands"][2]["runtime_socket"]
    runtime_socket["runtime_path"] = "/g009-fixture/other-runtime"
    runtime_socket["socket_path"] = "/g009-fixture/other-runtime/podway-501/podwayd.sock"
    reject(
        lambda: validate_checkpoint_payload(altered, "G009-GATE-LIFECYCLE"),
        "lifecycle coordinated runtime and socket-root mutation",
    )
    _require_lifecycle_digest("a" * 64, "a" * 64, "publication")
    reject(
        lambda: _require_lifecycle_digest("0" * 64, "1" * 64, "publication"),
        "lifecycle publication hash mutation",
        "lifecycle publication digest differs from reconstructed bytes",
    )
    snapshot_path = Path("/g009-lifecycle-sentinel/controller-wrapper")
    snapshot = {
        "artifacts": {
            "controller_wrapper": {
                "role": "controller_wrapper", "path": str(snapshot_path),
                "sha256": "a" * 64, "mode": 0o700, "bytes": 1,
            },
        },
        "canonical_ancestors": [],
    }
    cursor = snapshot_path.parent
    while True:
        snapshot["canonical_ancestors"].append(
            {"role": "controller_wrapper", "path": str(cursor), "mode": 0o700, "device": 1, "inode": 1}
        )
        if cursor == cursor.parent:
            break
        cursor = cursor.parent
    snapshot["canonical_ancestors"].sort(key=lambda item: (item["path"], item["role"]))
    _validate_lifecycle_snapshot(snapshot, {"role", "path", "sha256", "mode", "bytes"})
    relabeled_snapshot = json.loads(json.dumps(snapshot))
    relabeled_snapshot["canonical_ancestors"][0]["role"] = "relabelled"
    reject(
        lambda: _validate_lifecycle_snapshot(relabeled_snapshot, {"role", "path", "sha256", "mode", "bytes"}),
        "lifecycle role ancestor-chain relabel",
    )
    expected_policy_rows = [
        {"boundary": "staged_service", "target_class": target, "operation": operation,
         "denied": True, "errno": 1, "diagnostic": "Operation not permitted"}
        for target in ("candidate_root", "staged_publication")
        for operation in ("transient_create_then_delete", "existing_overwrite", "rename", "symlink")
    ]
    policy_inputs = {
        "writable": ["/g009-lifecycle-sentinel/runtime"],
        "protected": ["/g009-lifecycle-sentinel/protected"],
    }
    policy_probes = {
        "service_profile": {"policy_sha256": "a" * 64, "operations": expected_policy_rows},
        "command_policy": {
            "policy_sha256": sha256_bytes(
                _reconstruct_lifecycle_command_policy(policy_inputs).encode("utf-8")
            ),
            "inputs": policy_inputs,
            "operations": [{**row, "boundary": "command_policy"} for row in expected_policy_rows],
        },
    }
    _validate_lifecycle_policy_probes(policy_probes, "a" * 64)
    altered_policy_probes = json.loads(json.dumps(policy_probes))
    altered_policy_probes["command_policy"]["operations"][0]["boundary"] = "staged_service"
    reject(
        lambda: _validate_lifecycle_policy_probes(altered_policy_probes, "a" * 64),
        "lifecycle policy boundary mutation",
        "lifecycle policy probe does not bind its exact boundary matrix",
    )
    equal_policy_probes = json.loads(json.dumps(policy_probes))
    equal_policy_probes["command_policy"]["policy_sha256"] = "a" * 64
    reject(
        lambda: _validate_lifecycle_policy_probes(equal_policy_probes, "a" * 64),
        "lifecycle equal service and command policy digests",
        "lifecycle command policy digest is not independently reconstructed",
    )
    absence = {
        "target": f"gui/501/{LABEL}", "exit_code": 113, "stdout_sha256": sha256_bytes(b""),
        "stderr": f'Bad request.\nCould not find service "{LABEL}" in domain for user gui: 501',
    }
    absence["stderr_sha256"] = sha256_bytes(absence["stderr"].encode("utf-8"))
    _validate_launchctl_absence(absence, absence["target"])
    altered_absence = dict(absence)
    altered_absence["exit_code"] = 3
    reject(
        lambda: _validate_launchctl_absence(altered_absence, absence["target"]),
        "lifecycle launchctl absence tuple mutation",
        "lifecycle launchctl absence tuple is not an accepted exact platform observation",
    )
    for field, mutation in {
        "target": "gui/0/invalid", "exit_code": "113", "stdout_sha256": 1,
        "stderr_sha256": 1, "stderr": 1,
    }.items():
        altered_absence = dict(absence)
        altered_absence[field] = mutation
        reject(
            lambda altered_absence=altered_absence: _validate_launchctl_absence(altered_absence, absence["target"]),
            f"lifecycle launchctl absence {field} mutation",
        )
    reject(
        lambda: _validate_lifecycle_snapshot({"artifacts": {}, "canonical_ancestors": []}, {"role", "path", "sha256", "mode", "bytes"}),
        "lifecycle missing protected identity evidence",
        "lifecycle protected identity snapshot is incomplete",
    )
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        fuzz_executable = base / "fuzz-executable"
        fuzz_executable.write_bytes(b"g009-fuzz-executable-sentinel")
        pre_run_digest = _fuzz_executable_sha256(fuzz_executable)
        if pre_run_digest != sha256_file(fuzz_executable):
            raise AssertionError("fuzz executable pre-run digest sentinel failed")
        fuzz_executable.write_bytes(b"g009-fuzz-executable-sentinel-mutated")
        reject(
            lambda: _require_fuzz_executable_unchanged(fuzz_executable, pre_run_digest),
            "fuzz executable post-run digest mutation",
            "fuzz target executable changed during untrusted execution",
        )
        fuzz_executable.write_bytes(b"g009-fuzz-executable-sentinel")
        linked_fuzz_executable = base / "linked-fuzz-executable"
        os.link(fuzz_executable, linked_fuzz_executable)
        reject(
            lambda: _fuzz_executable_sha256(fuzz_executable),
            "fuzz executable hard-link replacement",
            "built fuzz target executable is absent or unsafe",
        )
        linked_fuzz_executable.unlink()
        frozen_profile = load_json(ROOT / "release/g009-qualification-v1.json")
        validate_protocol(ROOT / "release/g009-qualification-v1.json")
        reject(
            lambda: trusted_qualification_profile("x86_64-apple-darwin"),
            "unsupported qualification target",
        )
        release_policy = load_json(ROOT / "release/g009-release-policy-v1.json")
        validate_trust_policy(release_policy)
        publication_surface = workflow_run_surface(
            (ROOT / ".github/workflows/release-publish.yml").read_text(encoding="utf-8")
        )
        reject(
            lambda: validate_release_publication_state_machine(
                publication_surface.replace(
                    "python3 tools/g009_publication.py",
                    "",
                    1,
                )
            ),
            "release workflow without pinned controller invocation",
            "release workflow must invoke only the pinned serialized controller",
        )
        publication_source = (ROOT / "tools/g009_publication.py").read_text(encoding="utf-8")
        reject(
            lambda: validate_release_publication_state_machine(
                publication_surface,
                controller_source=publication_source.replace('method="PATCH"', 'method="DELETE"', 1),
            ),
            "alternate destructive publication mutation",
            "publication controller normalized runtime binding drift",
        )
        rc_sentinel = {
            "schema": "podway.g009.rc-intent/v1",
            "target": TARGET,
            "target_tuple": target_tuple(TARGET),
            "minimum_macos": None,
            "rust": None,
            "source": None,
            "host": None,
            "inputs": None,
            "signing": None,
            "archive_root": None,
            "binaries": None,
        }
        for label, mutate, expected in (
            ("RC missing target tuple field", lambda value: value["target_tuple"].pop("arch"), "RC target tuple must contain exactly four fields"),
            ("RC extra target tuple field", lambda value: value["target_tuple"].__setitem__("unexpected", True), "RC target tuple must contain exactly four fields"),
            ("RC six-field profile target tuple", lambda value: value["target_tuple"].update({"native_required": True, "universal_forbidden": True}), "RC target tuple must contain exactly four fields"),
            ("RC malformed target tuple", lambda value: value.__setitem__("target_tuple", []), "RC target tuple is malformed"),
            ("RC wrong target tuple", lambda value: value["target_tuple"].__setitem__("triple", "x86_64-apple-darwin"), "RC target tuple differs from RC target"),
        ):
            candidate = base / f"{label.replace(' ', '-')}.json"
            mutated = json.loads(json.dumps(rc_sentinel))
            mutate(mutated)
            candidate.write_text(json.dumps(mutated), encoding="utf-8")
            reject(lambda candidate=candidate: load_rc(candidate), label, expected)
        downgraded_policy = json.loads(json.dumps(release_policy))
        downgraded_policy["tool_policy"]["identity_requirement"] = "version-sha256-and-native-target-execution"
        reject(lambda: validate_trust_policy(downgraded_policy), "identity requirement downgrade")
        tuple_mismatch_policy = json.loads(json.dumps(release_policy))
        tuple_mismatch_policy["native_platform"]["tuples"].pop()
        reject(lambda: validate_trust_policy(tuple_mismatch_policy), "native tuple omission")
        relabeled_profile = json.loads(json.dumps(frozen_profile))
        relabeled_profile["target"]["triple"] = "x86_64-apple-darwin"
        relabeled_path = base / "relabelled-profile.json"
        relabeled_path.write_text(json.dumps(relabeled_profile), encoding="utf-8")
        reject(lambda: validate_protocol(relabeled_path), "target tuple relabel")
        for label, mutate in (
            ("unknown fuzz seed", lambda value: value["fuzz"]["seeds"][0].__setitem__("target", "unknown")),
            ("missing fuzz seed", lambda value: value["fuzz"]["seeds"].pop()),
            ("swapped fuzz seed", lambda value: value["fuzz"]["seeds"].__setitem__(slice(0, 2), list(reversed(value["fuzz"]["seeds"][:2])))),
            ("mutated fuzz seed bytes", lambda value: value["fuzz"]["seeds"][0].__setitem__("sha256", "0" * 64)),
        ):
            mutated = json.loads(json.dumps(frozen_profile))
            mutate(mutated)
            reject(lambda mutated=mutated: fuzz_seeds(mutated), label)
        smoke_mutation = json.loads(json.dumps(frozen_profile))
        smoke_mutation["fuzz"]["local_smoke"]["seconds_per_target"] = 6
        smoke_path = base / "fuzz-smoke-mutation.json"
        smoke_path.write_text(json.dumps(smoke_mutation), encoding="utf-8")
        reject(lambda: validate_protocol(smoke_path), "fuzz local smoke mutation")
        profile_digest = sha256_bytes(canonical_json(frozen_profile))
        rc_limits = {"stream_bytes": 1024 * 1024, "aggregate_bytes": 2 * 1024 * 1024,
                     "max_total_time": 3600, "timeout_seconds": 5, "rss_limit_mb": 512}
        smoke_limits = {**rc_limits, "max_total_time": 5}
        reject(lambda: validate_fuzz_policy_binding("local_smoke", profile_digest, rc_limits, frozen_profile),
               "fuzz policy mode/limit swap")
        reject(lambda: validate_fuzz_policy_binding("rc", profile_digest, smoke_limits, frozen_profile),
               "fuzz policy limit/mode swap")
        reject(lambda: validate_fuzz_policy_binding("rc", "0" * 64, rc_limits, frozen_profile),
               "fuzz policy profile digest swap")
        reject(lambda: validate_fuzz_policy_binding("rc", profile_digest, {**rc_limits, "unknown": 1}, frozen_profile),
               "fuzz unknown policy field")
        reject(lambda: trusted_fuzz_profile("x86_64-apple-darwin"), "unsupported fuzz native target")
        reject(lambda: trusted_fuzz_profile(None), "fuzz missing native target")
        for label, value, pattern in (
            ("fuzz manifest traversal", "../escape", r"fuzz-corpus-manifests/[0-9a-f]{64}\.json"),
            ("fuzz blob traversal", "fuzz/blobs/../x.bin", r"fuzz/blobs/[0-9a-f]{64}\.bin"),
            ("fuzz root swap", "fuzz\\blobs\\x.bin", r".+"),
        ):
            reject(lambda value=value, pattern=pattern: _fuzz_relative_path(value, pattern, "fuzz sentinel"), label)
        corpus_limits = {
            "corpus_member_count": 1, "corpus_path_depth": 10, "corpus_path_length": 64,
        }
        symlinked_corpus = base / "symlinked-corpus"
        symlinked_corpus.mkdir()
        (base / "corpus-target").mkdir()
        os.symlink(base / "corpus-target", symlinked_corpus / "linked")
        reject(
            lambda: _bounded_corpus_members(symlinked_corpus, corpus_limits),
            "fuzz corpus symlinked directory",
            "current fuzz corpus contains a symlink",
        )
        excessive_directories = base / "excessive-directories"
        excessive_directories.mkdir()
        for number in range(11):
            (excessive_directories / str(number)).mkdir()
        reject(
            lambda: _bounded_corpus_members(excessive_directories, corpus_limits),
            "fuzz corpus directory count",
            "current fuzz corpus member count exceeds frozen limit",
        )
        oversized_bundle = base / "oversized-bundle.zip"
        with zipfile.ZipFile(oversized_bundle, "w") as archive:
            archive.writestr("member", b"x")
        with zipfile.ZipFile(oversized_bundle) as archive:
            archive.infolist()[0].file_size = QUALIFICATION_ZIP_MEMBER_MAX_BYTES + 1
            reject(
                lambda: _preflight_qualification_bundle(archive),
                "qualification bundle member preflight",
                "qualification bundle member exceeds uncompressed size limit",
            )
        aggregate_bundle = base / "aggregate-bundle.zip"
        with zipfile.ZipFile(aggregate_bundle, "w") as archive:
            for number in range(5):
                archive.writestr(str(number), b"x")
        with zipfile.ZipFile(aggregate_bundle) as archive:
            for info in archive.infolist():
                info.file_size = QUALIFICATION_ZIP_MEMBER_MAX_BYTES
            reject(
                lambda: _preflight_qualification_bundle(archive),
                "qualification bundle aggregate preflight",
                "qualification bundle aggregate exceeds uncompressed size limit",
            )
        member_count_bundle = base / "member-count-bundle.zip"
        with zipfile.ZipFile(member_count_bundle, "w") as archive:
            for number in range(QUALIFICATION_ZIP_MEMBER_COUNT_MAX + 1):
                archive.writestr(f"{number:04d}", b"")
        with zipfile.ZipFile(member_count_bundle) as archive:
            reject(
                lambda: _preflight_qualification_bundle(archive),
                "qualification bundle member-count preflight",
                "qualification bundle member count exceeds frozen limit",
            )
        reject(
            lambda: validate_fuzz_gate(
                {"provenance": {}, "commands": [{"target": "frame_decoder"} for _ in FUZZ_TARGETS]},
                base,
            ),
            "fuzz missing provenance",
        )
        archive_corpora: set[str] = set()
        _require_unique_bundled_fuzz_corpus("artifacts/g009/fuzz/corpus/frame_decoder", archive_corpora)
        reject(
            lambda: _require_unique_bundled_fuzz_corpus("artifacts/g009/fuzz/corpus/not/canonical", set()),
            "immutable archive malformed fuzz corpus",
        )
        reject(
            lambda: _require_unique_bundled_fuzz_corpus("artifacts/g009/fuzz/corpus/frame_decoder", archive_corpora),
            "immutable archive reused fuzz corpus",
        )
        toolchain = {
            "channel": frozen_profile["fuzz"]["toolchain"]["channel"],
            "rustc": frozen_profile["fuzz"]["toolchain"]["rustc"],
            "tools": [
                {"id": tool_id, "path": f"/immutable/{tool_id}", "sha256": "0" * 64}
                for tool_id in FUZZ_TOOL_IDS
            ],
        }
        _validate_fuzz_toolchain_records(toolchain, frozen_profile["fuzz"]["toolchain"])
        duplicate_tools = json.loads(json.dumps(toolchain))
        duplicate_tools["tools"][2]["id"] = "cargo"
        reject(
            lambda: _validate_fuzz_toolchain_records(
                duplicate_tools, frozen_profile["fuzz"]["toolchain"],
            ),
            "immutable archive duplicate fuzz tool record",
        )
        manifest_receipt = {"host": host_manifest(), "run_identity": "0" * 64}
        manifest = {
            "schema": "podway.g009.fuzz-corpus-manifest/v1",
            "host": manifest_receipt["host"],
            "target": "frame_decoder",
            "phase": "before",
            "corpus": "artifacts/g009/fuzz/corpus/frame_decoder",
            "files": [],
            "run_identity": manifest_receipt["run_identity"],
        }
        _validate_fuzz_manifest_binding(
            manifest, manifest_receipt, "frame_decoder", manifest["corpus"], "before",
        )
        manifest_host_drift = json.loads(json.dumps(manifest))
        manifest_host_drift["host"] = {"drift": True}
        reject(
            lambda: _validate_fuzz_manifest_binding(
                manifest_host_drift, manifest_receipt, "frame_decoder", manifest["corpus"], "before",
            ),
            "immutable archive fuzz manifest host drift",
        )
        verified_archive = b"strictly-verified-qualification-archive"
        archive_authority = {"qualification_archive_sha256": sha256_bytes(verified_archive)}
        _validate_final_archive_binding(
            verified_archive, archive_authority, archive_authority, archive_authority,
        )
        reject(
            lambda: _validate_final_archive_binding(
                b"replaced-qualification-archive",
                archive_authority,
                archive_authority,
                archive_authority,
            ),
            "replaced qualification archive bytes",
        )
        passing_status = {"status": "pass"}
        _require_bundled_fuzz_pass_status(passing_status, passing_status, passing_status)
        for command_status in ("pass", "fail"):
            for receipt_ref_status in ("pass", "fail"):
                for receipt_status in ("pass", "fail"):
                    if (command_status, receipt_ref_status, receipt_status) == ("pass", "pass", "pass"):
                        continue
                    reject(
                        lambda command_status=command_status, receipt_ref_status=receipt_ref_status, receipt_status=receipt_status:
                        _require_bundled_fuzz_pass_status(
                            {"status": command_status},
                            {"status": receipt_ref_status},
                            {"status": receipt_status},
                        ),
                        f"contradictory bundled fuzz statuses {command_status}/{receipt_ref_status}/{receipt_status}",
                    )
        duplicate = base / "duplicate.json"; duplicate.write_text('{"x":1,"x":2}', encoding="utf-8")
        reject(lambda: load_json(duplicate), "duplicate JSON key")
        reject(lambda: canonical_json({"measurement": 1.0}), "rounded metric")
        reject(lambda: safe_extract_member("../escape", TARGET), "unsafe extraction")
        reject(lambda: safe_extract_member("other/bin/podway", TARGET), "wrong archive root")
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
        reject(lambda: inspect_archive(recursive, target=TARGET), "recursive member checksum", "internal payload manifest mismatch")
        escaping = base / "escaping.zip"
        with zipfile.ZipFile(escaping, "w") as archive:
            info = zipfile.ZipInfo("../outside")
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            archive.writestr(info, b"x")
        escaping.with_name(escaping.name + ".sha256").write_text(sha256_file(escaping) + "\n", encoding="ascii")
        reject(lambda: inspect_archive(escaping, target=TARGET), "archive traversal", "unsafe relative path")
    candidate_directory = tempfile.TemporaryDirectory()
    immutable_directory = tempfile.TemporaryDirectory()
    scratch_directory = tempfile.TemporaryDirectory()
    candidate_root = Path(candidate_directory.name)
    immutable_root = Path(immutable_directory.name)
    previous_candidate_root = os.environ.get("G009_CANDIDATE_ROOT")
    previous_immutable_root = os.environ.get("G009_IMMUTABLE_INPUT_ROOT")
    previous_scratch_root = os.environ.get("G009_SCRATCH_ROOT")
    os.environ["G009_CANDIDATE_ROOT"] = str(candidate_root)
    os.environ["G009_IMMUTABLE_INPUT_ROOT"] = str(immutable_root)
    os.environ["G009_SCRATCH_ROOT"] = scratch_directory.name
    try:
        (immutable_root / "input").write_bytes(b"immutable")
        source_directory = candidate_root / "src"
        source_directory.mkdir()
        source_file = source_directory / "lib.rs"
        source_file.write_text("pub fn stable() {}\n", encoding="utf-8")
        source_before = _candidate_source_manifest()
        marker = candidate_root / "marker"
        malicious_build = candidate_root / "build.rs"
        malicious_build.write_text(
            f'fn main() {{ std::fs::write("{marker}", "executed").unwrap(); }}\n',
            encoding="utf-8",
        )
        reject(
            _validate_candidate_build_surface,
            "marker-writing build script before Cargo execution",
            "candidate build scripts are forbidden",
        )
        if marker.exists():
            raise AssertionError("build-script marker existed before rejected Cargo execution")
        malicious_build.unlink()
        proc_macro_directory = candidate_root / "malicious-proc-macro"
        proc_macro_directory.mkdir()
        (proc_macro_directory / "Cargo.toml").write_text(
            '[package]\nname = "malicious-proc-macro"\nversion = "0.1.0"\n'
            '[lib]\nproc-macro = true\n',
            encoding="utf-8",
        )
        reject(
            _validate_candidate_build_surface,
            "marker-writing proc-macro before Cargo execution",
            "candidate-defined build hooks are forbidden",
        )
        if marker.exists():
            raise AssertionError("proc-macro marker existed before rejected Cargo execution")
        shutil.rmtree(proc_macro_directory)
        cargo_directory = candidate_root / ".cargo"
        cargo_directory.mkdir()
        (cargo_directory / "config.toml").write_text(
            '[alias]\nmarker = "!touch marker"\n',
            encoding="utf-8",
        )
        reject(
            _validate_candidate_build_surface,
            "marker-writing Cargo alias before Cargo execution",
            "candidate-local Cargo configuration is forbidden",
        )
        if marker.exists():
            raise AssertionError("Cargo configuration marker existed before rejected Cargo execution")
        shutil.rmtree(cargo_directory)
        attempted_marker = source_directory / "attempted-marker"
        result = bounded_process(
            sandboxed_candidate_argv((
                sys.executable,
                "-c",
                "from pathlib import Path; Path(__import__('sys').argv[1]).write_text('mutated')",
                str(attempted_marker),
            )),
            cwd=candidate_root,
            env={"PATH": os.environ.get("PATH", "")},
            timeout=5,
            stream_limit=1024,
            aggregate_limit=2048,
        )
        if result["terminal_mode"] == "success" or attempted_marker.exists():
            raise AssertionError("candidate source mutation escaped Cargo containment sentinel")
        if _candidate_source_manifest() != source_before:
            raise AssertionError("candidate source changed after rejected mutation sentinel")
        fixtures = (
            ((sys.executable, "-c", "import sys;sys.stdout.buffer.write(b'ok')"), 1.0, "completed", 0),
            ((sys.executable, "-c", "import sys;sys.exit(7)"), 1.0, "completed", 7),
            ((sys.executable, "-c", "import time;time.sleep(60)"), 0.01, "timeout", None),
            ((sys.executable, "-c", "import sys;sys.stdout.buffer.write(b'x'*(1024*1024+1))"), 1.0, "output_overflow", None),
        )
        for argv, timeout, reason, exit_code in fixtures:
            captured = bounded_process(
                argv,
                cwd=ROOT,
                env=dict(os.environ),
                timeout=timeout,
                stream_limit=1024 * 1024,
                aggregate_limit=2 * 1024 * 1024,
            )
            if captured["termination_reason"] != reason or (exit_code is not None and captured["exit_code"] != exit_code):
                raise AssertionError(f"fuzz streaming sentinel failed: {reason}")
    finally:
        if previous_candidate_root is None:
            os.environ.pop("G009_CANDIDATE_ROOT", None)
        else:
            os.environ["G009_CANDIDATE_ROOT"] = previous_candidate_root
        if previous_immutable_root is None:
            os.environ.pop("G009_IMMUTABLE_INPUT_ROOT", None)
        else:
            os.environ["G009_IMMUTABLE_INPUT_ROOT"] = previous_immutable_root
        if previous_scratch_root is None:
            os.environ.pop("G009_SCRATCH_ROOT", None)
        else:
            os.environ["G009_SCRATCH_ROOT"] = previous_scratch_root
        scratch_directory.cleanup()
        candidate_directory.cleanup()
        immutable_directory.cleanup()
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
    parser.add_argument("--product-acceptance-matrix", help="product acceptance matrix to validate")
    parser.add_argument("--g036-test-report", help="strict G036 direct-evidence report to validate")
    parser.add_argument("--traceability", help="G009 traceability registry to validate")
    parser.add_argument("--crash-registry", help="crash registry to validate when it declares coverage")
    parser.add_argument("--qualification-bundle", help="stage-1 qualification-bundle.json descriptor")
    parser.add_argument("--final-review", help="canonical stage-2 final review")
    parser.add_argument("--evidence-root", help="root containing controller evidence")
    parser.add_argument("--fuzz-receipt", help="fuzz receipt to validate with --evidence-root")
    parser.add_argument("--fuzz-gate", help="aggregate fuzz gate to validate with --evidence-root")
    parser.add_argument("--reviewer-keyring", help="repository-controlled final-review keyring")
    parser.add_argument("--reviewer-keyring-sha256", help="SHA-256 of --reviewer-keyring")
    parser.add_argument("--attestation", action="append", default=[], help="ROLE=PAYLOAD=SIGNATURE; repeat owner/E/F")
    parser.add_argument("--reviewer-fingerprint", action="append", default=[], help="ROLE=40-UPPERCASE-HEX primary fingerprint; repeat owner/E/F")
    parser.add_argument("--receipt-out", help="immutable strict-verifier receipt path under --evidence-root")
    parser.add_argument("--invocation-nonce", help="controller-created 64-hex nonce for this qualification invocation")
    args = parser.parse_args()
    try:
        final_requested = any((args.qualification_bundle, args.final_review))
        if not any((args.self_test, args.protocol, args.product_acceptance_matrix, args.g036_test_report, args.traceability, args.crash_registry, args.fuzz_receipt, args.fuzz_gate, final_requested)): parser.error("supply --self-test and/or validation inputs")
        if args.protocol: validate_protocol(Path(args.protocol))
        if args.product_acceptance_matrix: validate_product_acceptance_matrix(Path(args.product_acceptance_matrix))
        if args.g036_test_report: validate_g036_test_report(Path(args.g036_test_report))
        if args.traceability: validate_traceability(Path(args.traceability))
        if args.crash_registry: validate_crash_registry(Path(args.crash_registry))
        if args.fuzz_receipt:
            if not args.evidence_root: parser.error("--fuzz-receipt requires --evidence-root")
            validate_fuzz_receipt(Path(args.fuzz_receipt), Path(args.evidence_root))
        if args.fuzz_gate:
            if not args.evidence_root: parser.error("--fuzz-gate requires --evidence-root")
            validate_fuzz_gate(load_json(Path(args.fuzz_gate)), Path(args.evidence_root))
        receipt: dict[str, Any] | None = None
        if final_requested:
            if not all((args.qualification_bundle, args.final_review, args.evidence_root, args.reviewer_keyring, args.reviewer_keyring_sha256, args.invocation_nonce)) or len(args.attestation) != 3 or len(args.reviewer_fingerprint) != 3:
                parser.error("final validation requires --qualification-bundle --final-review --evidence-root --reviewer-keyring --reviewer-keyring-sha256 --invocation-nonce and three --attestation/--reviewer-fingerprint bindings")
            receipt = validate_final(Path(args.qualification_bundle), Path(args.final_review), Path(args.evidence_root), Path(args.reviewer_keyring), args.reviewer_keyring_sha256, args.attestation, args.reviewer_fingerprint, args.invocation_nonce)
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
