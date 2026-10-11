#!/usr/bin/env python3
"""Validate the current Procedure v2 quality and crash-boundary contracts."""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
CRASH_PATH = ROOT / "quality/crash-boundaries-v1.json"
REFERENCE_PATHS = ROOT / "tests/fixtures/v2/presets/reference-paths.json"
PAYLOAD_RECIPES = ROOT / "tests/fixtures/v2/payload"
FUNCTION_RE_TEMPLATE = r"\bfn\s+{name}\s*(?:<[^>]*>)?\s*\("


class ContractError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise ContractError(message)


def load_object(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"cannot read {path.relative_to(ROOT)}: {error}")
    if not isinstance(value, dict):
        fail(f"{path.relative_to(ROOT)} must contain a JSON object")
    return value


def repository_file(relative: Any, label: str) -> Path:
    if not isinstance(relative, str) or not relative or Path(relative).is_absolute():
        fail(f"{label} must be a non-empty repository-relative path")
    path = (ROOT / relative).resolve()
    if not path.is_relative_to(ROOT) or path.is_symlink() or not path.is_file():
        fail(f"{label} does not resolve to a regular repository file: {relative}")
    return path


def validate_v2_contract_semantics() -> int:
    session_start = load_object(
        repository_file(
            "assets/schemas/session-start-result-v4.schema.json",
            "v2 session-start schema",
        )
    )
    session_required = set(session_start.get("required", []))
    session_properties = session_start.get("properties", {})
    if (
        "goal_defined" not in session_required
        or "goal_defined" not in session_properties
        or "goal_required" in session_required
        or "goal_required" in session_properties
    ):
        fail("v2 session-start must require goal_defined and reject goal_required")

    compact_status = load_object(
        repository_file(
            "assets/schemas/compact-status-result-v5.schema.json",
            "v2 compact-status schema",
        )
    )
    status = load_object(
        repository_file("assets/schemas/status-result-v5.schema.json", "v2 status schema")
    )
    for label, schema in (("compact status", compact_status), ("status", status)):
        if "blockers" in schema.get("required", []) or "blockers" in schema.get(
            "properties", {}
        ):
            fail(f"v2 {label} must not expose a root blockers collection")
    if not {"blocker_window", "blockers_truncated"}.issubset(
        set(status.get("required", []))
    ):
        fail("v2 standard and verbose status must require the blocker window markers")

    components = load_object(
        repository_file(
            "assets/schemas/v2-result-components-v4.schema.json",
            "v2 result components schema",
        )
    )
    definitions = components.get("$defs", {})
    current_properties = definitions.get("currentIdentity", {}).get("properties", {})
    if "blockers_total" not in current_properties or "compactBlocker" in definitions:
        fail("v2 status components must retain blockers_total without compactBlocker")

    graph_cases = load_object(
        repository_file("tests/fixtures/v2/graphs/valid-cases.json", "v2 graph fixtures")
    )
    case_ids = {
        case.get("id")
        for case in graph_cases.get("cases", [])
        if isinstance(case, dict)
    }
    if "terminal-path-through-rework" not in case_ids:
        fail("v2 valid graph fixtures must cover terminal reachability through rework")
    return 6


