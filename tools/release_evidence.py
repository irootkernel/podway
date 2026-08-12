#!/usr/bin/env python3
"""Canonical Podway release-evidence shapes and atomic publication helpers."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import sys
import tempfile
from typing import Any


PRODUCT = "podway"
PROVENANCE_SCHEMA = "podway.release-provenance/v1"
HANDOFF_SCHEMA = "podway.dolgorae-compatibility-handoff/v2"
RELEASE_GATE = "make test + fuzzing: passed"
PASSED = "passed"
PENDING = "pending"
PACKAGED_CONFORMANCE_SCENARIOS = [
    "aut_t_id_custom_procedure_survives_restart_and_completes_the_fenced_lifecycle",
    "aut_t_id_and_recon_reject_conflicts_and_recover_an_admitted_timeout",
    "aut_t_recon_response_loss_is_reconciled_by_lookup_and_exact_replay",
    "aut_t_v2_public_admission_survives_restart_and_completes_rework_and_goal_closeout",
]
RELEASE_STATUS = {"notarization": "not-attempted", "signing": "unsigned"}
SHA256 = re.compile(r"[0-9a-f]{64}")
IDENTITY_SHA256 = re.compile(r"sha256:[0-9a-f]{64}")
GIT_OBJECT = re.compile(r"[0-9a-f]{40,64}")

PROVENANCE_KEYS = {
    "archive",
    "artifact_class",
    "binaries",
    "build_identity",
    "cargo_lock_sha256",
    "contract_manifest_digest",
    "contract_manifest_schema",
    "packaged_conformance",
    "product",
    "release_gate",
    "release_gate_result",
    "release_status",
    "schema",
    "source_commit",
    "source_dirty",
    "source_tree",
    "target",
    "toolchain",
    "version",
}
HANDOFF_KEYS = {
    "adapter",
    "adapter_catalog",
    "artifact",
    "binaries",
    "build_identity",
    "contract",
    "packaged_conformance",
    "product",
    "provenance",
    "release_gate",
    "release_gate_result",
    "release_status",
    "schema",
    "source",
    "target",
    "toolchain",
    "version",
}


class EvidenceError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise EvidenceError(message)


def canonical_bytes(value: dict[str, Any]) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_object(path: Path, label: str) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file: {path}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"{label} is not readable JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def atomic_write_json(path: Path, value: dict[str, Any]) -> None:
    if path.parent.is_symlink() or not path.parent.is_dir():
        fail(f"JSON output directory must be a regular directory: {path.parent}")
    descriptor, temporary_name = tempfile.mkstemp(
        dir=path.parent, prefix=f".{path.name}.", suffix=".tmp"
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as opened:
            opened.write(canonical_bytes(value))
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


def _exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    if set(value) != expected:
        fail(
            f"{label} fields mismatch: missing={sorted(expected - set(value))}, "
            f"unknown={sorted(set(value) - expected)}"
        )


def _exact_object(value: Any, expected: dict[str, Any], label: str) -> None:
    if json.dumps(value, sort_keys=True, separators=(",", ":")) != json.dumps(
        expected, sort_keys=True, separators=(",", ":")
    ):
        fail(f"{label} must equal {expected!r}")


def _digest(value: Any, label: str, *, prefixed: bool = False) -> str:
    pattern = IDENTITY_SHA256 if prefixed else SHA256
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        fail(f"{label} must be a lowercase SHA-256 identity")
    return value


def _string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{label} must be a non-empty string")
    return value


def validate_packaged_conformance(value: Any, expected_result: str) -> None:
    if expected_result not in {PENDING, PASSED}:
        fail(f"unsupported expected packaged-conformance result: {expected_result}")
    _exact_object(
        value,
        {"result": expected_result, "scenarios": PACKAGED_CONFORMANCE_SCENARIOS},
        "packaged_conformance",
    )


def validate_release_status(value: Any) -> None:
    _exact_object(value, RELEASE_STATUS, "release_status")


def validate_provenance(
    value: dict[str, Any],
    *,
    version: str,
    target: str,
    commit: str,
    tree: str,
    conformance_result: str,
) -> None:
    _exact_keys(value, PROVENANCE_KEYS, "provenance")
    expected = {
        "artifact_class": "distribution",
        "product": PRODUCT,
        "release_gate": RELEASE_GATE,
        "release_gate_result": PASSED,
        "schema": PROVENANCE_SCHEMA,
        "source_commit": commit,
        "source_dirty": False,
        "source_tree": tree,
        "target": target,
        "version": version,
    }
    mismatches = {
        key: {"expected": expected_value, "actual": value.get(key)}
        for key, expected_value in expected.items()
        if value.get(key) != expected_value
    }
    if mismatches:
        fail(f"provenance identity mismatch: {mismatches}")
    if value["source_dirty"] is not False:
        fail("provenance source_dirty must be the boolean false")
    if GIT_OBJECT.fullmatch(commit) is None or GIT_OBJECT.fullmatch(tree) is None:
        fail("qualified Git commit and tree must be lowercase object identities")
    archive = value["archive"]
    if not isinstance(archive, dict):
        fail("provenance archive must be an object")
    _exact_keys(archive, {"name", "sha256"}, "provenance archive")
    _string(archive["name"], "provenance archive name")
    _digest(archive["sha256"], "provenance archive digest")
    binaries = value["binaries"]
    if not isinstance(binaries, dict):
        fail("provenance binaries must be an object")
    _exact_keys(binaries, {"podway", "podwayd"}, "provenance binaries")
    for role in ("podway", "podwayd"):
        _digest(binaries[role], f"provenance {role} digest")
    _digest(value["build_identity"], "provenance build identity", prefixed=True)
    _digest(value["cargo_lock_sha256"], "provenance Cargo.lock digest")
    _digest(value["contract_manifest_digest"], "provenance manifest digest", prefixed=True)
    if value["contract_manifest_schema"] != "podway.contract-manifest/v1":
        fail("provenance contract manifest schema is invalid")
    _string(value["toolchain"], "provenance toolchain")
    validate_release_status(value["release_status"])
    validate_packaged_conformance(value["packaged_conformance"], conformance_result)


def handoff_from_provenance(
    provenance: dict[str, Any],
    provenance_name: str,
    provenance_sha256: str,
    adapter: dict[str, Any],
    adapter_catalog_sha256: str,
) -> dict[str, Any]:
    return {
        "adapter": adapter,
        "adapter_catalog": {
            "path": "release/dolgorae-v2-adapter-contract-v1.json",
            "sha256": adapter_catalog_sha256,
        },
        "artifact": provenance["archive"],
        "binaries": provenance["binaries"],
        "build_identity": provenance["build_identity"],
        "contract": {
            "digest": provenance["contract_manifest_digest"],
            "schema": provenance["contract_manifest_schema"],
        },
        "packaged_conformance": provenance["packaged_conformance"],
        "product": provenance["product"],
        "provenance": {"name": provenance_name, "sha256": provenance_sha256},
        "release_gate": provenance["release_gate"],
        "release_gate_result": provenance["release_gate_result"],
        "release_status": provenance["release_status"],
        "schema": HANDOFF_SCHEMA,
        "source": {
            "clean": not provenance["source_dirty"],
            "commit": provenance["source_commit"],
            "tree": provenance["source_tree"],
        },
        "target": provenance["target"],
        "toolchain": {
            "cargo_lock_sha256": provenance["cargo_lock_sha256"],
            "rustc": provenance["toolchain"],
        },
        "version": provenance["version"],
    }


def validate_handoff(
    value: dict[str, Any],
    provenance: dict[str, Any],
    provenance_name: str,
    provenance_sha256: str,
    adapter: dict[str, Any],
    adapter_catalog_sha256: str,
) -> None:
    _exact_keys(value, HANDOFF_KEYS, "handoff")
    expected = handoff_from_provenance(
        provenance,
        provenance_name,
        provenance_sha256,
        adapter,
        adapter_catalog_sha256,
    )
    if canonical_bytes(value) != canonical_bytes(expected):
        fail("handoff does not exactly and bidirectionally repeat provenance identities")


def mark_packaged_conformance_passed(path: Path, provenance: dict[str, Any]) -> dict[str, Any]:
    updated = dict(provenance)
    updated["packaged_conformance"] = {
        "result": PASSED,
        "scenarios": PACKAGED_CONFORMANCE_SCENARIOS,
    }
    atomic_write_json(path, updated)
    return updated


def self_test() -> dict[str, Any]:
    if len(PACKAGED_CONFORMANCE_SCENARIOS) != len(set(PACKAGED_CONFORMANCE_SCENARIOS)):
        fail("packaged conformance scenarios must be unique")
    if any(not name.startswith("aut_t_") for name in PACKAGED_CONFORMANCE_SCENARIOS):
        fail("packaged conformance scenarios must be acceptance tests")
    with tempfile.TemporaryDirectory(prefix="podway-evidence-self-test-") as name:
        root = Path(name)
        target = root / "evidence.json"
        target.write_text("original\n", encoding="utf-8")
        atomic_write_json(target, {"stable": True})
        if target.read_bytes() != canonical_bytes({"stable": True}):
            fail("atomic evidence publication produced unexpected bytes")
        victim = root / "victim"
        victim.write_text("unchanged\n", encoding="utf-8")
        target.unlink()
        target.symlink_to(victim)
        atomic_write_json(target, {"replacement": True})
        if victim.read_text(encoding="utf-8") != "unchanged\n" or target.is_symlink():
            fail("atomic evidence publication followed an output symlink")
    return {"mode": "self-test", "ok": True, "scenarios": len(PACKAGED_CONFORMANCE_SCENARIOS)}


def main() -> int:
    if sys.argv[1:] != ["self-test"]:
        print("usage: release_evidence.py self-test", file=sys.stderr)
        return 2
    try:
        result = self_test()
    except (EvidenceError, OSError, UnicodeError) as error:
        print(json.dumps({"error": str(error), "mode": "self-test", "ok": False}), file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
