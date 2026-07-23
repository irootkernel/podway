#!/usr/bin/env python3
"""Validate frozen Phase 0 fixtures and publish content-addressed handoff receipts.

The checker validates only imported assets and frozen contract/fixture shape.  It does
not claim that a Store, Git resolver, daemon, or service implementation behaves in
production.  Receipt publication requires explicit leader-supplied verification;
there are no implicit accepted gates, reviewers, proofs, or artifact digests.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import tempfile
import tomllib
from typing import Any

import run_verification
import verify_contracts

ROOT = Path(__file__).resolve().parent.parent
CANONICAL_IMPORT_PATH = Path("contracts/canonical-import.json")
ADJACENCY_PATH = Path("contracts/cargo-adjacency.json")
ROUTES_PATH = Path("contracts/command-routes.json")
HANDOFF_SCHEMA_PATH = Path("contracts/handoff-schema-v1.json")
ATTESTATION_SCHEMA_PATH = Path("contracts/verification-attestation-schema-v1.json")
EVIDENCE_DIRECTORY = Path("contracts/evidence")
FIXTURE_DIRECTORY = Path("tests/fixtures/phase0")
REFERENCE_PATH = FIXTURE_DIRECTORY / "reference-model.json"
SCHEMA_ZERO_PATH = FIXTURE_DIRECTORY / "schema-0-uninitialized.json"
SENTINELS_PATH = FIXTURE_DIRECTORY / "known-fail-sentinels.json"
HANDOFF_DIRECTORY = Path("contracts/handoffs")
LOCK_DIRECTORY = Path("contracts/locks")
SOURCE_DIRECTORIES = ("schemas", "spec", "presets")
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
PHASES = ("0A", "0B", "0C")
REQUIREMENT_GROUPS = (
    "product",
    "architecture",
    "procedure_and_domain",
    "queue_and_storage",
    "interfaces",
    "security_and_operations",
    "release",
)
NON_PRODUCTION_CLAIMS = [
    "production Store behavior",
    "production Git behavior",
    "production daemon behavior",
    "production Service behavior",
]
DIGEST_RE = re.compile(r"^[a-f0-9]{64}$")
REQUIREMENT_RE = re.compile(r"^(?:PRD|ARC|DOM|STO|API|SEC|OPS|REL)-[0-9]{3}$")
PHASE_RE = re.compile(r"^(?:0[A-C]|[1-8]|RC)$")
ACTOR_RE = re.compile(r"^[A-Z][A-Z0-9_-]*$")
MAX_JSON_BYTES = 8 * 1024 * 1024
GATE_RE = re.compile(r"^[A-Z0-9][A-Z0-9._-]*$")


class ContractError(Exception):
    """A frozen Phase 0 contract or receipt invariant was violated."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def fail(code: str, message: str) -> None:
    raise ContractError(code, message)


