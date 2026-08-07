#!/usr/bin/env python3
"""Verify canonical assets, crate boundaries, and command-route controls."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import shutil
import tempfile
import tomllib
from typing import Any, Callable

import repository_assets
import contract_manifest


ROOT = Path(__file__).resolve().parent.parent
ADJACENCY_PATH = Path("contracts/cargo-adjacency.json")
ROUTES_PATH = Path("contracts/command-routes.json")
ADJACENCY_VERSION = "podway.cargo-adjacency/v1"
ROUTES_VERSION = "podway.command-routes/v1"
MAKEFILE_PATH = Path("Makefile")
REQUIRED_MAKE_TARGETS = (
    "test",
    "test-prepare",
    "release-prepare",
    "dist-preflight",
    "lint-all",
    "test-rust",
    "test-unit",
    "test-int",
    "test-fuzzing",
    "test-e2e",
    "contract-verifier-test",
    "preset-create",
    "preset-import",
    "preset-tool-test",
    "dev-runtime-test",
    "dist",
    "contract-manifest",
)
REQUIRED_TEST_SEQUENCE = (
    "$(MAKE) test-prepare",
    "$(MAKE) test-rust",
    "$(MAKE) contract-verifier-test",
    "$(MAKE) test-e2e",
    "$(MAKE) preset-tool-test PRESET_VALIDATOR_READY=1",
    "$(MAKE) dev-runtime-test",
)
REQUIRED_PREPARE_COMMANDS = (
    "python3 tools/verify_docs.py",
    "cargo fmt --all -- --check",
    "cargo clippy --workspace --lib --bins --locked -- -D warnings",
    "cargo deny check",
    "python3 tools/verify_test_layout.py --check",
    "python3 tools/verify_quality_contracts.py",
    "python3 tools/verify_contracts.py --check",
    "python3 tools/verify_preset_tooling.py --podway",
    "python3 tools/contract_manifest.py --check",
    "--features release-contract-verifier",
    "CARGO_INCREMENTAL=0",
    "TEST_THREADS ?= 4",
    "--test-threads=$(TEST_THREADS)",
)
REQUIRED_RELEASE_PREPARE_COMMANDS = (
    "$(MAKE) lint-all",
    "cargo clippy --workspace --all-targets --locked -- -D warnings",
    "python3 tools/release_evidence.py self-test",
    "python3 tools/release_archive.py self-test",
    "python3 tools/qualify_distribution.py self-test",
    "python3 tools/create_dolgorae_handoff.py self-test",
    "python3 tools/verify_release_bundle.py self-test",
)
REQUIRED_RELEASE_COMMANDS = (
    "$(MAKE) dist-preflight",
    "$(MAKE) test",
    "$(MAKE) release-prepare",
    "$(MAKE) test-fuzzing",
    "cargo build --release --locked",
    "tools/release_archive.py package",
    "tools/qualify_distribution.py qualify",
    "tools/create_dolgorae_handoff.py create",
    "tools/verify_release_bundle.py verify",
)
CRATE_ORDER = (
    "podway-core",
    "podway-protocol",
    "podway-config",
    "podway-store",
    "podway-git",
    "podway-service",
    "podway-presets",
    "podway-daemon",
    "podway-cli",
)
EXPECTED_ADJACENCY = {
    "podway-core": set(),
    "podway-protocol": {"podway-core"},
    "podway-config": {"podway-core"},
    "podway-store": {"podway-core"},
    "podway-git": {"podway-core"},
    "podway-service": {"podway-core"},
    "podway-presets": {"podway-core", "podway-config"},
    "podway-daemon": {
        "podway-core",
        "podway-protocol",
        "podway-config",
        "podway-store",
        "podway-git",
        "podway-service",
        "podway-presets",
    },
    "podway-cli": {
        "podway-core",
        "podway-protocol",
        "podway-config",
        "podway-presets",
        "podway-service",
    },
}
LOCAL_COMMANDS = {
    "help",
    "version",
    "completions",
    "procedure.validate",
    "procedure.show",
    "procedure.format",
    "procedure.vet",
    "procedure.lint",
    "procedure.check",
    "procedure.graph",
    "procedure.preview",
    "procedure.scaffold",
    "procedure.convert",
    "preset.list",
    "preset.show",
    "preset.explain",
}
SERVICE_COMMANDS = {
    "daemon.install",
    "daemon.uninstall",
    "daemon.start",
    "daemon.stop",
    "daemon.restart",
    "daemon.status",
    "daemon.logs",
}
PROHIBITED_CAPABILITIES = {"command_runner", "git_mutation", "network"}
DEPENDENCY_TABLES = {"dependencies", "dev-dependencies", "build-dependencies"}
V2_ROUTE_DELTA = {
    "procedure.format", "procedure.vet", "procedure.lint", "procedure.check",
    "procedure.graph", "procedure.preview", "procedure.scaffold", "procedure.convert",
    "session.decide", "session.rework", "goal.define", "goal.revise",
    "goal.assess_criterion",
}
# V2_ROUTE_DELTA members whose owning task has landed. The delta itself never shrinks: a route
# stays registered forever, and this set records only which of them the build now serves.
V2_EXECUTABLE_ROUTES = frozenset({"procedure.format"})
V1_ROUTE_BASELINE = {
    "help", "version", "completions", "procedure.validate", "procedure.show",
    "preset.list", "preset.show", "preset.explain", "daemon.install", "daemon.uninstall",
    "daemon.start", "daemon.stop", "daemon.restart", "daemon.status", "daemon.terminate",
    "daemon.logs", "workspace.init", "workspace.doctor", "workspace.show", "workspace.repair",
    "session.start", "session.start_replace", "session.status", "session.next",
    "session.complete", "session.skip", "session.retry", "session.return", "session.block",
    "session.unblock", "session.cancel", "session.reopen", "session.reset",
    "workspace.reset_all", "item.check", "item.uncheck", "item.set", "item.add",
    "item.remove", "item.attach", "item.clear", "job.list", "job.lookup", "job.status",
    "job.wait", "job.cancel",
}
V1_RUNTIME_ERROR_BASELINE = (
    "DAEMON_NOT_INSTALLED", "DAEMON_UNAVAILABLE", "DAEMON_SHUTTING_DOWN",
    "DAEMON_VERSION_INCOMPATIBLE", "DAEMON_CONTRACT_MISMATCH",
    "PROTOCOL_VERSION_UNSUPPORTED", "REQUEST_TOO_LARGE", "REQUEST_INVALID",
    "SOCKET_ENDPOINT_INVALID", "NOT_A_GIT_WORKTREE", "BARE_GIT_REPOSITORY",
    "WORKTREE_GONE", "WORKSPACE_NOT_INITIALIZED", "WORKSPACE_ALREADY_INITIALIZED",
    "WORKSPACE_INIT_CONFLICT", "WORKSPACE_ID_CONFLICT", "WORKSPACE_UUID_MISMATCH",
    "WORKSPACE_CONFIG_INVALID", "WORKSPACE_STATE_UNREADABLE", "WORKSPACE_SCHEMA_UNSUPPORTED",
    "WORKSPACE_QUEUE_FULL", "WORKSPACE_MAINTENANCE", "WORKSPACE_PATH_UNSAFE",
    "PATH_OUTSIDE_WORKTREE", "MIGRATION_FAILED", "PROCEDURE_NOT_FOUND", "PROCEDURE_INVALID",
    "PROCEDURE_SCHEMA_UNSUPPORTED", "PROCEDURE_DIGEST_MISMATCH", "PRESET_NOT_FOUND",
    "SESSION_NOT_FOUND", "SESSION_ID_MISMATCH", "SESSION_ALREADY_EXISTS",
    "SESSION_NOT_RUNNING", "SESSION_NOT_COMPLETED", "SESSION_CANCELLED",
    "SESSION_REVISION_CONFLICT", "ATTEMPT_NOT_CURRENT", "STAGE_NOT_FOUND",
    "STAGE_NOT_SKIPPABLE", "RETURN_NOT_ALLOWED", "REOPEN_NOT_ALLOWED",
    "REQUIRED_ITEMS_MISSING", "BLOCKERS_PRESENT", "BLOCKER_LIMIT_REACHED", "ITEM_NOT_FOUND",
    "ITEM_TYPE_MISMATCH", "ITEM_CONSTRAINT_FAILED", "ITEM_REVISION_CONFLICT",
    "ITEM_ALREADY_SET", "LIST_VALUE_NOT_FOUND", "LIST_VALUE_DUPLICATE", "ARTIFACT_NOT_FOUND",
    "ARTIFACT_UNREADABLE", "ARTIFACT_CHANGED", "ARTIFACT_MEDIA_TYPE_NOT_ALLOWED",
    "BLOCKER_NOT_FOUND", "BLOCKER_NOT_CURRENT", "IDEMPOTENCY_KEY_REUSED", "JOB_NOT_FOUND",
    "JOB_NOT_CANCELLABLE", "JOB_WAIT_TIMEOUT", "MUTATION_OUTCOME_UNKNOWN",
    "CONFIRMATION_REQUIRED", "INTERNAL_ERROR",
)
V2_RUNTIME_ERROR_CODES = {
    "PROCEDURE_V2_SCHEMA_INVALID", "GRAPH_NODE_NOT_FOUND", "NODE_DEFINITION_NOT_FOUND",
    "GRAPH_NODE_TYPE_MISMATCH", "OPTION_NOT_ALLOWED", "ROUTE_NOT_ALLOWED",
    "DECISION_REASON_MISSING", "EVIDENCE_REFERENCE_UNRESOLVED",
    "EVIDENCE_REFERENCE_STALE", "MANUAL_REWORK_TARGET_NOT_ALLOWED",
    "MANUAL_REWORK_TARGET_NOT_ON_TRACE", "GOAL_TRACKING_NOT_ENABLED",
    "SESSION_GOAL_MISSING", "SESSION_GOAL_ALREADY_DEFINED", "GOAL_REVISION_STALE",
    "GOAL_REVISION_TARGET_NOT_ALLOWED", "GOAL_REVISION_TARGET_NOT_REVISION_SAFE",
    "REACTIVATION_FLAG_REQUIRED", "CRITERION_MODE_MIXED", "CRITERION_CITATION_INVALID",
    "CRITERION_RESULT_MISSING", "CRITERION_NOT_FOUND", "FRESH_GOAL_ASSESSMENT_MISSING",
    "GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED", "DIGEST_CONFIRMATION_REQUIRED",
    "UNSUPPORTED_V2_CAPABILITY",
}
V2_AUTHORING_DIAGNOSTIC_CODES = {
    "AUTHORING_SCHEMA_INVALID", "SOURCE_CONSTRUCT_UNSUPPORTED", "FORMAT_NOT_CANONICAL",
    "SOURCE_PROJECTION_BUDGET_EXCEEDED",
    "ENTRY_NODE_INVALID", "GRAPH_DEFINITION_UNKNOWN", "ROUTE_TARGET_NOT_FOUND",
    "UNREACHABLE_GRAPH_NODE", "NO_TERMINAL_PATH", "ACTION_DISPOSITION_INVALID",
    "DECISION_OPTION_ROUTE_MISSING", "DECISION_ROUTE_OPTION_UNDEFINED",
    "GOAL_ASSESSMENT_OPTION_UNMAPPED", "GOAL_ASSESSMENT_OUTCOME_UNKNOWN",
    "GOAL_ASSESSMENT_OUTCOME_UNREACHABLE",
    "GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING",
    "GOAL_ASSESSMENT_NOT_DOMINATING_TERMINAL", "EVIDENCE_SOURCE_UNKNOWN",
    "EVIDENCE_SOURCE_SELF_REFERENCE", "EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER",
    "SKIPPABLE_EVIDENCE_SOURCE", "EVIDENCE_SELECTOR_UNKNOWN_ITEM",
    "READBACK_BUDGET_EXCEEDED", "NEXT_STATIC_BUDGET_EXCEEDED",
    "DECISION_SKIP_NOT_ALLOWED", "GRAPH_CYCLE_INVALID", "REWORK_TARGET_NOT_DOMINATING",
    "MANUAL_REWORK_TARGET_UNKNOWN", "AMBIGUOUS_GRAPH_REFERENCE", "UNUSED_NODE_DEFINITION",
    "SINGLE_OPTION_DECISION", "INDISTINGUISHABLE_OPTION_LABELS", "IDENTICAL_EFFECTIVE_ROUTES",
    "WEAK_PURPOSE_GUIDANCE", "WEAK_INTENT_GUIDANCE", "WEAK_OBJECTIVE_GUIDANCE",
    "WEAK_PROMPT_GUIDANCE", "WEAK_CRITERIA_GUIDANCE", "WEAK_REASON_GUIDANCE",
    "EVIDENCE_GUIDANCE_MISSING", "OPTIONAL_EVIDENCE_UNRESOLVABLE",
    "GOAL_CLARIFICATION_PATH_MISSING", "GOAL_ASSESSMENT_TOO_EARLY",
    "MANUAL_REWORK_TARGETS_BROAD", "LARGE_OPTION_SET", "LARGE_CYCLE",
    "DUPLICATED_NODE_DEFINITION", "GRAPH_NODE_ID_CONFUSING", "REWORK_TOPOLOGY_CONFUSING",
    "NO_REACTIVATION_PATH", "GOAL_REVISION_TARGET_UNSAFE",
    "MULTIPLE_GOAL_ASSESSMENT_SOURCES",
}


class VerificationError(Exception):
    """A Phase 0A contract invariant was violated."""

    def __init__(self, message: str, code: str = "contract_verification_failed") -> None:
        super().__init__(message)
        self.code = code


def fail(message: str, code: str = "contract_verification_failed") -> None:
    raise VerificationError(message, code)


def read_json(root: Path, relative: Path, label: str) -> dict[str, Any]:
    path = repository_assets.checked_path(root, relative, label)
    if not path.is_file():
        fail(f"{label} is missing: {relative.as_posix()}")
    try:
        with path.open(encoding="utf-8") as handle:
            value = json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {label}: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def catalog_route_availability(root: Path) -> dict[str, str]:
    path = repository_assets.checked_path(
        root, Path("assets/specifications/command-catalog.yaml"), "command catalog"
    )
    if not path.is_file():
        fail("command catalog is missing")
    text = path.read_text(encoding="utf-8")
    if not re.search(r"^schema: podway\.command-catalog/v1$", text, flags=re.MULTILINE):
        fail("command catalog does not declare podway.command-catalog/v1")
    names = re.findall(r"^- name: ([a-z][a-z0-9._-]*)$", text, flags=re.MULTILINE)
    entries = re.findall(
        r"^- name: ([a-z][a-z0-9._-]*)\n  availability: ([a-z_]+)$",
        text,
        flags=re.MULTILINE,
    )
    if not names:
        fail("command catalog contains no commands")
    if len(names) != len(set(names)):
        fail("command catalog command names must be unique")
    if len(entries) != len(names):
        fail("every command catalog entry must declare availability immediately after its name")
    availability = dict(entries)
    if set(availability.values()) - {"executable", "reserved_contract"}:
        fail("command catalog availability must be executable or reserved_contract")
    return availability


def catalog_commands(root: Path) -> set[str]:
    return set(catalog_route_availability(root))


def expected_route_availability(command: str) -> str:
    """The single derivation both availability surfaces must agree with."""
    if command in V2_EXECUTABLE_ROUTES:
        return "executable"
    if command in V2_ROUTE_DELTA:
        return "reserved_contract"
    return "executable"


def validate_v2_catalog_delta(root: Path) -> int:
    availability = catalog_route_availability(root)
    commands = set(availability)
    if commands != V1_ROUTE_BASELINE | V2_ROUTE_DELTA:
        fail("command catalog must contain the 46-route baseline plus exactly 13 v2 routes")
    expected_availability = {command: expected_route_availability(command) for command in commands}
    if availability != expected_availability:
        fail("command catalog availability must match the served v1 baseline and v2 delta exactly")

    runtime = read_json(root, Path("assets/specifications/error-codes.json"), "error catalog")
    if set(runtime) != {"schema", "exit_codes", "errors"}:
        fail("error catalog has unexpected or missing top-level fields")
    entries = runtime.get("errors")
    if not isinstance(entries, list) or len(entries) != 91:
        fail("runtime error catalog must contain the 65-code baseline plus 26 v2 codes")
    runtime_codes = [entry.get("code") for entry in entries if isinstance(entry, dict)]
    if len(runtime_codes) != len(entries) or len(set(runtime_codes)) != len(runtime_codes):
        fail("runtime error catalog codes must be unique strings")
    if tuple(runtime_codes[:65]) != V1_RUNTIME_ERROR_BASELINE or set(runtime_codes[65:]) != V2_RUNTIME_ERROR_CODES:
        fail("runtime error catalog omits a required v2 code")
    for entry in entries:
        if not isinstance(entry.get("summary"), str) or not entry["summary"]:
            fail("runtime error entries require a summary")
        if not isinstance(entry.get("exit_code"), int) or str(entry["exit_code"]) not in runtime["exit_codes"]:
            fail("runtime error entry has an invalid exit code")
        if not isinstance(entry.get("retryable"), bool):
            fail("runtime error entry has an invalid retryability value")
    v2_details_schemas = {
        entry.get("code"): entry.get("details_schema") for entry in entries[65:]
    }
    if v2_details_schemas != dict.fromkeys(
        V2_RUNTIME_ERROR_CODES, "podway.v2-runtime-error-details/v1"
    ):
        fail("every v2 runtime error must bind the closed v2 details schema")

    authoring = read_json(
        root, Path("assets/specifications/authoring-diagnostics.json"),
        "authoring diagnostic catalog",
    )
    if set(authoring) != {"schema", "diagnostics"} or authoring.get("schema") != "podway.authoring-diagnostic-catalog/v1":
        fail("authoring diagnostic catalog has an invalid shape or identity")
    diagnostics = authoring.get("diagnostics")
    if not isinstance(diagnostics, list) or len(diagnostics) != len(V2_AUTHORING_DIAGNOSTIC_CODES):
        fail("authoring diagnostic catalog must be the exhaustive v2 inventory")
    diagnostic_codes = [entry.get("code") for entry in diagnostics if isinstance(entry, dict)]
    if set(diagnostic_codes) != V2_AUTHORING_DIAGNOSTIC_CODES or len(diagnostic_codes) != len(set(diagnostic_codes)):
        fail("authoring diagnostic catalog has missing, duplicate, or unapproved codes")
    if set(runtime_codes) & set(diagnostic_codes):
        fail("runtime errors and authoring diagnostics must use disjoint code namespaces")
    for entry in diagnostics:
        if set(entry) != {"code", "severity", "summary"}:
            fail("authoring diagnostic entry has unexpected or missing fields")
        if entry["severity"] not in {"error", "warning"} or not isinstance(entry["summary"], str) or not entry["summary"]:
            fail("authoring diagnostic entry has an invalid severity or summary")
    return len(entries) + len(diagnostics)


def validate_contract_identifiers(root: Path) -> int:
    adjacency = read_json(root, ADJACENCY_PATH, "Cargo adjacency contract")
    routes = read_json(root, ROUTES_PATH, "command route contract")
    expected_contract_versions = {
        "Cargo adjacency contract": (adjacency, ADJACENCY_VERSION),
        "command route contract": (routes, ROUTES_VERSION),
    }
    for label, (contract, expected_version) in expected_contract_versions.items():
        if contract.get("contract_version") != expected_version:
            fail(f"{label} does not declare {expected_version}")

    schema_files = sorted(
        path
        for path in repository_assets.regular_files(root, "assets/schemas", required=True)
        if path.endswith(".schema.json")
    )
    if not schema_files:
        fail("no versioned JSON schemas were found")
    for relative_name in schema_files:
        path = repository_assets.checked_path(root, Path(relative_name), "schema")
        try:
            schema = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            fail(f"cannot parse schema {relative_name}: {error}")
        match = re.fullmatch(r"(.+)-v([1-9][0-9]*)\.schema\.json", Path(relative_name).name)
        if match is None:
            fail(f"schema filename is not versioned: {relative_name}")
        basename, version = match.groups()
        expected_id = f"urn:podway:schema:{basename}:v{version}"
        if schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
            fail(f"schema draft identifier drift: {relative_name}")
        if schema.get("$id") != expected_id:
            fail(f"schema identifier drift: {relative_name}")

    error_catalog = read_json(root, Path("assets/specifications/error-codes.json"), "error catalog")
    if error_catalog.get("schema") != "podway.error-catalog/v1":
        fail("error catalog does not declare podway.error-catalog/v1")
    validate_v2_catalog_delta(root)
    catalog_commands(root)
    preset_files = sorted(
        path
        for path in repository_assets.regular_files(root, "assets/presets", required=True)
        if path.endswith(".yaml")
    )
    if not preset_files:
        fail("no v1 preset files were found")
    for relative_name in preset_files:
        path = repository_assets.checked_path(root, Path(relative_name), "preset")
        text = path.read_text(encoding="utf-8")
        if not re.search(r"^schema: podway\.procedure/v1$", text, flags=re.MULTILINE):
            fail(f"preset does not declare podway.procedure/v1: {relative_name}")
    return len(schema_files) + len(preset_files) + 5


def load_adjacency_contract(root: Path) -> list[dict[str, Any]]:
    contract = read_json(root, ADJACENCY_PATH, "Cargo adjacency contract")
    if set(contract) != {"contract_version", "owner", "crates"}:
        fail("Cargo adjacency contract has unexpected or missing top-level fields")
    if contract["contract_version"] != ADJACENCY_VERSION or contract["owner"] != "architecture":
        fail("Cargo adjacency contract has an invalid v1 identity or owner")
    crates = contract["crates"]
    if not isinstance(crates, list) or len(crates) != len(CRATE_ORDER):
        fail("Cargo adjacency contract must define all nine crates exactly once")

    seen: set[str] = set()
    known = set(CRATE_ORDER)
    for entry in crates:
        if not isinstance(entry, dict) or set(entry) != {
            "name",
            "path",
            "approved_dependencies",
            "forbidden_dependencies",
        }:
            fail("Cargo adjacency crate entry has unexpected or missing fields")
        name = entry["name"]
        path = entry["path"]
        approved = entry["approved_dependencies"]
        forbidden = entry["forbidden_dependencies"]
        if not isinstance(name, str) or name in seen or name not in known:
            fail("Cargo adjacency crate names must be unique approved crate names")
        if path != f"crates/{name}":
            fail(f"Cargo adjacency path drift for {name}")
        if not isinstance(approved, list) or not isinstance(forbidden, list):
            fail(f"Cargo adjacency dependencies must be lists for {name}")
        if not all(isinstance(value, str) for value in approved + forbidden):
            fail(f"Cargo adjacency dependency names must be strings for {name}")
        approved_set = set(approved)
        forbidden_set = set(forbidden)
        if len(approved_set) != len(approved) or len(forbidden_set) != len(forbidden):
            fail(f"Cargo adjacency dependencies must not contain duplicates for {name}")
        if approved_set != EXPECTED_ADJACENCY[name]:
            fail(f"Cargo approved edges drift for {name}")
        if forbidden_set != known - {name} - approved_set:
            fail(f"Cargo forbidden edges drift for {name}")
        seen.add(name)
    if seen != known:
        fail("Cargo adjacency contract does not define exactly the approved crates")
    return crates


def read_toml(path: Path, label: str) -> dict[str, Any]:
    if not path.is_file():
        fail(f"{label} is missing: {path.as_posix()}")
    try:
        with path.open("rb") as handle:
            value = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {label}: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a TOML table")
    return value


def internal_dependencies(table: dict[str, Any], known: set[str]) -> set[str]:
    discovered: set[str] = set()

    def visit(value: Any) -> None:
        if not isinstance(value, dict):
            return
        for key, child in value.items():
            if key in DEPENDENCY_TABLES:
                if not isinstance(child, dict):
                    fail(f"Cargo dependency table {key} is not a table")
                for dependency_name, specification in child.items():
                    if dependency_name in known:
                        discovered.add(dependency_name)
                    if isinstance(specification, dict):
                        package = specification.get("package")
                        if isinstance(package, str) and package in known:
                            discovered.add(package)
            else:
                visit(child)

    visit(table)
    return discovered


def validate_adjacency(root: Path) -> int:
    crates = load_adjacency_contract(root)
    known = set(CRATE_ORDER)
    root_manifest = read_toml(repository_assets.checked_path(root, Path("Cargo.toml"), "workspace manifest"), "workspace manifest")
    workspace = root_manifest.get("workspace")
    if not isinstance(workspace, dict) or not isinstance(workspace.get("members"), list):
        fail("workspace manifest must declare a members list")
    members = workspace["members"]
    expected_members = {entry["path"] for entry in crates}
    if not all(isinstance(member, str) for member in members) or set(members) != expected_members or len(members) != len(expected_members):
        fail("workspace members drift from the exact nine-crate adjacency contract")

    for crate in crates:
        name = crate["name"]
        manifest_path = repository_assets.checked_path(root, Path(crate["path"]) / "Cargo.toml", f"manifest for {name}")
        manifest = read_toml(manifest_path, f"manifest for {name}")
        package = manifest.get("package")
        if not isinstance(package, dict) or package.get("name") != name:
            fail(f"Cargo package name drift for {name}")
        actual = internal_dependencies(manifest, known)
        expected = EXPECTED_ADJACENCY[name]
        if actual != expected:
            fail(f"Cargo dependency adjacency drift for {name}; expected={sorted(expected)}, actual={sorted(actual)}")
    return len(crates)


def validate_routes(root: Path) -> int:
    contract = read_json(root, ROUTES_PATH, "command route contract")
    if set(contract) != {"contract_version", "owner", "prohibited_capabilities", "routes"}:
        fail("command route contract has unexpected or missing top-level fields")
    if contract["contract_version"] != ROUTES_VERSION or contract["owner"] != "architecture":
        fail("command route contract has an invalid v1 identity or owner")
    prohibited = contract["prohibited_capabilities"]
    if not isinstance(prohibited, list) or set(prohibited) != PROHIBITED_CAPABILITIES or len(prohibited) != len(PROHIBITED_CAPABILITIES):
        fail("command route contract must prohibit command_runner, git_mutation, and network")
    routes = contract["routes"]
    if not isinstance(routes, list) or len(routes) != 59:
        fail("command route contract routes must be a list")

    expected_commands = catalog_commands(root) | {"completions"}
    commands: set[str] = set()
    task_commands = expected_commands - LOCAL_COMMANDS - SERVICE_COMMANDS
    for route in routes:
        if not isinstance(route, dict) or set(route) != {
            "command", "availability", "owner", "path", "execution", "capabilities",
        }:
            fail("command route has unexpected or missing fields")
        command = route["command"]
        availability = route["availability"]
        owner = route["owner"]
        path = route["path"]
        execution = route["execution"]
        capabilities = route["capabilities"]
        if not isinstance(command, str) or command in commands or command not in expected_commands:
            fail("command routes must cover each approved command exactly once")
        if not isinstance(path, list) or not all(isinstance(item, str) for item in path):
            fail(f"route path is invalid for {command}")
        if not isinstance(capabilities, list) or not all(isinstance(item, str) for item in capabilities):
            fail(f"route capabilities are invalid for {command}")
        if capabilities or set(capabilities) & PROHIBITED_CAPABILITIES:
            fail(f"route capabilities violate the no-runner/no-Git-mutation/no-network policy: {command}")
        expected_availability = expected_route_availability(command)
        if availability != expected_availability:
            fail(f"route availability is invalid for {command}")
        if command in LOCAL_COMMANDS:
            expected_owner, expected_path, expected_execution = "podway-cli", ["podway-cli"], "local"
        elif command in SERVICE_COMMANDS:
            expected_owner, expected_path, expected_execution = (
                "podway-service",
                ["podway-cli", "podway-service"],
                "service_lifecycle",
            )
        elif command in task_commands:
            expected_owner, expected_path, expected_execution = (
                "podway-daemon",
                ["podway-cli", "podway-protocol", "podway-daemon"],
                "daemon",
            )
        else:
            fail(f"route has no approved ownership class: {command}")
        if owner != expected_owner or path != expected_path or execution != expected_execution:
            fail(f"route ownership or path bypass for {command}")
        commands.add(command)
    if commands != expected_commands:
        fail(f"command routes do not exactly cover catalog commands; missing={sorted(expected_commands - commands)}")
    return len(commands)
def validate_makefile_contract(root: Path) -> int:
    path = repository_assets.checked_path(root, MAKEFILE_PATH, "Makefile")
    if not path.is_file():
        fail("Makefile is missing", "makefile_contract_drift")
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read Makefile: {error}", "makefile_contract_drift")

    missing_targets = [
        target for target in REQUIRED_MAKE_TARGETS
        if re.search(rf"^{re.escape(target)}\s*:", text, flags=re.MULTILINE) is None
    ]
    if missing_targets:
        fail(f"Makefile omits required targets: {missing_targets}", "makefile_contract_drift")
    test_recipe = re.search(r"^test\s*:\s*\n((?:\t.*\n)+)", text, flags=re.MULTILINE)
    if test_recipe is None:
        fail("Makefile test target has no recipe", "makefile_contract_drift")
    commands = tuple(line.strip() for line in test_recipe.group(1).splitlines())
    if commands != REQUIRED_TEST_SEQUENCE:
        fail(
            "Makefile test target does not run the complete development gate sequentially",
            "makefile_contract_drift",
        )
    missing_commands = [command for command in REQUIRED_PREPARE_COMMANDS if command not in text]
    if missing_commands:
        fail(f"Makefile omits required prepare commands: {missing_commands}", "makefile_contract_drift")
    release_prepare_recipe = re.search(
        r"^release-prepare\s*:\s*\n((?:\t.*\n)+)", text, flags=re.MULTILINE
    )
    if release_prepare_recipe is None:
        fail("Makefile release-prepare target has no recipe", "makefile_contract_drift")
    release_prepare_text = release_prepare_recipe.group(1)
    if not release_prepare_text.lstrip().startswith(REQUIRED_RELEASE_PREPARE_COMMANDS[0]):
        fail(
            "Makefile release-prepare must run lint-all first",
            "makefile_contract_drift",
        )
    lint_all_recipe = re.search(r"^lint-all\s*:\s*\n((?:\t.*\n)+)", text, flags=re.MULTILINE)
    if lint_all_recipe is None or REQUIRED_RELEASE_PREPARE_COMMANDS[1] not in lint_all_recipe.group(1):
        fail("Makefile lint-all target omits all-target Clippy", "makefile_contract_drift")
    missing_release_sentinels = [
        command
        for command in REQUIRED_RELEASE_PREPARE_COMMANDS[2:]
        if command not in release_prepare_text
    ]
    if missing_release_sentinels:
        fail(
            f"Makefile release-prepare omits release sentinels: {missing_release_sentinels}",
            "makefile_contract_drift",
        )
    for target, command in (
        ("preset-create", "tools/manage_presets.py create"),
        ("preset-import", "tools/manage_presets.py import"),
        ("test-fuzzing", "python3 tools/run_fuzzing.py"),
        ("dev-runtime-test", "python3 tools/dev_runtime.py self-test"),
    ):
        recipe = re.search(rf"^{target}\s*:[^\n]*\n((?:\t.*\n)+)", text, flags=re.MULTILINE)
        if recipe is None or command not in recipe.group(1):
            fail(f"Makefile {target} target omits {command}", "makefile_contract_drift")
    dist_recipe = re.search(r"^dist\s*:\s*\n((?:\t.*\n)+)", text, flags=re.MULTILINE)
    if dist_recipe is None:
        fail("Makefile dist target has no recipe", "makefile_contract_drift")
    recipe_text = dist_recipe.group(1)
    positions = [recipe_text.find(command) for command in REQUIRED_RELEASE_COMMANDS]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        fail(
            "Makefile dist target does not run the complete release gate in order",
            "makefile_contract_drift",
        )
    return (
        len(REQUIRED_MAKE_TARGETS)
        + len(REQUIRED_PREPARE_COMMANDS)
        + len(REQUIRED_RELEASE_PREPARE_COMMANDS)
    )




def validate_release_contract_verifier(root: Path) -> int:
    archive = (root / "tools/release_archive.py").read_text(encoding="utf-8")
    qualification = (root / "tools/qualify_distribution.py").read_text(encoding="utf-8")
    required_archive = (
        "def verify_release_contract(",
        '"--features",\n            "release-contract-verifier"',
        '"--contract-root"',
        "contract_receipt = verify_release_contract(ROOT, podway, podwayd, source_commit)",
    )
    required_qualification = (
        "release_archive.verify_release_contract(",
        'extracted / "share/podway"',
    )
    if any(token not in archive for token in required_archive) or any(
        token not in qualification for token in required_qualification
    ):
        fail(
            "release tooling does not route source and packaged identity through the Rust verifier",
            "release_contract_verifier_drift",
        )
    forbidden = (
        "def verify_binary_contract_identity(",
        "def json_identity(",
        '.get("result", document)',
    )
    if any(token in archive or token in qualification for token in forbidden):
        fail(
            "release tooling reintroduced a partial Python identity validator",
            "release_contract_verifier_drift",
        )
    return len(required_archive) + len(required_qualification) + len(forbidden)


def copy_contracts(source_root: Path, destination_root: Path) -> None:
    shutil.copytree(source_root / "contracts", destination_root / "contracts")


def build_adjacency_fixture(source_root: Path, destination_root: Path) -> None:
    copy_contracts(source_root, destination_root)
    crates = load_adjacency_contract(destination_root)
    members = ", ".join(json.dumps(crate["path"]) for crate in crates)
    (destination_root / "Cargo.toml").write_text(f"[workspace]\nmembers = [{members}]\n", encoding="utf-8")
    for crate in crates:
        manifest = destination_root / crate["path"] / "Cargo.toml"
        manifest.parent.mkdir(parents=True, exist_ok=True)
        lines = ["[package]", f'name = "{crate["name"]}"', 'version = "0.1.0"']
        dependencies = crate["approved_dependencies"]
        if dependencies:
            lines.append("")
            lines.append("[dependencies]")
            for dependency in dependencies:
                lines.append(f'{dependency} = {{ path = "../{dependency}" }}')
        manifest.write_text("\n".join(lines) + "\n", encoding="utf-8")


def require_known_failure(label: str, operation: Callable[[], object]) -> None:
    try:
        operation()
    except (VerificationError, repository_assets.AssetError):
        return
    fail(f"known-fail sentinel did not fail: {label}")


def run_sentinels(root: Path) -> list[str]:
    completed: list[str] = []
    with tempfile.TemporaryDirectory(prefix="podway-phase-0a-") as temporary_name:
        temporary = Path(temporary_name)
        adjacency_fixture = temporary / "forbidden-edge"
        adjacency_fixture.mkdir()
        build_adjacency_fixture(root, adjacency_fixture)
        core_manifest = adjacency_fixture / "crates/podway-core/Cargo.toml"
        with core_manifest.open("a", encoding="utf-8") as handle:
            handle.write("\n[dependencies]\npodway-config = { path = \"../podway-config\" }\n")
        require_known_failure("forbidden Cargo edge", lambda: validate_adjacency(adjacency_fixture))
        completed.append("forbidden_edge")

        route_fixture = temporary / "route-bypass"
        route_fixture.mkdir()
        shutil.copytree(root / "assets", route_fixture / "assets")
        copy_contracts(root, route_fixture)
        route_path = route_fixture / ROUTES_PATH
        route_contract = json.loads(route_path.read_text(encoding="utf-8"))
        for route in route_contract["routes"]:
            if route["command"] == "session.status":
                route["path"] = ["podway-cli", "podway-daemon"]
                break
        else:
            fail("route bypass sentinel cannot find session.status")
        route_path.write_text(json.dumps(route_contract, sort_keys=True) + "\n", encoding="utf-8")
        require_known_failure("route bypass", lambda: validate_routes(route_fixture))
        completed.append("route_bypass")

        route_availability_fixture = temporary / "route-availability"
        route_availability_fixture.mkdir()
        shutil.copytree(root / "assets", route_availability_fixture / "assets")
        copy_contracts(root, route_availability_fixture)
        route_path = route_availability_fixture / ROUTES_PATH
        route_contract = json.loads(route_path.read_text(encoding="utf-8"))
        route_contract["routes"][-1]["availability"] = "executable"
        route_path.write_text(json.dumps(route_contract, sort_keys=True) + "\n", encoding="utf-8")
        require_known_failure(
            "route availability mismatch", lambda: validate_routes(route_availability_fixture)
        )
        completed.append("route_availability_mismatch")

        route_growth_fixture = temporary / "route-growth"
        route_growth_fixture.mkdir()
        shutil.copytree(root / "assets", route_growth_fixture / "assets")
        copy_contracts(root, route_growth_fixture)
        catalog_path = route_growth_fixture / "assets/specifications/command-catalog.yaml"
        catalog_path.write_text(
            catalog_path.read_text(encoding="utf-8")
            + "- name: procedure.future\n  cli:\n  - procedure\n  - future\n",
            encoding="utf-8",
        )
        require_known_failure("silent route growth", lambda: validate_v2_catalog_delta(route_growth_fixture))
        completed.append("silent_route_growth")

        catalog_availability_fixture = temporary / "catalog-availability"
        catalog_availability_fixture.mkdir()
        shutil.copytree(root / "assets", catalog_availability_fixture / "assets")
        copy_contracts(root, catalog_availability_fixture)
        catalog_path = catalog_availability_fixture / "assets/specifications/command-catalog.yaml"
        catalog_text = catalog_path.read_text(encoding="utf-8")
        catalog_path.write_text(
            catalog_text.replace(
                "- name: goal.assess_criterion\n  availability: reserved_contract",
                "- name: goal.assess_criterion\n  availability: executable",
                1,
            ),
            encoding="utf-8",
        )
        require_known_failure(
            "catalog availability mismatch",
            lambda: validate_v2_catalog_delta(catalog_availability_fixture),
        )
        completed.append("catalog_availability_mismatch")

        error_growth_fixture = temporary / "error-growth"
        error_growth_fixture.mkdir()
        shutil.copytree(root / "assets", error_growth_fixture / "assets")
        error_path = error_growth_fixture / "assets/specifications/error-codes.json"
        error_catalog = json.loads(error_path.read_text(encoding="utf-8"))
        error_catalog["errors"].append({
            "code": "FUTURE_ERROR", "exit_code": 1, "retryable": False,
            "summary": "Unapproved growth.",
        })
        error_path.write_text(json.dumps(error_catalog) + "\n", encoding="utf-8")
        require_known_failure("silent error growth", lambda: validate_v2_catalog_delta(error_growth_fixture))
        completed.append("silent_error_growth")

        diagnostic_overlap_fixture = temporary / "diagnostic-overlap"
        diagnostic_overlap_fixture.mkdir()
        shutil.copytree(root / "assets", diagnostic_overlap_fixture / "assets")
        diagnostic_path = diagnostic_overlap_fixture / "assets/specifications/authoring-diagnostics.json"
        diagnostic_catalog = json.loads(diagnostic_path.read_text(encoding="utf-8"))
        diagnostic_catalog["diagnostics"][0]["code"] = "PROCEDURE_INVALID"
        diagnostic_path.write_text(json.dumps(diagnostic_catalog) + "\n", encoding="utf-8")
        require_known_failure(
            "runtime authoring overlap",
            lambda: validate_v2_catalog_delta(diagnostic_overlap_fixture),
        )
        completed.append("runtime_authoring_overlap")
        completed.extend(f"contract_manifest_{item}" for item in contract_manifest.self_test(root))
    return completed


def receipt(mode: str, ok: bool, **details: Any) -> None:
    print(json.dumps({"mode": mode, "ok": ok, **details}, sort_keys=True, separators=(",", ":")))


def production_checks(root: Path) -> dict[str, int]:
    return {
        "canonical_assets": repository_assets.validate_layout(root),
        "contract_identifiers": validate_contract_identifiers(root),
        "v2_catalog_delta": validate_v2_catalog_delta(root),
        "cargo_adjacency": validate_adjacency(root),
        "command_routes": validate_routes(root),
        "makefile_contract": validate_makefile_contract(root),
        "release_contract_verifier": validate_release_contract_verifier(root),
        "contract_manifest_assets": contract_manifest.check(root),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="run all production contract validations")
    mode.add_argument("--sentinels", action="store_true", help="run isolated known-fail controls in temporary copies")
    mode.add_argument("--all", action="store_true", help="run production validations and isolated sentinels")
    arguments = parser.parse_args()
    selected_mode = "check" if arguments.check else "sentinels" if arguments.sentinels else "all"
    try:
        details: dict[str, Any] = {}
        if arguments.check or arguments.all:
            details["checks"] = production_checks(ROOT)
        if arguments.sentinels or arguments.all:
            details["sentinels"] = run_sentinels(ROOT)
        receipt(selected_mode, True, **details)
    except (
        VerificationError,
        repository_assets.AssetError,
        contract_manifest.ManifestError,
        OSError,
        tomllib.TOMLDecodeError,
    ) as error:
        if isinstance(error, VerificationError):
            code = error.code
        elif isinstance(error, contract_manifest.ManifestError):
            code = "contract_manifest_invalid"
        else:
            code = "contract_verification_failed"
        receipt(selected_mode, False, error={"code": code, "message": str(error)})
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
