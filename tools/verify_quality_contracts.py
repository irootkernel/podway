#!/usr/bin/env python3
"""Validate the generic product-acceptance and crash-boundary contracts."""

from __future__ import annotations

import copy
import hashlib
import json
import re
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
MATRIX_PATH = ROOT / "release/product-acceptance-matrix-v1.json"
DOLGI_MATRIX_PATH = ROOT / "release/dolgorae-acceptance-matrix-v1.json"
CRASH_PATH = ROOT / "quality/crash-boundaries-v1.json"
V2_ACCEPTANCE_PATH = ROOT / "quality/v2-acceptance-matrix-v1.json"
V2_COMPATIBILITY_PATH = ROOT / "quality/v2-compatibility-matrix-v1.json"
V2_PAYLOAD_PATH = ROOT / "quality/v2-payload-matrix-v1.json"
V2_COMPATIBILITY_FIXTURE_PATH = ROOT / "tests/fixtures/v2/compatibility/v1-boundaries.json"
V2_MAXIMUM_STATUS_FIXTURE_PATH = ROOT / "tests/fixtures/v2/payload/maximum-status-recipe.json"
V2_RELEASE_PATH = ROOT / "release/v2-release-gate-matrix-v1.json"
V2_SECTION_COUNTS = {
    "17.1": 6,
    "17.2": 14,
    "17.3": 16,
    "17.4": 13,
    "17.5": 7,
    "17.6": 9,
    "17.7": 7,
    "17.8": 4,
    "17.9": 11,
}
FUNCTION_RE_TEMPLATE = r"\bfn\s+{name}\s*(?:<[^>]*>)?\s*\("
TEST_FUNCTION_RE_TEMPLATE = (
    r"#\s*\[\s*test\s*\]\s*(?:#\s*\[[^\]]+\]\s*)*"
    r"(?:pub(?:\([^)]*\))?\s+)?fn\s+{name}\s*\("
)
PYTHON_FUNCTION_RE_TEMPLATE = r"^def\s+{name}\s*\("
DOLGI_TASKS = {
    "DOLGI001": {
        "title": "Build the controlled-PATH integration harness",
        "goal": "Complete AUT-T-PATH by installing through a controlled PATH from a sanitized arbitrary directory and verifying sibling, explicit, and PATH daemon resolution plus the canonical absolute plist path.",
        "reference_ids": ["AUT-PATH-001–003", "AUT-DAEMON-001–003", "AUT-T-PATH"],
        "evidence_ids": ["AUT-T-PATH"],
    },
    "DOLGI002": {
        "title": "Verify service and quiescent observation",
        "goal": "Install the daemon, connect through an explicit socket, initialize a worktree, and obtain compact idle status.",
        "reference_ids": ["AUT-SOCK-001–004", "AUT-SOCK-005", "AUT-OBS-001", "AUT-OBS-002–004"],
        "evidence_ids": ["AUT-T-OBS"],
    },
    "DOLGI003": {
        "title": "Verify session and item operations",
        "goal": "Exercise the full lifecycle with explicit identity fences.",
        "reference_ids": ["AUT-ID-001–007", "AUT-START-001–004"],
        "evidence_ids": ["AUT-T-ID", "AUT-T-START"],
    },
    "DOLGI004": {
        "title": "Verify conflict and reconciliation paths",
        "goal": "Exercise stale identity, digest and daemon mismatch, timeout, response loss, and lookup.",
        "reference_ids": ["AUT-T-ID", "AUT-T-RECON"],
        "evidence_ids": ["AUT-T-ID", "AUT-T-RECON"],
    },
    "DOLGI005": {
        "title": "Qualify the distribution archive",
        "goal": "Package the native arm64 release binaries once, verify archive identity and layout, and exercise the required Dolgorae scenarios against the extracted distribution.",
        "reference_ids": ["AUT-REL-001–003", "AUT-T-DIST"],
        "evidence_ids": ["AUT-T-DIST"],
    },
}


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
        repository_file(
            "assets/schemas/status-result-v2.schema.json",
            "v2 status schema",
        )
    )
    for label, schema in (("compact status", compact_status), ("status", status)):
        if "blockers" in schema.get("required", []) or "blockers" in schema.get(
            "properties", {}
        ):
            fail(f"v2 {label} must not expose a root blockers collection")
    status_required = set(status.get("required", []))
    if not {"blocker_window", "blockers_truncated"}.issubset(status_required):
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
        repository_file(
            "tests/fixtures/v2/graphs/valid-cases.json",
            "v2 valid graph fixtures",
        )
    )
    case_ids = {
        case.get("id") for case in graph_cases.get("cases", []) if isinstance(case, dict)
    }
    if "terminal-path-through-rework" not in case_ids:
        fail("v2 valid graph fixtures must cover terminal reachability through rework")

    procedure_spec = repository_file(
        "docs/specs/domain/procedure-and-item-specification.md",
        "v2 procedure specification",
    ).read_text(encoding="utf-8")
    dossier = repository_file(
        "docs/todo/TODO-podway-v2-full-feature-ga.md",
        "v2 adopted dossier",
    ).read_text(encoding="utf-8")
    if (
        "complete procedure graph, including declared rework edges" not in procedure_spec
        or "advance-only subgraph is acyclic" not in procedure_spec
        or "declared rework edges are excluded from the forward graph" in procedure_spec
        or "Every reachable graph node MUST have at least one finite route to a terminal"
        not in dossier
        or "subgraph formed by all advance edges is therefore acyclic" not in dossier
    ):
        fail("v2 graph reachability and cycle semantics drift between spec and dossier")
    return 6


def require_test_member(member: Any, criterion_id: str, proof_paths: set[str]) -> set[str]:
    if not isinstance(member, dict):
        fail(f"{criterion_id} proof member must be an object")
    required = {"command", "function", "path"}
    if not required.issubset(member):
        fail(f"{criterion_id} proof member is missing command, function, or path")
    relative = member["path"]
    function = member["function"]
    command = member["command"]
    if not all(isinstance(value, str) and value for value in (relative, function, command)):
        fail(f"{criterion_id} proof member fields must be non-empty strings")
    path = repository_file(relative, f"{criterion_id} proof path")
    source = path.read_text(encoding="utf-8")
    if re.search(FUNCTION_RE_TEMPLATE.format(name=re.escape(function)), source) is None:
        fail(f"{criterion_id} proof function is missing from {relative}: {function}")
    source_target = path.stem
    if source_target.startswith("int_"):
        cargo_target = "int_suite"
        test_name = f"{source_target}::{function}"
    elif source_target.startswith("e2e_"):
        cargo_target = "e2e_suite"
        test_name = f"{source_target}::{function}"
    else:
        cargo_target = source_target
        test_name = function
    required_tokens = ("cargo test ", f"--test {cargo_target} ", test_name, "--exact")
    direct_cargo_test = all(token in command for token in required_tokens)
    parts = Path(relative).parts
    package = parts[1] if len(parts) >= 4 and parts[0] == "crates" and parts[2] == "tests" else None
    exact_e2e_wrapper = (
        f"python3 tools/run_e2e.py --exact-test {package}::e2e_suite::{test_name}"
        if package is not None and source_target.startswith("e2e_")
        else None
    )
    if not direct_cargo_test and command != exact_e2e_wrapper:
        fail(f"{criterion_id} proof command is not bound to its exact test target and function")
    proof_paths.add(relative)
    obligations = member.get("obligation_ids", [])
    if not isinstance(obligations, list) or any(not isinstance(item, str) or not item for item in obligations):
        fail(f"{criterion_id} obligation_ids must be a string list")
    return set(obligations)


def require_release_step(member: Any, criterion_id: str, proof_paths: set[str]) -> None:
    if not isinstance(member, dict):
        fail(f"{criterion_id} release proof must be an object")
    required = {"command", "function", "kind", "path"}
    if set(member) != required or member.get("kind") != "release-step":
        fail(f"{criterion_id} release proof is malformed")
    relative = member["path"]
    function = member["function"]
    command = member["command"]
    if not all(isinstance(value, str) and value for value in (relative, function, command)):
        fail(f"{criterion_id} release proof fields must be non-empty strings")
    path = repository_file(relative, f"{criterion_id} release proof path")
    source = path.read_text(encoding="utf-8")
    if re.search(PYTHON_FUNCTION_RE_TEMPLATE.format(name=re.escape(function)), source, re.MULTILINE) is None:
        fail(f"{criterion_id} release proof function is missing from {relative}: {function}")
    if command != "make dist" or relative not in (ROOT / "Makefile").read_text(encoding="utf-8"):
        fail(f"{criterion_id} release proof is not part of make dist")
    proof_paths.add(relative)


