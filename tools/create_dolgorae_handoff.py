#!/usr/bin/env python3
"""Create the deterministic Dolgorae compatibility-pinning handoff."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from typing import Any

import release_archive
import release_evidence


ROOT = Path(__file__).resolve().parents[1]
VERSION = release_archive.PRODUCT_VERSION
TARGET = release_archive.TARGET
ARCHIVE_ROOT = release_archive.ARCHIVE_ROOT
SCHEMA = release_evidence.HANDOFF_SCHEMA


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
    except (OSError, json.JSONDecodeError) as error:
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


def handoff_from_provenance(
    provenance: dict[str, Any], provenance_name: str, provenance_sha256: str, source_tree: str
) -> dict[str, Any]:
    if provenance.get("source_tree") != source_tree:
        fail("release provenance source tree does not match the qualified Git tree")
    return release_evidence.handoff_from_provenance(
        provenance, provenance_name, provenance_sha256
    )


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
    handoff = handoff_from_provenance(
        provenance,
        provenance_path.name,
        release_archive.sha256_file(provenance_path),
        source_tree,
    )
    output = output_directory / f"{ARCHIVE_ROOT}.dolgorae-handoff.json"
    write_json(output, handoff)
    return {"handoff": str(output.resolve()), "mode": "create", "ok": True}


def self_test() -> dict[str, Any]:
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
    required = {"artifact", "binaries", "contract", "source", "toolchain"}
    if not required.issubset(first):
        fail("handoff omits a pinning identity")
    release_evidence.validate_handoff(first, provenance, "provenance.json", "2" * 64)
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
        input_symlink = temporary / "archive.tar.gz"
        input_symlink.symlink_to(victim)
        try:
            require_regular_file(input_symlink, "self-test input")
        except HandoffError:
            pass
        else:
            fail("handoff accepted a symlink input")
    return {"mode": "self-test", "ok": True, "sentinels": len(required) + 4}


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="mode", required=True)
    subparsers.add_parser("self-test")
    create_parser = subparsers.add_parser("create")
    create_parser.add_argument("--output-dir", type=Path, default=ROOT / "dist")
    arguments = parser.parse_args()
    try:
        result = self_test() if arguments.mode == "self-test" else create(arguments.output_dir)
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0
    except (HandoffError, KeyError, OSError) as error:
        print(
            json.dumps({"error": str(error), "mode": arguments.mode, "ok": False}, sort_keys=True, separators=(",", ":")),
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
