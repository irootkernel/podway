#!/usr/bin/env python3
"""Independently verify the final local Podway release bundle."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from typing import Any

import qualify_distribution
import release_archive
import release_evidence


ROOT = Path(__file__).resolve().parents[1]
VERSION = release_archive.PRODUCT_VERSION
TARGET = release_archive.TARGET
ARCHIVE_ROOT = release_archive.ARCHIVE_ROOT
ADAPTER_CONTRACT_RELATIVE = Path("release/dolgorae-adapter-contract-v2.json")


class VerificationError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise VerificationError(message)


def git_identity(revision: str) -> str:
    completed = subprocess.run(
        ["git", "rev-parse", revision],
        cwd=ROOT,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        fail(f"cannot resolve Git identity {revision}: {completed.stderr.strip()}")
    return completed.stdout.strip()


def paths(output_directory: Path) -> tuple[Path, Path, Path, Path]:
    archive = output_directory / f"{ARCHIVE_ROOT}.tar.gz"
    return (
        archive,
        output_directory / f"{archive.name}.sha256",
        output_directory / f"{ARCHIVE_ROOT}.provenance.json",
        output_directory / f"{ARCHIVE_ROOT}.dolgorae-handoff.json",
    )


def require_regular(path: Path, label: str) -> None:
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file: {path}")


def verify_detached_checksum(archive: Path, checksum: Path) -> str:
    digest = release_archive.sha256_file(archive)
    if checksum.read_text(encoding="utf-8") != f"{digest}  {archive.name}\n":
        fail("detached checksum does not exactly identify the final archive")
    return digest


def is_managed_release_daemon_process(command: str, uid: int) -> bool:
    prefix = f"/private/tmp/podway-release-{uid}-"
    return (
        command.startswith(prefix)
        and "/snapshots/" in command
        and command.endswith("/podwayd --dev")
    )


def managed_release_daemon_processes(uid: int) -> list[str]:
    completed = subprocess.run(
        ["ps", "-Ao", "command="],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        fail("cannot inspect processes for leftover release qualification daemons")
    return [
        command
        for line in completed.stdout.splitlines()
        if is_managed_release_daemon_process((command := line.strip()), uid)
    ]


def verify(output_directory: Path, gate: str) -> dict[str, Any]:
    release_archive.require_native_host()
    release_archive.require_clean_tree(False)
    commit = git_identity("HEAD")
    tree = git_identity("HEAD^{tree}")
    archive, checksum, provenance_path, handoff_path = paths(output_directory)
    for path, label in (
        (archive, "release archive"),
        (checksum, "detached archive checksum"),
        (provenance_path, "release provenance"),
        (handoff_path, "Dolgorae handoff"),
    ):
        require_regular(path, label)
    archive_digest = verify_detached_checksum(archive, checksum)
    try:
        provenance = release_evidence.read_object(provenance_path, "release provenance")
        expected_schema = (
            release_evidence.PATCH_PROVENANCE_SCHEMA
            if gate == "patch"
            else release_evidence.PROVENANCE_SCHEMA
        )
        if provenance.get("schema") != expected_schema:
            fail(f"release provenance does not match requested {gate} gate")
        release_evidence.validate_provenance(
            provenance,
            version=VERSION,
            target=TARGET,
            commit=commit,
            tree=tree,
            conformance_result=(release_evidence.PASSED if gate == "full" else None),
        )
    except release_evidence.EvidenceError as error:
        fail(str(error))
    if provenance["archive"] != {"name": archive.name, "sha256": archive_digest}:
        fail("provenance does not identify the final archive")
    if provenance["cargo_lock_sha256"] != release_archive.sha256_file(ROOT / "Cargo.lock"):
        fail("provenance Cargo.lock digest does not match the qualified source")
    if provenance["release_status"] != release_archive.release_status():
        fail("provenance release status does not match release policy")
    adapter_path = ROOT / ADAPTER_CONTRACT_RELATIVE
    require_regular(adapter_path, "Dolgorae v2 adapter contract")
    adapter = release_evidence.read_object(adapter_path, "Dolgorae v2 adapter contract")
    adapter_catalog_sha256 = f"sha256:{release_archive.sha256_file(adapter_path)}"
    try:
        handoff = release_evidence.read_object(handoff_path, "Dolgorae handoff")
        release_evidence.validate_handoff(
            handoff,
            provenance,
            provenance_path.name,
            release_archive.sha256_file(provenance_path),
            adapter,
            adapter_catalog_sha256,
        )
    except release_evidence.EvidenceError as error:
        fail(str(error))

    with tempfile.TemporaryDirectory(prefix="pw-final-", dir="/tmp") as temporary_name:
        extraction = Path(temporary_name)
        extracted = qualify_distribution.safe_extract(archive, extraction)
        cli = release_archive.require_native_binary(extracted / "bin/podway", "podway")
        daemon = release_archive.require_native_binary(extracted / "bin/podwayd", "podwayd")
        receipt = release_archive.verify_release_contract(
            extracted / "share/podway", cli, daemon, commit
        )
        packaged_adapter = extracted / "share/podway" / ADAPTER_CONTRACT_RELATIVE
        require_regular(packaged_adapter, "packaged Dolgorae v2 adapter contract")
        if packaged_adapter.read_bytes() != adapter_path.read_bytes():
            fail("packaged Dolgorae v2 adapter contract differs from the qualified source")
        for field in (
            "build_identity",
            "contract_manifest_digest",
            "contract_manifest_schema",
            "source_commit",
            "target",
            "version",
        ):
            if receipt.get(field) != provenance[field]:
                fail(f"extracted contract receipt disagrees with provenance for {field}")
        for role, binary in (("podway", cli), ("podwayd", daemon)):
            if release_archive.sha256_file(binary) != provenance["binaries"][role]:
                fail(f"extracted {role} digest disagrees with provenance")
            if (
                release_archive.test_isolation_capability(binary)
                is not release_archive.TestIsolationCapability.DISABLED
            ):
                fail(f"extracted {role} exposes or ambiguously handles test isolation")
        if (
            release_archive.development_v2_admission_capability(daemon)
            is not release_archive.TestIsolationCapability.DISABLED
        ):
            fail(
                "extracted podwayd exposes or ambiguously handles the "
                "development-v2 admission unlock"
            )
        sockets = list(extraction.glob("**/podwayd.sock"))
        if sockets:
            fail(f"final verification left daemon sockets behind: {sockets}")
    if gate == "full":
        processes = managed_release_daemon_processes(os.geteuid())
        if processes:
            fail(f"managed release qualification daemons remain running: {processes}")
    result = {
        "archive": {"name": archive.name, "sha256": archive_digest},
        "binaries": provenance["binaries"],
        "build_identity": provenance["build_identity"],
        "contract_manifest_digest": provenance["contract_manifest_digest"],
        "mode": "verify",
        "ok": True,
        "provenance_sha256": release_archive.sha256_file(provenance_path),
        "handoff_sha256": release_archive.sha256_file(handoff_path),
        "source": {"commit": commit, "tree": tree},
        "tag_candidate": f"v{VERSION}",
    }
    if gate == "full":
        result["packaged_conformance"] = provenance["packaged_conformance"]
    return result


def fixture_provenance() -> dict[str, Any]:
    return {
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
        "release_gate": release_evidence.RELEASE_GATE,
        "release_gate_result": release_evidence.PASSED,
        "release_status": release_evidence.RELEASE_STATUS,
        "schema": release_evidence.PROVENANCE_SCHEMA,
        "source_commit": "1" * 40,
        "source_dirty": False,
        "source_tree": "2" * 40,
        "target": TARGET,
        "toolchain": "rustc 1.97.1 (fixture)",
        "version": VERSION,
    }


def fixture_patch_provenance() -> dict[str, Any]:
    provenance = fixture_provenance()
    provenance.pop("packaged_conformance")
    provenance["release_gate"] = release_evidence.PATCH_RELEASE_GATE
    provenance["schema"] = release_evidence.PATCH_PROVENANCE_SCHEMA
    return provenance


def expect_rejection(action: Any, label: str) -> None:
    try:
        action()
    except release_evidence.EvidenceError:
        return
    fail(f"final-verifier self-test accepted {label}")


def self_test() -> dict[str, Any]:
    provenance = fixture_provenance()
    release_evidence.validate_provenance(
        provenance,
        version=VERSION,
        target=TARGET,
        commit="1" * 40,
        tree="2" * 40,
        conformance_result=release_evidence.PASSED,
    )
    adapter = {"schema": "podway.dolgorae-adapter-contract/v2", "sentinel": True}
    adapter_catalog_sha256 = f"sha256:{'4' * 64}"
    handoff = release_evidence.handoff_from_provenance(
        provenance,
        "provenance.json",
        "3" * 64,
        adapter,
        adapter_catalog_sha256,
    )
    release_evidence.validate_handoff(
        handoff,
        provenance,
        "provenance.json",
        "3" * 64,
        adapter,
        adapter_catalog_sha256,
    )
    sentinels = 0
    patch_provenance = fixture_patch_provenance()
    release_evidence.validate_provenance(
        patch_provenance,
        version=VERSION,
        target=TARGET,
        commit="1" * 40,
        tree="2" * 40,
        conformance_result=None,
    )
    patch_handoff = release_evidence.handoff_from_provenance(
        patch_provenance,
        "provenance.json",
        "3" * 64,
        adapter,
        adapter_catalog_sha256,
    )
    if (
        patch_handoff.get("schema") != release_evidence.PATCH_HANDOFF_SCHEMA
        or "packaged_conformance" in patch_handoff
    ):
        fail("patch handoff contains full-gate conformance claims")
    release_evidence.validate_handoff(
        patch_handoff,
        patch_provenance,
        "provenance.json",
        "3" * 64,
        adapter,
        adapter_catalog_sha256,
    )
    sentinels += 2
    expect_rejection(
        lambda: release_evidence.validate_provenance(
            patch_provenance,
            version=VERSION,
            target=TARGET,
            commit="1" * 40,
            tree="2" * 40,
            conformance_result=release_evidence.PASSED,
        ),
        "patch provenance with a packaged-conformance claim",
    )
    sentinels += 1
    uid = os.geteuid()
    managed = f"/private/tmp/podway-release-{uid}-fixture/snapshots/digest/podwayd --dev"
    production = "/Users/example/.local/bin/podwayd --service"
    if not is_managed_release_daemon_process(managed, uid) or is_managed_release_daemon_process(
        production, uid
    ):
        fail("managed release daemon process classifier self-test failed")
    sentinels += 1
    for field, bad in (
        ("product", "another-product"),
        ("release_gate_result", "pending"),
        ("source_tree", "4" * 40),
        ("cargo_lock_sha256", "not-a-digest"),
        ("source_dirty", 0),
    ):
        changed = json.loads(json.dumps(provenance))
        changed[field] = bad
        expect_rejection(
            lambda changed=changed: release_evidence.validate_provenance(
                changed,
                version=VERSION,
                target=TARGET,
                commit="1" * 40,
                tree="2" * 40,
                conformance_result=release_evidence.PASSED,
            ),
            f"provenance {field} drift",
        )
        sentinels += 1
    for mutation, label in (
        (lambda value: value.pop("release_status"), "missing provenance field"),
        (lambda value: value.update({"unknown": True}), "unknown provenance field"),
        (
            lambda value: value["packaged_conformance"].update({"result": "pending"}),
            "pending provenance",
        ),
        (
            lambda value: value["packaged_conformance"]["scenarios"].reverse(),
            "scenario drift",
        ),
    ):
        changed = json.loads(json.dumps(provenance))
        mutation(changed)
        expect_rejection(
            lambda changed=changed: release_evidence.validate_provenance(
                changed,
                version=VERSION,
                target=TARGET,
                commit="1" * 40,
                tree="2" * 40,
                conformance_result=release_evidence.PASSED,
            ),
            label,
        )
        sentinels += 1
    for mutation, label in (
        (lambda value: value.pop("release_status"), "missing handoff field"),
        (lambda value: value["artifact"].update({"sha256": "9" * 64}), "archive drift"),
        (lambda value: value["provenance"].update({"sha256": "8" * 64}), "provenance drift"),
        (lambda value: value["packaged_conformance"].update({"result": "pending"}), "pending handoff"),
        (lambda value: value["adapter"].update({"sentinel": False}), "adapter drift"),
        (
            lambda value: value["adapter_catalog"].update({"sha256": f"sha256:{'5' * 64}"}),
            "adapter catalog drift",
        ),
    ):
        changed = json.loads(json.dumps(handoff))
        mutation(changed)
        expect_rejection(
            lambda changed=changed: release_evidence.validate_handoff(
                changed,
                provenance,
                "provenance.json",
                "3" * 64,
                adapter,
                adapter_catalog_sha256,
            ),
            label,
        )
        sentinels += 1
    with tempfile.TemporaryDirectory(prefix="podway-bundle-self-test-") as name:
        root = Path(name)
        archive = root / "archive.tar.gz"
        checksum = root / "archive.tar.gz.sha256"
        archive.write_bytes(b"archive bytes")
        digest = release_archive.sha256_file(archive)
        checksum.write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
        verify_detached_checksum(archive, checksum)
        archive.write_bytes(b"changed archive bytes")
        try:
            verify_detached_checksum(archive, checksum)
        except VerificationError:
            pass
        else:
            fail("final-verifier self-test accepted archive/checksum drift")
        sentinels += 1
    return {"mode": "self-test", "ok": True, "sentinels": sentinels}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="mode", required=True)
    subparsers.add_parser("self-test")
    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("--output-dir", type=Path, default=ROOT / "dist")
    verify_parser.add_argument("--gate", choices=("full", "patch"), default="full")
    arguments = parser.parse_args()
    try:
        result = (
            self_test()
            if arguments.mode == "self-test"
            else verify(arguments.output_dir, arguments.gate)
        )
    except (
        VerificationError,
        release_archive.ReleaseError,
        release_evidence.EvidenceError,
        tarfile.TarError,
        OSError,
        UnicodeError,
    ) as error:
        print(
            json.dumps({"error": str(error), "mode": arguments.mode, "ok": False}, sort_keys=True, separators=(",", ":")),
            file=sys.stderr,
        )
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