def validate_acceptance_matrix() -> tuple[int, int]:
    matrix = load_object(MATRIX_PATH)
    if matrix.get("schema") != "podway.product-acceptance-matrix/v1" or matrix.get("version") != 4:
        fail("product acceptance matrix schema or version is unsupported")
    criteria = matrix.get("criteria")
    if not isinstance(criteria, list) or not criteria:
        fail("product acceptance matrix must contain criteria")
    source = matrix.get("source")
    if not isinstance(source, dict):
        fail("product acceptance matrix source binding is missing")
    source_path = repository_file(source.get("path"), "product acceptance source")
    source_lines = source_path.read_text(encoding="utf-8").splitlines()
    try:
        final_rule_line = source_lines.index("## Final acceptance rule") + 1
    except ValueError:
        fail("product acceptance source omits the final acceptance rule")
    source_bullets = {
        line_number: line.removeprefix("- ")
        for line_number, line in enumerate(source_lines, start=1)
        if line_number < final_rule_line and line.startswith("- ")
    }
    if len(criteria) != len(source_bullets):
        fail(
            "product acceptance matrix does not bind every mandatory acceptance bullet: "
            f"criteria={len(criteria)}, bullets={len(source_bullets)}"
        )
    expected_ids = [f"PAC-{number:03d}" for number in range(1, len(criteria) + 1)]
    proof_paths: set[str] = set()
    semantic_coverage: dict[str, set[str]] = {}
    seen_lines: set[int] = set()
    for expected_id, criterion in zip(expected_ids, criteria):
        if not isinstance(criterion, dict) or criterion.get("id") != expected_id:
            fail(f"product acceptance criteria must be ordered exactly; expected {expected_id}")
        if criterion.get("status") != "automated":
            fail(f"{expected_id} must be automated under the local release gate")
        line = criterion.get("line")
        text = criterion.get("text")
        if not isinstance(line, int) or line < 1 or line > len(source_lines) or line in seen_lines:
            fail(f"{expected_id} has an invalid or duplicate source line")
        if not isinstance(text, str) or source_lines[line - 1] != f"- {text}":
            fail(f"{expected_id} text does not match its bound acceptance bullet")
        seen_lines.add(line)
        proof = criterion.get("proof")
        if not isinstance(proof, dict):
            fail(f"{expected_id} proof must be an object")
        kind = proof.get("kind")
        if kind == "cargo-test":
            obligations = require_test_member(proof, expected_id, proof_paths)
            if obligations:
                fail(f"{expected_id} single-test proof must not declare semantic obligations")
        elif kind == "cargo-test-set":
            if proof.get("criterion_id") != expected_id or not isinstance(proof.get("members"), list) or not proof["members"]:
                fail(f"{expected_id} test-set proof is malformed")
            obligations: set[str] = set()
            for member in proof["members"]:
                if member.get("criterion_id") != expected_id:
                    fail(f"{expected_id} test-set member has the wrong criterion_id")
                member_obligations = require_test_member(member, expected_id, proof_paths)
                if obligations.intersection(member_obligations):
                    fail(f"{expected_id} repeats a semantic obligation")
                obligations.update(member_obligations)
            semantic_coverage[expected_id] = obligations
        elif kind == "release-step":
            require_release_step(proof, expected_id, proof_paths)
        else:
            fail(f"{expected_id} has unsupported proof kind: {kind}")
    if seen_lines != set(source_bullets):
        fail("product acceptance matrix source lines do not exactly cover every mandatory acceptance bullet")

    contracts = matrix.get("semantic_contracts")
    if not isinstance(contracts, list):
        fail("semantic_contracts must be a list")
    declared: dict[str, set[str]] = {}
    for contract in contracts:
        if not isinstance(contract, dict) or not isinstance(contract.get("criterion_id"), str):
            fail("semantic contract is malformed")
        criterion_id = contract["criterion_id"]
        obligations = contract.get("obligations")
        if criterion_id in declared or not isinstance(obligations, list) or not obligations:
            fail(f"semantic contract is duplicate or empty: {criterion_id}")
        ids = {item.get("id") for item in obligations if isinstance(item, dict)}
        if len(ids) != len(obligations) or any(not isinstance(item, str) or not item for item in ids):
            fail(f"semantic contract obligations are invalid: {criterion_id}")
        declared[criterion_id] = ids
    if declared != semantic_coverage:
        fail("semantic contract obligations do not exactly match test-set coverage")

    return len(criteria), len(proof_paths)


def roadmap_dolgi_tasks() -> dict[str, dict[str, Any]]:
    lines = (ROOT / "docs/roadmap/archive/v0.1.0.md").read_text(encoding="utf-8").splitlines()
    try:
        start = lines.index("## DOLGI — Dolgorae Integration Conformance")
    except ValueError:
        fail("roadmap omits the DOLGI epic")
    tasks: dict[str, dict[str, Any]] = {}
    for line in lines[start + 1 :]:
        if line.startswith("## "):
            break
        if not line.startswith("| `DOLGI"):
            continue
        columns = [column.strip() for column in line.strip().strip("|").split("|")]
        if len(columns) != 5:
            fail(f"roadmap has a malformed DOLGI task row: {line}")
        task_id = columns[0].strip("`")
        if task_id in tasks:
            fail(f"roadmap repeats DOLGI task {task_id}")
        tasks[task_id] = {
            "title": columns[1],
            "status": columns[2],
            "goal": columns[3],
            "reference_ids": re.findall(r"\[([^\]]+)\]\(", columns[4]),
        }
    return tasks


def rust_test_function_exists(source: str, function: str) -> bool:
    return (
        re.search(
            TEST_FUNCTION_RE_TEMPLATE.format(name=re.escape(function)),
            source,
            re.MULTILINE,
        )
        is not None
    )


def validate_dolgi_proof(member: Any, task_id: str, proof_paths: set[str]) -> None:
    if not isinstance(member, dict) or set(member) != {"command", "function", "kind", "path"}:
        fail(f"{task_id} proof must contain exactly command, function, kind, and path")
    relative = member["path"]
    function = member["function"]
    command = member["command"]
    kind = member["kind"]
    if not all(isinstance(value, str) and value for value in (relative, function, command, kind)):
        fail(f"{task_id} proof fields must be non-empty strings")
    path = repository_file(relative, f"{task_id} proof path")
    source = path.read_text(encoding="utf-8")
    if kind == "cargo-test":
        if not rust_test_function_exists(source, function):
            fail(f"{task_id} proof is not a #[test] function in {relative}: {function}")
        parts = Path(relative).parts
        if len(parts) < 4 or parts[0] != "crates" or parts[2] != "tests":
            fail(f"{task_id} cargo proof is outside a crate test directory")
        package = parts[1]
        source_target = path.stem
        cargo_target = "e2e_suite" if source_target.startswith("e2e_") else source_target
        test_name = f"{source_target}::{function}" if cargo_target == "e2e_suite" else function
        expected = (
            f"python3 tools/run_e2e.py --exact-test {package}::{cargo_target}::{test_name}"
            if cargo_target == "e2e_suite"
            else f"cargo test -p {package} --test {cargo_target} {test_name} --locked -- --exact"
        )
    elif kind == "python-self-test":
        if re.search(
            PYTHON_FUNCTION_RE_TEMPLATE.format(name=re.escape(function)),
            source,
            re.MULTILINE,
        ) is None:
            fail(f"{task_id} Python proof function is missing from {relative}: {function}")
        expected = f"python3 {relative} self-test"
    elif kind == "release-step":
        if re.search(
            PYTHON_FUNCTION_RE_TEMPLATE.format(name=re.escape(function)),
            source,
            re.MULTILINE,
        ) is None:
            fail(f"{task_id} release proof function is missing from {relative}: {function}")
        if relative not in (ROOT / "Makefile").read_text(encoding="utf-8"):
            fail(f"{task_id} release proof is not part of make dist")
        expected = "make dist"
    else:
        fail(f"{task_id} has unsupported proof kind: {kind}")
    if command != expected:
        fail(f"{task_id} proof command is not exact: expected={expected}, actual={command}")
    proof_paths.add(relative)


def validate_dolgorae_acceptance_matrix(
    matrix_override: dict[str, Any] | None = None,
    roadmap_override: dict[str, dict[str, Any]] | None = None,
) -> tuple[int, int]:
    matrix = load_object(DOLGI_MATRIX_PATH) if matrix_override is None else matrix_override
    if matrix.get("schema") != "podway.dolgorae-acceptance-matrix/v1" or matrix.get("version") != 1:
        fail("DOLGI acceptance matrix schema or version is unsupported")
    tasks = matrix.get("tasks")
    if not isinstance(tasks, list):
        fail("DOLGI acceptance matrix tasks must be a list")
    roadmap_tasks = roadmap_dolgi_tasks() if roadmap_override is None else roadmap_override
    if roadmap_tasks != {
        task_id: {
            "title": contract["title"],
            "status": "Completed",
            "goal": contract["goal"],
            "reference_ids": contract["reference_ids"],
        }
        for task_id, contract in DOLGI_TASKS.items()
    }:
        fail("roadmap DOLGI title, status, goal, or references do not match the accepted inventory")
    if [task.get("id") for task in tasks if isinstance(task, dict)] != list(DOLGI_TASKS):
        fail("DOLGI acceptance tasks must be complete, unique, and ordered")

    proof_paths: set[str] = set()
    for task in tasks:
        task_id = task["id"]
        contract = DOLGI_TASKS[task_id]
        if set(task) != {"evidence_ids", "id", "proofs", "status", "title"}:
            fail(f"{task_id} has unexpected or missing fields")
        if task["title"] != contract["title"] or task["status"] != "Completed":
            fail(f"{task_id} title or completion status does not match the roadmap")
        if task["evidence_ids"] != contract["evidence_ids"]:
            fail(f"{task_id} evidence IDs do not match the accepted inventory")
        proofs = task["proofs"]
        if not isinstance(proofs, list) or not proofs:
            fail(f"{task_id} must have at least one executable proof")
        seen_proofs: set[tuple[Any, Any]] = set()
        for proof in proofs:
            identity = (proof.get("path"), proof.get("function")) if isinstance(proof, dict) else (None, None)
            if identity in seen_proofs:
                fail(f"{task_id} repeats proof {identity}")
            seen_proofs.add(identity)
            validate_dolgi_proof(proof, task_id, proof_paths)

    return len(tasks), len(proof_paths)


