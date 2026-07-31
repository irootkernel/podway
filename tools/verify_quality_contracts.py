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
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
FUNCTION_RE_TEMPLATE = r"\bfn\s+{name}\s*(?:<[^>]*>)?\s*\("
PYTHON_FUNCTION_RE_TEMPLATE = r"^def\s+{name}\s*\("
DOLGI_TASKS = {
    "DOLGI001": ("Build the controlled-PATH integration harness", ["AUT-T-PATH"]),
    "DOLGI002": ("Verify service and quiescent observation", ["AUT-T-OBS"]),
    "DOLGI003": ("Verify session and item operations", ["AUT-T-ID", "AUT-T-START"]),
    "DOLGI004": ("Verify conflict and reconciliation paths", ["AUT-T-ID", "AUT-T-RECON"]),
    "DOLGI005": ("Verify the packaged test-fixture archive", ["AUT-T-DIST"]),
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


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def repository_file(relative: Any, label: str) -> Path:
    if not isinstance(relative, str) or not relative or Path(relative).is_absolute():
        fail(f"{label} must be a non-empty repository-relative path")
    path = (ROOT / relative).resolve()
    if not path.is_relative_to(ROOT) or path.is_symlink() or not path.is_file():
        fail(f"{label} does not resolve to a regular repository file: {relative}")
    return path


def require_test_member(member: Any, criterion_id: str, source_files: set[str]) -> set[str]:
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
    source_files.add(relative)
    obligations = member.get("obligation_ids", [])
    if not isinstance(obligations, list) or any(not isinstance(item, str) or not item for item in obligations):
        fail(f"{criterion_id} obligation_ids must be a string list")
    return set(obligations)


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
    proof_source_files: set[str] = set()
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
            obligations = require_test_member(proof, expected_id, proof_source_files)
            if obligations:
                fail(f"{expected_id} single-test proof must not declare semantic obligations")
        elif kind == "cargo-test-set":
            if proof.get("criterion_id") != expected_id or not isinstance(proof.get("members"), list) or not proof["members"]:
                fail(f"{expected_id} test-set proof is malformed")
            obligations: set[str] = set()
            for member in proof["members"]:
                if member.get("criterion_id") != expected_id:
                    fail(f"{expected_id} test-set member has the wrong criterion_id")
                member_obligations = require_test_member(member, expected_id, proof_source_files)
                if obligations.intersection(member_obligations):
                    fail(f"{expected_id} repeats a semantic obligation")
                obligations.update(member_obligations)
            semantic_coverage[expected_id] = obligations
        else:
            fail(f"{expected_id} has unsupported proof kind: {kind}")
    if seen_lines != set(source_bullets):
        fail("product acceptance matrix source lines do not exactly cover every mandatory acceptance bullet")

    source_files = matrix.get("source_files")
    if not isinstance(source_files, dict) or set(source_files) != proof_source_files:
        fail("product acceptance source_files must equal the exact proof-path set")
    for relative, expected_digest in source_files.items():
        if not isinstance(expected_digest, str) or DIGEST_RE.fullmatch(expected_digest) is None:
            fail(f"product acceptance source digest is malformed: {relative}")
        if sha256_file(repository_file(relative, "product acceptance source file")) != expected_digest:
            fail(f"product acceptance source file is stale: {relative}")

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

    closure = matrix.get("input_closure")
    if not isinstance(closure, dict) or closure.get("requireCompleteFileDigests") is not True:
        fail("product acceptance input closure is malformed")
    globs = closure.get("globs")
    if not isinstance(globs, list) or any(not isinstance(item, str) or not item for item in globs):
        fail("product acceptance input globs must be a non-empty string list")
    return len(criteria), len(proof_source_files)


def roadmap_dolgi_tasks() -> dict[str, tuple[str, str]]:
    lines = (ROOT / "docs/roadmap.md").read_text(encoding="utf-8").splitlines()
    try:
        start = lines.index("## DOLGI — Dolgorae Integration Conformance")
    except ValueError:
        fail("roadmap omits the DOLGI epic")
    tasks: dict[str, tuple[str, str]] = {}
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
        tasks[task_id] = (columns[1], columns[2])
    return tasks


def validate_dolgi_proof(member: Any, task_id: str, source_files: set[str]) -> None:
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
        if re.search(FUNCTION_RE_TEMPLATE.format(name=re.escape(function)), source) is None:
            fail(f"{task_id} proof function is missing from {relative}: {function}")
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
    else:
        fail(f"{task_id} has unsupported proof kind: {kind}")
    if command != expected:
        fail(f"{task_id} proof command is not exact: expected={expected}, actual={command}")
    source_files.add(relative)


def validate_dolgorae_acceptance_matrix(
    matrix_override: dict[str, Any] | None = None,
) -> tuple[int, int]:
    matrix = load_object(DOLGI_MATRIX_PATH) if matrix_override is None else matrix_override
    if matrix.get("schema") != "podway.dolgorae-acceptance-matrix/v1" or matrix.get("version") != 1:
        fail("DOLGI acceptance matrix schema or version is unsupported")
    tasks = matrix.get("tasks")
    if not isinstance(tasks, list):
        fail("DOLGI acceptance matrix tasks must be a list")
    roadmap_tasks = roadmap_dolgi_tasks()
    if roadmap_tasks != {
        task_id: (title, "Completed") for task_id, (title, _evidence) in DOLGI_TASKS.items()
    }:
        fail("roadmap DOLGI tasks do not match the completed acceptance inventory")
    if [task.get("id") for task in tasks if isinstance(task, dict)] != list(DOLGI_TASKS):
        fail("DOLGI acceptance tasks must be complete, unique, and ordered")

    proof_source_files: set[str] = set()
    for task in tasks:
        task_id = task["id"]
        title, expected_evidence = DOLGI_TASKS[task_id]
        if set(task) != {"evidence_ids", "id", "proofs", "status", "title"}:
            fail(f"{task_id} has unexpected or missing fields")
        if task["title"] != title or task["status"] != "Completed":
            fail(f"{task_id} title or completion status does not match the roadmap")
        if task["evidence_ids"] != expected_evidence:
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
            validate_dolgi_proof(proof, task_id, proof_source_files)

    source_files = matrix.get("source_files")
    if not isinstance(source_files, dict) or set(source_files) != proof_source_files:
        fail("DOLGI source_files must equal the exact proof-path set")
    for relative, expected_digest in source_files.items():
        if not isinstance(expected_digest, str) or DIGEST_RE.fullmatch(expected_digest) is None:
            fail(f"DOLGI proof source digest is malformed: {relative}")
        if sha256_file(repository_file(relative, "DOLGI proof source file")) != expected_digest:
            fail(f"DOLGI proof source file is stale: {relative}")
    return len(tasks), len(proof_source_files)


def self_test_dolgorae_acceptance_matrix() -> int:
    baseline = load_object(DOLGI_MATRIX_PATH)

    def expect_failure(matrix: dict[str, Any], expected: str) -> None:
        try:
            validate_dolgorae_acceptance_matrix(matrix)
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

    stale_digest = copy.deepcopy(baseline)
    first_source = next(iter(stale_digest["source_files"]))
    stale_digest["source_files"][first_source] = "0" * 64
    expect_failure(stale_digest, "proof source file is stale")
    return 4


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


def main() -> int:
    try:
        criteria, proof_files = validate_acceptance_matrix()
        dolgi_sentinels = self_test_dolgorae_acceptance_matrix()
        dolgi_tasks, dolgi_proof_files = validate_dolgorae_acceptance_matrix()
        crash_windows = validate_crash_registry()
    except ContractError as error:
        print(f"quality contract verification failed: {error}")
        return 1
    print(
        f"quality contracts verified: {criteria} acceptance criteria, "
        f"{proof_files} proof files, {dolgi_tasks} DOLGI tasks, "
        f"{dolgi_proof_files} DOLGI proof files, {dolgi_sentinels} DOLGI sentinels, "
        f"{crash_windows} crash windows"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
