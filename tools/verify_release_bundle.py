#!/usr/bin/env python3
"""Independently verify the final local Podway release bundle."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
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
ADAPTER_CONTRACT_RELATIVE = Path("release/dolgorae-adapter-contract-v3.json")


class VerificationError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise VerificationError(message)


def git_identity(revision: str) -> str:
    return release_archive.run(["git", "rev-parse", revision], label="bundle Git identity probe").decode().strip()


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


def verify_release_status(provenance: dict[str, Any]) -> None:
    artifact_class = provenance["artifact_class"]
    if provenance["release_status"] != release_archive.release_status(artifact_class):
        fail("provenance release status does not match release policy")


def verify(output_directory: Path, gate: str) -> dict[str, Any]:
    release_archive.require_native_host()
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
    provenance_digest = release_archive.sha256_file(provenance_path)
    handoff_digest = release_archive.sha256_file(handoff_path)
    try:
        provenance = release_evidence.read_object(provenance_path, "release provenance")
        expected_schema = (
            release_evidence.PATCH_PROVENANCE_SCHEMA
            if gate == "patch"
            else release_evidence.PROVENANCE_SCHEMA
        )
        if provenance.get("schema") != expected_schema:
            fail(f"release provenance does not match requested {gate} gate")
        release_archive.require_source_binding(provenance, gate)
        release_evidence.validate_provenance(
            provenance,
            version=VERSION,
            target=TARGET,
            commit=commit,
            tree=tree,
            conformance_result=(None if gate == "patch" else release_evidence.PASSED),
        )
    except release_evidence.EvidenceError as error:
        fail(str(error))
    if provenance["archive"] != {"name": archive.name, "sha256": archive_digest}:
        fail("provenance does not identify the final archive")
    if provenance["cargo_lock_sha256"] != release_archive.sha256_file(ROOT / "Cargo.lock"):
        fail("provenance Cargo.lock digest does not match the qualified source")
    verify_release_status(provenance)
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
            provenance_digest,
            adapter,
            adapter_catalog_sha256,
        )
    except release_evidence.EvidenceError as error:
        fail(str(error))

    with tempfile.TemporaryDirectory(prefix="pw-final-", dir=release_archive.scratch_parent()) as temporary_name:
        extraction = Path(temporary_name)
        extracted = qualify_distribution.safe_extract(archive, extraction)
        cli = release_archive.require_native_binary(extracted / "bin/podway", "podway")
        receipt = release_archive.verify_release_contract(
            extracted / "share/podway", cli, commit
        )
        release_archive.verify_source_payloads(extracted, cli)
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
        for role, binary in (("podway", cli),):
            if release_archive.sha256_file(binary) != provenance["binaries"][role]:
                fail(f"extracted {role} digest disagrees with provenance")
            if (
                release_archive.test_isolation_capability(binary)
                is not release_archive.TestIsolationCapability.DISABLED
            ):
                fail(f"extracted {role} exposes or ambiguously handles test isolation")
    release_archive.require_source_binding(provenance, gate)
    if verify_detached_checksum(archive, checksum) != archive_digest:
        fail("archive changed during final verification")
    if release_archive.sha256_file(provenance_path) != provenance_digest or release_archive.sha256_file(handoff_path) != handoff_digest:
        fail("provenance or handoff changed during final verification")
    result = {
        "archive": {"name": archive.name, "sha256": archive_digest},
        "binaries": provenance["binaries"],
        "build_identity": provenance["build_identity"],
        "contract_manifest_digest": provenance["contract_manifest_digest"],
        "mode": "verify",
        "ok": True,
        "provenance_sha256": provenance_digest,
        "handoff_sha256": handoff_digest,
        "source": {"commit": commit, "tree": tree},
        "artifact_class": provenance["artifact_class"],
        "publishable": gate != "development",
    }
    if gate != "development":
        result["tag_candidate"] = f"v{VERSION}"
    if gate != "patch":
        result["packaged_conformance"] = provenance["packaged_conformance"]
    return result


def fixture_provenance() -> dict[str, Any]:
    return {
        "archive": {"name": "podway.tar.gz", "sha256": "a" * 64},
        "artifact_class": "distribution",
        "binaries": {"podway": "b" * 64},
        "build_identity": f"sha256:{'d' * 64}",
        "cargo_lock_sha256": "e" * 64,
        "contract_manifest_digest": f"sha256:{'f' * 64}",
        "contract_manifest_schema": "podway.contract-manifest/v2",
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
        "source_snapshot_sha256": "7" * 64,
        "target": TARGET,
        "toolchain": "rustc 1.97.1 (fixture)",
        "version": VERSION,
    }


def fixture_patch_provenance() -> dict[str, Any]:
    provenance = fixture_provenance()
    provenance.pop("packaged_conformance")
    provenance["patch_gate"] = {
        "prior_make_test_passed": True,
        "tested_baseline_commit": "5" * 40,
    }
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
    development = fixture_provenance()
    development["artifact_class"] = "development-qualification"
    development["source_dirty"] = True
    development["release_gate"] = release_evidence.DEVELOPMENT_GATE
    release_evidence.validate_provenance(development, version=VERSION, target=TARGET,
        commit="1" * 40, tree="2" * 40, conformance_result=release_evidence.PASSED)
    for field, bad in (("release_gate", release_evidence.RELEASE_GATE), ("artifact_class", "test-fixture"),
                       ("source_dirty", 1), ("source_snapshot_sha256", "not-a-digest")):
        changed = json.loads(json.dumps(development))
        changed[field] = bad
        expect_rejection(lambda changed=changed: release_evidence.validate_provenance(changed,
            version=VERSION, target=TARGET, commit="1" * 40, tree="2" * 40,
            conformance_result=release_evidence.PASSED), f"development {field} drift")
    observed_artifact_classes = []
    original_release_status = release_archive.release_status
    release_archive.release_status = lambda artifact_class: (
        observed_artifact_classes.append(artifact_class) or provenance["release_status"]
    )
    try:
        verify_release_status(provenance)
    finally:
        release_archive.release_status = original_release_status
    if observed_artifact_classes != ["distribution"]:
        fail("final verifier did not use the provenance artifact class")
    adapter = {"schema": "podway.dolgorae-adapter-contract/v3", "sentinel": True}
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
    sentinels = 6
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
        or patch_handoff.get("patch_gate") != patch_provenance["patch_gate"]
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
    for mutation, label in (
        (lambda value: value.pop("patch_gate"), "missing patch gate"),
        (
            lambda value: value["patch_gate"].update({"prior_make_test_passed": False}),
            "negative prior make-test confirmation",
        ),
        (
            lambda value: value["patch_gate"].update({"tested_baseline_commit": "HEAD~1"}),
            "symbolic tested baseline",
        ),
    ):
        changed = json.loads(json.dumps(patch_provenance))
        mutation(changed)
        expect_rejection(
            lambda changed=changed: release_evidence.validate_provenance(
                changed,
                version=VERSION,
                target=TARGET,
                commit="1" * 40,
                tree="2" * 40,
                conformance_result=None,
            ),
            label,
        )
        sentinels += 1
    changed_handoff = json.loads(json.dumps(patch_handoff))
    changed_handoff["patch_gate"]["tested_baseline_commit"] = "6" * 40
    expect_rejection(
        lambda: release_evidence.validate_handoff(
            changed_handoff,
            patch_provenance,
            "provenance.json",
            "3" * 64,
            adapter,
            adapter_catalog_sha256,
        ),
        "patch handoff baseline drift",
    )
    sentinels += 1
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
    for field, bad in (
        ("product", "another-product"),
        ("release_gate_result", "pending"),
        ("source_tree", "4" * 40),
        ("cargo_lock_sha256", "not-a-digest"),
        ("source_dirty", 0),
        ("source_snapshot_sha256", "not-a-digest"),
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
        (lambda value: value["source"].update({"snapshot_sha256": "0" * 64}), "source snapshot drift"),
        (lambda value: value.update({"artifact_class": "development-qualification"}), "artifact class drift"),
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
    with tempfile.TemporaryDirectory(prefix="podway-bundle-self-test-", dir=release_archive.scratch_parent()) as name:
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
    verify_parser.add_argument("--gate", choices=("full", "patch", "development"), default="full")
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