def self_test_dolgorae_acceptance_matrix() -> int:
    baseline = load_object(DOLGI_MATRIX_PATH)

    def expect_failure(
        matrix: dict[str, Any],
        expected: str,
        roadmap: dict[str, dict[str, Any]] | None = None,
    ) -> None:
        try:
            validate_dolgorae_acceptance_matrix(matrix, roadmap)
        except ContractError as error:
            if expected not in str(error):
                fail(f"DOLGI matrix self-test failed for {expected}: {error}")
        else:
            fail(f"DOLGI matrix self-test unexpectedly accepted {expected}")

    missing_task = copy.deepcopy(baseline)
    missing_task["tasks"].pop()
    expect_failure(missing_task, "complete, unique, and ordered")

    duplicate_proof = copy.deepcopy(baseline)
    duplicate_proof["tasks"][3]["proofs"].append(
        copy.deepcopy(duplicate_proof["tasks"][3]["proofs"][0])
    )
    expect_failure(duplicate_proof, "repeats proof")

    wrong_evidence = copy.deepcopy(baseline)
    wrong_evidence["tasks"][0]["evidence_ids"] = ["AUT-T-OBS"]
    expect_failure(wrong_evidence, "evidence IDs")

    wrong_goal = copy.deepcopy(roadmap_dolgi_tasks())
    wrong_goal["DOLGI005"]["goal"] = "Weakened goal."
    expect_failure(baseline, "title, status, goal, or references", wrong_goal)

    wrong_references = copy.deepcopy(roadmap_dolgi_tasks())
    wrong_references["DOLGI005"]["reference_ids"] = ["AUT-T-DIST"]
    expect_failure(baseline, "title, status, goal, or references", wrong_references)

    if not rust_test_function_exists("#[test]\nfn proof() {}", "proof"):
        fail("DOLGI matrix self-test rejected a real #[test] function")
    if rust_test_function_exists("fn proof() {}", "proof"):
        fail("DOLGI matrix self-test accepted a non-test helper function")
    return 7


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
    expected = [f"C{number:02d}" for number in range(1, 17)] + ["P01", "D01", "D02", "S01", "S02", "S03"]
    coverage = registry.get("coverage")
    windows = registry.get("windows")
    if not isinstance(coverage, dict) or not isinstance(windows, list):
        fail("crash registry coverage or windows is missing")
    observed = [window.get("id") for window in windows if isinstance(window, dict)]
    if coverage.get("required") != expected or coverage.get("covered") != expected or coverage.get("percent") != 100 or observed != expected:
        fail("crash registry must provide exact ordered 100% coverage")
    required_proof = {"failpoint", "test", "termination", "recovery", "invariant", "source_locator"}
    for window in windows:
        boundary_id = window["id"]
        proof = window.get("proof")
        if not isinstance(proof, dict) or set(proof) != required_proof:
            fail(f"{boundary_id} crash proof has unexpected or missing fields")
        if any(not isinstance(value, str) or not value for value in proof.values()):
            fail(f"{boundary_id} crash proof fields must be non-empty strings")
        test_path, test_function = locator_parts(proof["test"], f"{boundary_id} test")
        source_path, source_symbol = locator_parts(proof["source_locator"], f"{boundary_id} source locator")
        test_text = test_path.read_text(encoding="utf-8")
        source_text = source_path.read_text(encoding="utf-8")
        if re.search(FUNCTION_RE_TEMPLATE.format(name=re.escape(test_function)), test_text) is None:
            fail(f"{boundary_id} crash test function is missing")
        symbol = source_symbol.rsplit("::", 1)[-1]
        if symbol not in source_text:
            fail(f"{boundary_id} crash source symbol is missing")
    return len(windows)


def v2_source_bullets() -> dict[str, list[tuple[int, str]]]:
    source = ROOT / "docs/todo/TODO-podway-v2-full-feature-ga.md"
    lines = source.read_text(encoding="utf-8").splitlines()
    sections: dict[str, list[tuple[int, list[str]]]] = {
        section: [] for section in V2_SECTION_COUNTS
    }
    current: str | None = None
    for line_number, line in enumerate(lines, start=1):
        if line.startswith("### 17."):
            candidate = line.split()[1]
            current = candidate if candidate in sections else None
            continue
        if line.startswith("## 18."):
            current = None
        if current is None:
            continue
        if line.startswith("- "):
            sections[current].append((line_number, [line[2:]]))
        elif sections[current] and line and not line.startswith("#"):
            sections[current][-1][1].append(line.strip())
    result = {
        section: [(line, " ".join(parts)) for line, parts in bullets]
        for section, bullets in sections.items()
    }
    observed = {section: len(bullets) for section, bullets in result.items()}
    if observed != V2_SECTION_COUNTS:
        fail(f"v2 acceptance source section counts drift: {observed}")
    return result


def roadmap_v2_tasks() -> set[str]:
    text = (ROOT / "docs/roadmap/README.md").read_text(encoding="utf-8")
    return set(re.findall(r"\| `(V2[A-Z]{3}-[0-9]{3})` \|", text))


def require_exact_keys(value: Any, expected: set[str], label: str) -> None:
    if not isinstance(value, dict) or set(value) != expected:
        fail(f"{label} has unexpected or missing fields")


def validate_v2_contract_ref(reference: Any, contract_paths: list[str], label: str) -> None:
    require_exact_keys(reference, {"path", "anchor"}, label)
    relative = reference["path"]
    anchor = reference["anchor"]
    if not isinstance(relative, str) or relative not in contract_paths:
        fail(f"{label} path must be one of the criterion contract paths")
    if not isinstance(anchor, str) or not anchor:
        fail(f"{label} anchor must be a non-empty string")
    path = repository_file(relative, f"{label} path")
    if anchor.startswith("/"):
        value: Any = load_object(path)
        try:
            for raw_part in anchor[1:].split("/"):
                part = raw_part.replace("~1", "/").replace("~0", "~")
                value = value[int(part)] if isinstance(value, list) else value[part]
        except (KeyError, IndexError, TypeError, ValueError):
            fail(f"{label} JSON Pointer does not resolve: {anchor}")
    elif anchor not in path.read_text(encoding="utf-8").splitlines():
        fail(f"{label} text anchor does not resolve: {anchor}")


def validate_v2_proof(proof: Any, criterion_id: str, proof_paths: set[str]) -> None:
    if not isinstance(proof, dict):
        fail(f"{criterion_id} proof must be an object")
    kind = proof.get("kind")
    if kind == "cargo-test-set":
        require_exact_keys(proof, {"kind", "members"}, f"{criterion_id} proof")
        members = proof["members"]
        if not isinstance(members, list) or not members:
            fail(f"{criterion_id} cargo-test-set proof must contain members")
        seen: set[tuple[Any, Any]] = set()
        for member in members:
            identity = (
                member.get("path"),
                member.get("function"),
            ) if isinstance(member, dict) else (None, None)
            if identity in seen:
                fail(f"{criterion_id} repeats proof {identity}")
            seen.add(identity)
            validate_v2_proof(member, criterion_id, proof_paths)
        return
    if kind != "cargo-test":
        validate_dolgi_proof(proof, criterion_id, proof_paths)
        return
    require_exact_keys(proof, {"kind", "path", "function", "command"}, f"{criterion_id} proof")
    relative = proof["path"]
    function = proof["function"]
    command = proof["command"]
    if not all(isinstance(value, str) and value for value in (relative, function, command)):
        fail(f"{criterion_id} proof fields must be non-empty strings")
    path = repository_file(relative, f"{criterion_id} proof path")
    if not rust_test_function_exists(path.read_text(encoding="utf-8"), function):
        fail(f"{criterion_id} proof is not a #[test] function in {relative}: {function}")
    parts = Path(relative).parts
    if len(parts) < 4 or parts[0] != "crates" or parts[2] != "tests":
        fail(f"{criterion_id} cargo proof is outside a crate test directory")
    package = parts[1]
    source_target = path.stem
    if source_target.startswith("e2e_"):
        expected = f"python3 tools/run_e2e.py --exact-test {package}::e2e_suite::{source_target}::{function}"
    elif source_target.startswith("int_"):
        expected = f"cargo test -p {package} --test int_suite {source_target}::{function} --locked -- --exact"
    else:
        expected = f"cargo test -p {package} --test {source_target} {function} --locked -- --exact"
    if command != expected:
        fail(f"{criterion_id} proof command is not exact: expected={expected}, actual={command}")
    proof_paths.add(relative)


