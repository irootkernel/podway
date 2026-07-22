#!/usr/bin/env python3
"""Verify Phase 0A canonical import, crate, and command-route controls."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import shutil
import tempfile
import tomllib
from typing import Any, Callable

import import_sot


ROOT = Path(__file__).resolve().parent.parent
ADJACENCY_PATH = Path("contracts/cargo-adjacency.json")
ROUTES_PATH = Path("contracts/command-routes.json")
ADJACENCY_VERSION = "podway.cargo-adjacency/v1"
ROUTES_VERSION = "podway.command-routes/v1"
MAKEFILE_PATH = Path("Makefile")
REQUIRED_MAKE_TARGETS = ("test", "test-prepare", "test-unit", "test-int", "test-e2e", "dist")
REQUIRED_TEST_SEQUENCE = (
    "$(MAKE) test-prepare",
    "$(MAKE) test-unit",
    "$(MAKE) test-int",
    "$(MAKE) test-e2e",
)
REQUIRED_PREPARE_COMMANDS = (
    "python3 tools/import_sot.py --write",
    "cargo fmt --all",
    "cargo check --workspace --all-targets --locked",
    "cargo clippy --workspace --all-targets --locked -- -D warnings",
    "cargo deny check",
    "python3 tools/verify_test_layout.py --check",
    "python3 tools/verify_quality_contracts.py",
    "python3 tools/verify_contracts.py --all",
    "python3 tools/phase0_receipts.py --check",
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


class VerificationError(Exception):
    """A Phase 0A contract invariant was violated."""

    def __init__(self, message: str, code: str = "contract_verification_failed") -> None:
        super().__init__(message)
        self.code = code


def fail(message: str, code: str = "contract_verification_failed") -> None:
    raise VerificationError(message, code)


def read_json(root: Path, relative: Path, label: str) -> dict[str, Any]:
    path = import_sot.checked_path(root, relative, label)
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


def validate_canonical_import(root: Path) -> int:
    try:
        _, mappings = import_sot.validate_contract(root)
        import_sot.validate_sources(root, mappings)
        import_sot.validate_destination_tree(root, mappings, require_content=True)
    except import_sot.ContractError as error:
        fail(str(error))
    return len(mappings)


def validate_checksums(root: Path) -> int:
    checksum_relative = Path("sot/checksums.sha256")
    checksum_path = import_sot.checked_path(root, checksum_relative, "SOT checksums")
    if not checksum_path.is_file():
        fail("SOT checksums file is missing")
    try:
        lines = checksum_path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        fail(f"cannot read SOT checksums: {error}")
    if not lines:
        fail("SOT checksums file is empty")

    entries: dict[str, str] = {}
    pattern = re.compile(r"^([0-9a-f]{64})  ([^\s].*)$")
    for line_number, line in enumerate(lines, start=1):
        match = pattern.fullmatch(line)
        if match is None:
            fail(f"SOT checksum format is invalid at line {line_number}")
        digest, recorded_path = match.groups()
        relative = import_sot.relative_path(recorded_path, f"SOT checksum path at line {line_number}")
        if recorded_path in entries:
            fail(f"SOT checksum path is duplicated: {recorded_path}")
        target = import_sot.checked_path(root, Path("sot") / relative, "SOT checksum target")
        if not target.is_file():
            fail(f"SOT checksum target is not a regular file: {recorded_path}")
        if import_sot.digest_file(target) != digest:
            fail(f"SOT checksum digest drift: {recorded_path}")
        entries[recorded_path] = digest

    shipped = import_sot.regular_files(root, "sot", required=True)
    expected = {path.removeprefix("sot/") for path in shipped if path != "sot/checksums.sha256"}
    recorded = set(entries)
    if recorded != expected:
        fail(
            "SOT checksums do not exactly cover shipped files; "
            f"missing={sorted(expected - recorded)}, extra={sorted(recorded - expected)}"
        )
    return len(entries)


def catalog_commands(root: Path) -> set[str]:
    path = import_sot.checked_path(root, Path("sot/spec/command-catalog.yaml"), "command catalog")
    if not path.is_file():
        fail("command catalog is missing")
    text = path.read_text(encoding="utf-8")
    if not re.search(r"^schema: podway\.command-catalog/v1$", text, flags=re.MULTILINE):
        fail("command catalog does not declare podway.command-catalog/v1")
    names = set(re.findall(r"^- name: ([a-z][a-z0-9._-]*)$", text, flags=re.MULTILINE))
    if not names:
        fail("command catalog contains no commands")
    return names


def validate_v1_identifiers(root: Path) -> int:
    canonical = read_json(root, Path("contracts/canonical-import.json"), "canonical import contract")
    adjacency = read_json(root, ADJACENCY_PATH, "Cargo adjacency contract")
    routes = read_json(root, ROUTES_PATH, "command route contract")
    expected_contract_versions = {
        "canonical import contract": (canonical, import_sot.CONTRACT_VERSION),
        "Cargo adjacency contract": (adjacency, ADJACENCY_VERSION),
        "command route contract": (routes, ROUTES_VERSION),
    }
    for label, (contract, expected_version) in expected_contract_versions.items():
        if contract.get("contract_version") != expected_version:
            fail(f"{label} does not declare {expected_version}")

    schema_files = sorted(
        path
        for path in import_sot.regular_files(root, "sot/schemas", required=True)
        if path.endswith("-v1.schema.json")
    )
    if not schema_files:
        fail("no v1 JSON schemas were found")
    for relative_name in schema_files:
        path = import_sot.checked_path(root, Path(relative_name), "schema")
        try:
            schema = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            fail(f"cannot parse schema {relative_name}: {error}")
        basename = Path(relative_name).name.removesuffix("-v1.schema.json")
        expected_id = f"urn:podway:schema:{basename}:v1"
        if schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
            fail(f"schema draft identifier drift: {relative_name}")
        if schema.get("$id") != expected_id:
            fail(f"schema v1 identifier drift: {relative_name}")

    error_catalog = read_json(root, Path("sot/spec/error-codes.json"), "error catalog")
    if error_catalog.get("schema") != "podway.error-catalog/v1":
        fail("error catalog does not declare podway.error-catalog/v1")
    catalog_commands(root)
    preset_files = sorted(
        path for path in import_sot.regular_files(root, "sot/presets", required=True) if path.endswith(".yaml")
    )
    if not preset_files:
        fail("no v1 preset files were found")
    for relative_name in preset_files:
        path = import_sot.checked_path(root, Path(relative_name), "preset")
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
    root_manifest = read_toml(import_sot.checked_path(root, Path("Cargo.toml"), "workspace manifest"), "workspace manifest")
    workspace = root_manifest.get("workspace")
    if not isinstance(workspace, dict) or not isinstance(workspace.get("members"), list):
        fail("workspace manifest must declare a members list")
    members = workspace["members"]
    expected_members = {entry["path"] for entry in crates}
    if not all(isinstance(member, str) for member in members) or set(members) != expected_members or len(members) != len(expected_members):
        fail("workspace members drift from the exact nine-crate adjacency contract")

    for crate in crates:
        name = crate["name"]
        manifest_path = import_sot.checked_path(root, Path(crate["path"]) / "Cargo.toml", f"manifest for {name}")
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
    if not isinstance(routes, list):
        fail("command route contract routes must be a list")

    expected_commands = catalog_commands(root) | {"completions"}
    commands: set[str] = set()
    task_commands = expected_commands - LOCAL_COMMANDS - SERVICE_COMMANDS
    for route in routes:
        if not isinstance(route, dict) or set(route) != {"command", "owner", "path", "execution", "capabilities"}:
            fail("command route has unexpected or missing fields")
        command = route["command"]
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
    path = import_sot.checked_path(root, MAKEFILE_PATH, "Makefile")
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
        fail("Makefile test target does not run prepare, unit, int, and e2e sequentially", "makefile_contract_drift")
    missing_commands = [command for command in REQUIRED_PREPARE_COMMANDS if command not in text]
    if missing_commands:
        fail(f"Makefile omits required prepare commands: {missing_commands}", "makefile_contract_drift")
    dist_recipe = re.search(r"^dist\s*:\s*\n((?:\t.*\n)+)", text, flags=re.MULTILINE)
    if dist_recipe is None or "$(MAKE) test" not in dist_recipe.group(1):
        fail("Makefile dist target must run the sole release gate first", "makefile_contract_drift")
    for required in ("cargo build --release --locked", "tools/release_archive.py package"):
        if required not in text:
            fail(f"Makefile dist target omits {required}", "makefile_contract_drift")
    return len(REQUIRED_MAKE_TARGETS) + len(REQUIRED_PREPARE_COMMANDS)




def copy_contracts(source_root: Path, destination_root: Path) -> None:
    shutil.copytree(source_root / "contracts", destination_root / "contracts")


def build_import_fixture(source_root: Path, destination_root: Path) -> None:
    shutil.copytree(source_root / "sot", destination_root / "sot")
    copy_contracts(source_root, destination_root)
    _, mappings = import_sot.validate_contract(destination_root)
    for mapping in mappings:
        source = destination_root / mapping["source"]
        destination = destination_root / mapping["destination"]
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)


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
    except (VerificationError, import_sot.ContractError):
        return
    fail(f"known-fail sentinel did not fail: {label}")


def run_sentinels(root: Path) -> list[str]:
    completed: list[str] = []
    with tempfile.TemporaryDirectory(prefix="podway-phase-0a-") as temporary_name:
        temporary = Path(temporary_name)
        import_fixture = temporary / "tampered-import"
        import_fixture.mkdir()
        build_import_fixture(root, import_fixture)
        with (import_fixture / "schemas/README.md").open("ab") as handle:
            handle.write(b"tampered\n")
        require_known_failure("tampered import", lambda: validate_canonical_import(import_fixture))
        completed.append("tampered_import")

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
        shutil.copytree(root / "sot", route_fixture / "sot")
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
    return completed


def receipt(mode: str, ok: bool, **details: Any) -> None:
    print(json.dumps({"mode": mode, "ok": ok, **details}, sort_keys=True, separators=(",", ":")))


def production_checks(root: Path) -> dict[str, int]:
    return {
        "canonical_imports": validate_canonical_import(root),
        "sot_checksums": validate_checksums(root),
        "v1_identifiers": validate_v1_identifiers(root),
        "cargo_adjacency": validate_adjacency(root),
        "command_routes": validate_routes(root),
        "makefile_contract": validate_makefile_contract(root),
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
    except (VerificationError, import_sot.ContractError, OSError, tomllib.TOMLDecodeError) as error:
        code = error.code if isinstance(error, VerificationError) else "contract_verification_failed"
        receipt(selected_mode, False, error={"code": code, "message": str(error)})
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