def validate_reference_paths() -> int:
    fixture = load_object(REFERENCE_PATHS)
    if fixture.get("schema") != "podway.v2-fixture-recipe/v1":
        fail("reference path fixture schema is unsupported")
    if (
        fixture.get("fixture_class") != "known-answer"
        or fixture.get("evidence_level") != "contract-recipe"
        or fixture.get("implementation_status") != "automated"
    ):
        fail("reference path fixture metadata is invalid")
    cases = fixture.get("cases")
    if not isinstance(cases, list) or not cases:
        fail("reference path fixture must contain cases")

    expected = {
        "analysis-v2": {
            "analysis-clean", "analysis-evidence-gap-rework",
            "analysis-source-review-rework", "analysis-analysis-review-rework",
            "analysis-source-findings-missing", "analysis-analysis-findings-missing",
            "analysis-goal-not-achieved", "analysis-goal-superseded",
            "analysis-manual-define", "analysis-manual-collect",
            "analysis-manual-analyze", "analysis-manual-challenge",
            "analysis-manual-synthesize", "analysis-manual-review",
        },
        "small-change-v2": {
            "small-clean", "small-review-rework", "small-manual-inspect",
            "small-manual-implement", "small-manual-verify",
        },
        "bug-fix-v2": {
            "bug-clean", "bug-verification-retry", "bug-review-rework",
            "bug-goal-not-achieved", "bug-goal-superseded",
            "bug-manual-reproduce", "bug-manual-diagnose",
            "bug-manual-implement", "bug-manual-verify", "bug-manual-review",
        },
        "sw-dev-v2": {
            "sw-clean", "sw-verification-retry",
            "sw-review-implementation-rework", "sw-review-documentation-rework",
            "sw-documentation-updated", "sw-documentation-summary-missing",
            "sw-verification-observations-missing", "sw-paging-continuation",
            "sw-paging-stale-token", "sw-goal-not-achieved",
            "sw-goal-superseded", "sw-manual-plan", "sw-manual-implement",
            "sw-manual-verify", "sw-manual-document", "sw-manual-review",
        },
    }
    observed = {preset_id: set() for preset_id in expected}
    required_fields = {"id", "preset_id", "operation", "expected"}
    for index, case in enumerate(cases):
        if not isinstance(case, dict) or set(case) != required_fields:
            fail(f"reference path case {index} has unexpected or missing fields")
        if any(not isinstance(case[field], str) or not case[field] for field in required_fields):
            fail(f"reference path case {index} fields must be non-empty strings")
        preset_id = case["preset_id"]
        if preset_id not in observed or case["id"] in observed[preset_id]:
            fail(f"reference path case {index} has an unknown preset or duplicate ID")
        observed[preset_id].add(case["id"])
    if observed != expected:
        fail("reference path fixture does not cover the exact accepted path inventory")
    case_ids = set().union(*observed.values())
    runtime_proofs = fixture.get("runtime_proofs")
    if not isinstance(runtime_proofs, dict) or set(runtime_proofs) != case_ids:
        fail("reference path fixture must bind every accepted path to one runtime proof")
    for case_id, locator in runtime_proofs.items():
        proof_path, proof_symbol = locator_parts(locator, f"{case_id} runtime proof")
        proof_text = proof_path.read_text(encoding="utf-8")
        if proof_path.suffix == ".rs":
            pattern = FUNCTION_RE_TEMPLATE.format(name=re.escape(proof_symbol))
        elif proof_path.suffix == ".py":
            pattern = rf"\bdef\s+{re.escape(proof_symbol)}\s*\("
        else:
            fail(f"{case_id} runtime proof must be Rust or Python")
        if re.search(pattern, proof_text) is None:
            fail(f"{case_id} runtime proof function is missing")
        if proof_path.suffix == ".rs":
            require_test_route(proof_path, proof_symbol)
    return len(cases)


def locator_parts(locator: Any, label: str) -> tuple[Path, str]:
    if not isinstance(locator, str) or "::" not in locator:
        fail(f"{label} must be PATH::SYMBOL")
    relative, symbol = locator.split("::", 1)
    if not symbol:
        fail(f"{label} has an empty symbol")
    return repository_file(relative, label), symbol


def require_test_route(path: Path, function: str) -> None:
    """A proof must be a registered test with a documented executable route."""
    from run_native_qualification import CASES
    from verify_test_layout import target_sources

    crate = path.relative_to(ROOT).parts[1]
    manifest = tomllib.loads((ROOT / "crates" / crate / "Cargo.toml").read_text())
    selected = None
    for target in manifest.get("test", []):
        if target["name"].startswith(("arch_", "int_", "e2e_")) and path in target_sources(ROOT / "crates" / crate / target["path"]):
            selected = target["name"]
            break
    # Library and binary unit tests have the standard --lib/--bins route.
    if selected is None and "src" not in path.relative_to(ROOT / "crates" / crate).parts:
        fail(f"unregistered Rust test source: {path.relative_to(ROOT)}")
    match = re.search(
        rf"(?P<attributes>(?:#\[[^\]]*\]\s*)+)fn\s+{re.escape(function.rsplit('::', 1)[-1])}\s*\(",
        path.read_text(),
    )
    if match is None or "#[test]" not in match["attributes"]:
        fail(f"proof locator is not a Rust test: {path.relative_to(ROOT)}::{function}")
    if "#[ignore" in match["attributes"] and (
        crate, f"{path.stem}::{function}"
    ) not in CASES:
        fail(f"ignored proof has no documented native execution route: {path.relative_to(ROOT)}::{function}")


def validate_payload_proof(proof: Any, label: str) -> int:
    if not isinstance(proof, dict):
        fail(f"{label} must be an object")
    kind = proof.get("kind")
    if kind == "cargo-test-set":
        members = proof.get("members")
        if not isinstance(members, list) or not members:
            fail(f"{label} cargo-test-set must contain members")
        return sum(
            validate_payload_proof(member, f"{label} member {index}")
            for index, member in enumerate(members)
        )
    if kind not in {"cargo-test", "cargo-unit-test"}:
        fail(f"{label} has unsupported proof kind {kind!r}")
    path = repository_file(proof.get("path"), f"{label} path")
    function = proof.get("function")
    command = proof.get("command")
    if not isinstance(function, str) or not function:
        fail(f"{label} function must be a non-empty string")
    if not isinstance(command, str) or not command:
        fail(f"{label} command must be a non-empty string")
    if function not in command:
        fail(f"{label} command does not select its function")
    source = path.read_text(encoding="utf-8")
    if re.search(FUNCTION_RE_TEMPLATE.format(name=re.escape(function)), source) is None:
        fail(f"{label} function is missing")
    require_test_route(path, function)
    return 1