def validate_v2_acceptance_matrix(matrix: dict[str, Any] | None = None) -> dict[str, dict[str, Any]]:
    matrix = load_object(V2_ACCEPTANCE_PATH) if matrix is None else matrix
    require_exact_keys(
        matrix,
        {"schema", "version", "source", "evidence_policy", "criteria"},
        "v2 acceptance matrix",
    )
    if matrix["schema"] != "podway.v2-acceptance-matrix/v1" or matrix["version"] != 1:
        fail("v2 acceptance matrix schema or version is unsupported")
    if matrix["evidence_policy"] != "Planned entries are obligations without proof; automated entries bind exact executable proof commands.":
        fail("v2 acceptance evidence policy drift")
    source = matrix["source"]
    require_exact_keys(source, {"path", "heading", "section_counts"}, "v2 acceptance source")
    if source != {
        "path": "docs/todo/TODO-podway-v2-full-feature-ga.md",
        "heading": "## 17. Verification and Acceptance",
        "section_counts": V2_SECTION_COUNTS,
    }:
        fail("v2 acceptance source binding drift")
    criteria = matrix["criteria"]
    if not isinstance(criteria, list) or len(criteria) != 87:
        fail("v2 acceptance matrix must contain exactly 87 criteria")
    source_bullets = v2_source_bullets()
    tasks = roadmap_v2_tasks()
    allowed_classes = {
        "parser-contract", "graph-property", "runtime-integration", "goal-state-table",
        "authoring-golden", "confirmation-e2e", "compatibility-e2e", "automation-e2e",
        "payload-boundary",
    }
    expected = [
        (section, ordinal, line, text)
        for section, bullets in source_bullets.items()
        for ordinal, (line, text) in enumerate(bullets, start=1)
    ]
    indexed: dict[str, dict[str, Any]] = {}
    proof_paths: set[str] = set()
    for number, (criterion, source_item) in enumerate(zip(criteria, expected), start=1):
        identifier = f"V2ACC-{number:03d}"
        expected_keys = {
            "id", "section", "ordinal", "line", "text", "contract_paths",
            "contract_refs", "test_class", "owning_tasks", "implementation_status",
        }
        status = criterion.get("implementation_status") if isinstance(criterion, dict) else None
        if status == "automated":
            expected_keys.add("proof")
        require_exact_keys(criterion, expected_keys, identifier)
        section, ordinal, line, text = source_item
        if (criterion["id"], criterion["section"], criterion["ordinal"], criterion["line"], criterion["text"]) != (
            identifier, section, ordinal, line, text
        ):
            fail(f"{identifier} does not exactly match its §17 source bullet")
        contract_paths = criterion["contract_paths"]
        if not isinstance(contract_paths, list) or not contract_paths:
            fail(f"{identifier} must bind at least one contract path")
        for relative in contract_paths:
            repository_file(relative, f"{identifier} contract path")
        references = criterion["contract_refs"]
        if not isinstance(references, list) or not references:
            fail(f"{identifier} must bind at least one anchored contract reference")
        seen_references: set[tuple[str, str]] = set()
        for reference_number, reference in enumerate(references, start=1):
            validate_v2_contract_ref(
                reference,
                contract_paths,
                f"{identifier} contract reference {reference_number}",
            )
            key = (reference["path"], reference["anchor"])
            if key in seen_references:
                fail(f"{identifier} contract references must be unique")
            seen_references.add(key)
        if criterion["test_class"] not in allowed_classes:
            fail(f"{identifier} has an unsupported test class")
        owning_tasks = criterion["owning_tasks"]
        if not isinstance(owning_tasks, list) or not owning_tasks or len(owning_tasks) != len(set(owning_tasks)) or any(task not in tasks for task in owning_tasks):
            fail(f"{identifier} owning tasks must be non-empty, unique, and registered in the roadmap")
        if status == "automated":
            validate_v2_proof(criterion["proof"], identifier, proof_paths)
        elif status != "planned":
            fail(f"{identifier} implementation status must be planned or automated")
        indexed[identifier] = criterion
    required_admission_paths = {
        "assets/schemas/output-v2.schema.json",
        "assets/schemas/detached-admission-result-v2.schema.json",
        "assets/schemas/job-result-v2.schema.json",
        "tests/fixtures/v2/protocol/result-families.json",
        "crates/podway-protocol/tests/int_v2_result_contract.rs",
    }
    if not required_admission_paths.issubset(indexed["V2ACC-076"]["contract_paths"]):
        fail("V2ACC-076 must bind the inherited admission and terminal-receipt contracts")
    return indexed


def validate_v2_compatibility_matrix(acceptance: dict[str, dict[str, Any]], matrix: dict[str, Any] | None = None) -> int:
    matrix = load_object(V2_COMPATIBILITY_PATH) if matrix is None else matrix
    require_exact_keys(matrix, {"schema", "version", "source_acceptance_matrix", "evidence_policy", "result_family_inventories", "requirements", "surface_cases", "v1_frozen_assets"}, "v2 compatibility matrix")
    if matrix["schema"] != "podway.v2-compatibility-matrix/v1" or matrix["version"] != 1:
        fail("v2 compatibility matrix schema or version is unsupported")
    if matrix["evidence_policy"] != "Requirement evidence references the canonical acceptance proof; automated surface cases bind exact executable proof commands.":
        fail("v2 compatibility evidence policy drift")
    expected_inventories = {
        "existing_route_v2": [
            "assets/schemas/procedure-validation-result-v2.schema.json", "assets/schemas/detached-admission-result-v2.schema.json",
            "assets/schemas/session-start-result-v2.schema.json", "assets/schemas/status-result-v2.schema.json",
            "assets/schemas/compact-status-result-v2.schema.json", "assets/schemas/next-result-v2.schema.json",
            "assets/schemas/stage-transition-result-v2.schema.json", "assets/schemas/item-mutation-result-v2.schema.json",
            "assets/schemas/job-lookup-result-v2.schema.json", "assets/schemas/job-result-v2.schema.json",
        ],
        "new_route_v1": [
            "assets/schemas/procedure-source-result-v1.schema.json", "assets/schemas/procedure-diagnostics-result-v1.schema.json",
            "assets/schemas/procedure-graph-result-v1.schema.json", "assets/schemas/procedure-preview-result-v1.schema.json",
            "assets/schemas/decision-result-v1.schema.json", "assets/schemas/rework-result-v1.schema.json",
            "assets/schemas/goal-definition-result-v1.schema.json", "assets/schemas/goal-revision-result-v1.schema.json",
            "assets/schemas/criterion-assessment-result-v1.schema.json",
        ],
        "registry": "crates/podway-protocol/src/result_contract.rs",
        "command_catalog": "assets/specifications/command-catalog.yaml",
    }
    if matrix["result_family_inventories"] != expected_inventories:
        fail("v2 compatibility result family inventories drift")
    for value in expected_inventories.values():
        for relative in value if isinstance(value, list) else [value]:
            repository_file(relative, "v2 compatibility result family inventory")
    boundary_fixture = load_object(V2_COMPATIBILITY_FIXTURE_PATH)
    family_cases = [
        item
        for item in boundary_fixture.get("cases", [])
        if isinstance(item, dict) and item.get("id") == "existing-route-v2-result-family"
    ]
    expected_operation = "validate every existing-route v2 result schema listed by the compatibility matrix against its exact command inventory"
    if (
        len(family_cases) != 1
        or family_cases[0].get("operation") != expected_operation
        or family_cases[0].get("expected_family_count") != len(expected_inventories["existing_route_v2"])
    ):
        fail("v2 compatibility fixture existing-route family count or operation drift")
    requirements = matrix["requirements"]
    expected_ids = [identifier for identifier, item in acceptance.items() if item["section"] == "17.7"]
    if not isinstance(requirements, list) or [item.get("acceptance_id") for item in requirements if isinstance(item, dict)] != expected_ids:
        fail("v2 compatibility matrix must cover §17.7 exactly once and in order")
    expected_requirement_boundaries = [
        "v1-behavior-and-fixtures", "v1-storage-migration", "unsupported-peer",
        "v1-command-semantics", "reactivation-semantics", "release-admission-fence",
        "current-task-retention",
    ]
    proof_paths: set[str] = set()
    for number, item in enumerate(requirements, start=1):
        expected_keys = {"id", "acceptance_id", "text", "boundary", "contract_paths", "test_class", "owning_tasks", "implementation_status"}
        if isinstance(item, dict) and item.get("implementation_status") == "automated":
            expected_keys.add("proof_acceptance_id")
        require_exact_keys(item, expected_keys, f"V2COMP-{number:03d}")
        criterion = acceptance[item["acceptance_id"]]
        if item["id"] != f"V2COMP-{number:03d}" or item["text"] != criterion["text"] or item["boundary"] != expected_requirement_boundaries[number - 1] or item["contract_paths"] != criterion["contract_paths"] or item["test_class"] != "compatibility-e2e" or item["owning_tasks"] != criterion["owning_tasks"] or item["implementation_status"] != criterion["implementation_status"]:
            fail(f"V2COMP-{number:03d} drifts from its acceptance criterion")
        if item["implementation_status"] == "automated" and item["proof_acceptance_id"] != item["acceptance_id"]:
            fail(f"V2COMP-{number:03d} proof reference must match its acceptance criterion")
        for relative in item["contract_paths"]:
            repository_file(relative, f"V2COMP-{number:03d} contract path")
    expected_boundaries = [
        "existing-route-v2-result-family",
        "new-route-v1-result-family",
        "v2-never-extends-v1-result-family",
        "registered-unserved-route-is-unsupported-capability",
        "absent-route-is-unknown-command-or-usage",
        "manifest-digest-is-capability-discovery",
        "v1-reopen-is-not-v2-reactivation",
        "released-v1-result-families-byte-for-byte",
    ]
    surface_cases = matrix["surface_cases"]
    if not isinstance(surface_cases, list) or [item.get("boundary") for item in surface_cases if isinstance(item, dict)] != expected_boundaries:
        fail("v2 compatibility specialized surface cases are incomplete or unordered")
    for number, item in enumerate(surface_cases, start=1):
        expected_keys = {"id", "boundary", "contract_paths", "owning_tasks", "implementation_status"}
        if isinstance(item, dict) and item.get("implementation_status") == "automated":
            expected_keys.add("proof")
        require_exact_keys(item, expected_keys, f"V2COMP-SURFACE-{number:03d}")
        owning_tasks = item["owning_tasks"]
        if item["id"] != f"V2COMP-SURFACE-{number:03d}" or item["implementation_status"] not in {"planned", "automated"} or not isinstance(owning_tasks, list) or not owning_tasks or len(owning_tasks) != len(set(owning_tasks)) or any(task not in roadmap_v2_tasks() for task in owning_tasks):
            fail(f"V2COMP-SURFACE-{number:03d} is malformed")
        if item["implementation_status"] == "automated":
            validate_v2_proof(item["proof"], item["id"], proof_paths)
        for relative in item["contract_paths"]:
            repository_file(relative, f"V2COMP-SURFACE-{number:03d} contract path")
    frozen = matrix["v1_frozen_assets"]
    frozen_hashes = {
        "assets/schemas/compact-status-result-v1.schema.json": "2cded139839dafcc20308f85dd9faea720dee9b6e985ead3b0ba05ea0b6aed57",
        "assets/schemas/daemon-status-result-v1.schema.json": "998d14853d3d86b226d9a67f239ee6f0d3e90590ecbd7eaa15c1c91952552486",
        "assets/schemas/detached-admission-result-v1.schema.json": "4ea7b6744261ff08174d6a7c076d9ee9c1adb43e01f5e55234826102bd5ed8e7",
        "assets/schemas/error-v1.schema.json": "371ccd1e07a0f503bc70a5a4b167ff43a0dfbe9ed8e5b78533cd9a7848d06bf8",
        "assets/schemas/item-mutation-result-v1.schema.json": "f62a24e178cc5816bc4c306cb9d5a24d6f2f594704a7b2128e791e49ede726b9",
        "assets/schemas/job-lookup-result-v1.schema.json": "1159cf428e4152554389dc02380fb0a3c84d2e5006c682eeacf24e4d1fd03c3f",
        "assets/schemas/job-result-v1.schema.json": "e7710f80320ef431853a7e7dee4f7e4548030106e056d1cec08e79e09a429541",
        "assets/schemas/next-result-v1.schema.json": "a27e51dad161a9ef8c6de67da6f372a7b3d2337ff3ca2598f1ecb4f1ae627f56",
        "assets/schemas/output-v1.schema.json": "19355e4f256fba8b17a4813f332006603b4e103fd747786f5e13d6447c2c55cd",
        "assets/schemas/procedure-validation-result-v1.schema.json": "fc4b23b6416904bf25a3209218fab730f142295b62803b441ee2bb75563a3fe5",
        "assets/schemas/session-start-result-v1.schema.json": "36137a660d17292619e3eb82abe1cb7dd38f72fd16b5e7836f13c9a83d87dc0d",
        "assets/schemas/stage-transition-result-v1.schema.json": "228ef46d296b34034dfe779d4bd86f2b858aea414234f7f92d29627bf5e4b0ab",
        "assets/schemas/status-result-v1.schema.json": "50e8a1da908dee02751bd70a19b820460925e40b2cb16ee8f6dc749725102032",
        "assets/schemas/version-result-v1.schema.json": "fe92513aa0cb4f75bd02e220b9feb5bf19795105cf364518f4119689e02baf7c",
        "assets/schemas/workspace-init-result-v1.schema.json": "51e68c2f017a576b036d25329e1d86be5ff12c16af5504640ab521eaa396bbc7",
    }
    expected_frozen = [
        {"path": path, "sha256": f"sha256:{digest}"}
        for path, digest in frozen_hashes.items()
    ]
    if frozen != expected_frozen:
        fail("v2 compatibility matrix must bind the exact 13 released v1 result families plus output and error envelopes")
    for entry in frozen:
        path = repository_file(entry["path"], "v1 frozen asset")
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        if actual != frozen_hashes[entry["path"]]:
            fail(f"v1 frozen asset digest drift: {entry['path']}")
    return len(requirements)