def canonical_json(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def digest_json(value: Any) -> str:
    return digest_bytes(canonical_json(value))


def require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        fail("invalid_value", f"{label} must be a non-empty string")
    return value


def require_digest(value: Any, label: str) -> str:
    text = require_string(value, label)
    if DIGEST_RE.fullmatch(text) is None:
        fail("invalid_digest", f"{label} must be a lowercase SHA-256 digest")
    return text


def require_list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        fail("invalid_value", f"{label} must be a list")
    return value


def require_object(value: Any, label: str, keys: set[str] | None = None) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail("invalid_value", f"{label} must be an object")
    if keys is not None and set(value) != keys:
        fail("invalid_shape", f"{label} has unexpected or missing fields")
    return value


def require_unique(values: list[Any], label: str) -> None:
    encoded = [canonical_json(value) for value in values]
    if len(set(encoded)) != len(encoded):
        fail("duplicate_value", f"{label} must not contain duplicates")


def relative_path(value: Any, label: str) -> Path:
    text = require_string(value, label)
    if "\\" in text:
        fail("invalid_path", f"{label} must use POSIX separators")
    parts = text.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        fail("invalid_path", f"{label} is not a normalized relative path")
    pure = PurePosixPath(text)
    if pure.is_absolute():
        fail("invalid_path", f"{label} must be relative")
    return Path(*pure.parts)


def is_under(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def checked_path(root: Path, relative: Path, label: str) -> Path:
    root = root.resolve()
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            fail("unsafe_path", f"{label} contains a symlink: {relative.as_posix()}")
    if not is_under(current.resolve(strict=False), root):
        fail("unsafe_path", f"{label} escapes the repository root")
    return current


def regular_files(root: Path, relative_directory: str, required: bool) -> set[str]:
    root = root.resolve()
    directory = checked_path(root, relative_path(relative_directory, "directory"), "directory")
    if not directory.exists():
        if required:
            fail("missing_directory", f"required directory is missing: {relative_directory}")
        return set()
    if not directory.is_dir():
        fail("invalid_directory", f"directory is not a directory: {relative_directory}")

    files: set[str] = set()
    for current_name, child_directories, child_files in os.walk(directory, followlinks=False):
        current = Path(current_name)
        for child_name in child_directories:
            child = current / child_name
            if child.is_symlink():
                fail("unsafe_path", f"directory contains a symlink: {child.relative_to(root).as_posix()}")
        for child_name in child_files:
            child = current / child_name
            if child.is_symlink() or not child.is_file():
                fail("unsafe_path", f"directory contains a non-regular file: {child.relative_to(root).as_posix()}")
            files.add(child.relative_to(root).as_posix())
    return files


def load_bounded_json(path: Path, label: str, error_code: str) -> Any:
    try:
        if path.stat().st_size > MAX_JSON_BYTES:
            fail("json_too_large", f"{label} exceeds its {MAX_JSON_BYTES}-byte JSON ceiling")
        raw = path.read_bytes()
    except OSError as error:
        fail(error_code, f"cannot read {label}: {error}")
    if len(raw) > MAX_JSON_BYTES:
        fail("json_too_large", f"{label} exceeds its {MAX_JSON_BYTES}-byte JSON ceiling")
    try:
        return json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(error_code, f"cannot parse {label}: {error}")


def read_json(root: Path, relative: Path, label: str) -> dict[str, Any]:
    path = checked_path(root, relative, label)
    if not path.is_file():
        fail("missing_file", f"{label} is missing: {relative.as_posix()}")
    return require_object(load_bounded_json(path, label, "invalid_json"), label)


def read_toml(root: Path, relative: Path, label: str) -> dict[str, Any]:
    path = checked_path(root, relative, label)
    if not path.is_file():
        fail("missing_file", f"{label} is missing: {relative.as_posix()}")
    try:
        with path.open("rb") as handle:
            value = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail("invalid_toml", f"cannot parse {label}: {error}")
    return require_object(value, label)


def read_text(root: Path, relative: Path, label: str) -> str:
    path = checked_path(root, relative, label)
    if not path.is_file():
        fail("missing_file", f"{label} is missing: {relative.as_posix()}")
    try:
        return path.read_text(encoding="utf-8")
    except OSError as error:
        fail("unreadable_file", f"cannot read {label}: {error}")


def canonical_import_mappings(root: Path) -> list[dict[str, str]]:
    contract = read_json(root, CANONICAL_IMPORT_PATH, "canonical import contract")
    require_object(
        contract,
        "canonical import contract",
        {"contract_version", "generator_version", "owner", "copy_mode", "imports"},
    )
    if contract["contract_version"] != "podway.canonical-import/v1":
        fail("identifier_drift", "canonical import contract version drift")
    if contract["generator_version"] != "docs-assets-v1" or contract["owner"] != "docs" or contract["copy_mode"] != "exact":
        fail("canonical_import_contract_drift", "canonical import contract ownership or copy mode drift")

    mappings: list[dict[str, str]] = []
    sources: set[str] = set()
    destinations: set[str] = set()
    for index, value in enumerate(require_list(contract["imports"], "canonical import mappings")):
        mapping = require_object(
            value,
            f"canonical import mapping {index}",
            {"source", "destination", "source_sha256", "copy_mode", "owner", "generator_version"},
        )
        source = relative_path(mapping["source"], f"canonical import mapping {index} source")
        destination = relative_path(mapping["destination"], f"canonical import mapping {index} destination")
        source_name = source.as_posix()
        destination_name = destination.as_posix()
        if len(source.parts) < 3 or source.parts[0] != "docs" or source.parts[1] not in SOURCE_DIRECTORIES:
            fail("canonical_import_contract_drift", f"canonical import source is outside documentation assets: {source_name}")
        if destination.parts != source.parts[1:]:
            fail("canonical_import_contract_drift", f"canonical import destination does not mirror source: {source_name}")
        if mapping["copy_mode"] != "exact" or mapping["owner"] != "docs" or mapping["generator_version"] != "docs-assets-v1":
            fail("canonical_import_contract_drift", f"canonical import mapping policy drift: {source_name}")
        require_digest(mapping["source_sha256"], f"canonical import mapping {index} digest")
        if source_name in sources or destination_name in destinations:
            fail("canonical_import_contract_drift", "canonical import mappings must be one-to-one")
        sources.add(source_name)
        destinations.add(destination_name)
        mappings.append({key: mapping[key] for key in mapping})

    expected_sources: set[str] = set()
    for directory in SOURCE_DIRECTORIES:
        expected_sources.update(regular_files(root, f"docs/{directory}", required=True))
    if sources != expected_sources:
        fail("canonical_import_contract_drift", "canonical import mappings do not exactly cover documentation assets")
    return mappings


def validate_canonical_import(root: Path) -> int:
    mappings = canonical_import_mappings(root)
    expected_destinations = {mapping["destination"] for mapping in mappings}
    actual_destinations: set[str] = set()
    for directory in SOURCE_DIRECTORIES:
        actual_destinations.update(regular_files(root, directory, required=True))
    if actual_destinations != expected_destinations:
        fail("canonical_import_destination_tree_drift", "imported destination tree does not exactly match canonical mappings")

    for mapping in mappings:
        source = checked_path(root, relative_path(mapping["source"], "canonical import source"), "canonical import source")
        destination = checked_path(root, relative_path(mapping["destination"], "canonical import destination"), "canonical import destination")
        if not source.is_file() or not destination.is_file():
            fail("canonical_import_missing_file", "canonical import source or destination is not a regular file")
        if digest_file(source) != mapping["source_sha256"]:
            fail("canonical_import_source_digest_drift", f"documentation source digest drift: {mapping['source']}")
        if digest_file(destination) != mapping["source_sha256"]:
            fail("canonical_import_digest_drift", f"imported asset digest drift: {mapping['destination']}")
    return len(mappings)

def expected_phase_lock_entries(root: Path, phase: str) -> set[str]:
    if phase == "0A":
        entries = {
            "Cargo.toml",
            "Cargo.lock",
            ".gitignore",
            "rust-toolchain.toml",
            "Makefile",
            "README.md",
            CANONICAL_IMPORT_PATH.as_posix(),
            ADJACENCY_PATH.as_posix(),
            ROUTES_PATH.as_posix(),
            "contracts/requirement-evidence-schema-v1.json",
            ATTESTATION_SCHEMA_PATH.as_posix(),
            HANDOFF_SCHEMA_PATH.as_posix(),
        }
        entries.update(mapping["destination"] for mapping in canonical_import_mappings(root))
        return entries
    if phase == "0B":
        entries: set[str] = set()
        for directory in ("contracts/interfaces", "tests/fixtures/phase0", "crates"):
            entries.update(regular_files(root, directory, required=True))
        return entries
    if phase == "0C":
        return {
            (LOCK_DIRECTORY / "phase-0a-contract-lock.json").as_posix(),
            (LOCK_DIRECTORY / "phase-0b-contract-lock.json").as_posix(),
            "tools/sync_docs_assets.py",
            "tools/verify_contracts.py",
            "tools/phase0_receipts.py",
            "tools/run_verification.py",
        }
    fail("phase_lock_drift", f"unsupported Phase 0 lock: {phase}")
def phase_lock_path(phase: str) -> Path:
    if phase not in PHASES:
        fail("phase_lock_drift", f"unsupported Phase 0 lock: {phase}")
    return LOCK_DIRECTORY / f"phase-{phase.lower()}-contract-lock.json"




def validate_phase_locks(root: Path, reference: dict[str, Any]) -> dict[str, str]:
    digests: dict[str, str] = {}
    for phase in PHASES:
        relative = phase_lock_path(phase)
        lock = read_json(root, relative, f"phase {phase} lock")
        require_object(lock, f"phase {phase} lock", {"schema_version", "phase", "artifact", "entries"})
        if lock["schema_version"] != "podway.phase0.lock/v1" or lock["phase"] != phase:
            fail("phase_lock_drift", f"phase {phase} lock identity drift")
        artifact = require_object(lock["artifact"], f"phase {phase} lock artifact", {"kind", "name", "version"})
        if artifact != reference["handoffs"][phase]["artifact"]:
            fail("phase_lock_drift", f"phase {phase} lock artifact drift")

        entries: list[dict[str, str]] = []
        for index, raw_entry in enumerate(require_list(lock["entries"], f"phase {phase} lock entries")):
            entry = require_object(raw_entry, f"phase {phase} lock entry {index}", {"path", "sha256"})
            path = relative_path(entry["path"], f"phase {phase} lock entry {index} path")
            entries.append(
                {
                    "path": path.as_posix(),
                    "sha256": require_digest(entry["sha256"], f"phase {phase} lock entry {index} digest"),
                }
            )
        paths = [entry["path"] for entry in entries]
        if paths != sorted(paths) or len(paths) != len(set(paths)):
            fail("phase_lock_drift", f"phase {phase} lock entries must be sorted and unique")
        if set(paths) != expected_phase_lock_entries(root, phase):
            fail("phase_lock_drift", f"phase {phase} lock coverage drift")
        for entry in entries:
            path = checked_path(root, Path(entry["path"]), f"phase {phase} lock entry")
            if not path.is_file():
                fail("phase_lock_drift", f"phase {phase} lock entry is not a regular file: {entry['path']}")
            if digest_file(path) != entry["sha256"]:
                fail("phase_lock_digest_drift", f"phase {phase} lock entry digest drift: {entry['path']}")
        digests[phase] = digest_file(checked_path(root, relative, f"phase {phase} lock"))
    return digests


def internal_dependencies(value: dict[str, Any], known: set[str]) -> set[str]:
    discovered: set[str] = set()

    def visit(current: Any) -> None:
        if not isinstance(current, dict):
            return
        for key, child in current.items():
            if key in {"dependencies", "dev-dependencies", "build-dependencies"}:
                if not isinstance(child, dict):
                    fail("invalid_toml", f"Cargo dependency table {key} is not a table")
                for dependency, specification in child.items():
                    if dependency in known:
                        discovered.add(dependency)
                    if isinstance(specification, dict) and specification.get("package") in known:
                        discovered.add(specification["package"])
            else:
                visit(child)

    visit(value)
    return discovered


def expected_cargo_entries(reference: dict[str, Any]) -> dict[str, dict[str, Any]]:
    entries = require_list(reference["cargo_dag"], "reference cargo_dag")
    if len(entries) != len(CRATE_ORDER):
        fail("reference_model_drift", "reference model must bind exactly nine crates")
    result: dict[str, dict[str, Any]] = {}
    for index, entry in enumerate(entries):
        value = require_object(entry, f"reference cargo crate {index}", {"name", "approved_dependencies", "forbidden_dependencies"})
        name = require_string(value["name"], f"reference cargo crate {index} name")
        approved = require_list(value["approved_dependencies"], f"reference cargo crate {name} approved dependencies")
        forbidden = require_list(value["forbidden_dependencies"], f"reference cargo crate {name} forbidden dependencies")
        if not all(isinstance(item, str) for item in approved + forbidden):
            fail("reference_model_drift", f"reference cargo dependencies must be strings for {name}")
        require_unique(approved, f"reference approved dependencies for {name}")
        require_unique(forbidden, f"reference forbidden dependencies for {name}")
        if name in result:
            fail("reference_model_drift", "reference cargo crate names must be unique")
        result[name] = value
    if tuple(result) != CRATE_ORDER:
        fail("reference_model_drift", "reference cargo DAG order or names drift")
    known = set(CRATE_ORDER)
    for name, entry in result.items():
        approved = set(entry["approved_dependencies"])
        forbidden = set(entry["forbidden_dependencies"])
        if approved & forbidden or approved | forbidden != known - {name}:
            fail("reference_model_drift", f"reference cargo DAG is not closed for {name}")
    return result


def validate_cargo_dag(root: Path, reference: dict[str, Any]) -> int:
    expected = expected_cargo_entries(reference)
    contract = read_json(root, ADJACENCY_PATH, "Cargo adjacency contract")
    require_object(contract, "Cargo adjacency contract", {"contract_version", "owner", "crates"})
    if contract["contract_version"] != reference["v1_identifiers"]["cargo_adjacency"] or contract["owner"] != "architecture":
        fail("cargo_dag_drift", "Cargo adjacency contract identity drift")
    entries = require_list(contract["crates"], "Cargo adjacency crates")
    if len(entries) != len(CRATE_ORDER):
        fail("cargo_dag_drift", "Cargo adjacency contract must define exactly nine crates")

    actual_names: list[str] = []
    for index, entry in enumerate(entries):
        value = require_object(
            entry,
            f"Cargo adjacency crate {index}",
            {"name", "path", "approved_dependencies", "forbidden_dependencies"},
        )
        name = require_string(value["name"], f"Cargo adjacency crate {index} name")
        actual_names.append(name)
        if name not in expected or value["path"] != f"crates/{name}":
            fail("cargo_dag_drift", f"Cargo adjacency crate identity drift: {name}")
        for field in ("approved_dependencies", "forbidden_dependencies"):
            dependencies = require_list(value[field], f"Cargo adjacency {field} for {name}")
            if not all(isinstance(item, str) for item in dependencies):
                fail("cargo_dag_drift", f"Cargo adjacency {field} must contain strings for {name}")
            require_unique(dependencies, f"Cargo adjacency {field} for {name}")
        actual_approved = set(value["approved_dependencies"])
        actual_forbidden = set(value["forbidden_dependencies"])
        expected_approved = set(expected[name]["approved_dependencies"])
        expected_forbidden = set(expected[name]["forbidden_dependencies"])
        if actual_approved != expected_approved or actual_forbidden != expected_forbidden:
            if actual_approved - expected_approved or actual_forbidden != expected_forbidden:
                fail("forbidden_crate_edge", f"Cargo adjacency contains a forbidden edge for {name}")
            fail("cargo_dag_drift", f"Cargo adjacency drift for {name}")
    if tuple(actual_names) != CRATE_ORDER:
        fail("cargo_dag_drift", "Cargo adjacency crate order drift")

    workspace = read_toml(root, Path("Cargo.toml"), "workspace manifest")
    workspace_table = require_object(workspace.get("workspace"), "workspace manifest workspace")
    members = require_list(workspace_table.get("members"), "workspace members")
    if members != [f"crates/{name}" for name in CRATE_ORDER]:
        fail("cargo_dag_drift", "workspace members drift from the exact nine-crate DAG")
    package = require_object(workspace_table.get("package"), "workspace package")
    if package.get("version") != reference["product_version"]:
        fail("product_version_drift", "workspace package version drift")

    known = set(CRATE_ORDER)
    for name in CRATE_ORDER:
        manifest = read_toml(root, Path("crates") / name / "Cargo.toml", f"manifest for {name}")
        package = require_object(manifest.get("package"), f"package for {name}")
        if package.get("name") != name or package.get("version") != reference["product_version"]:
            fail("cargo_dag_drift", f"package identity or version drift for {name}")
        actual = internal_dependencies(manifest, known)
        expected_dependencies = set(expected[name]["approved_dependencies"])
        if actual != expected_dependencies:
            if actual - expected_dependencies:
                fail("forbidden_crate_edge", f"manifest contains a forbidden crate edge for {name}")
            fail("cargo_dag_drift", f"manifest dependency graph drift for {name}")
    return len(CRATE_ORDER)


def expected_route_map(reference: dict[str, Any]) -> dict[str, dict[str, Any]]:
    routes = require_object(reference["command_routes"], "reference command_routes", {"prohibited_capabilities", "classes"})
    prohibited = require_list(routes["prohibited_capabilities"], "reference prohibited capabilities")
    if set(prohibited) != {"command_runner", "git_mutation", "network"} or len(prohibited) != 3:
        fail("reference_model_drift", "reference prohibited capabilities drift")
    result: dict[str, dict[str, Any]] = {}
    for index, value in enumerate(require_list(routes["classes"], "reference route classes")):
        route_class = require_object(
            value,
            f"reference route class {index}",
            {"name", "owner", "path", "execution", "capabilities", "commands"},
        )
        require_string(route_class["name"], f"reference route class {index} name")
        path = require_list(route_class["path"], f"reference route class {index} path")
        commands = require_list(route_class["commands"], f"reference route class {index} commands")
        capabilities = require_list(route_class["capabilities"], f"reference route class {index} capabilities")
        if not all(isinstance(item, str) for item in path + commands + capabilities):
            fail("reference_model_drift", "reference route fields must contain strings")
        if capabilities:
            fail("reference_model_drift", "reference command routes must not enable capabilities")
        require_unique(commands, f"reference route class {index} commands")
        for command in commands:
            if command in result:
                fail("reference_model_drift", f"reference command route is duplicated: {command}")
            result[command] = {
                "command": command,
                "owner": route_class["owner"],
                "path": path,
                "execution": route_class["execution"],
                "capabilities": capabilities,
            }
    if not result:
        fail("reference_model_drift", "reference command routes must not be empty")
    return result


def catalog_commands(root: Path) -> set[str]:
    text = read_text(root, Path("spec/command-catalog.yaml"), "command catalog")
    if not re.search(r"^schema: podway\.command-catalog/v1$", text, flags=re.MULTILINE):
        fail("identifier_drift", "command catalog schema identifier drift")
    commands = set(re.findall(r"^- name: ([a-z][a-z0-9._-]*)$", text, flags=re.MULTILINE))
    if not commands:
        fail("command_routes_drift", "command catalog contains no commands")
    return commands


def validate_command_routes(root: Path, reference: dict[str, Any]) -> int:
    expected = expected_route_map(reference)
    contract = read_json(root, ROUTES_PATH, "command route contract")
    require_object(contract, "command route contract", {"contract_version", "owner", "prohibited_capabilities", "routes"})
    if contract["contract_version"] != reference["v1_identifiers"]["command_routes"] or contract["owner"] != "architecture":
        fail("route_bypass", "command route contract identity drift")
    prohibited = require_list(contract["prohibited_capabilities"], "command route prohibited capabilities")
    if set(prohibited) != set(reference["command_routes"]["prohibited_capabilities"]) or len(prohibited) != 3:
        fail("route_bypass", "command route prohibited capabilities drift")

    actual: dict[str, dict[str, Any]] = {}
    for index, item in enumerate(require_list(contract["routes"], "command routes")):
        route = require_object(item, f"command route {index}", {"command", "owner", "path", "execution", "capabilities"})
        command = require_string(route["command"], f"command route {index} command")
        path = require_list(route["path"], f"command route {command} path")
        capabilities = require_list(route["capabilities"], f"command route {command} capabilities")
        if not all(isinstance(value, str) for value in path + capabilities):
            fail("route_bypass", f"command route {command} path or capabilities are invalid")
        if command in actual or capabilities or set(capabilities) & set(prohibited):
            fail("route_bypass", f"command route {command} is duplicated or enables a prohibited capability")
        actual[command] = route
    if set(actual) != set(expected):
        fail("route_bypass", "command route coverage drift")
    for command, expected_route in expected.items():
        if actual[command] != expected_route:
            fail("route_bypass", f"command route bypass or ownership drift: {command}")
    if set(actual) != catalog_commands(root) | {"completions"}:
        fail("route_bypass", "command routes do not exactly cover the command catalog")
    return len(actual)


def interface_documents(root: Path, reference: dict[str, Any]) -> dict[str, dict[str, Any]]:
    interfaces = require_object(reference["interfaces"], "reference interfaces")
    if set(interfaces) != {"store", "git_resolver", "service_manager"}:
        fail("reference_model_drift", "reference model must bind Store, Git, and Service interfaces")
    documents: dict[str, dict[str, Any]] = {}
    for name, expected_value in interfaces.items():
        expected = require_object(
            expected_value,
            f"reference interface {name}",
            {"path", "id", "contract_version", "owner_crate", "phase", "allowed_dependencies", "consumers", "contract_obligation_ids"},
        )
        document = read_json(root, relative_path(expected["path"], f"reference interface {name} path"), f"interface {name}")
        for field in ("$id", "$schema", "contract_version", "owner_crate", "phase", "allowed_dependencies", "consumers", "invariant_ids", "operations"):
            if field not in document:
                fail("interface_contract_drift", f"interface {name} is missing {field}")
        if document["$id"] != expected["id"] or document["contract_version"] != expected["contract_version"]:
            fail("interface_contract_drift", f"interface identity drift: {name}")
        if document["$schema"] != reference["v1_identifiers"]["json_schema_draft"]:
            fail("interface_contract_drift", f"interface schema draft drift: {name}")
        for field in ("owner_crate", "phase", "allowed_dependencies", "consumers"):
            if document[field] != expected[field]:
                fail("interface_contract_drift", f"interface {field} drift: {name}")
        invariant_ids = require_list(document["invariant_ids"], f"interface {name} invariant_ids")
        operations = require_list(document["operations"], f"interface {name} operations")
        obligation_ids: list[str] = []
        for item in invariant_ids:
            obligation_ids.append(require_string(item, f"interface {name} invariant identifier"))
        for index, operation_value in enumerate(operations):
            operation = require_object(operation_value, f"interface {name} operation {index}")
            requirement_ids = require_list(operation.get("requirement_ids"), f"interface {name} operation {index} requirement_ids")
            obligation_ids.extend(require_string(item, f"interface {name} operation {index} requirement identifier") for item in requirement_ids)
        if set(obligation_ids) != set(expected["contract_obligation_ids"]) or len(set(obligation_ids)) != len(expected["contract_obligation_ids"]):
            fail("interface_contract_drift", f"interface obligation identifiers drift: {name}")
        documents[name] = document
    return documents


def validate_interfaces(root: Path, reference: dict[str, Any]) -> dict[str, str]:
    return {name: digest_json(document) for name, document in interface_documents(root, reference).items()}


def validate_interface_digest_baseline(root: Path, reference: dict[str, Any], baseline: dict[str, str]) -> None:
    interfaces = require_object(reference["interfaces"], "reference interfaces")
    actual = {
        name: digest_json(read_json(root, relative_path(expected["path"], f"reference interface {name} path"), f"interface {name}"))
        for name, expected in interfaces.items()
    }
    if actual != baseline:
        fail("interface_digest_drift", "canonical interface digest drift")


def validate_identifiers(root: Path, reference: dict[str, Any]) -> int:
    identifiers = require_object(reference["v1_identifiers"], "reference v1_identifiers")
    expected_identifier_keys = {
        "canonical_import",
        "cargo_adjacency",
        "command_routes",
        "command_catalog",
        "error_catalog",
        "handoff_receipt",
        "handoff_schema_id",
        "json_schema_draft",
        "schema_v1",
        "schemas",
    }
    require_object(identifiers, "reference v1_identifiers", expected_identifier_keys)
    schemas = require_object(identifiers["schemas"], "reference v1 schema identifiers")
    if set(schemas) != {"error", "ipc-request", "next-result", "output", "procedure", "registry", "status-result", "workspace"}:
        fail("reference_model_drift", "reference v1 schema identifiers drift")
    for name, identifier in schemas.items():
        schema = read_json(root, Path("schemas") / f"{name}-v1.schema.json", f"schema {name}")
        if schema.get("$id") != identifier or schema.get("$schema") != identifiers["json_schema_draft"]:
            fail("identifier_drift", f"schema v1 identifier drift: {name}")
    error_catalog = read_json(root, Path("spec/error-codes.json"), "error catalog")
    if error_catalog.get("schema") != identifiers["error_catalog"]:
        fail("identifier_drift", "error catalog identifier drift")
    catalog_commands(root)
    for preset_name in ("analysis", "bug-fix", "docs-only", "sw-dev"):
        text = read_text(root, Path("presets") / f"{preset_name}.yaml", f"preset {preset_name}")
        if not re.search(r"^schema: podway\.procedure/v1$", text, flags=re.MULTILINE):
            fail("identifier_drift", f"preset procedure identifier drift: {preset_name}")

    handoff_schema = read_json(root, HANDOFF_SCHEMA_PATH, "handoff schema")
    if handoff_schema.get("$id") != identifiers["handoff_schema_id"] or handoff_schema.get("$schema") != identifiers["json_schema_draft"]:
        fail("identifier_drift", "handoff schema identity drift")
    required = handoff_schema.get("required")
    expected_required = {
        "acceptance",
        "artifact",
        "consumer_proofs",
        "enabled_gates",
        "invalidation",
        "phase",
        "prerequisites",
        "producer_proof",
        "receipt_identity",
        "reviewers",
        "schema_version",
    }
    if not isinstance(required, list) or set(required) != expected_required or len(required) != len(expected_required):
        fail("handoff_schema_drift", "handoff schema required fields drift")
    properties = require_object(handoff_schema.get("properties"), "handoff schema properties")
    if properties.get("phase", {}).get("const") != "0C" or properties.get("schema_version", {}).get("const") != identifiers["handoff_receipt"]:
        fail("handoff_schema_drift", "handoff schema v1 phase or version drift")
    return len(schemas) + 4


def validate_requirement_groups(root: Path, reference: dict[str, Any]) -> int:
    groups = require_object(reference["requirement_groups"], "reference requirement groups")
    if set(groups) != set(REQUIREMENT_GROUPS):
        fail("reference_model_drift", "reference requirement group names drift")
    bound: list[str] = []
    for name in REQUIREMENT_GROUPS:
        values = require_list(groups[name], f"reference requirement group {name}")
        if not values or not all(isinstance(value, str) and REQUIREMENT_RE.fullmatch(value) for value in values):
            fail("reference_model_drift", f"reference requirement group {name} contains an invalid identifier")
        require_unique(values, f"reference requirement group {name}")
        bound.extend(values)
    require_unique(bound, "reference requirement identifiers")
    traceability = read_text(root, Path("docs/reference/quality/62-requirements-traceability.md"), "requirements traceability")
    source_ids = set(re.findall(r"\b(?:PRD|ARC|DOM|STO|API|SEC|OPS|REL)-[0-9]{3}\b", traceability))
    if source_ids != set(bound):
        fail("requirement_traceability_drift", "reference requirement groups do not exactly bind the traceability matrix")
    return len(bound)


def validate_reference_model(root: Path) -> dict[str, Any]:
    reference = read_json(root, REFERENCE_PATH, "Phase 0 reference model")
    require_object(
        reference,
        "Phase 0 reference model",
        {
            "fixture_version",
            "product_version",
            "v1_identifiers",
            "cargo_dag",
            "command_routes",
            "schema_zero_migration",
            "interfaces",
            "requirement_groups",
            "handoffs",
        },
    )
    if reference["fixture_version"] != "podway.phase0.reference-model/v1" or reference["product_version"] != "0.1.0":
        fail("reference_model_drift", "Phase 0 reference model identity or product version drift")
    expected_cargo_entries(reference)
    expected_route_map(reference)
    migration = require_object(
        reference["schema_zero_migration"],
        "reference schema-0 migration",
        {
            "predecessor_identity",
            "predecessor_schema",
            "predecessor_storage",
            "target_schema",
            "target_user_version",
            "ddl_path",
            "required_pragmas",
            "requirement_ids",
        },
    )
    if (
        migration["predecessor_identity"] != "schema-0-uninitialized"
        or migration["predecessor_schema"] != "schema-0"
        or migration["predecessor_storage"] != "on-disk-sqlite-empty"
        or migration["target_schema"] != "schema-v1"
        or migration["target_user_version"] != 1
    ):
        fail("reference_model_drift", "reference schema-0 migration identity drift")
    expected_pragmas = {"foreign_keys": "ON", "journal_mode": "WAL", "synchronous": "FULL", "busy_timeout": 5000, "trusted_schema": "OFF"}
    if migration["required_pragmas"] != expected_pragmas or migration["requirement_ids"] != ["STO-011", "REL-006"]:
        fail("reference_model_drift", "reference schema-0 migration contract drift")
    handoffs = require_object(reference["handoffs"], "reference handoffs")
    if set(handoffs) != set(PHASES):
        fail("reference_model_drift", "reference handoffs must bind Phases 0A, 0B, and 0C")
    for phase in PHASES:
        handoff = require_object(
            handoffs[phase],
            f"reference handoff {phase}",
            {"file", "artifact", "producer", "consumers", "prerequisite_phases", "enabled_gates", "affected_phases"},
        )
        artifact = require_object(handoff["artifact"], f"reference handoff {phase} artifact", {"kind", "name", "version"})
        if artifact["version"] != reference["product_version"] or handoff["producer"] != f"PHASE_{phase}":
            fail("reference_model_drift", f"reference handoff identity drift: {phase}")
        for field in ("consumers", "prerequisite_phases", "enabled_gates", "affected_phases"):
            values = require_list(handoff[field], f"reference handoff {phase} {field}")
            require_unique(values, f"reference handoff {phase} {field}")
        if not all(isinstance(value, str) and ACTOR_RE.fullmatch(value) for value in handoff["consumers"]):
            fail("reference_model_drift", f"reference handoff consumers drift: {phase}")
        if not all(value in PHASES for value in handoff["prerequisite_phases"]):
            fail("reference_model_drift", f"reference handoff prerequisites drift: {phase}")
        if not all(isinstance(value, str) and GATE_RE.fullmatch(value) for value in handoff["enabled_gates"]):
            fail("reference_model_drift", f"reference handoff enabled gates drift: {phase}")
        if not all(isinstance(value, str) and PHASE_RE.fullmatch(value) for value in handoff["affected_phases"]):
            fail("reference_model_drift", f"reference handoff affected phases drift: {phase}")
    return reference


def validate_schema_zero_fixture(root: Path, reference: dict[str, Any]) -> None:
    fixture = read_json(root, SCHEMA_ZERO_PATH, "schema-0 uninitialized fixture")
    require_object(
        fixture,
        "schema-0 uninitialized fixture",
        {"fixture_version", "fixture_kind", "scope", "predecessor", "expected_schema_v1", "expected_assertions", "requirement_ids"},
    )
    if fixture["fixture_version"] != "podway.phase0.schema-0-uninitialized/v1" or fixture["fixture_kind"] != "empty-predecessor-contract":
        fail("schema_zero_fixture_drift", "schema-0 fixture identity drift")
    scope = require_object(fixture["scope"], "schema-0 fixture scope", {"proves", "does_not_prove"})
    if scope["proves"] != [
        "schema-0/uninitialized identity",
        "empty predecessor has no user-schema objects or rows",
    ] or scope["does_not_prove"] != ["an on-disk SQLite database", *NON_PRODUCTION_CLAIMS]:
        fail("schema_zero_fixture_drift", "schema-0 fixture scope must remain compatibility-only")
    predecessor = require_object(
        fixture["predecessor"],
        "schema-0 fixture predecessor",
        {"identity", "schema", "storage", "is_database_file", "tables", "rows"},
    )
    migration = reference["schema_zero_migration"]
    if (
        predecessor["identity"] != migration["predecessor_identity"]
        or predecessor["schema"] != migration["predecessor_schema"]
        or predecessor["storage"] != migration["predecessor_storage"]
        or predecessor["is_database_file"] is not True
        or predecessor["tables"] != []
        or predecessor["rows"] != []
    ):
        fail("schema_zero_fixture_drift", "schema-0 fixture must be an empty on-disk schema-0 database with no user rows")
    expected_schema = require_object(
        fixture["expected_schema_v1"],
        "schema-0 fixture expected schema-v1",
        {"identity", "user_version", "ddl_path", "required_pragmas"},
    )
    if (
        expected_schema["identity"] != migration["target_schema"]
        or expected_schema["user_version"] != migration["target_user_version"]
        or expected_schema["ddl_path"] != migration["ddl_path"]
        or expected_schema["required_pragmas"] != migration["required_pragmas"]
        or fixture["requirement_ids"] != migration["requirement_ids"]
    ):
        fail("schema_zero_fixture_drift", "schema-0 fixture expected schema-v1 contract drift")
    assertions = require_list(fixture["expected_assertions"], "schema-0 fixture assertions")
    expected_assertion_ids = ["initial-migration-atomicity", "initial-migration-no-loss", "initial-migration-no-duplicate"]
    if [item.get("id") if isinstance(item, dict) else None for item in assertions] != expected_assertion_ids:
        fail("schema_zero_fixture_drift", "schema-0 fixture must assert atomicity, no-loss, and no-duplicate")
    for index, assertion_value in enumerate(assertions):
        assertion = require_object(assertion_value, f"schema-0 fixture assertion {index}", {"id", "assertion"})
        require_string(assertion["assertion"], f"schema-0 fixture assertion {index} text")

    pragma_lines = [
        "PRAGMA foreign_keys = ON;",
        "PRAGMA journal_mode = WAL;",
        "PRAGMA synchronous = FULL;",
        "PRAGMA busy_timeout = 5000;",
        "PRAGMA trusted_schema = OFF;",
    ]
    ddl = read_text(root, relative_path(migration["ddl_path"], "schema-0 fixture DDL path"), "schema-v1 DDL")
    if ddl.splitlines()[: len(pragma_lines)] != pragma_lines or "PRAGMA user_version = 1;" not in ddl:
        fail("schema_zero_fixture_drift", "schema-v1 DDL pragma or user version drift")


def sentinel_definitions() -> list[dict[str, str]]:
    return [
        {
            "id": "tampered_canonical_import",
            "target": "schemas/README.md",
            "mutation": "append-byte",
            "validator": "canonical_import_digest",
            "expected_error": "canonical_import_digest_drift",
        },
        {
            "id": "forbidden_crate_edge",
            "target": "crates/podway-core/Cargo.toml",
            "mutation": "add-podway-config-dependency",
            "validator": "cargo_dag",
            "expected_error": "forbidden_crate_edge",
        },
        {
            "id": "route_bypass",
            "target": "contracts/command-routes.json#session.status",
            "mutation": "remove-podway-protocol-hop",
            "validator": "command_routes",
            "expected_error": "route_bypass",
        },
        {
            "id": "interface_digest_drift",
            "target": "contracts/interfaces/store-v1.json",
            "mutation": "change-contract-version",
            "validator": "interface_digest",
            "expected_error": "interface_digest_drift",
        },
        {
            "id": "handoff_consumer_mismatch",
            "target": "contracts/handoffs/phase-0b-handoff.json#consumer_proofs",
            "mutation": "replace-PHASE_0C-with-PHASE_1",
            "validator": "handoff_consumers",
            "expected_error": "handoff_consumer_mismatch",
        },
        {
            "id": "proof_digest_drift",
            "target": "contracts/canonical-import.json#proof_ref",
            "mutation": "replace-proof-digest-with-zeroes",
            "validator": "proof_digest",
            "expected_error": "proof_digest_drift",
        },
        {
            "id": "ephemeral_artifact_proof",
            "target": "artifacts/phase0/verification-report.json#proof_ref",
            "mutation": "reference-ignored-runtime-report",
            "validator": "proof_location",
            "expected_error": "ephemeral_proof",
        },
        {
            "id": "wrong_file_right_digest_artifact_proof",
            "target": "contracts/locks/phase-0a-contract-lock.json#producer_proof",
            "mutation": "replace-lock-path-with-unrelated-copy",
            "validator": "artifact_proof_binding",
            "expected_error": "artifact_proof_mismatch",
        },
        {
            "id": "makefile_contract_drift",
            "target": "Makefile#test",
            "mutation": "replace---all-with---check",
            "validator": "makefile_contract",
            "expected_error": "makefile_contract_drift",
        },
        {
            "id": "extra_handoff_file",
            "target": "contracts/handoffs/unexpected.json",
            "mutation": "add-extra-receipt-file",
            "validator": "handoff_directory_entries",
            "expected_error": "handoff_receipt_drift",
        },
    ]


def validate_sentinel_fixture(root: Path) -> list[dict[str, str]]:
    fixture = read_json(root, SENTINELS_PATH, "known-fail sentinels")
    require_object(fixture, "known-fail sentinels", {"fixture_version", "fixture_kind", "scope", "sentinels"})
    if fixture["fixture_version"] != "podway.phase0.known-fail-sentinels/v1" or fixture["fixture_kind"] != "contract-tamper-sentinels":
        fail("sentinel_fixture_drift", "known-fail sentinel fixture identity drift")
    scope = require_object(fixture["scope"], "known-fail sentinel scope", {"proves", "does_not_prove"})
    if scope["proves"] != ["frozen contract validation rejects the named mutations", "fixture and handoff compatibility controls fail closed"] or scope["does_not_prove"] != NON_PRODUCTION_CLAIMS:
        fail("sentinel_fixture_drift", "known-fail sentinels must remain contract-only")
    sentinels = require_list(fixture["sentinels"], "known-fail sentinel definitions")
    actual: list[dict[str, str]] = []
    for index, item in enumerate(sentinels):
        sentinel = require_object(item, f"known-fail sentinel {index}", {"id", "target", "mutation", "validator", "expected_error"})
        if not all(isinstance(value, str) and value for value in sentinel.values()):
            fail("sentinel_fixture_drift", f"known-fail sentinel {index} contains an invalid value")
        actual.append({key: sentinel[key] for key in sentinel})
    if actual != sentinel_definitions():
        fail("sentinel_fixture_drift", "known-fail sentinel definitions drift")
    return actual


def validate_proof_ref(root: Path, value: Any, label: str) -> dict[str, str]:
    proof = require_object(value, label, {"digest", "kind", "location"})
    digest = require_digest(proof["digest"], f"{label} digest")
    location = relative_path(proof["location"], f"{label} location")
    kind = require_string(proof["kind"], f"{label} kind")
    if location.parts and location.parts[0] == "artifacts":
        fail("ephemeral_proof", f"{label} cannot reference ignored runtime artifacts: {location.as_posix()}")
    path = checked_path(root, location, label)
    if not path.is_file():
        fail("missing_proof", f"{label} is not a regular file: {location.as_posix()}")
    if digest_file(path) != digest:
        fail("proof_digest_drift", f"{label} digest does not match {location.as_posix()}")
    if kind == "test-report":
        if location.parent != EVIDENCE_DIRECTORY:
            fail("unstable_test_report", f"{label} test report must be stable contract evidence")
        expected_name = f"{run_verification.ATTESTATION_PREFIX}{digest}.json"
        if location.name != expected_name:
            fail("proof_digest_drift", f"{label} attestation filename does not match its content digest")
        attestation = load_bounded_json(path, f"{label} attestation", "invalid_attestation")
        try:
            validated = run_verification.validate_attestation_shape(attestation)
        except run_verification.VerificationError as error:
            fail("invalid_attestation", f"{label} attestation is invalid: {error.message}")
        if validated["product_version"] != run_verification.workspace_product_version(root):
            fail("invalid_attestation", f"{label} attestation product version drift")
    return {
        "digest": digest,
        "kind": kind,
        "location": location.as_posix(),
    }


def validate_proof_refs(root: Path, value: Any, label: str) -> list[dict[str, str]]:
    proofs = [
        validate_proof_ref(root, item, f"{label} {index}")
        for index, item in enumerate(require_list(value, label))
    ]
    if not proofs:
        fail("missing_proof", f"{label} must contain at least one proof")
    require_unique(proofs, label)
    return proofs
def validate_phase_artifact_proof(
    root: Path,
    phase: str,
    artifact_digest: str,
    value: Any,
    phase_lock_digests: dict[str, str],
    label: str,
) -> list[dict[str, str]]:
    expected_digest = require_digest(
        phase_lock_digests.get(phase),
        f"validated phase {phase} lock digest",
    )
    expected_location = phase_lock_path(phase).as_posix()
    if artifact_digest != expected_digest:
        fail("artifact_proof_mismatch", f"{label} artifact digest does not match {expected_location}")
    proofs = validate_proof_refs(root, value, label)
    if not any(
        proof["kind"] == "artifact"
        and proof["location"] == expected_location
        and proof["digest"] == expected_digest
        for proof in proofs
    ):
        fail("artifact_proof_mismatch", f"{label} must bind {expected_location}")
    return proofs




def validate_handoff_consumer_proofs(
    root: Path,
    value: Any,
    phase: str,
    reference: dict[str, Any],
) -> list[dict[str, Any]]:
    expected_consumers = reference["handoffs"][phase]["consumers"]
    unvalidated: list[tuple[str, Any]] = []
    for index, item in enumerate(require_list(value, f"handoff {phase} consumer_proofs")):
        proof = require_object(
            item,
            f"handoff {phase} consumer proof {index}",
            {"consumer", "proof_refs"},
        )
        consumer = require_string(
            proof["consumer"],
            f"handoff {phase} consumer proof {index} consumer",
        )
        if ACTOR_RE.fullmatch(consumer) is None:
            fail("handoff_consumer_mismatch", f"handoff {phase} consumer name is invalid")
        unvalidated.append((consumer, proof["proof_refs"]))
    consumers = [consumer for consumer, _ in unvalidated]
    if set(consumers) != set(expected_consumers) or len(consumers) != len(expected_consumers):
        fail("handoff_consumer_mismatch", f"handoff {phase} consumers do not match the frozen handoff")
    proofs = [
        {
            "consumer": consumer,
            "proof_refs": validate_proof_refs(
                root,
                proof_refs,
                f"handoff {phase} consumer {consumer} proofs",
            ),
        }
        for consumer, proof_refs in unvalidated
    ]
    require_unique(proofs, f"handoff {phase} consumer proofs")
    return proofs


def receipt_content_digest(receipt: dict[str, Any]) -> str:
    identity = require_object(receipt.get("receipt_identity"), "receipt identity")
    digest = identity.pop("digest", None)
    try:
        return digest_json(receipt)
    finally:
        if digest is not None:
            identity["digest"] = digest


def validate_receipt_identity(value: Any, label: str) -> dict[str, str]:
    identity = require_object(value, label, {"algorithm", "canonicalization", "digest", "kind"})
    if identity["algorithm"] != "sha256" or identity["canonicalization"] != "podway.canonical-json/v1" or identity["kind"] != "content-addressed":
        fail("handoff_receipt_drift", f"{label} identity policy drift")
    return {
        "algorithm": identity["algorithm"],
        "canonicalization": identity["canonicalization"],
        "digest": require_digest(identity["digest"], f"{label} digest"),
        "kind": identity["kind"],
    }


def validate_receipt(
    root: Path,
    receipt: Any,
    phase: str,
    reference: dict[str, Any],
    phase_lock_digests: dict[str, str],
    prerequisite_receipts: dict[str, dict[str, Any]],
) -> None:
    value = require_object(
        receipt,
        f"handoff {phase} receipt",
        {
            "acceptance",
            "artifact",
            "consumer_proofs",
            "enabled_gates",
            "invalidation",
            "phase",
            "prerequisites",
            "producer_proof",
            "receipt_identity",
            "reviewers",
            "schema_version",
        },
    )
    if value["schema_version"] != reference["v1_identifiers"]["handoff_receipt"] or value["phase"] != phase or value["acceptance"] != "accepted":
        fail("handoff_receipt_drift", f"handoff {phase} receipt schema, phase, or acceptance drift")
    handoff = reference["handoffs"][phase]
    artifact = require_object(value["artifact"], f"handoff {phase} artifact", {"digest", "kind", "name", "version"})
    if artifact["kind"] != handoff["artifact"]["kind"] or artifact["name"] != handoff["artifact"]["name"] or artifact["version"] != handoff["artifact"]["version"]:
        fail("handoff_receipt_drift", f"handoff {phase} artifact identity drift")
    artifact_digest = require_digest(artifact["digest"], f"handoff {phase} artifact digest")
    producer = require_object(value["producer_proof"], f"handoff {phase} producer proof", {"producer", "proof_refs"})
    if producer["producer"] != handoff["producer"]:
        fail("handoff_receipt_drift", f"handoff {phase} producer mismatch")
    validate_phase_artifact_proof(
        root,
        phase,
        artifact_digest,
        producer["proof_refs"],
        phase_lock_digests,
        f"handoff {phase} producer proofs",
    )
    validate_handoff_consumer_proofs(root, value["consumer_proofs"], phase, reference)

    gates = require_list(value["enabled_gates"], f"handoff {phase} enabled gates")
    if set(gates) != set(handoff["enabled_gates"]) or len(gates) != len(handoff["enabled_gates"]) or not all(isinstance(gate, str) and GATE_RE.fullmatch(gate) for gate in gates):
        fail("handoff_receipt_drift", f"handoff {phase} enabled gates drift")
    invalidation = require_object(
        value["invalidation"],
        f"handoff {phase} invalidation",
        {"affected_phases", "change_refs", "requires_0c_replay", "requires_downstream_replay", "requires_rc_replay"},
    )
    affected = require_list(invalidation["affected_phases"], f"handoff {phase} invalidation affected_phases")
    if set(affected) != set(handoff["affected_phases"]) or len(affected) != len(handoff["affected_phases"]):
        fail("handoff_receipt_drift", f"handoff {phase} invalidation phases drift")
    if not all(isinstance(item, str) and PHASE_RE.fullmatch(item) for item in affected):
        fail("handoff_receipt_drift", f"handoff {phase} invalidation phase is invalid")
    validate_proof_refs(
        root,
        invalidation["change_refs"],
        f"handoff {phase} invalidation change_refs",
    )
    if any(invalidation[field] is not True for field in ("requires_0c_replay", "requires_downstream_replay", "requires_rc_replay")):
        fail("handoff_receipt_drift", f"handoff {phase} invalidation replay controls must be explicit true")

    reviewers: list[dict[str, str]] = []
    for index, item in enumerate(require_list(value["reviewers"], f"handoff {phase} reviewers")):
        reviewer = require_object(item, f"handoff {phase} reviewer {index}", {"reviewer", "role"})
        reviewers.append({"reviewer": require_string(reviewer["reviewer"], "reviewer"), "role": require_string(reviewer["role"], "reviewer role")})
    if not reviewers:
        fail("missing_reviewer", f"handoff {phase} must have at least one reviewer")
    require_unique(reviewers, f"handoff {phase} reviewers")

    prerequisites = require_list(value["prerequisites"], f"handoff {phase} prerequisites")
    expected_phases = handoff["prerequisite_phases"]
    if len(prerequisites) != len(expected_phases):
        fail("handoff_prerequisite_mismatch", f"handoff {phase} prerequisite count drift")
    for index, prerequisite_phase in enumerate(expected_phases):
        prerequisite = require_object(prerequisites[index], f"handoff {phase} prerequisite {index}", {"digest", "receipt_identity"})
        prior = prerequisite_receipts.get(prerequisite_phase)
        if prior is None:
            fail("handoff_prerequisite_mismatch", f"handoff {phase} prerequisite receipt is unavailable: {prerequisite_phase}")
        prior_identity = validate_receipt_identity(prior["receipt_identity"], f"handoff {prerequisite_phase} receipt identity")
        identity = validate_receipt_identity(prerequisite["receipt_identity"], f"handoff {phase} prerequisite {index} identity")
        if prerequisite["digest"] != prior_identity["digest"] or identity != prior_identity:
            fail("handoff_prerequisite_mismatch", f"handoff {phase} prerequisite digest or identity drift: {prerequisite_phase}")

    identity = validate_receipt_identity(value["receipt_identity"], f"handoff {phase} receipt identity")
    if identity["digest"] != receipt_content_digest(value):
        fail("handoff_receipt_digest_drift", f"handoff {phase} receipt content digest drift")


def expected_handoff_files(reference: dict[str, Any]) -> set[str]:
    return {reference["handoffs"][phase]["file"] for phase in PHASES}


def validate_handoff_directory_entries(directory: Path, reference: dict[str, Any]) -> None:
    entries = list(directory.iterdir())
    actual_files = {path.name for path in entries if path.is_file() and not path.is_symlink()}
    if actual_files != expected_handoff_files(reference) or any(path.is_symlink() or not path.is_file() for path in entries):
        fail("handoff_receipt_drift", "handoff directory must contain exactly the current three regular receipts")

def validate_handoff_replacement_entries(directory: Path, reference: dict[str, Any]) -> None:
    entries = list(directory.iterdir())
    if any(path.is_symlink() or not path.is_file() for path in entries):
        fail("handoff_receipt_drift", "handoff replacement accepts only regular receipt files")
    actual_files = {path.name for path in entries}
    if not actual_files.issubset(expected_handoff_files(reference)):
        fail("handoff_receipt_drift", "handoff replacement contains an unexpected receipt file")


def existing_handoffs(root: Path, reference: dict[str, Any], phase_lock_digests: dict[str, str]) -> int:
    directory = checked_path(root, HANDOFF_DIRECTORY, "handoff directory")
    if not directory.exists():
        fail("missing_handoff_receipts", "handoff directory is missing")
    if not directory.is_dir():
        fail("handoff_receipt_drift", "handoff path is not a directory")
    validate_handoff_directory_entries(directory, reference)
    receipts = {
        phase: read_json(root, HANDOFF_DIRECTORY / reference["handoffs"][phase]["file"], f"handoff {phase} receipt")
        for phase in PHASES
    }
    for phase in PHASES:
        validate_receipt(root, receipts[phase], phase, reference, phase_lock_digests, receipts)
    return len(receipts)


def copy_sentinel_root(source_root: Path, temporary_root: Path) -> None:
    for directory in ("docs", "schemas", "spec", "presets", "contracts"):
        shutil.copytree(source_root / directory, temporary_root / directory)
    makefile = temporary_root / verify_contracts.MAKEFILE_PATH
    shutil.copy2(source_root / verify_contracts.MAKEFILE_PATH, makefile)
    shutil.copy2(source_root / "Cargo.toml", temporary_root / "Cargo.toml")
    for crate in CRATE_ORDER:
        destination = temporary_root / "crates" / crate
        destination.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source_root / "crates" / crate / "Cargo.toml", destination / "Cargo.toml")


def expect_known_failure(expected_code: str, operation: Any) -> None:
    try:
        operation()
    except ContractError as error:
        if error.code == expected_code:
            return
        fail("sentinel_failure_mismatch", f"known-fail sentinel returned {error.code}, expected {expected_code}")
    fail("sentinel_did_not_fail", f"known-fail sentinel did not fail with {expected_code}")

def validate_receipt_directory_policies(
    reference: dict[str, Any],
    phase_lock_digests: dict[str, str],
) -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-receipt-policies-") as temporary_name:
        temporary = Path(temporary_name)
        missing_root = temporary / "missing"
        missing_root.mkdir()
        expect_known_failure(
            "missing_handoff_receipts",
            lambda: existing_handoffs(missing_root, reference, phase_lock_digests),
        )
        prepare_receipt_replacement(missing_root, reference)

        partial_root = temporary / "partial"
        directory = partial_root / HANDOFF_DIRECTORY
        directory.mkdir(parents=True)
        first_receipt = reference["handoffs"]["0A"]["file"]
        (directory / first_receipt).write_text("{}\n", encoding="utf-8")
        prepare_receipt_replacement(partial_root, reference)
    return [
        "missing-check-rejected",
        "absent-replacement-accepted",
        "partial-replacement-accepted",
    ]


def run_sentinels(
    root: Path,
    reference: dict[str, Any],
    sentinels: list[dict[str, str]],
    phase_lock_digests: dict[str, str],
) -> list[str]:
    completed: list[str] = []
    with tempfile.TemporaryDirectory(prefix="podway-phase0-sentinels-") as temporary_name:
        temporary = Path(temporary_name)
        for sentinel in sentinels:
            fixture_root = temporary / sentinel["id"]
            copy_sentinel_root(root, fixture_root)
            identifier = sentinel["id"]
            if identifier == "tampered_canonical_import":
                with (fixture_root / "schemas/README.md").open("ab") as handle:
                    handle.write(b"x")
                expect_known_failure(sentinel["expected_error"], lambda: validate_canonical_import(fixture_root))
            elif identifier == "forbidden_crate_edge":
                manifest = fixture_root / "crates/podway-core/Cargo.toml"
                with manifest.open("a", encoding="utf-8") as handle:
                    handle.write("\npodway-config = { path = \"../podway-config\" }\n")
                expect_known_failure(sentinel["expected_error"], lambda: validate_cargo_dag(fixture_root, reference))
            elif identifier == "route_bypass":
                route_path = fixture_root / ROUTES_PATH
                routes = read_json(fixture_root, ROUTES_PATH, "route bypass sentinel")
                for route in routes["routes"]:
                    if route["command"] == "session.status":
                        route["path"] = ["podway-cli", "podway-daemon"]
                        break
                else:
                    fail("sentinel_fixture_drift", "route bypass sentinel target is unavailable")
                route_path.write_bytes(canonical_json(routes) + b"\n")
                expect_known_failure(sentinel["expected_error"], lambda: validate_command_routes(fixture_root, reference))
            elif identifier == "interface_digest_drift":
                baseline = validate_interfaces(fixture_root, reference)
                interface_path = fixture_root / "contracts/interfaces/store-v1.json"
                document = read_json(fixture_root, Path("contracts/interfaces/store-v1.json"), "interface digest sentinel")
                document["contract_version"] = "podway.store-contract/v1-tampered"
                interface_path.write_bytes(canonical_json(document) + b"\n")
                expect_known_failure(
                    sentinel["expected_error"],
                    lambda: validate_interface_digest_baseline(fixture_root, reference, baseline),
                )
            elif identifier == "handoff_consumer_mismatch":
                expect_known_failure(
                    sentinel["expected_error"],
                    lambda: validate_handoff_consumer_proofs(
                        fixture_root,
                        [{"consumer": "PHASE_1", "proof_refs": [{"digest": "0" * 64, "kind": "fixture", "location": "sentinel"}]}],
                        "0B",
                        reference,
                    ),
                )
            elif identifier == "proof_digest_drift":
                expect_known_failure(
                    sentinel["expected_error"],
                    lambda: validate_proof_refs(
                        fixture_root,
                        [
                            {
                                "digest": "0" * 64,
                                "kind": "artifact",
                                "location": CANONICAL_IMPORT_PATH.as_posix(),
                            }
                        ],
                        "tampered proof",
                    ),
                )
            elif identifier == "ephemeral_artifact_proof":
                expect_known_failure(
                    sentinel["expected_error"],
                    lambda: validate_proof_refs(
                        fixture_root,
                        [
                            {
                                "digest": "0" * 64,
                                "kind": "test-report",
                                "location": "artifacts/phase0/verification-report.json",
                            }
                        ],
                        "ephemeral proof",
                    ),
                )
            elif identifier == "wrong_file_right_digest_artifact_proof":
                unrelated = fixture_root / "contracts/unrelated-lock-copy.json"
                shutil.copy2(fixture_root / phase_lock_path("0A"), unrelated)
                expect_known_failure(
                    sentinel["expected_error"],
                    lambda: validate_phase_artifact_proof(
                        fixture_root,
                        "0A",
                        phase_lock_digests["0A"],
                        [
                            {
                                "digest": phase_lock_digests["0A"],
                                "kind": "artifact",
                                "location": "contracts/unrelated-lock-copy.json",
                            }
                        ],
                        phase_lock_digests,
                        "wrong-file artifact proof",
                    ),
                )
            elif identifier == "makefile_contract_drift":
                makefile = fixture_root / verify_contracts.MAKEFILE_PATH
                text = makefile.read_text(encoding="utf-8")
                updated = text.replace(
                    "python3 tools/verify_contracts.py --all",
                    "python3 tools/verify_contracts.py --check",
                    1,
                )
                if updated == text:
                    fail("sentinel_fixture_drift", "Makefile contract sentinel target is unavailable")
                makefile.write_text(updated, encoding="utf-8")
                expect_known_failure(sentinel["expected_error"], lambda: validate_makefile_contract(fixture_root))
            elif identifier == "extra_handoff_file":
                extra = fixture_root / HANDOFF_DIRECTORY / "unexpected.json"
                extra.write_text("{}\n", encoding="utf-8")
                expect_known_failure(
                    sentinel["expected_error"],
                    lambda: prepare_receipt_replacement(fixture_root, reference),
                )
            else:
                fail("sentinel_fixture_drift", f"unknown known-fail sentinel: {identifier}")
            completed.append(identifier)
    return completed


def validate_baseline(
    root: Path,
    include_sentinels: bool,
    validate_existing: bool,
) -> tuple[dict[str, Any], dict[str, Any]]:
    reference = validate_reference_model(root)
    phase_lock_digests = validate_phase_locks(root, reference)
    results: dict[str, Any] = {
        "canonical_imports": validate_canonical_import(root),
        "cargo_crates": validate_cargo_dag(root, reference),
        "command_routes": validate_command_routes(root, reference),
        "v1_identifiers": validate_identifiers(root, reference),
        "interface_digests": validate_interfaces(root, reference),
        "requirement_ids": validate_requirement_groups(root, reference),
        "makefile_contract": validate_makefile_contract(root),
        "phase_locks": phase_lock_digests,
    }
    validate_schema_zero_fixture(root, reference)
    sentinels = validate_sentinel_fixture(root)
    results["fixtures"] = {"schema_zero": "validated", "known_fail_sentinels": len(sentinels)}
    results["receipt_directory_policies"] = validate_receipt_directory_policies(
        reference,
        phase_lock_digests,
    )
    if validate_existing:
        results["existing_receipts"] = existing_handoffs(root, reference, phase_lock_digests)
    if include_sentinels:
        results["known_fail_sentinels"] = run_sentinels(root, reference, sentinels, phase_lock_digests)
    return reference, results


def normalized_proofs(root: Path, value: Any, label: str) -> list[dict[str, str]]:
    return sorted(validate_proof_refs(root, value, label), key=canonical_json)
def validate_makefile_contract(root: Path) -> int:
    try:
        return verify_contracts.validate_makefile_contract(root)
    except verify_contracts.VerificationError as error:
        fail(error.code, str(error))



def normalized_consumer_proofs(
    root: Path,
    value: Any,
    phase: str,
    reference: dict[str, Any],
) -> list[dict[str, Any]]:
    proofs = validate_handoff_consumer_proofs(root, value, phase, reference)
    return sorted(
        [{"consumer": proof["consumer"], "proof_refs": sorted(proof["proof_refs"], key=canonical_json)} for proof in proofs],
        key=lambda proof: proof["consumer"],
    )


def normalized_reviewers(value: Any, phase: str) -> list[dict[str, str]]:
    reviewers: list[dict[str, str]] = []
    for index, item in enumerate(require_list(value, f"verification {phase} reviewers")):
        reviewer = require_object(item, f"verification {phase} reviewer {index}", {"reviewer", "role"})
        reviewers.append({"reviewer": require_string(reviewer["reviewer"], "reviewer"), "role": require_string(reviewer["role"], "reviewer role")})
    if not reviewers:
        fail("missing_reviewer", f"verification {phase} must name at least one reviewer")
    require_unique(reviewers, f"verification {phase} reviewers")
    return sorted(reviewers, key=canonical_json)


def normalized_invalidation(root: Path, value: Any, phase: str, reference: dict[str, Any]) -> dict[str, Any]:
    invalidation = require_object(
        value,
        f"verification {phase} invalidation",
        {"affected_phases", "change_refs", "requires_0c_replay", "requires_downstream_replay", "requires_rc_replay"},
    )
    expected = reference["handoffs"][phase]["affected_phases"]
    affected = require_list(invalidation["affected_phases"], f"verification {phase} invalidation affected_phases")
    if set(affected) != set(expected) or len(affected) != len(expected) or not all(isinstance(item, str) and PHASE_RE.fullmatch(item) for item in affected):
        fail("verification_invalid", f"verification {phase} invalidation phases do not match the frozen handoff")
    if any(invalidation[field] is not True for field in ("requires_0c_replay", "requires_downstream_replay", "requires_rc_replay")):
        fail("verification_invalid", f"verification {phase} must explicitly enable every replay control")
    return {
        "affected_phases": expected,
        "change_refs": normalized_proofs(
            root,
            invalidation["change_refs"],
            f"verification {phase} invalidation change_refs",
        ),
        "requires_0c_replay": True,
        "requires_downstream_replay": True,
        "requires_rc_replay": True,
    }


def load_verification(
    root: Path,
    verification_path: Path,
    reference: dict[str, Any],
    phase_lock_digests: dict[str, str],
) -> dict[str, dict[str, Any]]:
    if verification_path.is_absolute():
        path = verification_path
    else:
        path = checked_path(root, verification_path, "verification input")
    if path.is_symlink() or not path.is_file():
        fail("missing_verification", "verification input must be a regular file")
    verification = load_bounded_json(path, "verification input", "invalid_verification")
    value = require_object(verification, "verification input", {"schema_version", "product_version", "phases"})
    if value["schema_version"] != "podway.phase0.verification/v1" or value["product_version"] != reference["product_version"]:
        fail("verification_invalid", "verification input identity or product version drift")
    phase_entries = require_list(value["phases"], "verification phases")
    if len(phase_entries) != len(PHASES):
        fail("verification_invalid", "verification input must contain exactly Phases 0A, 0B, and 0C")
    result: dict[str, dict[str, Any]] = {}
    for index, raw_entry in enumerate(phase_entries):
        entry = require_object(
            raw_entry,
            f"verification phase {index}",
            {"phase", "acceptance", "artifact", "producer_proof", "consumer_proofs", "enabled_gates", "invalidation", "reviewers"},
        )
        phase = require_string(entry["phase"], f"verification phase {index} name")
        if phase not in PHASES or phase in result:
            fail("verification_invalid", "verification phases must be unique 0A, 0B, and 0C")
        if entry["acceptance"] != "accepted":
            fail("verification_unaccepted", f"verification phase {phase} is not explicitly accepted")
        handoff = reference["handoffs"][phase]
        artifact = require_object(entry["artifact"], f"verification {phase} artifact", {"digest", "kind", "name", "version"})
        if artifact["kind"] != handoff["artifact"]["kind"] or artifact["name"] != handoff["artifact"]["name"] or artifact["version"] != handoff["artifact"]["version"]:
            fail("verification_invalid", f"verification {phase} artifact does not match the frozen handoff")
        artifact = {"digest": require_digest(artifact["digest"], f"verification {phase} artifact digest"), "kind": artifact["kind"], "name": artifact["name"], "version": artifact["version"]}
        producer = require_object(entry["producer_proof"], f"verification {phase} producer proof", {"producer", "proof_refs"})
        if producer["producer"] != handoff["producer"]:
            fail("verification_invalid", f"verification {phase} producer does not match the frozen handoff")
        producer_proofs = sorted(
            validate_phase_artifact_proof(
                root,
                phase,
                artifact["digest"],
                producer["proof_refs"],
                phase_lock_digests,
                f"verification {phase} producer proofs",
            ),
            key=canonical_json,
        )
        gates = require_list(entry["enabled_gates"], f"verification {phase} enabled gates")
        if set(gates) != set(handoff["enabled_gates"]) or len(gates) != len(handoff["enabled_gates"]) or not all(isinstance(gate, str) and GATE_RE.fullmatch(gate) for gate in gates):
            fail("verification_invalid", f"verification {phase} enabled gates do not match the frozen handoff")
        result[phase] = {
            "artifact": artifact,
            "producer_proof": {"producer": handoff["producer"], "proof_refs": producer_proofs},
            "consumer_proofs": normalized_consumer_proofs(root, entry["consumer_proofs"], phase, reference),
            "enabled_gates": sorted(gates),
            "invalidation": normalized_invalidation(root, entry["invalidation"], phase, reference),
            "reviewers": normalized_reviewers(entry["reviewers"], phase),
        }
    if set(result) != set(PHASES):
        fail("verification_invalid", "verification phases do not exactly cover 0A, 0B, and 0C")
    return result


def build_receipts(
    root: Path,
    verification: dict[str, dict[str, Any]],
    reference: dict[str, Any],
    phase_lock_digests: dict[str, str],
) -> dict[str, dict[str, Any]]:
    receipts: dict[str, dict[str, Any]] = {}
    for phase in PHASES:
        handoff = reference["handoffs"][phase]
        prerequisites: list[dict[str, Any]] = []
        for prerequisite_phase in handoff["prerequisite_phases"]:
            prior = receipts[prerequisite_phase]
            identity = prior["receipt_identity"]
            prerequisites.append({"digest": identity["digest"], "receipt_identity": dict(identity)})
        receipt: dict[str, Any] = {
            "acceptance": "accepted",
            "artifact": verification[phase]["artifact"],
            "consumer_proofs": verification[phase]["consumer_proofs"],
            "enabled_gates": verification[phase]["enabled_gates"],
            "invalidation": verification[phase]["invalidation"],
            "phase": phase,
            "prerequisites": prerequisites,
            "producer_proof": verification[phase]["producer_proof"],
            "receipt_identity": {
                "algorithm": "sha256",
                "canonicalization": "podway.canonical-json/v1",
                "kind": "content-addressed",
            },
            "reviewers": verification[phase]["reviewers"],
            "schema_version": reference["v1_identifiers"]["handoff_receipt"],
        }
        receipt["receipt_identity"]["digest"] = receipt_content_digest(receipt)
        validate_receipt(root, receipt, phase, reference, phase_lock_digests, receipts | {phase: receipt})
        receipts[phase] = receipt
    return receipts


def atomic_write(path: Path, content: bytes) -> None:
    if path.exists() and (path.is_symlink() or not path.is_file()):
        fail("unsafe_path", f"receipt destination is not a regular file: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        directory_descriptor = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory_descriptor)
        finally:
            os.close(directory_descriptor)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise

def validate_lock_inputs(root: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    reference = validate_reference_model(root)
    checks: dict[str, Any] = {
        "canonical_imports": validate_canonical_import(root),
        "cargo_crates": validate_cargo_dag(root, reference),
        "command_routes": validate_command_routes(root, reference),
        "v1_identifiers": validate_identifiers(root, reference),
        "interface_digests": validate_interfaces(root, reference),
        "requirement_ids": validate_requirement_groups(root, reference),
    }
    validate_schema_zero_fixture(root, reference)
    sentinels = validate_sentinel_fixture(root)
    checks["fixtures"] = {
        "schema_zero": "validated",
        "known_fail_sentinels": len(sentinels),
    }
    return reference, checks


def write_phase_locks(
    root: Path,
    reference: dict[str, Any],
) -> list[dict[str, str]]:
    published: list[dict[str, str]] = []
    for phase in PHASES:
        relative = LOCK_DIRECTORY / f"phase-{phase.lower()}-contract-lock.json"
        entries = [
            {
                "path": entry,
                "sha256": digest_file(checked_path(root, Path(entry), f"phase {phase} lock input")),
            }
            for entry in sorted(expected_phase_lock_entries(root, phase))
        ]
        document = {
            "schema_version": "podway.phase0.lock/v1",
            "phase": phase,
            "artifact": reference["handoffs"][phase]["artifact"],
            "entries": entries,
        }
        atomic_write(checked_path(root, relative, f"phase {phase} lock destination"), canonical_json(document) + b"\n")
        published.append(
            {
                "phase": phase,
                "path": relative.as_posix(),
                "digest": digest_file(checked_path(root, relative, f"phase {phase} lock")),
            }
        )
    validated = validate_phase_locks(root, reference)
    if any(item["digest"] != validated[item["phase"]] for item in published):
        fail("phase_lock_digest_drift", "published phase lock digest mismatch")
    return published


def prepare_receipt_replacement(root: Path, reference: dict[str, Any]) -> None:
    directory = checked_path(root, HANDOFF_DIRECTORY, "handoff directory")
    if not directory.exists():
        return
    if not directory.is_dir():
        fail("unsafe_path", "handoff destination is not a directory")
    validate_handoff_replacement_entries(directory, reference)


def replace_receipts(
    root: Path,
    receipts: dict[str, dict[str, Any]],
    reference: dict[str, Any],
    phase_lock_digests: dict[str, str],
) -> list[dict[str, str]]:
    prepare_receipt_replacement(root, reference)
    published: list[dict[str, str]] = []
    for phase in PHASES:
        relative = HANDOFF_DIRECTORY / reference["handoffs"][phase]["file"]
        path = checked_path(root, relative, f"handoff {phase} destination")
        content = canonical_json(receipts[phase]) + b"\n"
        atomic_write(path, content)
        published.append({"phase": phase, "path": relative.as_posix(), "digest": receipts[phase]["receipt_identity"]["digest"]})
    existing_handoffs(root, reference, phase_lock_digests)
    return published


def receipt(mode: str, ok: bool, **details: Any) -> None:
    print(json.dumps({"mode": mode, "ok": ok, **details}, ensure_ascii=False, sort_keys=True, separators=(",", ":")))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="validate imported assets, contracts, fixtures, and existing receipts")
    mode.add_argument("--write", action="store_true", help="validate inputs and atomically publish receipts")
    mode.add_argument("--write-locks", action="store_true", help="validate lock inputs and atomically regenerate Phase 0 locks")
    parser.add_argument("--verification", type=Path, help="leader-supplied Phase 0 verification JSON; required with --write")
    arguments = parser.parse_args()
    if arguments.check:
        selected_mode = "check"
    elif arguments.write_locks:
        selected_mode = "write-locks"
    else:
        selected_mode = "write"
    if not arguments.write and arguments.verification is not None:
        parser.error("--verification is only valid with --write")
    if arguments.write and arguments.verification is None:
        parser.error("--write requires --verification <path>")
    try:
        if arguments.write_locks:
            reference, checks = validate_lock_inputs(ROOT)
            locks = write_phase_locks(ROOT, reference)
            phase_lock_digests = {item["phase"]: item["digest"] for item in locks}
            sentinels = validate_sentinel_fixture(ROOT)
            checks["known_fail_sentinels"] = run_sentinels(
                ROOT,
                reference,
                sentinels,
                phase_lock_digests,
            )
            receipt(selected_mode, True, checks=checks, locks=locks)
            return 0

        reference, checks = validate_baseline(
            ROOT,
            include_sentinels=arguments.check,
            validate_existing=arguments.check,
        )
        if arguments.check:
            receipt(selected_mode, True, checks=checks)
        else:
            phase_lock_digests = checks["phase_locks"]
            verification = load_verification(ROOT, arguments.verification, reference, phase_lock_digests)
            receipts = build_receipts(ROOT, verification, reference, phase_lock_digests)
            published = replace_receipts(ROOT, receipts, reference, phase_lock_digests)
            receipt(selected_mode, True, checks=checks, receipts=published)
    except (ContractError, OSError, json.JSONDecodeError, tomllib.TOMLDecodeError, TypeError, ValueError, KeyError) as error:
        code = error.code if isinstance(error, ContractError) else "phase0_receipts_failed"
        message = error.message if isinstance(error, ContractError) else str(error)
        receipt(selected_mode, False, error={"code": code, "message": message})
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