def validate_payload_recipes() -> int:
    proof_count = 0
    paths = sorted(PAYLOAD_RECIPES.glob("*.json"))
    if not paths:
        fail("payload recipe inventory is empty")
    for path in paths:
        recipe = load_object(path)
        if recipe.get("schema") != "podway.v2-fixture-recipe/v1":
            fail(f"{path.relative_to(ROOT)} schema is unsupported")
        recipe_proofs = 0
        if "production_proof" in recipe:
            recipe_proofs += validate_payload_proof(
                recipe["production_proof"],
                f"{path.relative_to(ROOT)} production proof",
            )
        production_proofs = recipe.get("production_proofs")
        if production_proofs is not None:
            if not isinstance(production_proofs, dict) or not production_proofs:
                fail(f"{path.relative_to(ROOT)} production_proofs must be a non-empty object")
            for proof_name, proof in production_proofs.items():
                recipe_proofs += validate_payload_proof(
                    proof,
                    f"{path.relative_to(ROOT)} production proof {proof_name}",
                )
        cases = recipe.get("cases")
        if cases is not None:
            if not isinstance(cases, list) or not cases:
                fail(f"{path.relative_to(ROOT)} cases must be a non-empty array")
            for index, case in enumerate(cases):
                if not isinstance(case, dict):
                    fail(f"{path.relative_to(ROOT)} case {index} must be an object")
                case_id = case.get("id")
                if not isinstance(case_id, str) or not case_id:
                    fail(f"{path.relative_to(ROOT)} case {index} must have an ID")
                recipe_proofs += validate_payload_proof(
                    case.get("proof"), f"{path.relative_to(ROOT)} case {case_id} proof"
                )
        if recipe_proofs == 0:
            fail(f"{path.relative_to(ROOT)} contains no production proof locator")
        proof_count += recipe_proofs
    return proof_count


def validate_crash_registry() -> int:
    registry = load_object(CRASH_PATH)
    if registry.get("schema") != "podway.crash-boundaries/v1" or registry.get("version") != 1:
        fail("crash registry schema or version is unsupported")
    coverage = registry.get("coverage")
    windows = registry.get("windows")
    if not isinstance(coverage, dict) or not isinstance(windows, list):
        fail("crash registry coverage or windows is missing")
    observed = [window.get("id") for window in windows if isinstance(window, dict)]
    expected = coverage.get("required")
    if (
        not isinstance(expected, list)
        or not expected
        or len(expected) != len(set(expected))
        or coverage.get("covered") != expected
        or coverage.get("percent") != 100
        or observed != expected
    ):
        fail("crash registry must provide exact ordered 100% coverage")
    required_proof = {
        "failpoint",
        "test",
        "termination",
        "recovery",
        "invariant",
        "source_locator",
    }
    for window in windows:
        boundary_id = window["id"]
        proof = window.get("proof")
        if not isinstance(proof, dict) or set(proof) != required_proof:
            fail(f"{boundary_id} crash proof has unexpected or missing fields")
        if any(not isinstance(value, str) or not value for value in proof.values()):
            fail(f"{boundary_id} crash proof fields must be non-empty strings")
        test_path, test_function = locator_parts(proof["test"], f"{boundary_id} test")
        source_path, source_symbol = locator_parts(
            proof["source_locator"], f"{boundary_id} source locator"
        )
        test_text = test_path.read_text(encoding="utf-8")
        source_text = source_path.read_text(encoding="utf-8")
        if re.search(
            FUNCTION_RE_TEMPLATE.format(name=re.escape(test_function)), test_text
        ) is None:
            fail(f"{boundary_id} crash test function is missing")
        require_test_route(test_path, test_function)
        if source_symbol.rsplit("::", 1)[-1] not in source_text:
            fail(f"{boundary_id} crash source symbol is missing")
    return len(windows)


def main() -> int:
    try:
        semantic_checks = validate_v2_contract_semantics()
        reference_paths = validate_reference_paths()
        payload_proofs = validate_payload_recipes()
        crash_windows = validate_crash_registry()
    except ContractError as error:
        print(f"quality contract verification failed: {error}")
        return 1
    print(
        "quality contracts verified: "
        f"{semantic_checks} v2 semantic checks, {reference_paths} reference paths, "
        f"{payload_proofs} payload proofs, "
        f"{crash_windows} crash windows"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