def selector_base(selector: Any) -> str:
    if not isinstance(selector, str) or not selector:
        fail("v2 payload selector must be a non-empty string")
    return selector.split("[", 1)[0]


def schema_registry() -> dict[str, dict[str, Any]]:
    registry: dict[str, dict[str, Any]] = {}
    for path in (ROOT / "assets/schemas").rglob("*.schema.json"):
        value = load_object(path)
        identifier = value.get("$id")
        if isinstance(identifier, str):
            registry[identifier] = value
    return registry


def resolve_schema_reference(reference: str, current: dict[str, Any], registry: dict[str, dict[str, Any]]) -> dict[str, Any]:
    base, _, fragment = reference.partition("#")
    target = registry.get(base) if base else current
    if target is None:
        fail(f"v2 payload schema reference is unregistered: {reference}")
    value: Any = target
    if fragment:
        if not fragment.startswith("/"):
            fail(f"v2 payload schema reference fragment is invalid: {reference}")
        for token in fragment[1:].split("/"):
            token = token.replace("~1", "/").replace("~0", "~")
            if not isinstance(value, dict) or token not in value:
                fail(f"v2 payload schema reference fragment is missing: {reference}")
            value = value[token]
    if not isinstance(value, dict):
        fail(f"v2 payload schema reference is not an object: {reference}")
    return value


def schema_leaf_paths(schema: dict[str, Any], path: str, current: dict[str, Any], registry: dict[str, dict[str, Any]], active: set[int] | None = None) -> set[str]:
    active = set() if active is None else active
    marker = id(schema)
    if marker in active:
        fail(f"v2 payload schema recursion is not a bounded leaf contract: {path}")
    active.add(marker)
    try:
        if "$ref" in schema:
            target = resolve_schema_reference(schema["$ref"], current, registry)
            target_root = registry.get(schema["$ref"].partition("#")[0], current)
            leaves = schema_leaf_paths(target, path, target_root, registry, active)
            structural_siblings = {
                key: value
                for key, value in schema.items()
                if key != "$ref" and key in {"properties", "items", "allOf", "anyOf", "oneOf"}
            }
            if structural_siblings:
                leaves.update(schema_leaf_paths(structural_siblings, path, current, registry, active))
            return leaves
        properties = schema.get("properties")
        if isinstance(properties, dict):
            leaves = set()
            for name, child in properties.items():
                if isinstance(child, dict):
                    leaves.update(schema_leaf_paths(child, f"{path}.{name}" if path else name, current, registry, active))
            return leaves
        items = schema.get("items")
        if isinstance(items, dict):
            return schema_leaf_paths(items, f"{path}[]", current, registry, active)
        alternatives = [schema[key] for key in ("allOf", "anyOf", "oneOf") if key in schema]
        if alternatives:
            leaves: set[str] = set()
            for group in alternatives:
                if not isinstance(group, list):
                    fail(f"v2 payload schema combinator is not an array: {path}")
                for branch in group:
                    if isinstance(branch, dict) and branch.get("type") != "null":
                        leaves.update(schema_leaf_paths(branch, path, current, registry, active))
            return leaves or {path}
        return {path}
    finally:
        active.remove(marker)


