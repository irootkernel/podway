#!/usr/bin/env python3
"""Qualify the native release archive in a disposable macOS account."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import stat
import subprocess
import tarfile
import tempfile
from typing import Any

import release_archive


ROOT = Path(__file__).resolve().parents[1]
VERSION = release_archive.PRODUCT_VERSION
TARGET = release_archive.TARGET
ARCHIVE_ROOT = release_archive.ARCHIVE_ROOT
ACCOUNT_PREFIX = "pwrel10"
QUALIFICATION_SCHEMA = "podway.distribution-qualification/v1"
REQUIRED_TESTS = [
    "aut_t_path_installs_explicit_sibling_and_path_daemons_from_a_sanitized_directory",
    "aut_t_obs_installed_service_returns_compact_quiescent_status_on_the_explicit_socket",
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
    allow_failure: bool = False,
) -> subprocess.CompletedProcess[bytes]:
    completed = subprocess.run(
        arguments,
        cwd=cwd,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0 and not allow_failure:
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


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_bytes(canonical_json(value))
    os.replace(temporary, path)


def require_clean_native_tree() -> str:
    release_archive.require_native_host()
    if os.geteuid() == 0:
        fail("distribution qualification must be orchestrated by the invoking non-root user")
    status = run(
        ["git", "status", "--porcelain=v1", "--untracked-files=normal"],
        label="Git worktree inspection",
    ).stdout
    if status:
        fail("distribution qualification requires a clean Git worktree")
    return run(["git", "rev-parse", "HEAD"], label="source commit probe").stdout.decode().strip()


def expected_paths(output_directory: Path) -> tuple[Path, Path, Path, Path]:
    archive = output_directory / f"{ARCHIVE_ROOT}.tar.gz"
    checksum = output_directory / f"{archive.name}.sha256"
    provenance = output_directory / f"{ARCHIVE_ROOT}.provenance.json"
    receipt = output_directory / f"{ARCHIVE_ROOT}.qualification.json"
    return archive, checksum, provenance, receipt


def verify_checksum(archive: Path, checksum: Path) -> str:
    if checksum.is_symlink() or not checksum.is_file():
        fail(f"checksum must be a regular non-symlink file: {checksum}")
    digest = release_archive.sha256_file(archive)
    expected = f"{digest}  {archive.name}\n"
    if checksum.read_text(encoding="utf-8") != expected:
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
    source_commit: str,
) -> tuple[dict[str, Any], dict[str, Any], Path, Path, str]:
    provenance = read_json(provenance_path, "release provenance")
    required = {
        "schema": "podway.release-provenance/v1",
        "artifact_class": "distribution",
        "release_gate": "make test: passed",
        "source_commit": source_commit,
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
    archive_digest = verify_checksum(archive, checksum)
    if provenance.get("archive") != {"name": archive.name, "sha256": archive_digest}:
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
    for field in ("build_identity", "contract_manifest_digest", "contract_manifest_schema", "source_commit", "target", "version"):
        if identities["podway"].get(field) != provenance.get(field):
            fail(f"packaged identity and provenance mismatch for {field}")
    for role, binary in (("podway", cli), ("podwayd", daemon)):
        if provenance.get("binaries", {}).get(role) != release_archive.sha256_file(binary):
            fail(f"packaged {role} digest does not match provenance")
        capability = release_archive.test_isolation_capability(binary)
        if capability is not release_archive.TestIsolationCapability.DISABLED:
            fail(f"packaged {role} exposes or ambiguously handles debug isolation")
    return provenance, identities["podway"], cli, daemon, archive_digest


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
        label="distribution qualification harness build",
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


def existing_accounts() -> dict[str, int]:
    completed = run(["/usr/bin/dscl", ".", "-list", "/Users", "UniqueID"], label="account inventory")
    accounts: dict[str, int] = {}
    for raw_line in completed.stdout.decode("utf-8", errors="strict").splitlines():
        fields = raw_line.split()
        if len(fields) == 2 and fields[1].isdigit():
            accounts[fields[0]] = int(fields[1])
    return accounts


def account_attribute(name: str, attribute: str) -> str | None:
    completed = run(
        ["/usr/bin/dscl", ".", "-read", f"/Users/{name}", attribute],
        label=f"disposable account {attribute} probe",
        allow_failure=True,
    )
    if completed.returncode != 0:
        return None
    output = completed.stdout.decode("utf-8", errors="strict").strip()
    prefix = f"{attribute}:"
    if not output.startswith(prefix):
        fail(f"disposable account {attribute} probe returned an unexpected record")
    return " ".join(output.removeprefix(prefix).split())


def account_exists(name: str) -> bool:
    return account_attribute(name, "RecordName") is not None


def allocate_account() -> tuple[str, int, Path]:
    accounts = existing_accounts()
    stale = sorted(name for name in accounts if re.fullmatch(rf"{ACCOUNT_PREFIX}[0-9a-f]{{6}}", name))
    if stale:
        fail(f"stale qualification accounts require explicit cleanup: {', '.join(stale)}")
    used = set(accounts.values())
    uid = next((candidate for candidate in range(550, 600) if candidate not in used), None)
    if uid is None:
        fail("no disposable qualification UID is available in the reserved 550-599 range")
    for _ in range(32):
        name = f"{ACCOUNT_PREFIX}{secrets.token_hex(3)}"
        if name not in accounts:
            return name, uid, Path("/Users") / name
    fail("could not allocate a unique disposable qualification account name")


def validate_account_target(name: str, uid: int, home: Path) -> None:
    if re.fullmatch(rf"{ACCOUNT_PREFIX}[0-9a-f]{{6}}", name) is None:
        fail(f"refusing unsafe qualification account name: {name}")
    if uid not in range(550, 600):
        fail(f"refusing unsafe qualification UID: {uid}")
    if home != Path("/Users") / name:
        fail(f"refusing unsafe qualification home: {home}")


def sudo(arguments: list[str], label: str, *, allow_failure: bool = False) -> subprocess.CompletedProcess[bytes]:
    return run(["/usr/bin/sudo", "-n", *arguments], label=label, allow_failure=allow_failure)


def create_account(name: str, uid: int, home: Path) -> None:
    validate_account_target(name, uid, home)
    record = f"/Users/{name}"
    attributes = (
        ("RecordName", name),
        ("RealName", "Podway REL10 Qualification"),
        ("UniqueID", str(uid)),
        ("PrimaryGroupID", "20"),
        ("NFSHomeDirectory", str(home)),
        ("UserShell", "/bin/zsh"),
        ("IsHidden", "1"),
    )
    for attribute, value in attributes:
        sudo(
            ["/usr/bin/dscl", ".", "-create", record, attribute, value],
            f"disposable account {attribute} creation",
        )
    sudo(["/usr/bin/dscl", ".", "-create", record, "Password", "*"], "disable disposable login")
    observed = existing_accounts().get(name)
    if observed != uid:
        fail(f"disposable account was not created with UID {uid}")
    sudo(["/usr/bin/install", "-d", "-o", name, "-g", "staff", "-m", "0700", str(home)], "prepare disposable home")
    sudo(["/bin/launchctl", "bootstrap", f"gui/{uid}"], "isolated GUI domain bootstrap")
    sudo(["/bin/launchctl", "print", f"gui/{uid}"], "isolated GUI domain probe")


def cleanup_account(name: str, uid: int, home: Path) -> None:
    validate_account_target(name, uid, home)
    errors: list[str] = []
    bootout = sudo(["/bin/launchctl", "bootout", f"gui/{uid}"], "isolated GUI domain removal", allow_failure=True)
    if bootout.returncode != 0 and sudo(["/bin/launchctl", "print", f"gui/{uid}"], "isolated GUI domain absence", allow_failure=True).returncode == 0:
        errors.append("isolated GUI domain remained after bootout")
    if account_exists(name):
        observed_uid = existing_accounts().get(name)
        marker = account_attribute(name, "RealName")
        if observed_uid not in (None, uid) or marker != "Podway REL10 Qualification":
            errors.append("refusing to delete a disposable account with unexpected identity")
        else:
            deleted = sudo(
                ["/usr/bin/dscl", ".", "-delete", f"/Users/{name}"],
                "disposable account deletion",
                allow_failure=True,
            )
            if deleted.returncode != 0 or account_exists(name):
                errors.append("disposable account deletion failed")
    if home.exists():
        removed = sudo(["/bin/rm", "-rf", str(home)], "disposable account home removal", allow_failure=True)
        if removed.returncode != 0 or home.exists():
            errors.append(f"disposable account home remained: {home}")
    if errors:
        fail("; ".join(errors))


def run_packaged_suite(
    account: str,
    uid: int,
    home: Path,
    harness: Path,
    cli: Path,
    daemon: Path,
) -> None:
    qualification_root = home / "qualification"
    arguments = [
        "/usr/bin/sudo",
        "-n",
        "-u",
        account,
        "/bin/launchctl",
        "asuser",
        str(uid),
        "/usr/bin/env",
        "-i",
        "PATH=/usr/bin:/bin",
        f"PODWAY_DISTRIBUTION_QUALIFICATION_ROOT={qualification_root}",
        f"PODWAY_DISTRIBUTION_ACCOUNT_HOME={home}",
        f"PODWAY_TEST_CLI_BINARY={cli}",
        f"PODWAYD_TEST_BINARY={daemon}",
        str(harness),
        "e2e_dolgorae_conformance::aut_t_",
        "--nocapture",
        "--include-ignored",
        "--test-threads=1",
    ]
    completed = run(arguments, label="packaged Dolgorae distribution suite", cwd=home)
    stdout = completed.stdout.decode("utf-8", errors="strict")
    for test in REQUIRED_TESTS:
        if test not in stdout:
            fail(f"packaged distribution suite omitted required test: {test}")


def qualify(output_directory: Path) -> dict[str, Any]:
    source_commit = require_clean_native_tree()
    archive, checksum, provenance_path, receipt_path = expected_paths(output_directory)
    harness = build_harness()
    with tempfile.TemporaryDirectory(prefix="podway-rel10-qualification-") as temporary_name:
        temporary = Path(temporary_name)
        temporary.chmod(0o755)
        provenance, identity, cli, daemon, archive_digest = verify_distribution(
            archive,
            checksum,
            provenance_path,
            temporary / "extracted",
            source_commit,
        )
        staged_harness = temporary / "e2e_suite"
        shutil.copyfile(harness, staged_harness)
        staged_harness.chmod(0o755)
        account, uid, home = allocate_account()
        validate_account_target(account, uid, home)
        print(
            json.dumps(
                {"account": account, "domain": f"gui/{uid}", "home": str(home), "phase": "create"},
                sort_keys=True,
                separators=(",", ":"),
            ),
            flush=True,
        )
        created = False
        suite_error: BaseException | None = None
        try:
            create_account(account, uid, home)
            created = True
            run_packaged_suite(account, uid, home, staged_harness, cli, daemon)
        except BaseException as error:
            suite_error = error
        cleanup_error: BaseException | None = None
        if created or account_exists(account):
            try:
                cleanup_account(account, uid, home)
            except BaseException as error:
                cleanup_error = error
        if suite_error is not None or cleanup_error is not None:
            fail(f"qualification failed: suite={suite_error!s}; cleanup={cleanup_error!s}")
        receipt = {
            "archive": {"name": archive.name, "sha256": archive_digest},
            "artifact_class": "distribution",
            "build_identity": identity["build_identity"],
            "contract_manifest_digest": provenance["contract_manifest_digest"],
            "contract_manifest_schema": provenance["contract_manifest_schema"],
            "isolation": {"account": "disposable", "launchd_domain": "gui/<uid>"},
            "ok": True,
            "scenarios": REQUIRED_TESTS,
            "schema": QUALIFICATION_SCHEMA,
            "source_commit": source_commit,
            "target": TARGET,
            "version": VERSION,
        }
        write_json(receipt_path, receipt)
        return {"mode": "qualify", "ok": True, "receipt": str(receipt_path.resolve())}


def self_test() -> dict[str, Any]:
    validate_account_target("pwrel10abcdef", 550, Path("/Users/pwrel10abcdef"))
    rejected = 0
    for name, uid, home in (
        ("draccoon", 550, Path("/Users/draccoon")),
        ("pwrel10abcdef", 501, Path("/Users/pwrel10abcdef")),
        ("pwrel10abcdef", 550, Path("/Users/draccoon")),
    ):
        try:
            validate_account_target(name, uid, home)
        except QualificationError:
            rejected += 1
        else:
            fail("unsafe disposable account target was accepted")
    sample = {"schema": QUALIFICATION_SCHEMA, "ok": True}
    if canonical_json(sample) != canonical_json(sample):
        fail("qualification JSON encoding is not deterministic")
    return {"mode": "self-test", "ok": True, "sentinels": rejected + 1}


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
