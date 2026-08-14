#!/usr/bin/env python3
"""Validate the current Procedure v2 quality and crash-boundary contracts."""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
CRASH_PATH = ROOT / "quality/crash-boundaries-v1.json"
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
            "assets/schemas/session-start-result-v2.schema.json",
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
            "assets/schemas/compact-status-result-v2.schema.json",
            "v2 compact-status schema",
        )
    )
    status = load_object(
        repository_file("assets/schemas/status-result-v2.schema.json", "v2 status schema")
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
            "assets/schemas/v2-result-components-v1.schema.json",
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


def locator_parts(locator: Any, label: str) -> tuple[Path, str]:
    if not isinstance(locator, str) or "::" not in locator:
        fail(f"{label} must be PATH::SYMBOL")
    relative, symbol = locator.split("::", 1)
    if not symbol:
        fail(f"{label} has an empty symbol")
    return repository_file(relative, label), symbol


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
        if source_symbol.rsplit("::", 1)[-1] not in source_text:
            fail(f"{boundary_id} crash source symbol is missing")
    return len(windows)


def main() -> int:
    try:
        semantic_checks = validate_v2_contract_semantics()
        crash_windows = validate_crash_registry()
    except ContractError as error:
        print(f"quality contract verification failed: {error}")
        return 1
    print(
        "quality contracts verified: "
        f"{semantic_checks} v2 semantic checks, {crash_windows} crash windows"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