def validate_v2_payload_matrix(acceptance: dict[str, dict[str, Any]], matrix: dict[str, Any] | None = None) -> int:
    matrix = load_object(V2_PAYLOAD_PATH) if matrix is None else matrix
    require_exact_keys(matrix, {"schema", "version", "source_acceptance_matrix", "evidence_policy", "frame_bytes", "next_budget_bytes", "headroom_bytes", "output_envelope", "terminal_receipt_contract", "next_schema", "resolved_next_shape", "components", "suggestion_partition", "status_contract", "projection_bounds", "requirements"}, "v2 payload matrix")
    if matrix["schema"] != "podway.v2-payload-matrix/v1" or matrix["version"] != 1:
        fail("v2 payload matrix schema or version is unsupported")
    if (matrix["frame_bytes"], matrix["next_budget_bytes"], matrix["headroom_bytes"]) != (1048576, 1015808, 32768):
        fail("v2 payload whole-frame arithmetic drift")
    schema = load_object(repository_file(matrix["next_schema"], "v2 next schema"))
    fields = set(schema.get("properties", {}))
    registry = schema_registry()
    all_leaves = schema_leaf_paths(schema, "", schema, registry)
    if not {"suggestions[].command", "suggestions[].argv[]", "suggestions[].item_id"}.issubset(all_leaves):
        fail("v2 payload schema leaf resolution did not reach suggestion array members")
    output_envelope = matrix["output_envelope"]
    require_exact_keys(output_envelope, {"schema", "component", "selectors", "framing_selectors"}, "v2 output envelope binding")
    output_schema = load_object(repository_file(output_envelope["schema"], "v2 output envelope schema"))
    output_fields = set(output_schema.get("properties", {}))
    if (
        output_envelope["schema"] != "assets/schemas/output-v2.schema.json"
        or output_envelope["component"] != "ENVELOPE_RESERVE"
        or output_envelope["selectors"] != list(output_schema.get("properties", {}))
        or set(output_envelope["selectors"]) != output_fields
        or output_envelope["framing_selectors"] != ["4-byte-big-endian-payload-length"]
    ):
        fail("v2 output envelope binding must cover every output/v2 field in schema order")
    envelope_leaves = schema_leaf_paths(output_schema, "", output_schema, registry)
    if not {"warnings[].code", "workspace.uuid", "job.id", "session.id", "result"}.issubset(envelope_leaves):
        fail("v2 output envelope binding does not resolve retained nested fields and warnings")
    terminal_receipt = matrix["terminal_receipt_contract"]
    candidate_result_schemas = [
        "assets/schemas/session-start-result-v2.schema.json",
        "assets/schemas/stage-transition-result-v2.schema.json",
        "assets/schemas/item-mutation-result-v2.schema.json",
        "assets/schemas/decision-result-v1.schema.json",
        "assets/schemas/rework-result-v1.schema.json",
        "assets/schemas/goal-definition-result-v1.schema.json",
        "assets/schemas/goal-revision-result-v1.schema.json",
        "assets/schemas/criterion-assessment-result-v1.schema.json",
    ]
    error_catalog = load_object(
        repository_file("assets/specifications/error-codes.json", "v2 error catalog")
    )
    candidate_error_codes = [
        entry.get("code")
        for entry in error_catalog.get("errors", [])
        if isinstance(entry, dict)
        and entry.get("details_schema") == "podway.v2-runtime-error-details/v1"
    ]
    if len(candidate_error_codes) != 26 or len(set(candidate_error_codes)) != 26:
        fail("v2 terminal error candidate inventory must contain 26 unique catalog-bound codes")
    expected_terminal_receipt = {
        "schema": "assets/schemas/job-result-v2.schema.json",
        "terminal_success_ref": "urn:podway:schema:output:v2#/$defs/terminalMutationOutput",
        "terminal_error_ref": "urn:podway:schema:error:v1",
        "terminal_error_details_ref": "urn:podway:schema:v2-runtime-error-details:v1",
        "v2_error_message_max_characters": 512,
        "maximum_wrapper_depth": 1,
        "fixture": "tests/fixtures/v2/payload/maximum-terminal-receipt-recipe.json",
        "candidate_error_codes": candidate_error_codes,
        "candidate_result_schemas": candidate_result_schemas,
    }
    if terminal_receipt != expected_terminal_receipt:
        fail("v2 terminal receipt binding or candidate family inventory drift")
    job_schema = load_object(repository_file(terminal_receipt["schema"], "v2 job result schema"))
    success_ref = job_schema.get("$defs", {}).get("terminalSuccessResponse", {}).get("$ref")
    if success_ref != terminal_receipt["terminal_success_ref"]:
        fail("v2 job result schema does not use the non-recursive terminal mutation output")
    terminal_error = job_schema.get("$defs", {}).get("terminalErrorResponse", {})
    error_constraints = terminal_error.get("allOf")
    if not isinstance(error_constraints, list) or len(error_constraints) != 2:
        fail("v2 job result schema must contextually close terminal errors")
    error_ref = error_constraints[0].get("$ref") if isinstance(error_constraints[0], dict) else None
    error_commands = (
        error_constraints[1]
        .get("properties", {})
        .get("command", {})
        .get("enum")
        if isinstance(error_constraints[1], dict)
        else None
    )
    mutation_commands = (
        output_schema.get("$defs", {})
        .get("detachedAdmission", {})
        .get("properties", {})
        .get("command", {})
        .get("enum")
    )
    if error_ref != terminal_receipt["terminal_error_ref"] or error_commands != mutation_commands:
        fail("v2 terminal errors must use error/v1 and exactly the v2 mutation command set")
    for candidate in candidate_result_schemas:
        repository_file(candidate, "v2 terminal receipt candidate schema")
    receipt_recipe = load_object(repository_file(terminal_receipt["fixture"], "v2 terminal receipt recipe"))
    require_exact_keys(
        receipt_recipe,
        {"schema", "fixture_class", "evidence_level", "implementation_status", "production_proof_required", "frame_bytes", "outer_schema", "terminal_result_schema", "terminal_success_ref", "terminal_error_ref", "terminal_error_details_ref", "v2_error_message_max_characters", "maximum_wrapper_depth", "candidate_error_codes", "candidate_result_schemas", "construction", "forbidden_nested_results", "future_assertions"},
        "v2 terminal receipt recipe",
    )
    if (
        receipt_recipe["schema"] != "podway.v2-fixture-recipe/v1"
        or receipt_recipe["fixture_class"] != "maximum-size"
        or receipt_recipe["evidence_level"] != "contract-recipe"
        or receipt_recipe["implementation_status"] != "planned"
        or receipt_recipe["production_proof_required"] is not True
        or receipt_recipe["frame_bytes"] != matrix["frame_bytes"]
        or receipt_recipe["outer_schema"] != "assets/schemas/output-v2.schema.json"
        or receipt_recipe["terminal_result_schema"] != terminal_receipt["schema"]
        or receipt_recipe["terminal_success_ref"] != terminal_receipt["terminal_success_ref"]
        or receipt_recipe["terminal_error_ref"] != terminal_receipt["terminal_error_ref"]
        or receipt_recipe["terminal_error_details_ref"] != terminal_receipt["terminal_error_details_ref"]
        or receipt_recipe["v2_error_message_max_characters"] != terminal_receipt["v2_error_message_max_characters"]
        or receipt_recipe["maximum_wrapper_depth"] != terminal_receipt["maximum_wrapper_depth"]
        or receipt_recipe["candidate_error_codes"] != candidate_error_codes
        or receipt_recipe["candidate_result_schemas"] != candidate_result_schemas
        or receipt_recipe["forbidden_nested_results"] != ["query", "authoring", "detached-admission", "job-result"]
    ):
        fail("v2 terminal receipt recipe overstates evidence or drifts from ADR-0018")
    resolved_shape = matrix["resolved_next_shape"]
    if resolved_shape != {"unique_instance_leaf_paths": 112, "non_suggestion_leaf_paths": 109, "suggestion_leaf_paths": 3, "conditional_assignments": 115, "v2_command_values": 19}:
        fail("v2 resolved next shape counts drift")
    if len(all_leaves) != 112 or len([leaf for leaf in all_leaves if leaf.startswith("suggestions[]")]) != 3:
        fail("v2 next schema resolved leaf inventory drift")
    components = matrix["components"]
    if not isinstance(components, list) or [item.get("constant") for item in components if isinstance(item, dict)] != ["ENVELOPE_RESERVE", "NEXT_STATIC_BUDGET", "READBACK_BUDGET", "GOAL_DISPLAY_MAX", "BLOCKER_WINDOW_MAX", "COUNTERS_MAX"]:
        fail("v2 payload components are incomplete or unordered")
    if sum(item.get("bytes", 0) for item in components) != matrix["next_budget_bytes"]:
        fail("v2 payload component sum does not match next_budget_bytes")
    selectors = [selector for component in components for selector in component.get("selectors", [])]
    bases = [selector_base(selector) for selector in selectors]
    if set(bases) != fields:
        fail(f"v2 payload field coverage drift: missing={sorted(fields - set(bases))}, extra={sorted(set(bases) - fields)}")
    duplicates = {field for field in bases if bases.count(field) > 1}
    partition = matrix["suggestion_partition"]
    require_exact_keys(partition, {"field", "exhaustive", "disjoint", "predicates"}, "v2 suggestion partition")
    expected_predicates = ["command!=goal.assess_criterion&&command!=goal.define", "command==goal.assess_criterion||command==goal.define"]
    if duplicates != {"suggestions"} or partition != {"field": "suggestions", "exhaustive": True, "disjoint": True, "predicates": expected_predicates}:
        fail("v2 payload selectors must assign every field once with the exact exhaustive suggestion partition")
    expected_suggestion_selectors = [f"suggestions[{predicate}]" for predicate in expected_predicates]
    if [selector for selector in selectors if selector_base(selector) == "suggestions"] != expected_suggestion_selectors:
        fail("v2 suggestion selectors do not match the declared partition")
    properties = schema["properties"]
    covered_leaves: list[str] = []
    for base in bases:
        if base != "suggestions":
            covered_leaves.extend(schema_leaf_paths(properties[base], base, schema, registry))
    non_suggestion_leaves = {leaf for leaf in all_leaves if not leaf.startswith("suggestions[]")}
    if set(covered_leaves) != non_suggestion_leaves or len(covered_leaves) != len(set(covered_leaves)):
        fail("v2 payload selectors do not assign every resolved non-suggestion leaf exactly once")
    suggestion_leaves = schema_leaf_paths(properties["suggestions"], "suggestions", schema, registry)
    if not suggestion_leaves or any(not leaf.startswith("suggestions[]") for leaf in suggestion_leaves):
        fail("v2 payload suggestion leaf expansion is invalid")
    component_counts = {
        component["constant"]: sum(
            len(schema_leaf_paths(properties[selector_base(selector)], selector_base(selector), schema, registry))
            for selector in component["selectors"]
        )
        for component in components
    }
    if component_counts != {"ENVELOPE_RESERVE": 18, "NEXT_STATIC_BUDGET": 23, "READBACK_BUDGET": 51, "GOAL_DISPLAY_MAX": 14, "BLOCKER_WINDOW_MAX": 5, "COUNTERS_MAX": 4} or sum(component_counts.values()) != 115:
        fail("v2 payload resolved component assignment counts drift")
    command_schema = resolve_schema_reference("urn:podway:schema:v2-result-components:v1#/$defs/v2Command", schema, registry)
    command_values = command_schema.get("enum")
    if not isinstance(command_values, list) or len(command_values) != 19 or len(set(command_values)) != 19:
        fail("v2 suggestion partition is not bound to all 19 command values")
    goal_commands = {"goal.assess_criterion", "goal.define"}
    if not goal_commands.issubset(command_values) or not set(command_values).difference(goal_commands):
        fail("v2 suggestion command partition is not exhaustive and disjoint")
    status_contract = matrix["status_contract"]
    expected_status = {
        "status_values_max_bytes": 262144,
        "item_value_max_characters": 2048,
        "item_value_marker": "value_truncated",
        "values_window_markers": ["items_total", "items_truncated"],
        "blocker_window_max_bytes": 49152,
        "blocker_window_order": "newest-first-complete-entries",
        "blocker_window_markers": ["blockers_total", "blockers_truncated"],
        "trace_window_max_bytes": 65536,
        "trace_window_count": 6,
        "trace_window_markers": ["trace_truncated", "trace_window"],
        "compact_exclusions": ["trace_entries", "history", "windows", "readback_values", "prompts", "instructions", "statements", "suggestion_argv"],
        "status_readback_values": False,
    }
    if status_contract != expected_status:
        fail("v2 status payload bounds, exclusions, or truncation semantics drift")
    status_schema = load_object(repository_file("assets/schemas/status-result-v2.schema.json", "v2 status schema"))
    history_fields = {
        "current_trace_history",
        "stale_attempt_history",
        "decision_history",
        "rework_history",
        "stale_goal_revision_history",
        "stale_goal_assessment_history",
    }
    if history_fields.difference(status_schema.get("properties", {})) or len(history_fields) != status_contract["trace_window_count"]:
        fail("v2 verbose history family count drifts from the status payload contract")
    maximum_status_fixture = load_object(V2_MAXIMUM_STATUS_FIXTURE_PATH)
    stale_reference_assertion = "stale-attempt history carries at most eight stale or unresolved reference metadata entries without read-back values"
    if stale_reference_assertion not in maximum_status_fixture.get("future_assertions", []):
        fail("v2 maximum-status recipe omits the bounded stale-reference projection")
    expected_projections = [
        {"projection": "compact-status", "schema": "assets/schemas/compact-status-result-v2.schema.json", "maximum_bytes": 262144, "test_class": "payload-boundary", "implementation_status": "planned"},
        {"projection": "standard-status", "schema": "assets/schemas/status-result-v2.schema.json", "maximum_bytes": 1048576, "test_class": "payload-boundary", "implementation_status": "planned"},
        {"projection": "verbose-status", "schema": "assets/schemas/status-result-v2.schema.json", "maximum_bytes": 1048576, "window_bytes": status_contract["trace_window_max_bytes"], "window_count": status_contract["trace_window_count"], "test_class": "payload-boundary", "implementation_status": "planned"},
    ]
    if matrix["projection_bounds"] != expected_projections:
        fail("v2 payload projection bounds or planned evidence status drift")
    for projection in matrix["projection_bounds"]:
        repository_file(projection["schema"], f"{projection['projection']} schema")
    requirements = matrix["requirements"]
    expected_ids = [identifier for identifier, item in acceptance.items() if item["section"] == "17.9"]
    if not isinstance(requirements, list) or [item.get("acceptance_id") for item in requirements if isinstance(item, dict)] != expected_ids:
        fail("v2 payload matrix must cover §17.9 exactly once and in order")
    for number, item in enumerate(requirements, start=1):
        require_exact_keys(item, {"id", "acceptance_id", "test_class", "owning_tasks", "implementation_status"}, f"V2PAY-{number:03d}")
        if item["id"] != f"V2PAY-{number:03d}" or item["test_class"] != "payload-boundary" or item["implementation_status"] != "planned" or item["owning_tasks"] != acceptance[item["acceptance_id"]]["owning_tasks"]:
            fail(f"V2PAY-{number:03d} overstates evidence or drifts from its acceptance criterion")
    return len(requirements)


