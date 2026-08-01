#!/usr/bin/env python3
"""Qualify the packaged release through the isolated foreground dev daemon."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
from typing import Any

import release_archive


ROOT = Path(__file__).resolve().parents[1]
VERSION = release_archive.PRODUCT_VERSION
TARGET = release_archive.TARGET
ARCHIVE_ROOT = release_archive.ARCHIVE_ROOT
REQUIRED_TESTS = [
    "aut_t_id_custom_procedure_survives_restart_and_completes_the_fenced_lifecycle",
    "aut_t_id_and_recon_reject_conflicts_and_recover_an_admitted_timeout",
    "aut_t_recon_response_loss_is_reconciled_by_lookup_and_exact_replay",
]


class QualificationError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise QualificationError(message)


def run(
    arguments: list[str],
    *,
    label: str,
    cwd: Path = ROOT,
    environment: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[bytes]:
    completed = subprocess.run(
        arguments,
        cwd=cwd,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        stdout = completed.stdout.decode("utf-8", errors="replace").strip()
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        fail(f"{label} failed with exit {completed.returncode}: stdout={stdout} stderr={stderr}")
    return completed


def read_json(path: Path, label: str) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file: {path}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"cannot read {label}: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def source_commit() -> str:
    release_archive.require_native_host()
    return run(["git", "rev-parse", "HEAD"], label="source commit probe").stdout.decode().strip()


def expected_paths(output_directory: Path) -> tuple[Path, Path, Path]:
    archive = output_directory / f"{ARCHIVE_ROOT}.tar.gz"
    checksum = output_directory / f"{archive.name}.sha256"
    provenance = output_directory / f"{ARCHIVE_ROOT}.provenance.json"
    return archive, checksum, provenance


def verify_checksum(archive: Path, checksum: Path) -> str:
    if checksum.is_symlink() or not checksum.is_file():
        fail(f"checksum must be a regular non-symlink file: {checksum}")
    digest = release_archive.sha256_file(archive)
    if checksum.read_text(encoding="utf-8") != f"{digest}  {archive.name}\n":
        fail("distribution checksum does not match the archive")
    return digest


def safe_extract(archive_path: Path, destination: Path) -> Path:
    release_archive.inspect_archive(archive_path)
    with tarfile.open(archive_path, mode="r:gz") as archive:
        for member in archive.getmembers():
            target = destination.joinpath(*Path(member.name).parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                target.chmod(0o755)
                continue
            source = archive.extractfile(member)
            if source is None:
                fail(f"archive member has no regular payload: {member.name}")
            target.parent.mkdir(parents=True, exist_ok=True)
            with target.open("xb") as output:
                shutil.copyfileobj(source, output)
            target.chmod(member.mode)
    return destination / ARCHIVE_ROOT


def json_identity(binary: Path, role: str) -> dict[str, Any]:
    arguments = [str(binary), "--json", "version"]
    if role == "podway":
        arguments.append("--identity")
    completed = run(arguments, label=f"{role} identity probe")
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        fail(f"{role} identity probe returned invalid JSON: {error}")
    if role == "podway":
        value = value.get("result") if isinstance(value, dict) else None
    if not isinstance(value, dict):
        fail(f"{role} identity probe did not return an object")
    return value


def verify_distribution(
    archive: Path,
    checksum: Path,
    provenance_path: Path,
    extraction: Path,
    commit: str,
) -> tuple[Path, Path, dict[str, Any]]:
    provenance = read_json(provenance_path, "release provenance")
    required = {
        "schema": "podway.release-provenance/v1",
        "artifact_class": "distribution",
        "release_gate": "make test: passed",
        "source_commit": commit,
        "source_dirty": False,
        "target": TARGET,
        "version": VERSION,
    }
    mismatches = {
        key: {"expected": expected, "actual": provenance.get(key)}
        for key, expected in required.items()
        if provenance.get(key) != expected
    }
    if mismatches:
        fail(f"release provenance mismatch: {mismatches}")
    digest = verify_checksum(archive, checksum)
    if provenance.get("archive") != {"name": archive.name, "sha256": digest}:
        fail("release provenance archive identity does not match the checksum")
    extracted = safe_extract(archive, extraction)
    cli = extracted / "bin/podway"
    daemon = extracted / "bin/podwayd"
    identities = {"podway": json_identity(cli, "podway"), "podwayd": json_identity(daemon, "podwayd")}
    for field in (
        "build_identity",
        "contract_manifest_digest",
        "contract_manifest_schema",
        "source_commit",
        "target",
        "version",
    ):
        if identities["podway"].get(field) != identities["podwayd"].get(field):
            fail(f"packaged binary identity mismatch for {field}")
        if identities["podway"].get(field) != provenance.get(field):
            fail(f"packaged identity and provenance mismatch for {field}")
    for role, binary in (("podway", cli), ("podwayd", daemon)):
        if provenance.get("binaries", {}).get(role) != release_archive.sha256_file(binary):
            fail(f"packaged {role} digest does not match provenance")
        if release_archive.test_isolation_capability(binary) is not release_archive.TestIsolationCapability.DISABLED:
            fail(f"packaged {role} exposes or ambiguously handles debug isolation")
    return cli, daemon, provenance


def build_harness() -> Path:
    completed = run(
        [
            "cargo",
            "test",
            "-p",
            "podway-cli",
            "--test",
            "e2e_suite",
            "--no-run",
            "--locked",
            "--message-format=json",
        ],
        label="distribution conformance harness build",
    )
    executable: Path | None = None
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = message.get("target", {})
        if (
            message.get("reason") == "compiler-artifact"
            and target.get("name") == "e2e_suite"
            and "test" in target.get("kind", [])
            and isinstance(message.get("executable"), str)
        ):
            executable = Path(message["executable"])
    if executable is None or executable.is_symlink() or not executable.is_file():
        fail("Cargo did not report the e2e_suite test executable")
    return executable.resolve()


def run_packaged_suite(root: Path, harness: Path, cli: Path, daemon: Path) -> None:
    home = root / "home"
    cases = root / "cases"
    root.mkdir(mode=0o700)
    home.mkdir(mode=0o700)
    cases.mkdir(mode=0o700)
    environment = {
        "PATH": "/usr/bin:/bin",
        "PODWAY_DISTRIBUTION_QUALIFICATION_ROOT": str(cases),
        "PODWAY_DISTRIBUTION_ACCOUNT_HOME": str(home),
        "PODWAY_TEST_CLI_BINARY": str(cli),
        "PODWAYD_TEST_BINARY": str(daemon),
    }
    for test in REQUIRED_TESTS:
        completed = run(
            [
                str(harness),
                f"e2e_dolgorae_conformance::{test}",
                "--exact",
                "--nocapture",
                "--include-ignored",
                "--test-threads=1",
            ],
            label=f"packaged dev-mode Dolgorae scenario {test}",
            cwd=root,
            environment=environment,
        )
        if test not in completed.stdout.decode("utf-8", errors="strict"):
            fail(f"packaged distribution suite omitted required test: {test}")
    remaining = list(root.glob("**/podwayd.sock"))
    if remaining:
        fail(f"packaged dev-mode suite left daemon sockets behind: {remaining}")


def qualify(output_directory: Path) -> dict[str, Any]:
    commit = source_commit()
    archive, checksum, provenance_path = expected_paths(output_directory)
    harness = build_harness()
    # Keep the account-derived dev socket below macOS's Unix-domain path limit.
    with tempfile.TemporaryDirectory(prefix="pw-rel10-", dir="/tmp") as temporary_name:
        temporary = Path(temporary_name)
        temporary.chmod(0o700)
        cli, daemon, provenance = verify_distribution(
            archive, checksum, provenance_path, temporary / "extracted", commit
        )
        run_packaged_suite(temporary / "runtime", harness, cli, daemon)
        return {
            "archive": archive.name,
            "build_identity": provenance["build_identity"],
            "mode": "qualify",
            "ok": True,
            "scenarios": REQUIRED_TESTS,
        }


def self_test() -> dict[str, Any]:
    if len(REQUIRED_TESTS) != len(set(REQUIRED_TESTS)):
        fail("required packaged scenarios must be unique")
    if any(not name.startswith("aut_t_") for name in REQUIRED_TESTS):
        fail("required packaged scenarios must be acceptance tests")
    return {"mode": "self-test", "ok": True, "scenarios": len(REQUIRED_TESTS)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    qualify_parser = subparsers.add_parser("qualify")
    qualify_parser.add_argument("--output-dir", type=Path, required=True)
    subparsers.add_parser("self-test")
    arguments = parser.parse_args()
    try:
        result = self_test() if arguments.command == "self-test" else qualify(arguments.output_dir)
    except (OSError, QualificationError, release_archive.ReleaseError, tarfile.TarError, UnicodeError) as error:
        print(json.dumps({"error": str(error), "mode": arguments.command, "ok": False}, sort_keys=True, separators=(",", ":")))
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
