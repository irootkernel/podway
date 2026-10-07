#!/usr/bin/env python3
"""Qualify the immutable packaged CLI on owner-private scratch worktrees."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
from typing import Any

import release_archive
import release_evidence


ROOT = Path(__file__).resolve().parents[1]
VERSION = release_archive.PRODUCT_VERSION
TARGET = release_archive.TARGET
ARCHIVE_ROOT = release_archive.ARCHIVE_ROOT
REQUIRED_TESTS = release_evidence.PACKAGED_CONFORMANCE_SCENARIOS
SCENARIO_TIMEOUT_SECONDS = 300


class QualificationError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise QualificationError(message)


def run(arguments: list[str], *, label: str, cwd: Path = ROOT,
    environment: dict[str, str] | None = None, timeout_seconds: float = SCENARIO_TIMEOUT_SECONDS,
    isolate_process_group: bool = True) -> subprocess.CompletedProcess[bytes]:
    if not isolate_process_group:
        fail("qualification children require an owned process group")
    try:
        completed = release_evidence.run_bounded(arguments, cwd=cwd, environment=environment,
            timeout_seconds=timeout_seconds, label=label)
    except release_evidence.EvidenceError as error:
        fail(str(error))
    if completed.returncode != 0:
        fail(f"{label} failed with exit {completed.returncode}: "
             f"stdout={completed.stdout[-65536:].decode('utf-8', errors='replace').strip()} "
             f"stderr={completed.stderr[-65536:].decode('utf-8', errors='replace').strip()}")
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


def source_tree() -> str:
    return run(["git", "rev-parse", "HEAD^{tree}"], label="source tree probe").stdout.decode().strip()


def expected_paths(output_directory: Path) -> tuple[Path, Path, Path]:
    archive = output_directory / f"{ARCHIVE_ROOT}.tar.gz"
    checksum = output_directory / f"{archive.name}.sha256"
    provenance = output_directory / f"{ARCHIVE_ROOT}.provenance.json"
    return archive, checksum, provenance


def verify_checksum(archive: Path, checksum: Path) -> str:
    release_archive.require_regular_file(archive, "release archive")
    if checksum.is_symlink() or not checksum.is_file():
        fail(f"checksum must be a regular non-symlink file: {checksum}")
    digest = release_archive.sha256_file(archive)
    if checksum.read_text(encoding="utf-8") != f"{digest}  {archive.name}\n":
        fail("distribution checksum does not match the archive")
    return digest


def safe_extract(archive_path: Path, destination: Path) -> Path:
    if destination.exists():
        if destination.is_symlink() or not destination.is_dir() or any(destination.iterdir()):
            fail("archive extraction requires an empty regular owned directory")
    else:
        destination.mkdir(mode=0o700, parents=True)
    snapshot = release_archive.snapshot_file(archive_path, destination / ".archive-input.tar.gz", "release archive")
    release_archive.inspect_archive(snapshot)
    with tarfile.open(snapshot, mode="r:gz") as archive:
        for member in archive:
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
            target.chmod(member.mode & ~0o222)
    return destination / ARCHIVE_ROOT


def verify_distribution(
    archive: Path,
    checksum: Path,
    provenance_path: Path,
    extraction: Path,
    commit: str,
    tree: str,
    gate: str,
) -> tuple[Path, dict[str, Any]]:
    provenance = read_json(provenance_path, "release provenance")
    release_archive.require_source_binding(provenance, gate)
    try:
        release_evidence.validate_provenance(
            provenance,
            version=VERSION,
            target=TARGET,
            commit=commit,
            tree=tree,
            conformance_result=release_evidence.PENDING,
        )
    except release_evidence.EvidenceError as error:
        fail(str(error))
    digest = verify_checksum(archive, checksum)
    if provenance.get("archive") != {"name": archive.name, "sha256": digest}:
        fail("release provenance archive identity does not match the checksum")
    extracted = safe_extract(archive, extraction)
    cli = release_archive.require_native_binary(extracted / "bin/podway", "podway")
    contract_receipt = release_archive.verify_release_contract(
        extracted / "share/podway", cli, commit
    )
    release_archive.verify_source_payloads(extracted, cli)
    for field in (
        "build_identity",
        "contract_manifest_digest",
        "contract_manifest_schema",
        "source_commit",
        "target",
        "version",
    ):
        if contract_receipt.get(field) != provenance.get(field):
            fail(f"packaged identity and provenance mismatch for {field}")
    for role, binary in (("podway", cli),):
        if provenance.get("binaries", {}).get(role) != release_archive.sha256_file(binary):
            fail(f"packaged {role} digest does not match provenance")
        if release_archive.test_isolation_capability(binary) is not release_archive.TestIsolationCapability.DISABLED:
            fail(f"packaged {role} exposes or ambiguously handles debug isolation")
    return cli, provenance


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


def require_fixture_locks_released(root: Path) -> None:
    for path in root.rglob("*.lock"):
        if path.is_symlink() or not path.is_file():
            fail(f"fixture lock is not a regular owned file: {path}")
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(descriptor, fcntl.LOCK_UN)
        except BlockingIOError:
            fail(f"fixture lock remains held: {path}")
        finally:
            os.close(descriptor)


def run_packaged_suite(root: Path, harness: Path, cli: Path) -> None:
    fixtures = root / "worktrees"
    fixtures.mkdir(mode=0o700)
    snapshot = release_archive.snapshot_executable(cli, root / "snapshots/podway", "podway")
    expected_digest = release_archive.sha256_file(cli)
    environment = {
        "PATH": "/usr/bin:/bin",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "TMPDIR": str(fixtures),
        "PODWAY_TEST_FIXTURE_ROOT": str(fixtures),
        "PODWAY_TEST_CLI_BINARY": str(snapshot),
        "PODWAY_RELEASE_TEST_BINARY": str(snapshot),
    }
    for test in REQUIRED_TESTS:
        completed = run(
            [str(harness), test, "--exact", "--nocapture", "--include-ignored", "--test-threads=1"],
            label=f"packaged direct CLI scenario {test}", cwd=root, environment=environment,
            timeout_seconds=SCENARIO_TIMEOUT_SECONDS, isolate_process_group=True,
        )
        output = completed.stdout.decode("utf-8", errors="strict")
        if f"test {test} ... ok" not in output or "1 passed; 0 failed;" not in output:
            fail(f"packaged suite did not execute exactly the required passing test: {test}")
        if release_archive.sha256_file(snapshot) != expected_digest:
            fail("packaged CLI snapshot changed during qualification")
    require_fixture_locks_released(fixtures)
    remaining = list(fixtures.iterdir())
    if remaining:
        fail(f"packaged scenarios did not clean exact owned worktrees: {remaining}")


def qualify(output_directory: Path, gate: str) -> dict[str, Any]:
    if gate not in {"full", "development"}:
        fail("packaged qualification requires full or development gate")
    commit = source_commit()
    tree = source_tree()
    archive, checksum, provenance_path = expected_paths(output_directory)
    release_archive.require_regular_file(provenance_path, "pending release provenance")
    pending_bytes = provenance_path.read_bytes()
    provenance_digest = release_archive.sha256_file(provenance_path)
    temporary = Path(tempfile.mkdtemp(prefix="podway-release-qa-", dir=release_archive.scratch_parent())).resolve()
    temporary.chmod(0o700)
    try:
        cli, provenance = verify_distribution(
            archive, checksum, provenance_path, temporary / "extracted", commit, tree, gate
        )
        harness = build_harness()
        run_packaged_suite(temporary, harness, cli)
        release_archive.require_source_binding(provenance, gate)
        if provenance_path.read_bytes() != pending_bytes or release_archive.sha256_file(provenance_path) != provenance_digest:
            fail("pending provenance changed during qualification")
        if verify_checksum(archive, checksum) != provenance["archive"]["sha256"]:
            fail("archive changed during qualification")
    except BaseException as error:
        raise QualificationError(f"{error}; recoverable qualification root: {temporary}") from error
    try:
        shutil.rmtree(temporary)
    except OSError as error:
        raise QualificationError(f"qualification cleanup failed; recoverable root: {temporary}: {error}") from error
    if temporary.exists():
        fail(f"qualification cleanup left owned root behind: {temporary}")
    passed = release_evidence.mark_packaged_conformance_passed(provenance_path, provenance)
    return {"archive": archive.name, "artifact_class": passed["artifact_class"],
        "build_identity": passed["build_identity"], "mode": "qualify", "ok": True,
        "publishable": gate != "development", "cleanup": True, "scenarios": REQUIRED_TESTS}


def self_test() -> dict[str, Any]:
    if len(REQUIRED_TESTS) != len(set(REQUIRED_TESTS)):
        fail("required packaged scenarios must be unique")
    if any("::" not in name for name in REQUIRED_TESTS):
        fail("required packaged scenarios must be acceptance tests")
    leaking_child = (
        "import subprocess,sys;"
        "subprocess.Popen(['/bin/sleep','60']);"
        "sys.exit(7)"
    )
    try:
        run(
            [sys.executable, "-c", leaking_child],
            label="leaking descendant sentinel",
            timeout_seconds=0.1,
            isolate_process_group=True,
        )
    except QualificationError as error:
        if "timed out after 0.1s" not in str(error):
            fail(f"leaking descendant sentinel failed incorrectly: {error}")
    else:
        fail("leaking descendant sentinel did not time out")
    with tempfile.TemporaryDirectory(prefix="podway-qualification-self-test-", dir=release_archive.scratch_parent()) as name:
        path = Path(name) / "provenance.json"
        pending = {
            "packaged_conformance": {
                "result": release_evidence.PENDING,
                "scenarios": REQUIRED_TESTS,
            }
        }
        release_evidence.atomic_write_json(path, pending)
        original = path.read_bytes()
        try:
            raise QualificationError("simulated packaged scenario failure")
        except QualificationError:
            pass
        if path.read_bytes() != original:
            fail("failed qualification changed pending provenance")
        passed = release_evidence.mark_packaged_conformance_passed(path, pending)
        if passed["packaged_conformance"]["result"] != release_evidence.PASSED:
            fail("successful qualification did not publish passed evidence")
    return {"mode": "self-test", "ok": True, "scenarios": len(REQUIRED_TESTS)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    qualify_parser = subparsers.add_parser("qualify")
    qualify_parser.add_argument("--output-dir", type=Path, required=True)
    qualify_parser.add_argument("--gate", choices=("full", "development"), default="full")
    subparsers.add_parser("self-test")
    arguments = parser.parse_args()
    try:
        result = self_test() if arguments.command == "self-test" else qualify(arguments.output_dir, arguments.gate)
    except (
        OSError,
        QualificationError,
        release_archive.ReleaseError,
        release_evidence.EvidenceError,
        tarfile.TarError,
        UnicodeError,
    ) as error:
        print(json.dumps({"error": str(error), "mode": arguments.command, "ok": False}, sort_keys=True, separators=(",", ":")))
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