def validate_v2_release_matrix(acceptance: dict[str, dict[str, Any]], matrix: dict[str, Any] | None = None) -> int:
    matrix = load_object(V2_RELEASE_PATH) if matrix is None else matrix
    require_exact_keys(matrix, {"schema", "version", "source_acceptance_matrix", "admission_rule", "admission_policy", "task_test_policy", "categories", "final_gates"}, "v2 release gate matrix")
    if matrix["schema"] != "podway.v2-release-gate-matrix/v1" or matrix["version"] != 1:
        fail("v2 release gate matrix schema or version is unsupported")
    expected_admission = {
        "read_only_authoring_during_development": True,
        "normal_v2_session_admission": "closed",
        "development_unlock_requires_all": ["explicit-development-only-build-feature", "development-mode", "disposable-workspace-marker", "separate-socket", "separate-state-directory"],
        "development_unlock_refuses": ["installed-daemon", "launch-agent", "normally-registered-workspace"],
        "development_state_migration_promise": False,
        "release_qualification_requires_unlock_absent": True,
    }
    if matrix["admission_policy"] != expected_admission:
        fail("v2 admission and development-unlock policy drift")
    expected_task_policy = {
        "focused_success_and_failure_tests": True,
        "affected_specs_and_machine_contracts_same_change": True,
        "v1_regression_required": True,
        "make_test_before_executable_task_completion": True,
        "final_integrated_make_test_task": "V2REL-005",
        "sole_release_readiness_task": "V2REL-006",
    }
    if matrix["task_test_policy"] != expected_task_policy:
        fail("v2 per-task development gate policy drift")
    categories = matrix["categories"]
    if not isinstance(categories, list) or len(categories) != 9:
        fail("v2 release gate matrix must contain nine acceptance categories")
    observed: list[str] = []
    gate_owners = ["V2REL-001", "V2REL-005", "V2REL-003", "V2REL-005", "V2REL-005", "V2REL-003", "V2REL-001", "V2REL-003", "V2REL-002"]
    gates = ["make test", "make test", "make test-e2e", "make test", "make test", "make test-e2e", "make test", "make test-e2e", "make test-fuzzing"]
    roadmap_tasks = roadmap_v2_tasks()
    for number, (category, section) in enumerate(zip(categories, V2_SECTION_COUNTS), start=1):
        require_exact_keys(category, {"id", "section", "acceptance_ids", "required_tasks", "gate_owner_task", "gate", "implementation_status"}, f"V2GATE-{number:02d}")
        expected = [identifier for identifier, item in acceptance.items() if item["section"] == section]
        expected_tasks: list[str] = []
        for identifier in expected:
            for task in acceptance[identifier]["owning_tasks"]:
                if task not in expected_tasks:
                    expected_tasks.append(task)
        expected_status = "automated" if all(acceptance[identifier]["implementation_status"] == "automated" for identifier in expected) else "planned"
        if category["id"] != f"V2GATE-{number:02d}" or category["section"] != section or category["acceptance_ids"] != expected or category["required_tasks"] != expected_tasks or category["gate_owner_task"] != gate_owners[number - 1] or category["gate"] != gates[number - 1] or category["implementation_status"] != expected_status:
            fail(f"V2GATE-{number:02d} does not exactly cover its §17 category")
        if any(task not in roadmap_tasks for task in category["required_tasks"] + [category["gate_owner_task"]]):
            fail(f"V2GATE-{number:02d} references an unregistered roadmap task")
        observed.extend(category["acceptance_ids"])
    if observed != list(acceptance):
        fail("v2 release categories must partition all 87 acceptance criteria")
    gates = matrix["final_gates"]
    expected_gates = [
        {"task": "V2REL-005", "command": "make test", "implementation_status": "planned"},
        {"task": "V2REL-006", "command": "make dist", "implementation_status": "planned"},
        {"task": "V2REL-007", "command": "explicit release authorization", "implementation_status": "planned", "conditions": ["all-prior-pv2ga-tasks-completed", "public-v2-admission-enabled-only-in-qualified-artifacts", "immutable-release-evidence-recorded", "explicit-release-authorization", "dossier-and-roadmap-archived"]},
    ]
    if gates != expected_gates:
        fail("v2 final release gates drift")
    return len(categories)


