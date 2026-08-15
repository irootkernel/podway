#!/usr/bin/env python3
"""Create the deterministic Dolgorae compatibility-pinning handoff."""

from __future__ import annotations

import argparse
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from typing import Any

import release_archive
import release_evidence
import repository_assets


ROOT = Path(__file__).resolve().parents[1]
VERSION = release_archive.PRODUCT_VERSION
TARGET = release_archive.TARGET
ARCHIVE_ROOT = release_archive.ARCHIVE_ROOT
SCHEMA = release_evidence.HANDOFF_SCHEMA
ADAPTER_CONTRACT_SCHEMA = "podway.dolgorae-adapter-contract/v2"
ADAPTER_CONTRACT_PATH = ROOT / "release/dolgorae-adapter-contract-v2.json"


class HandoffError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise HandoffError(message)


def run_git(*arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=ROOT,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        fail(f"Git {' '.join(arguments)} failed: {completed.stderr.strip()}")
    return completed.stdout.strip()


def read_object(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"{label} is not readable JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def require_regular_file(path: Path, label: str) -> Path:
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file: {path}")
    return path


def canonical_bytes(value: dict[str, Any]) -> bytes:
    return release_evidence.canonical_bytes(value)


def write_json(path: Path, value: dict[str, Any]) -> None:
    try:
        release_evidence.atomic_write_json(path, value)
    except release_evidence.EvidenceError as error:
        fail(str(error))


def write_reviewable_json(path: Path, value: dict[str, Any]) -> None:
    if path.parent.is_symlink() or not path.parent.is_dir():
        fail(f"adapter contract directory must be a regular directory: {path.parent}")
    payload = (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    descriptor, temporary_name = tempfile.mkstemp(
        dir=path.parent, prefix=f".{path.name}.", suffix=".tmp"
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as opened:
            opened.write(payload)
            opened.flush()
            os.fsync(opened.fileno())
        temporary.chmod(0o644)
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def handoff_from_provenance(
    provenance: dict[str, Any],
    provenance_name: str,
    provenance_sha256: str,
    source_tree: str,
    adapter: dict[str, Any] | None = None,
    adapter_catalog_sha256: str | None = None,
) -> dict[str, Any]:
    if provenance.get("source_tree") != source_tree:
        fail("release provenance source tree does not match the qualified Git tree")
    if adapter is None:
        adapter = read_object(ADAPTER_CONTRACT_PATH, "Dolgorae v2 adapter contract")
    validate_adapter_contract(adapter)
    if adapter_catalog_sha256 is None:
        adapter_catalog_sha256 = f"sha256:{release_archive.sha256_file(ADAPTER_CONTRACT_PATH)}"
    return release_evidence.handoff_from_provenance(
        provenance,
        provenance_name,
        provenance_sha256,
        adapter,
        adapter_catalog_sha256,
    )


def catalog_codes(value: dict[str, Any], key: str, label: str) -> list[str]:
    entries = value.get(key)
    if not isinstance(entries, list) or not entries:
        fail(f"{label} entries must be a non-empty list")
    codes = [entry.get("code") for entry in entries if isinstance(entry, dict)]
    if len(codes) != len(entries) or any(not isinstance(code, str) or not code for code in codes):
        fail(f"{label} contains an invalid code")
    if len(codes) != len(set(codes)):
        fail(f"{label} contains duplicate codes")
    return codes


def logical_result_family_paths(value: Any, label: str) -> list[str]:
    if not isinstance(value, list) or not value:
        fail(f"{label} must be a non-empty list")
    if any(not isinstance(path, str) or not path.startswith("assets/schemas/") for path in value):
        fail(f"{label} must contain only canonical schema paths")
    paths = [path.removeprefix("assets/") for path in value]
    if len(paths) != len(set(paths)):
        fail(f"{label} contains duplicate schema paths")
    return paths


def adapter_contract() -> dict[str, Any]:
    manifest_path = ROOT / "contracts/contract-manifest-v1.json"
    manifest = read_object(manifest_path, "contract manifest")
    if (
        manifest.get("schema_version") != "podway.contract-manifest/v1"
        or manifest.get("product") != "podway"
        or manifest.get("product_version") != VERSION
    ):
        fail("contract manifest identity is invalid")

    assets = manifest.get("assets")
    if not isinstance(assets, list):
        fail("contract manifest assets must be a list")
    route_catalog = read_object(ROOT / "contracts/command-routes.json", "command route catalog")
    routes = route_catalog.get("routes")
    if not isinstance(routes, list) or not routes:
        fail("command route catalog routes must be a non-empty list")
    route_inventory = [route.get("command") for route in routes if isinstance(route, dict)]
    if (
        len(route_inventory) != len(routes)
        or any(not isinstance(route, str) or not route for route in route_inventory)
        or len(route_inventory) != len(set(route_inventory))
    ):
        fail("command route catalog contains invalid or duplicate commands")
    if len(route_inventory) != 57:
        fail("command route catalog does not contain the v2-only route inventory")

    error_catalog = read_object(ROOT / "assets/specifications/error-codes.json", "error catalog")
    runtime_error_codes = catalog_codes(error_catalog, "errors", "error catalog")
    if len(runtime_error_codes) != 88:
        fail("error catalog does not contain the v2-only runtime inventory")

    diagnostic_catalog = read_object(
        ROOT / "assets/specifications/authoring-diagnostics.json", "authoring diagnostic catalog"
    )
    diagnostic_codes = catalog_codes(
        diagnostic_catalog, "diagnostics", "authoring diagnostic catalog"
    )
    if len(diagnostic_codes) != 53:
        fail("authoring diagnostic catalog does not contain the exact v2 inventory")

    schema_assets = [
        asset for asset in assets if isinstance(asset, dict) and asset.get("kind") == "schema"
    ]
    if any(
        not isinstance(asset.get("path"), str) or not asset.get("path")
        for asset in schema_assets
    ):
        fail("contract manifest contains an invalid schema path")
    current_schema_paths = {asset["path"] for asset in schema_assets}
    schema_pins: list[dict[str, str]] = []
    for asset in sorted(schema_assets, key=lambda item: item["path"]):
        path = asset["path"]
        digest = asset.get("sha256")
        if not isinstance(path, str) or not isinstance(digest, str):
            fail("contract manifest contains an invalid schema asset")
        source = require_regular_file(
            ROOT / repository_assets.logical_source(path), f"schema asset {path}"
        )
        schema = read_object(source, f"schema asset {path}")
        schema_id = schema.get("$id")
        if not isinstance(schema_id, str) or not schema_id:
            fail(f"schema asset has no $id: {path}")
        if f"sha256:{release_archive.sha256_file(source)}" != digest:
            fail(f"contract manifest schema digest is stale for {path}")
        schema_pins.append({"id": schema_id, "path": path, "sha256": digest})
    if len(schema_pins) != len(current_schema_paths) or len(
        {pin["id"] for pin in schema_pins}
    ) != len(schema_pins):
        fail("contract manifest schemas must have unique identifiers")

    return {
        "adapter_acceptance": [
            {
                "expected": "manifest-and-adapter-catalog-pins-match-before-dispatch",
                "id": "DOLV2-001",
                "check": "manifest-identity",
            },
            {"expected": "all-pinned-bytes-validate-closed", "id": "DOLV2-002", "check": "schema-pins"},
            {"expected": "v2-only-routes-dispatch", "id": "DOLV2-003", "check": "route-surface"},
            {
                "expected": "all-listed-results-use-output-v3-and-errors-use-error-v1",
                "id": "DOLV2-004",
                "check": "envelope-dispatch",
            },
            {"expected": "adapter-never-writes-worktree-state", "id": "DOLV2-005", "check": "migration-boundary"},
            {"expected": "completed-to-running-notice-refreshes-status", "id": "DOLV2-006", "check": "reactivation"},
            {
                "expected": "identity-revision-attempt-idempotency-detached-replay-and-job-invariants-retained",
                "id": "DOLV2-007",
                "check": "automation-invariants",
            },
            {
                "expected": "unsupported-peer-fails-explicitly-without-version-text-inference",
                "id": "DOLV2-008",
                "check": "unsupported-peer",
            },
            {
                "expected": "packaged-catalog-bytes-match-the-manifest-bound-source",
                "id": "DOLV2-009",
                "check": "packaged-byte-verification",
            },
        ],
        "contract_surface": {
            "authoring_diagnostics": diagnostic_catalog["diagnostics"],
            "routes": route_inventory,
            "runtime_errors": error_catalog["errors"],
            "schema_pins": schema_pins,
        },
        "current_surface_counts": {
            "authoring_diagnostics": 53,
            "routes": len(route_inventory),
            "runtime_errors": len(runtime_error_codes),
            "schemas": len(schema_pins),
        },
        "migration_boundary": {
            "adapter_database_access": "forbidden",
            "development_state_migration_promise": False,
            "released_workspace_upgrade": "transactional-empty-or-v2-only-predecessor-to-v4",
            "released_workspace_upgrade_owner": "podway-daemon",
            "legacy_procedure_state": "rejected-without-conversion-or-deletion",
            "v2_downgrade": "forbidden",
        },
        "product": "podway",
        "reactivation_notice": {
            "adapter_action": "invalidate-terminal-cache-and-refresh-session-status",
            "cancelled_session_reactivation": "forbidden",
            "lifecycle_transition": "completed-to-running",
            "notices": [
                {
                    "command": "session.rework",
                    "input_flag": None,
                    "result_pointer": "/result/reactivated",
                    "result_schema": "podway.rework-result/v1",
                    "trigger_value": True,
                },
                {
                    "command": "goal.revise",
                    "input_flag": "--reactivate",
                    "result_pointer": "/result/reactivated",
                    "result_schema": "podway.goal-revision-result/v1",
                    "trigger_value": True,
                },
            ],
        },
        "release_activation_owner": "V2REL-006",
        "schema": ADAPTER_CONTRACT_SCHEMA,
        "status": "prepared-not-released",
        "version": 2,
    }


def validate_adapter_contract(value: dict[str, Any]) -> None:
    expected = adapter_contract()
    if canonical_bytes(value) != canonical_bytes(expected):
        fail("Dolgorae v2 adapter contract is stale or does not match repository contracts")


def prepare_adapter_contract(output: Path) -> dict[str, Any]:
    value = adapter_contract()
    write_reviewable_json(output, value)
    return {"contract": str(output.resolve()), "mode": "prepare-adapter", "ok": True}


def packaged_adapter_contract(archive: Path) -> tuple[dict[str, Any], str]:
    source = require_regular_file(ADAPTER_CONTRACT_PATH, "Dolgorae v2 adapter contract")
    source_bytes = source.read_bytes()
    member_name = (
        f"{ARCHIVE_ROOT}/share/podway/release/"
        f"{ADAPTER_CONTRACT_PATH.name}"
    )
    try:
        with tarfile.open(archive, mode="r:gz") as opened:
            members = [member for member in opened.getmembers() if member.name == member_name]
            if len(members) != 1 or not members[0].isreg():
                fail("release archive does not contain one regular v2 adapter contract")
            extracted = opened.extractfile(members[0])
            if extracted is None:
                fail("release archive v2 adapter contract is unreadable")
            packaged_bytes = extracted.read(len(source_bytes) + 1)
    except (OSError, tarfile.TarError) as error:
        fail(f"cannot read release archive v2 adapter contract: {error}")
    if packaged_bytes != source_bytes:
        fail("release archive v2 adapter contract differs from the source contract")
    adapter = read_object(source, "Dolgorae v2 adapter contract")
    validate_adapter_contract(adapter)
    return adapter, f"sha256:{release_archive.sha256_file(source)}"


def create(output_directory: Path) -> dict[str, Any]:
    release_archive.require_native_host()
    if run_git("status", "--porcelain=v1", "--untracked-files=normal"):
        fail("Dolgorae handoff requires a clean Git worktree")
    commit = run_git("rev-parse", "HEAD")
    source_tree = run_git("rev-parse", "HEAD^{tree}")
    archive = output_directory / f"{ARCHIVE_ROOT}.tar.gz"
    checksum = output_directory / f"{archive.name}.sha256"
    provenance_path = output_directory / f"{ARCHIVE_ROOT}.provenance.json"
    require_regular_file(archive, "release archive")
    require_regular_file(checksum, "release archive checksum")
    require_regular_file(provenance_path, "release provenance")
    provenance = read_object(provenance_path, "release provenance")
    try:
        release_evidence.validate_provenance(
            provenance,
            version=VERSION,
            target=TARGET,
            commit=commit,
            tree=source_tree,
            conformance_result=release_evidence.PASSED,
        )
    except release_evidence.EvidenceError as error:
        fail(str(error))
    archive_digest = release_archive.sha256_file(archive)
    if checksum.read_text(encoding="utf-8") != f"{archive_digest}  {archive.name}\n":
        fail("release archive checksum does not match")
    if provenance.get("archive") != {"name": archive.name, "sha256": archive_digest}:
        fail("release provenance archive identity does not match")
    adapter, adapter_catalog_sha256 = packaged_adapter_contract(archive)
    handoff = handoff_from_provenance(
        provenance,
        provenance_path.name,
        release_archive.sha256_file(provenance_path),
        source_tree,
        adapter,
        adapter_catalog_sha256,
    )
    output = output_directory / f"{ARCHIVE_ROOT}.dolgorae-handoff.json"
    write_json(output, handoff)
    return {"handoff": str(output.resolve()), "mode": "create", "ok": True}


def self_test() -> dict[str, Any]:
    adapter = read_object(ADAPTER_CONTRACT_PATH, "Dolgorae v2 adapter contract")
    validate_adapter_contract(adapter)
    sentinels = 1
    try:
        logical_result_family_paths([None], "malformed result family inventory")
    except HandoffError:
        pass
    else:
        fail("adapter contract accepted a non-string result family path")
    sentinels += 1
    for mutation, label in (
        (
            lambda value: value["contract_surface"]["routes"].pop(),
            "route inventory drift",
        ),
        (
            lambda value: value["contract_surface"]["schema_pins"].pop(),
            "schema pin drift",
        ),
        (
            lambda value: value["contract_surface"]["runtime_errors"].pop(),
            "runtime error drift",
        ),
        (
            lambda value: value["contract_surface"]["authoring_diagnostics"].pop(),
            "authoring diagnostic drift",
        ),
        (
            lambda value: value["reactivation_notice"].update(
                {"adapter_action": "ignore"}
            ),
            "reactivation notice drift",
        ),
        (
            lambda value: value["migration_boundary"].update(
                {"adapter_database_access": "allowed"}
            ),
            "migration boundary drift",
        ),
        (
            lambda value: value["adapter_acceptance"].pop(),
            "adapter acceptance drift",
        ),
    ):
        changed = json.loads(json.dumps(adapter))
        mutation(changed)
        try:
            validate_adapter_contract(changed)
        except HandoffError:
            pass
        else:
            fail(f"adapter contract accepted {label}")
        sentinels += 1
    adapter_catalog_sha256 = f"sha256:{release_archive.sha256_file(ADAPTER_CONTRACT_PATH)}"
    provenance = {
        "archive": {"name": "podway.tar.gz", "sha256": "a" * 64},
        "artifact_class": "distribution",
        "binaries": {"podway": "b" * 64, "podwayd": "c" * 64},
        "build_identity": f"sha256:{'d' * 64}",
        "cargo_lock_sha256": "e" * 64,
        "contract_manifest_digest": f"sha256:{'f' * 64}",
        "contract_manifest_schema": "podway.contract-manifest/v1",
        "packaged_conformance": {
            "result": release_evidence.PASSED,
            "scenarios": release_evidence.PACKAGED_CONFORMANCE_SCENARIOS,
        },
        "product": release_evidence.PRODUCT,
        "release_gate": "make test + fuzzing: passed",
        "release_gate_result": "passed",
        "release_status": release_evidence.RELEASE_STATUS,
        "schema": release_evidence.PROVENANCE_SCHEMA,
        "source_commit": "1" * 40,
        "source_dirty": False,
        "source_tree": "3" * 40,
        "target": TARGET,
        "toolchain": "rustc 1.97.1 (test)",
        "version": VERSION,
    }
    first = handoff_from_provenance(provenance, "provenance.json", "2" * 64, "3" * 40)
    second = handoff_from_provenance(provenance, "provenance.json", "2" * 64, "3" * 40)
    if canonical_bytes(first) != canonical_bytes(second):
        fail("handoff encoding is not deterministic")
    sentinels += 1
    required = {
        "adapter",
        "adapter_catalog",
        "artifact",
        "binaries",
        "contract",
        "source",
        "toolchain",
    }
    if not required.issubset(first):
        fail("handoff omits a pinning identity")
    sentinels += len(required)
    release_evidence.validate_handoff(
        first,
        provenance,
        "provenance.json",
        "2" * 64,
        adapter,
        adapter_catalog_sha256,
    )
    adapter_tamper = json.loads(json.dumps(first))
    adapter_tamper["adapter"]["contract_surface"]["routes"].pop()
    try:
        release_evidence.validate_handoff(
            adapter_tamper,
            provenance,
            "provenance.json",
            "2" * 64,
            adapter,
            adapter_catalog_sha256,
        )
    except release_evidence.EvidenceError:
        pass
    else:
        fail("handoff accepted an incomplete v2 adapter route inventory")
    sentinels += 1
    pending = json.loads(json.dumps(provenance))
    pending["packaged_conformance"]["result"] = release_evidence.PENDING
    try:
        release_evidence.validate_provenance(
            pending,
            version=VERSION,
            target=TARGET,
            commit="1" * 40,
            tree="3" * 40,
            conformance_result=release_evidence.PASSED,
        )
    except release_evidence.EvidenceError:
        pass
    else:
        fail("handoff accepted pending packaged-conformance evidence")
    sentinels += 1
    with tempfile.TemporaryDirectory(prefix="podway-handoff-self-test-") as temporary_name:
        temporary = Path(temporary_name)
        victim = temporary / "victim.json"
        victim.write_text("do not replace\n", encoding="utf-8")
        output = temporary / "handoff.json"
        output.symlink_to(victim)
        write_json(output, first)
        if victim.read_text(encoding="utf-8") != "do not replace\n":
            fail("handoff publication followed an output symlink")
        if output.is_symlink() or output.read_bytes() != canonical_bytes(first):
            fail("handoff publication did not atomically replace the output path")
        sentinels += 1
        input_symlink = temporary / "archive.tar.gz"
        input_symlink.symlink_to(victim)
        try:
            require_regular_file(input_symlink, "self-test input")
        except HandoffError:
            pass
        else:
            fail("handoff accepted a symlink input")
        sentinels += 1
        non_utf8 = temporary / "non-utf8.json"
        non_utf8.write_bytes(b"\xff")
        try:
            read_object(non_utf8, "self-test non-UTF-8 input")
        except HandoffError:
            pass
        else:
            fail("handoff accepted non-UTF-8 JSON input")
        sentinels += 1

        member_name = (
            f"{ARCHIVE_ROOT}/share/podway/release/{ADAPTER_CONTRACT_PATH.name}"
        )
        source_bytes = ADAPTER_CONTRACT_PATH.read_bytes()

        def write_archive(
            path: Path, members: list[tuple[str, bytes | None]]
        ) -> None:
            with tarfile.open(path, mode="w:gz") as opened:
                for name, payload in members:
                    member = tarfile.TarInfo(name)
                    if payload is None:
                        member.type = tarfile.DIRTYPE
                        member.mode = 0o755
                        opened.addfile(member)
                    else:
                        member.size = len(payload)
                        member.mode = 0o644
                        opened.addfile(member, io.BytesIO(payload))

        valid_archive = temporary / "valid.tar.gz"
        write_archive(valid_archive, [(member_name, source_bytes)])
        packaged, packaged_digest = packaged_adapter_contract(valid_archive)
        if canonical_bytes(packaged) != canonical_bytes(adapter):
            fail("packaged adapter fixture did not preserve the source contract")
        if packaged_digest != adapter_catalog_sha256:
            fail("packaged adapter fixture did not preserve the source digest")
        sentinels += 1
        for name, members, label in (
            ("missing.tar.gz", [], "missing adapter member"),
            (
                "duplicate.tar.gz",
                [(member_name, source_bytes), (member_name, source_bytes)],
                "duplicate adapter member",
            ),
            ("non-regular.tar.gz", [(member_name, None)], "non-regular adapter member"),
            ("drift.tar.gz", [(member_name, source_bytes + b"x")], "adapter byte drift"),
        ):
            archive = temporary / name
            write_archive(archive, members)
            try:
                packaged_adapter_contract(archive)
            except HandoffError:
                pass
            else:
                fail(f"handoff accepted {label}")
            sentinels += 1
    return {"mode": "self-test", "ok": True, "sentinels": sentinels}


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="mode", required=True)
    subparsers.add_parser("self-test")
    prepare_parser = subparsers.add_parser("prepare-adapter")
    prepare_parser.add_argument("--output", type=Path, default=ADAPTER_CONTRACT_PATH)
    create_parser = subparsers.add_parser("create")
    create_parser.add_argument("--output-dir", type=Path, default=ROOT / "dist")
    arguments = parser.parse_args()
    try:
        if arguments.mode == "self-test":
            result = self_test()
        elif arguments.mode == "prepare-adapter":
            result = prepare_adapter_contract(arguments.output)
        else:
            result = create(arguments.output_dir)
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0
    except (
        HandoffError,
        KeyError,
        OSError,
        UnicodeError,
        release_archive.ReleaseError,
        repository_assets.AssetError,
    ) as error:
        print(
            json.dumps({"error": str(error), "mode": arguments.mode, "ok": False}, sort_keys=True, separators=(",", ":")),
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