def self_test_v2_matrices() -> int:
    baseline = load_object(V2_ACCEPTANCE_PATH)
    acceptance = validate_v2_acceptance_matrix(baseline)

    def expect_failure(action: Any, expected: str, label: str) -> None:
        try:
            action()
        except ContractError as error:
            if expected not in str(error):
                fail(f"{label} sentinel failed incorrectly: {error}")
        else:
            fail(f"{label} sentinel unexpectedly passed")

    missing = copy.deepcopy(baseline)
    missing["criteria"].pop()
    expect_failure(lambda: validate_v2_acceptance_matrix(missing), "exactly 87", "v2 acceptance missing-entry")
    wrong_line = copy.deepcopy(baseline)
    wrong_line["criteria"][0]["line"] += 1
    expect_failure(lambda: validate_v2_acceptance_matrix(wrong_line), "source bullet", "v2 acceptance source-drift")
    missing_proof = copy.deepcopy(baseline)
    del missing_proof["criteria"][0]["proof"]
    expect_failure(lambda: validate_v2_acceptance_matrix(missing_proof), "unexpected or missing fields", "v2 acceptance missing-proof")
    planned_with_proof = copy.deepcopy(baseline)
    automated = next(item for item in planned_with_proof["criteria"] if item["implementation_status"] == "automated")
    planned = next(item for item in planned_with_proof["criteria"] if item["implementation_status"] == "planned")
    planned["proof"] = copy.deepcopy(automated["proof"])
    expect_failure(lambda: validate_v2_acceptance_matrix(planned_with_proof), "unexpected or missing fields", "v2 acceptance planned-proof")
    wrong_proof_command = copy.deepcopy(baseline)
    wrong_proof_command["criteria"][0]["proof"]["command"] += " --ignored"
    expect_failure(lambda: validate_v2_acceptance_matrix(wrong_proof_command), "proof command is not exact", "v2 acceptance proof-command")
    wrong_acceptance_policy = copy.deepcopy(baseline)
    wrong_acceptance_policy["evidence_policy"] = "Automated means reviewed."
    expect_failure(lambda: validate_v2_acceptance_matrix(wrong_acceptance_policy), "evidence policy drift", "v2 acceptance policy")
    missing_anchor = copy.deepcopy(baseline)
    del missing_anchor["criteria"][0]["contract_refs"]
    expect_failure(lambda: validate_v2_acceptance_matrix(missing_anchor), "unexpected or missing fields", "v2 acceptance missing-anchor")
    wrong_anchor = copy.deepcopy(baseline)
    wrong_anchor["criteria"][0]["contract_refs"][0]["anchor"] = "## Missing contract heading"
    expect_failure(lambda: validate_v2_acceptance_matrix(wrong_anchor), "text anchor does not resolve", "v2 acceptance wrong-anchor")
    wrong_anchor_path = copy.deepcopy(baseline)
    wrong_anchor_path["criteria"][0]["contract_refs"][0]["path"] = "docs/specs/domain/domain-model.md"
    expect_failure(lambda: validate_v2_acceptance_matrix(wrong_anchor_path), "must be one of the criterion contract paths", "v2 acceptance anchor-path")
    duplicate_anchor = copy.deepcopy(baseline)
    duplicate_anchor["criteria"][0]["contract_refs"].append(copy.deepcopy(duplicate_anchor["criteria"][0]["contract_refs"][0]))
    expect_failure(lambda: validate_v2_acceptance_matrix(duplicate_anchor), "contract references must be unique", "v2 acceptance duplicate-anchor")

    compatibility = load_object(V2_COMPATIBILITY_PATH)
    missing_surface = copy.deepcopy(compatibility)
    missing_surface["surface_cases"].pop()
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, missing_surface), "specialized surface cases", "v2 compatibility missing-surface")
    orphan_surface = copy.deepcopy(compatibility)
    orphan_surface["surface_cases"].append(copy.deepcopy(orphan_surface["surface_cases"][-1]))
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, orphan_surface), "specialized surface cases", "v2 compatibility orphan-surface")
    missing_frozen = copy.deepcopy(compatibility)
    missing_frozen["v1_frozen_assets"].pop()
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, missing_frozen), "exact 13 released", "v2 compatibility frozen-family")
    wrong_frozen_digest = copy.deepcopy(compatibility)
    wrong_frozen_digest["v1_frozen_assets"][0]["sha256"] = f"sha256:{'f' * 64}"
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, wrong_frozen_digest), "exact 13 released", "v2 compatibility frozen-digest")
    missing_family = copy.deepcopy(compatibility)
    missing_family["result_family_inventories"]["existing_route_v2"].pop()
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, missing_family), "result family inventories", "v2 compatibility family-inventory")
    wrong_boundary = copy.deepcopy(compatibility)
    wrong_boundary["requirements"][2]["boundary"] = "v1-output"
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, wrong_boundary), "drifts from its acceptance criterion", "v2 compatibility boundary")
    wrong_compatibility_class = copy.deepcopy(compatibility)
    wrong_compatibility_class["requirements"][0]["test_class"] = "unit"
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, wrong_compatibility_class), "drifts from its acceptance criterion", "v2 compatibility test-class")
    wrong_compatibility_path = copy.deepcopy(compatibility)
    wrong_compatibility_path["requirements"][0]["contract_paths"] = ["missing-contract"]
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, wrong_compatibility_path), "drifts from its acceptance criterion", "v2 compatibility contract-path")
    wrong_compatibility_policy = copy.deepcopy(compatibility)
    wrong_compatibility_policy["evidence_policy"] = "All entries are planned."
    expect_failure(lambda: validate_v2_compatibility_matrix(acceptance, wrong_compatibility_policy), "evidence policy drift", "v2 compatibility policy")

    payload = load_object(V2_PAYLOAD_PATH)
    missing_leaf = copy.deepcopy(payload)
    missing_leaf["components"][1]["selectors"].remove("title")
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, missing_leaf), "field coverage drift", "v2 payload missing-leaf")
    double_leaf = copy.deepcopy(payload)
    double_leaf["components"][0]["selectors"].append("title")
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, double_leaf), "assign every field once", "v2 payload double-leaf")
    wrong_sum = copy.deepcopy(payload)
    wrong_sum["components"][0]["bytes"] += 1
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_sum), "component sum", "v2 payload arithmetic")
    wrong_partition = copy.deepcopy(payload)
    wrong_partition["suggestion_partition"]["predicates"][0] = "command!=goal.define"
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_partition), "exact exhaustive suggestion partition", "v2 payload suggestion-partition")
    wrong_projection = copy.deepcopy(payload)
    wrong_projection["projection_bounds"][0]["maximum_bytes"] += 1
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_projection), "projection bounds", "v2 payload projection")
    wrong_window_count = copy.deepcopy(payload)
    wrong_window_count["projection_bounds"][2]["window_count"] -= 1
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_window_count), "projection bounds", "v2 payload window-count")
    wrong_status = copy.deepcopy(payload)
    wrong_status["status_contract"]["status_readback_values"] = True
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_status), "status payload bounds", "v2 payload status")
    wrong_envelope = copy.deepcopy(payload)
    wrong_envelope["output_envelope"]["selectors"].pop()
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_envelope), "output envelope binding", "v2 payload envelope")
    wrong_envelope_component = copy.deepcopy(payload)
    wrong_envelope_component["output_envelope"]["component"] = "NEXT_STATIC_BUDGET"
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_envelope_component), "output envelope binding", "v2 payload envelope-component")
    wrong_shape = copy.deepcopy(payload)
    wrong_shape["resolved_next_shape"]["unique_instance_leaf_paths"] += 1
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_shape), "resolved next shape", "v2 payload resolved-shape")
    wrong_payload_class = copy.deepcopy(payload)
    wrong_payload_class["requirements"][0]["test_class"] = "unit"
    expect_failure(lambda: validate_v2_payload_matrix(acceptance, wrong_payload_class), "V2PAY-001", "v2 payload test-class")

    release = load_object(V2_RELEASE_PATH)
    wrong_admission = copy.deepcopy(release)
    wrong_admission["admission_policy"]["normal_v2_session_admission"] = "open"
    expect_failure(lambda: validate_v2_release_matrix(acceptance, wrong_admission), "admission and development-unlock", "v2 release admission")
    wrong_task_policy = copy.deepcopy(release)
    wrong_task_policy["task_test_policy"]["make_test_before_executable_task_completion"] = False
    expect_failure(lambda: validate_v2_release_matrix(acceptance, wrong_task_policy), "per-task development gate", "v2 release task-policy")
    wrong_category = copy.deepcopy(release)
    wrong_category["categories"][0]["acceptance_ids"].pop()
    expect_failure(lambda: validate_v2_release_matrix(acceptance, wrong_category), "exactly cover", "v2 release category")
    wrong_closeout = copy.deepcopy(release)
    wrong_closeout["final_gates"][2]["conditions"].pop()
    expect_failure(lambda: validate_v2_release_matrix(acceptance, wrong_closeout), "final release gates", "v2 release closeout")
    wrong_category_task = copy.deepcopy(release)
    wrong_category_task["categories"][0]["required_tasks"].pop()
    expect_failure(lambda: validate_v2_release_matrix(acceptance, wrong_category_task), "exactly cover", "v2 release category-task")
    wrong_category_gate = copy.deepcopy(release)
    wrong_category_gate["categories"][0]["gate"] = "make dist"
    expect_failure(lambda: validate_v2_release_matrix(acceptance, wrong_category_gate), "exactly cover", "v2 release category-gate")
    overstated_category = copy.deepcopy(release)
    overstated_category["categories"][6]["implementation_status"] = "automated"
    expect_failure(lambda: validate_v2_release_matrix(acceptance, overstated_category), "exactly cover", "v2 release category-status")
    return 35


def main() -> int:
    try:
        criteria, proof_files = validate_acceptance_matrix()
        dolgi_sentinels = self_test_dolgorae_acceptance_matrix()
        dolgi_tasks, dolgi_proof_files = validate_dolgorae_acceptance_matrix()
        crash_windows = validate_crash_registry()
        v2_acceptance = validate_v2_acceptance_matrix()
        v2_compatibility = validate_v2_compatibility_matrix(v2_acceptance)
        v2_payload = validate_v2_payload_matrix(v2_acceptance)
        v2_release = validate_v2_release_matrix(v2_acceptance)
        v2_sentinels = self_test_v2_matrices()
        v2_contract_semantics = validate_v2_contract_semantics()
    except ContractError as error:
        print(f"quality contract verification failed: {error}")
        return 1
    print(
        f"quality contracts verified: {criteria} acceptance criteria, "
        f"{proof_files} proof files, {dolgi_tasks} DOLGI tasks, "
        f"{dolgi_proof_files} DOLGI proof files, {dolgi_sentinels} DOLGI sentinels, "
        f"{crash_windows} crash windows, {len(v2_acceptance)} v2 acceptance criteria, "
        f"{v2_compatibility} v2 compatibility requirements, {v2_payload} v2 payload requirements, "
        f"{v2_release} v2 release categories, {v2_sentinels} v2 sentinels, "
        f"{v2_contract_semantics} v2 contract semantic checks"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
