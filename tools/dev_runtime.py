#!/usr/bin/env python3
"""Isolated contributor development runtime for Podway debug binaries."""

from __future__ import annotations

import argparse
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pwd
import secrets
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
from typing import Any, Callable

import release_archive
import release_evidence
from run_g005_vertical import cargo_target_directory


ROOT = Path(__file__).resolve().parents[1]
PINNED_RUST_TOOLCHAIN = "1.97.1"
SCHEMA = "podway.dev-runtime/v1"
METADATA_NAME = "runtime.json"
TMP_ROOT = Path("/private/tmp")
MACOS_UNIX_SOCKET_PATH_CAPACITY = 104
DIRECTORY_MODE = 0o700
FILE_MODE = 0o600
EXECUTABLE_MODE = 0o755
ACCOUNT_NAME = "account"
DEV_HOME_NAME = "dev"
SANDBOX_NAME = "sandbox"
SNAPSHOTS_NAME = "snapshots"
BUILD_TIMEOUT_SECONDS = 600
COMMAND_TIMEOUT_SECONDS = 60
DAEMON_READY_TIMEOUT_SECONDS = 15
FORBIDDEN_RUN_FLAGS = frozenset({"--socket", "--worktree", "--dev"})
DAEMON_LIFECYCLE_COMMANDS = frozenset(
    {"install", "uninstall", "start", "stop", "restart", "status", "logs"}
)


class DevRuntimeError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise DevRuntimeError(message)


def euid() -> int:
    return os.geteuid()


def canonical_checkout(path: Path | None = None) -> Path:
    checkout = (ROOT if path is None else path).resolve()
    if checkout.is_symlink() or not checkout.is_dir():
        fail(f"checkout path must be a real directory: {checkout}")
    return checkout


def checkout_digest(checkout: Path) -> str:
    return hashlib.sha256(os.fsencode(checkout.as_posix())).hexdigest()[:12]


def managed_root_for(checkout: Path, uid: int | None = None) -> Path:
    return TMP_ROOT / f"podway-dev-{euid() if uid is None else uid}-{checkout_digest(checkout)}"


def production_runtime_lock_path() -> Path:
    """Derive the effective-user production lock path without touching it."""
    account_home = Path(pwd.getpwuid(euid()).pw_dir)
    if not account_home.is_absolute():
        fail("effective account home must be an absolute path")
    return account_home / ".podway" / "run" / "podwayd.lock"


def layout_paths(root: Path) -> dict[str, Path]:
    return {
        "root": root,
        "account_root": root / ACCOUNT_NAME,
        "dev_home": root / DEV_HOME_NAME,
        "sandbox": root / SANDBOX_NAME,
        "snapshots": root / SNAPSHOTS_NAME,
        "metadata": root / METADATA_NAME,
        "lock": root / ACCOUNT_NAME / ".podway" / "run" / "podwayd.lock",
        "socket": root / DEV_HOME_NAME / "run" / "podwayd.sock",
    }


def expected_identity(checkout: Path, root: Path) -> dict[str, Any]:
    paths = layout_paths(root)
    return {
        "checkout": checkout.as_posix(),
        "uid": euid(),
        "root": root.as_posix(),
        "account_root": paths["account_root"].as_posix(),
        "dev_home": paths["dev_home"].as_posix(),
        "sandbox": paths["sandbox"].as_posix(),
    }


def assert_disjoint_from_production(root: Path) -> None:
    production_lock = production_runtime_lock_path()
    managed_lock = layout_paths(root)["lock"]
    for label, path in (("managed root", root), ("managed lock", managed_lock)):
        if path.as_posix() == production_lock.as_posix():
            fail(f"{label} collides with the production runtime lock path")
    try:
        production_lock.relative_to(root)
    except ValueError:
        pass
    else:
        fail("production runtime lock path is nested under the managed root")
    try:
        root.relative_to(production_lock)
    except ValueError:
        return
    fail("managed root is nested under the production runtime lock path")


def validate_socket_capacity(socket_path: Path) -> None:
    encoded = os.fsencode(socket_path.as_posix())
    if len(encoded) > MACOS_UNIX_SOCKET_PATH_CAPACITY:
        fail(
            "dev socket path exceeds macOS capacity "
            f"({len(encoded)} > {MACOS_UNIX_SOCKET_PATH_CAPACITY}): {socket_path}"
        )


def lstat_path(path: Path) -> os.stat_result:
    try:
        return path.lstat()
    except OSError as error:
        fail(f"cannot inspect path {path}: {error}")


def require_owned_directory(path: Path, *, label: str, uid: int) -> None:
    metadata = lstat_path(path)
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        fail(f"{label} must be a real directory: {path}")
    if metadata.st_uid != uid or stat.S_IMODE(metadata.st_mode) != DIRECTORY_MODE:
        fail(f"{label} must be current-user owned mode {DIRECTORY_MODE:04o}: {path}")


def require_owned_regular_file(path: Path, *, label: str, uid: int, mode: int) -> None:
    metadata = lstat_path(path)
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file: {path}")
    if metadata.st_uid != uid or stat.S_IMODE(metadata.st_mode) != mode:
        fail(f"{label} must be current-user owned mode {mode:04o}: {path}")


def is_exact_child(root: Path, candidate: Path) -> bool:
    try:
        candidate.relative_to(root)
    except ValueError:
        return False
    return candidate.parts[: len(root.parts)] == root.parts


def reject_dot_components(path: Path, *, label: str) -> None:
    if any(part in {".", ".."} for part in path.parts):
        fail(f"{label} rejects dot/dot-dot path components: {path}")


def require_trusted_snapshot_binary(root: Path, raw: str, *, label: str) -> Path:
    path = Path(raw)
    if not path.is_absolute():
        fail(f"{label} must be an absolute path: {path}")
    reject_dot_components(path, label=label)
    reject_dot_components(root, label="managed root")
    root_resolved = root.resolve()
    resolved = path.resolve()
    if not is_exact_child(root_resolved, resolved):
        fail(f"{label} escapes managed root after resolution: {path}")
    require_owned_regular_file(path, label=label, uid=euid(), mode=EXECUTABLE_MODE)
    if path.resolve() != resolved:
        fail(f"{label} path changed during trust validation: {path}")
    return path


def ensure_private_directory(path: Path, *, uid: int) -> Path:
    if path.exists():
        require_owned_directory(path, label="managed path", uid=uid)
        return path
    path.mkdir(mode=DIRECTORY_MODE, parents=False, exist_ok=False)
    os.chmod(path, DIRECTORY_MODE)
    require_owned_directory(path, label="managed path", uid=uid)
    return path


def walk_private_tree(root: Path, *, uid: int, repair_modes: bool) -> None:
    stack = [root]
    while stack:
        current = stack.pop()
        if current != root and not is_exact_child(root, current):
            fail(f"path escapes managed root: {current}")
        metadata = lstat_path(current)
        if stat.S_ISLNK(metadata.st_mode):
            fail(f"managed tree must not contain symlinks: {current}")
        if metadata.st_uid != uid:
            fail(f"managed tree node has unsafe owner: {current}")
        if stat.S_ISDIR(metadata.st_mode):
            if repair_modes:
                os.chmod(current, DIRECTORY_MODE)
            elif stat.S_IMODE(metadata.st_mode) != DIRECTORY_MODE:
                fail(f"managed directory must have mode {DIRECTORY_MODE:04o}: {current}")
            with os.scandir(current) as entries:
                stack.extend(Path(entry.path) for entry in entries)
            continue
        if stat.S_ISSOCK(metadata.st_mode):
            continue
        if not stat.S_ISREG(metadata.st_mode):
            fail(f"managed tree contains an unsupported node: {current}")
        mode = EXECUTABLE_MODE if metadata.st_mode & stat.S_IXUSR else FILE_MODE
        if repair_modes:
            os.chmod(current, mode)
        elif stat.S_IMODE(metadata.st_mode) not in {FILE_MODE, EXECUTABLE_MODE}:
            fail(
                f"managed file has unsupported mode "
                f"{stat.S_IMODE(metadata.st_mode):04o}: {current}"
            )


def audit_managed_tree(
    root: Path,
    *,
    expected: Path,
    uid: int,
    repair_modes: bool = False,
) -> dict[str, Path]:
    if root != expected:
        fail(f"managed root mismatch: expected {expected}, observed {root}")
    require_owned_directory(root, label="managed root", uid=uid)
    paths = layout_paths(root)
    validate_socket_capacity(paths["socket"])
    walk_private_tree(root, uid=uid, repair_modes=repair_modes)
    for key in ("account_root", "dev_home", "sandbox", "snapshots"):
        if paths[key].exists():
            require_owned_directory(paths[key], label=key.replace("_", " "), uid=uid)
    return paths


def ensure_managed_tree(checkout: Path) -> dict[str, Path]:
    uid = euid()
    tmp_meta = lstat_path(TMP_ROOT)
    if stat.S_ISLNK(tmp_meta.st_mode) or not stat.S_ISDIR(tmp_meta.st_mode):
        fail(f"temporary root must be a real directory: {TMP_ROOT}")
    root = managed_root_for(checkout, uid)
    assert_disjoint_from_production(root)
    validate_socket_capacity(layout_paths(root)["socket"])
    if root.exists():
        require_owned_directory(root, label="managed root", uid=uid)
    else:
        root.mkdir(mode=DIRECTORY_MODE, parents=False, exist_ok=False)
        os.chmod(root, DIRECTORY_MODE)
    paths = layout_paths(root)
    for key in ("account_root", "dev_home", "sandbox", "snapshots"):
        ensure_private_directory(paths[key], uid=uid)
        if not is_exact_child(root, paths[key]):
            fail(f"managed path escaped the root: {paths[key]}")
    # Debug account-root isolation needs synthetic ~/.podway before lock acquisition.
    ensure_private_directory(paths["account_root"] / ".podway", uid=uid)
    assert_disjoint_from_production(root)
    return paths


def prepare_managed_tree(checkout: Path) -> dict[str, Path]:
    paths = ensure_managed_tree(checkout)
    return audit_managed_tree(
        paths["root"],
        expected=managed_root_for(checkout),
        uid=euid(),
        repair_modes=True,
    )


def atomic_write_private_json(path: Path, value: dict[str, Any]) -> None:
    if path.parent.is_symlink() or not path.parent.is_dir():
        fail(f"metadata directory must be a regular directory: {path.parent}")
    descriptor, temporary_name = tempfile.mkstemp(
        dir=path.parent, prefix=f".{path.name}.", suffix=".tmp"
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as opened:
            opened.write(release_evidence.canonical_bytes(value))
            opened.flush()
            os.fsync(opened.fileno())
        os.chmod(temporary, FILE_MODE)
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    require_owned_regular_file(path, label="runtime metadata", uid=euid(), mode=FILE_MODE)


def read_metadata(path: Path) -> dict[str, Any]:
    require_owned_regular_file(path, label="runtime metadata", uid=euid(), mode=FILE_MODE)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"runtime metadata is not readable JSON: {error}")
    if not isinstance(value, dict) or value.get("schema") != SCHEMA:
        fail("runtime metadata schema is unsupported")
    return value


def isolation_environment(account_root: Path, dev_home: Path) -> dict[str, str]:
    return {
        "PATH": "/usr/bin:/bin",
        "PODWAY_TEST_ACCOUNT_ROOT": account_root.as_posix(),
        "PODWAY_DEV_HOME": dev_home.as_posix(),
    }


def pinned_cargo() -> Path:
    """Resolve the repository-pinned cargo even for direct python invocation."""
    completed = subprocess.run(
        ["rustup", "which", "--toolchain", PINNED_RUST_TOOLCHAIN, "cargo"],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        text=True,
    )
    if completed.returncode != 0 or not completed.stdout.strip():
        detail = completed.stderr.strip() or completed.stdout.strip() or "rustup lookup failed"
        fail(
            f"Rust {PINNED_RUST_TOOLCHAIN} cargo is required by rust-toolchain.toml; "
            f"install with `rustup toolchain install {PINNED_RUST_TOOLCHAIN} --profile minimal` "
            f"({detail})"
        )
    cargo = Path(completed.stdout.strip())
    if cargo.is_symlink() or not cargo.is_file():
        fail(f"pinned cargo must be a regular non-symlink file: {cargo}")
    probe = subprocess.run(
        [str(cargo), "--version"],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        text=True,
    )
    if probe.returncode != 0 or PINNED_RUST_TOOLCHAIN not in probe.stdout:
        fail(
            f"pinned cargo does not report Rust {PINNED_RUST_TOOLCHAIN}: "
            f"{probe.stdout.strip() or probe.stderr.strip()}"
        )
    return cargo


def build_debug_binaries() -> tuple[Path, Path]:
    cargo = pinned_cargo()
    environment = os.environ.copy()
    completed = subprocess.run(
        [
            str(cargo),
            "build",
            "--locked",
            "-p",
            "podway-cli",
            "--bin",
            "podway",
            "-p",
            "podway-daemon",
            "--bin",
            "podwayd",
        ],
        cwd=ROOT,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=BUILD_TIMEOUT_SECONDS,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        fail(f"debug binary build failed with exit {completed.returncode}: {detail}")
    target = cargo_target_directory()
    cli = target / "debug" / "podway"
    daemon = target / "debug" / "podwayd"
    for path, label in ((cli, "podway"), (daemon, "podwayd")):
        if path.is_symlink() or not path.is_file():
            fail(f"built {label} is missing or is a symlink: {path}")
        if (
            release_archive.test_isolation_capability(path)
            is not release_archive.TestIsolationCapability.ENABLED
        ):
            fail(f"built {label} lacks debug test-isolation capability: {path}")
    return cli.resolve(), daemon.resolve()


def snapshot_pair(
    paths: dict[str, Path],
    cli: Path,
    daemon: Path,
    checkout: Path,
) -> dict[str, Any]:
    cli_digest = release_archive.sha256_file(cli)
    daemon_digest = release_archive.sha256_file(daemon)
    snapshot_id = hashlib.sha256(f"{cli_digest}:{daemon_digest}".encode()).hexdigest()[:12]
    snapshot_dir = paths["snapshots"] / snapshot_id
    if snapshot_dir.exists():
        require_owned_directory(snapshot_dir, label="snapshot directory", uid=euid())
    else:
        ensure_private_directory(snapshot_dir, uid=euid())
    snapshotted_cli = snapshot_dir / "podway"
    snapshotted_daemon = snapshot_dir / "podwayd"
    if not snapshotted_cli.exists():
        release_archive.snapshot_executable(cli, snapshotted_cli, "podway")
        os.chmod(snapshotted_cli, EXECUTABLE_MODE)
    if not snapshotted_daemon.exists():
        release_archive.snapshot_executable(daemon, snapshotted_daemon, "podwayd")
        os.chmod(snapshotted_daemon, EXECUTABLE_MODE)
    for path, expected, label in (
        (snapshotted_cli, cli_digest, "podway"),
        (snapshotted_daemon, daemon_digest, "podwayd"),
    ):
        require_owned_regular_file(path, label=f"snapshot {label}", uid=euid(), mode=EXECUTABLE_MODE)
        actual = release_archive.sha256_file(path)
        if actual != expected:
            fail(f"snapshot {label} digest mismatch: expected {expected}, observed {actual}")
        if (
            release_archive.test_isolation_capability(path)
            is not release_archive.TestIsolationCapability.ENABLED
        ):
            fail(f"snapshot {label} lacks debug test-isolation capability")
    return {
        "schema": SCHEMA,
        "checkout": checkout.as_posix(),
        "uid": euid(),
        "root": paths["root"].as_posix(),
        "account_root": paths["account_root"].as_posix(),
        "dev_home": paths["dev_home"].as_posix(),
        "sandbox": paths["sandbox"].as_posix(),
        "snapshot": {
            "id": snapshot_id,
            "directory": snapshot_dir.as_posix(),
            "podway": snapshotted_cli.as_posix(),
            "podwayd": snapshotted_daemon.as_posix(),
            "podway_sha256": cli_digest,
            "podwayd_sha256": daemon_digest,
        },
    }


def endpoint_is_live(socket_path: Path) -> bool:
    if not socket_path.exists():
        return False
    metadata = lstat_path(socket_path)
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISSOCK(metadata.st_mode):
        return False
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.settimeout(0.2)
        client.connect(socket_path.as_posix())
    except OSError:
        return False
    finally:
        client.close()
    return True


def open_isolated_lock(lock_path: Path) -> int:
    """Open and exclusively lock the isolated daemon lock, creating it if needed.

    The caller must keep the returned descriptor open across rename-to-trash so the
    flock remains on the lock inode after the well-known path is vacated.
    """
    uid = euid()
    lock_parent = lock_path.parent
    ensure_private_directory(lock_parent.parent, uid=uid)  # .podway
    ensure_private_directory(lock_parent, uid=uid)  # run
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(lock_path, flags, FILE_MODE)
    except OSError as error:
        fail(f"cannot open isolated lock: {lock_path}: {error}")
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            fail(f"isolated lock must be a regular file: {lock_path}")
        if metadata.st_uid != uid:
            fail(f"isolated lock has unsafe owner: {lock_path}")
        os.fchmod(descriptor, FILE_MODE)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            if error.errno in {errno.EACCES, errno.EAGAIN}:
                fail("isolated daemon lock is held; stop the managed daemon before continuing")
            raise
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def prove_endpoint_idle(socket_path: Path) -> None:
    if endpoint_is_live(socket_path):
        fail(f"live endpoint still owns the managed socket: {socket_path}")


def prove_isolated_state_idle(paths: dict[str, Path]) -> None:
    descriptor = open_isolated_lock(paths["lock"])
    try:
        prove_endpoint_idle(paths["socket"])
    finally:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
        finally:
            os.close(descriptor)


def current_snapshot(paths: dict[str, Path], *, checkout: Path) -> dict[str, Any]:
    if not paths["metadata"].exists():
        fail("no managed snapshot metadata; run `python3 tools/dev_runtime.py daemon` first")
    metadata = read_metadata(paths["metadata"])
    expected = expected_identity(checkout, paths["root"])
    for key, value in expected.items():
        if metadata.get(key) != value:
            fail(
                f"runtime metadata {key} mismatch: "
                f"expected {value!r}, observed {metadata.get(key)!r}"
            )
    snapshot = metadata.get("snapshot")
    if not isinstance(snapshot, dict):
        fail("runtime metadata is missing snapshot identity")
    for key in ("podway", "podwayd", "directory", "podway_sha256", "podwayd_sha256"):
        if not isinstance(snapshot.get(key), str) or not snapshot[key]:
            fail(f"runtime metadata snapshot field is invalid: {key}")
    reject_dot_components(Path(snapshot["directory"]), label="snapshot directory")
    if not is_exact_child(paths["root"], Path(snapshot["directory"]).resolve()):
        fail("snapshot directory escapes the managed root")
    cli = require_trusted_snapshot_binary(
        paths["root"], snapshot["podway"], label="snapshot podway"
    )
    daemon = require_trusted_snapshot_binary(
        paths["root"], snapshot["podwayd"], label="snapshot podwayd"
    )
    if release_archive.sha256_file(cli) != snapshot["podway_sha256"]:
        fail("snapshot podway digest does not match metadata")
    if release_archive.sha256_file(daemon) != snapshot["podwayd_sha256"]:
        fail("snapshot podwayd digest does not match metadata")
    return metadata


def adopt_snapshot_when_idle(paths: dict[str, Path], metadata: dict[str, Any]) -> None:
    prove_isolated_state_idle(paths)
    atomic_write_private_json(paths["metadata"], metadata)


def run_git(sandbox: Path, arguments: list[str]) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ["git", *arguments],
        cwd=sandbox,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=COMMAND_TIMEOUT_SECONDS,
    )


def initialize_sandbox(sandbox: Path) -> None:
    uid = euid()
    require_owned_directory(sandbox, label="sandbox", uid=uid)
    git_dir = sandbox / ".git"
    if git_dir.exists():
        if git_dir.is_symlink():
            fail(f"sandbox .git must not be a symlink: {git_dir}")
        return
    for arguments in (
        ["init", "--quiet"],
        ["config", "user.email", "podway-dev-runtime@localhost"],
        ["config", "user.name", "Podway Dev Runtime"],
    ):
        completed = run_git(sandbox, arguments)
        if completed.returncode != 0:
            detail = completed.stderr.decode("utf-8", errors="replace").strip()
            fail(f"git {' '.join(arguments)} failed: {detail}")
    readme = sandbox / "README.md"
    if not readme.exists():
        readme.write_text("# Podway managed disposable sandbox\n", encoding="utf-8")
        os.chmod(readme, FILE_MODE)
    if run_git(sandbox, ["add", "README.md"]).returncode != 0:
        fail("git add failed while initializing the managed sandbox")
    completed = run_git(sandbox, ["commit", "--quiet", "-m", "Initialize managed Podway sandbox"])
    if completed.returncode != 0:
        status = run_git(sandbox, ["status", "--porcelain"])
        if status.returncode != 0 or status.stdout.strip():
            detail = completed.stderr.decode("utf-8", errors="replace").strip()
            fail(f"git commit failed while initializing the managed sandbox: {detail}")
    walk_private_tree(sandbox, uid=uid, repair_modes=True)


def reject_run_arguments(arguments: list[str]) -> None:
    if not arguments:
        fail("run requires podway arguments after `--`")
    for index, token in enumerate(arguments):
        if token in FORBIDDEN_RUN_FLAGS:
            fail(f"run rejects {token}; the managed runtime supplies isolation itself")
        if token.startswith("--socket=") or token.startswith("--worktree="):
            fail("run rejects explicit endpoint or worktree overrides")
        if token == "terminate":
            fail("run rejects terminate; stop the managed daemon instead")
        if token == "daemon" and (
            index + 1 >= len(arguments) or arguments[index + 1] in DAEMON_LIFECYCLE_COMMANDS
        ):
            fail("run rejects daemon lifecycle commands")


def run_snapshotted_cli(
    paths: dict[str, Path],
    metadata: dict[str, Any],
    arguments: list[str],
) -> subprocess.CompletedProcess[bytes]:
    cli = require_trusted_snapshot_binary(
        paths["root"], metadata["snapshot"]["podway"], label="snapshot podway"
    )
    return subprocess.run(
        [cli.as_posix(), "--dev", *arguments],
        cwd=paths["sandbox"],
        env=isolation_environment(paths["account_root"], paths["dev_home"]),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=COMMAND_TIMEOUT_SECONDS,
    )


def allocate_trash_path(root: Path) -> Path:
    parent = root.parent
    for _ in range(32):
        candidate = parent / f"{root.name}.t-{os.getpid()}-{secrets.token_hex(6)}"
        if candidate.exists():
            continue
        validate_socket_capacity(layout_paths(candidate)["socket"])
        return candidate
    fail("cannot allocate a unique non-existing trash path for cleanup")


def retire_managed_root_to_trash(root: Path) -> Path:
    """Rename the audited root to a unique trash path. Caller must hold the lock fd."""
    prove_endpoint_idle(layout_paths(root)["socket"])
    audit_managed_tree(root, expected=root, uid=euid(), repair_modes=False)
    trash = allocate_trash_path(root)
    try:
        os.rename(root, trash)
    except OSError as error:
        fail(f"cannot rename managed root for cleanup (deleted nothing): {error}")
    if root.exists():
        # Well-known name may already have been recreated; that tree must survive.
        pass
    if not trash.exists():
        fail(f"cleanup trash path missing after rename: {trash}")
    audit_managed_tree(trash, expected=trash, uid=euid(), repair_modes=False)
    prove_endpoint_idle(layout_paths(trash)["socket"])
    return trash


def delete_trash_tree(trash: Path) -> None:
    try:
        shutil.rmtree(trash)
    except OSError as error:
        fail(
            "failed to delete cleanup trash tree; recoverable path "
            f"{trash.as_posix()}: {error}"
        )
    if trash.exists():
        fail(f"cleanup trash tree still present; recoverable path {trash.as_posix()}")


def command_daemon() -> int:
    checkout = canonical_checkout()
    paths = prepare_managed_tree(checkout)
    prove_isolated_state_idle(paths)
    cli, daemon = build_debug_binaries()
    metadata = snapshot_pair(paths, cli, daemon, checkout)
    adopt_snapshot_when_idle(paths, metadata)
    snapshotted_daemon = require_trusted_snapshot_binary(
        paths["root"], metadata["snapshot"]["podwayd"], label="snapshot podwayd"
    )
    environment = isolation_environment(paths["account_root"], paths["dev_home"])
    os.chdir(paths["root"])
    os.execve(
        snapshotted_daemon.as_posix(),
        [snapshotted_daemon.as_posix(), "--dev"],
        environment,
    )
    fail("execve returned unexpectedly")
    return 1


def command_init() -> int:
    checkout = canonical_checkout()
    paths = prepare_managed_tree(checkout)
    metadata = current_snapshot(paths, checkout=checkout)
    initialize_sandbox(paths["sandbox"])
    completed = run_snapshotted_cli(paths, metadata, ["init"])
    audit_managed_tree(paths["root"], expected=paths["root"], uid=euid(), repair_modes=True)
    sys.stdout.buffer.write(completed.stdout)
    sys.stderr.buffer.write(completed.stderr)
    return completed.returncode


def command_run(arguments: list[str]) -> int:
    reject_run_arguments(arguments)
    checkout = canonical_checkout()
    paths = prepare_managed_tree(checkout)
    if not (paths["sandbox"] / ".git").exists():
        fail("managed sandbox is not initialized; run `python3 tools/dev_runtime.py init` first")
    metadata = current_snapshot(paths, checkout=checkout)
    completed = run_snapshotted_cli(paths, metadata, arguments)
    audit_managed_tree(paths["root"], expected=paths["root"], uid=euid(), repair_modes=True)
    sys.stdout.buffer.write(completed.stdout)
    sys.stderr.buffer.write(completed.stderr)
    return completed.returncode


def command_clean(*, yes: bool) -> int:
    if not yes:
        fail("clean requires --yes")
    checkout = canonical_checkout()
    expected = managed_root_for(checkout)
    if not expected.exists():
        print(json.dumps({"ok": True, "removed": False, "root": expected.as_posix()}, sort_keys=True))
        return 0
    assert_disjoint_from_production(expected)
    paths = audit_managed_tree(expected, expected=expected, uid=euid(), repair_modes=True)
    # Hold flock on the lock inode, rename the whole root to a unique trash path,
    # then delete only that trash tree. A recreated well-known root must survive.
    descriptor = open_isolated_lock(paths["lock"])
    trash: Path | None = None
    try:
        trash = retire_managed_root_to_trash(expected)
        delete_trash_tree(trash)
        trash = None
    finally:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
        finally:
            os.close(descriptor)
    result = {
        "ok": True,
        "removed": True,
        "root": expected.as_posix(),
        "recreated_root_preserved": expected.exists(),
    }
    print(json.dumps(result, sort_keys=True))
    return 0


def write_probe_script(path: Path, *, marker: str) -> None:
    token = release_archive.ISOLATION_PROBE_TOKEN
    path.write_text(
        "#!/bin/sh\n"
        f"# {marker}\n"
        f'if [ "$1" = "--podway-test-isolation-probe" ] && '
        f'[ "$PODWAY_TEST_ISOLATION_PROBE" = "{token}" ]; then\n'
        f'  printf "%s\\n" "{token}"\n'
        "  exit 0\n"
        "fi\n"
        "exit 1\n",
        encoding="utf-8",
    )
    os.chmod(path, EXECUTABLE_MODE)


def expect_failure(action: Callable[[], Any], fragment: str) -> None:
    try:
        action()
    except DevRuntimeError as error:
        if fragment not in str(error):
            fail(f"expected failure containing {fragment!r}, observed {error}")
        return
    fail(f"expected failure containing {fragment!r}")


def wait_for_socket(socket_path: Path, timeout_seconds: float) -> None:
    deadline = time.time() + timeout_seconds
    while time.time() < deadline:
        if endpoint_is_live(socket_path):
            return
        time.sleep(0.05)
    fail(f"daemon socket did not become ready: {socket_path}")


def start_isolated_daemon(
    daemon: Path,
    account_root: Path,
    dev_home: Path,
) -> subprocess.Popen[bytes]:
    for path in (account_root, account_root / ".podway", dev_home):
        path.mkdir(mode=DIRECTORY_MODE, parents=True, exist_ok=True)
        os.chmod(path, DIRECTORY_MODE)
    validate_socket_capacity(dev_home / "run" / "podwayd.sock")
    process = subprocess.Popen(
        [daemon.as_posix(), "--dev"],
        cwd=account_root,
        env=isolation_environment(account_root, dev_home),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    try:
        wait_for_socket(dev_home / "run" / "podwayd.sock", DAEMON_READY_TIMEOUT_SECONDS)
    except Exception:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        detail = b""
        if process.stderr is not None:
            detail = process.stderr.read()
        fail("failed to start isolated daemon: " + detail.decode("utf-8", errors="replace").strip())
    return process


def stop_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    process.send_signal(signal.SIGTERM)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def make_synthetic_checkout() -> Path:
    checkout = Path(f"/private/tmp/podway-dev-checkout-{os.getpid()}-{secrets.token_hex(4)}")
    checkout.mkdir(mode=DIRECTORY_MODE, parents=True, exist_ok=False)
    os.chmod(checkout, DIRECTORY_MODE)
    return checkout.resolve()


def prepare_synthetic_runtime(checkout: Path) -> dict[str, Path]:
    root = managed_root_for(checkout)
    assert_disjoint_from_production(root)
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(mode=DIRECTORY_MODE)
    os.chmod(root, DIRECTORY_MODE)
    paths = layout_paths(root)
    for key in ("account_root", "dev_home", "sandbox", "snapshots"):
        ensure_private_directory(paths[key], uid=euid())
    ensure_private_directory(paths["account_root"] / ".podway", uid=euid())
    return paths


def self_test_path_safety() -> int:
    sentinels = 0
    checkout = make_synthetic_checkout()
    try:
        root = managed_root_for(checkout)
        if root.parent != TMP_ROOT or len(checkout_digest(checkout)) != 12:
            fail("managed root derivation is invalid")
        assert_disjoint_from_production(root)
        if layout_paths(root)["lock"].as_posix() == production_runtime_lock_path().as_posix():
            fail("synthetic managed lock must stay disjoint from production")
        sentinels += 1

        paths = ensure_managed_tree(checkout)
        audit_managed_tree(paths["root"], expected=root, uid=euid())
        sentinels += 1

        symlink = paths["root"] / "escape-link"
        symlink.symlink_to("/etc")
        expect_failure(
            lambda: audit_managed_tree(paths["root"], expected=root, uid=euid()),
            "symlinks",
        )
        symlink.unlink()
        sentinels += 1

        if is_exact_child(paths["root"], (paths["root"] / "..").resolve()):
            fail("parent path must not count as an exact child")
        sentinels += 1

        os.chmod(paths["sandbox"], 0o755)
        expect_failure(
            lambda: audit_managed_tree(paths["root"], expected=root, uid=euid()),
            "mode",
        )
        os.chmod(paths["sandbox"], DIRECTORY_MODE)
        sentinels += 1

        long_dev = paths["root"] / ("d" * 80)
        expect_failure(
            lambda: validate_socket_capacity(long_dev / "run" / "podwayd.sock"),
            "capacity",
        )
        sentinels += 1

        escape = paths["root"] / "snapshots" / ".." / ".." / "outside" / "bin"
        expect_failure(
            lambda: require_trusted_snapshot_binary(
                paths["root"], escape.as_posix(), label="snapshot podway"
            ),
            "dot-dot",
        )
        sentinels += 1
    finally:
        managed = managed_root_for(checkout)
        if managed.exists():
            shutil.rmtree(managed, ignore_errors=True)
        shutil.rmtree(checkout, ignore_errors=True)
    return sentinels


def self_test_command_escape() -> int:
    sentinels = 0
    cases = [
        (["--socket", "/tmp/x.sock", "status"], "--socket"),
        (["--worktree", "/tmp/wt", "status"], "--worktree"),
        (["--dev", "status"], "--dev"),
        (["terminate"], "terminate"),
        (["daemon", "status"], "daemon lifecycle"),
        (["daemon", "install"], "daemon lifecycle"),
    ]
    for arguments, fragment in cases:
        expect_failure(lambda arguments=arguments: reject_run_arguments(arguments), fragment)
        sentinels += 1
    reject_run_arguments(["--json", "status"])
    sentinels += 1
    return sentinels


def self_test_snapshot_and_clean() -> int:
    sentinels = 0
    checkout = make_synthetic_checkout()
    root: Path | None = None
    try:
        paths = prepare_synthetic_runtime(checkout)
        root = paths["root"]
        source_dir = root / "build"
        ensure_private_directory(source_dir, uid=euid())
        cli_source = source_dir / "podway"
        daemon_source = source_dir / "podwayd"
        write_probe_script(cli_source, marker="cli-probe")
        write_probe_script(daemon_source, marker="daemon-probe")
        cli_digest = release_archive.sha256_file(cli_source)
        daemon_digest = release_archive.sha256_file(daemon_source)
        if cli_digest == daemon_digest:
            fail("snapshot self-test requires distinct CLI and daemon probe digests")
        metadata = snapshot_pair(paths, cli_source, daemon_source, checkout)
        snapshot = metadata["snapshot"]
        if Path(snapshot["podway"]) != paths["snapshots"] / snapshot["id"] / "podway":
            fail("metadata podway path does not identify the snapshot CLI")
        if Path(snapshot["podwayd"]) != paths["snapshots"] / snapshot["id"] / "podwayd":
            fail("metadata podwayd path does not identify the snapshot daemon")
        if snapshot["podway_sha256"] != cli_digest:
            fail("metadata lost the CLI digest")
        if snapshot["podwayd_sha256"] != daemon_digest:
            fail("metadata lost the daemon digest")
        adopt_snapshot_when_idle(paths, metadata)
        revalidated = current_snapshot(paths, checkout=checkout)["snapshot"]
        if revalidated["podway"] != snapshot["podway"] or revalidated["podwayd"] != snapshot["podwayd"]:
            fail("current_snapshot changed snapshot paths")
        if (
            revalidated["podway_sha256"] != cli_digest
            or revalidated["podwayd_sha256"] != daemon_digest
        ):
            fail("current_snapshot failed to revalidate both digests")
        sentinels += 1

        for key, wrong in (
            ("checkout", "/tmp/wrong-checkout"),
            ("uid", euid() + 1),
            ("root", (root / "mutated").as_posix()),
            ("account_root", (root / "wrong-account").as_posix()),
            ("dev_home", (root / "wrong-dev").as_posix()),
            ("sandbox", (root / "wrong-sandbox").as_posix()),
        ):
            tampered = json.loads(json.dumps(metadata))
            tampered[key] = wrong
            atomic_write_private_json(paths["metadata"], tampered)
            expect_failure(
                lambda key=key: current_snapshot(paths, checkout=checkout),
                key,
            )
        atomic_write_private_json(paths["metadata"], metadata)
        current_snapshot(paths, checkout=checkout)
        sentinels += 1

        holder = subprocess.Popen(
            [
                sys.executable,
                "-c",
                (
                    "import fcntl, os, sys, time\n"
                    "fd = os.open(sys.argv[1], os.O_RDWR)\n"
                    "fcntl.flock(fd, fcntl.LOCK_EX)\n"
                    "sys.stdout.write('ready\\n')\n"
                    "sys.stdout.flush()\n"
                    "time.sleep(30)\n"
                ),
                paths["lock"].as_posix(),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            assert holder.stdout is not None
            ready = holder.stdout.readline().strip()
            if ready != "ready" or holder.poll() is not None:
                detail = (holder.stderr.read() if holder.stderr else "") or holder.stdout.read()
                fail(f"lock holder process failed: {detail}")
            expect_failure(lambda: prove_isolated_state_idle(paths), "lock is held")
        finally:
            holder.kill()
            holder.wait(timeout=5)
        sentinels += 1

        marker = checkout / "keep-me"
        marker.write_text("keep\n", encoding="utf-8")
        descriptor = open_isolated_lock(paths["lock"])
        try:
            trash = retire_managed_root_to_trash(root)
            # Recreate the well-known root while the old inode remains locked/trashed.
            root.mkdir(mode=DIRECTORY_MODE)
            os.chmod(root, DIRECTORY_MODE)
            survivor = root / "survivor"
            survivor.write_text("alive\n", encoding="utf-8")
            os.chmod(survivor, FILE_MODE)
            delete_trash_tree(trash)
            if not survivor.exists():
                fail("recreated well-known root did not survive trash deletion")
            if not marker.exists():
                fail("cleanup disturbed the unrelated checkout marker")
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)
        sentinels += 1
    finally:
        if root is not None and root.exists():
            shutil.rmtree(root, ignore_errors=True)
        shutil.rmtree(checkout, ignore_errors=True)
    return sentinels


def self_test_dual_daemon(_cli: Path, daemon: Path) -> int:
    with tempfile.TemporaryDirectory(prefix="pw-dev-dual-", dir="/private/tmp") as name:
        root = Path(name)
        os.chmod(root, DIRECTORY_MODE)
        first_account, first_dev = root / "a1", root / "d1"
        second_account, second_dev = root / "a2", root / "d2"
        first = start_isolated_daemon(daemon, first_account, first_dev)
        second = start_isolated_daemon(daemon, second_account, second_dev)
        try:
            if not endpoint_is_live(first_dev / "run" / "podwayd.sock"):
                fail("first isolated daemon endpoint is not live")
            if not endpoint_is_live(second_dev / "run" / "podwayd.sock"):
                fail("second isolated daemon endpoint is not live")
            conflicting = subprocess.Popen(
                [daemon.as_posix(), "--dev"],
                cwd=first_account,
                env=isolation_environment(first_account, first_dev),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
            )
            try:
                conflict_code = conflicting.wait(timeout=5)
            except subprocess.TimeoutExpired:
                stop_process(conflicting)
                fail("duplicate isolated daemon did not exit while the lock was held")
            if conflict_code == 0:
                fail("duplicate isolated daemon unexpectedly acquired the same account root")
            stop_process(first)
            deadline = time.time() + 5
            while time.time() < deadline and endpoint_is_live(first_dev / "run" / "podwayd.sock"):
                time.sleep(0.05)
            if endpoint_is_live(first_dev / "run" / "podwayd.sock"):
                fail("stopped daemon is still live")
            if not endpoint_is_live(second_dev / "run" / "podwayd.sock"):
                fail("stopping one isolated daemon disturbed the other")
            if second.poll() is not None:
                fail("surviving isolated daemon process exited unexpectedly")
        finally:
            stop_process(first)
            stop_process(second)
    return 1


def self_test_toolchain_and_target_dir() -> int:
    cargo = pinned_cargo()
    if PINNED_RUST_TOOLCHAIN not in subprocess.run(
        [str(cargo), "--version"],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        text=True,
    ).stdout:
        fail("pinned cargo self-test observed the wrong toolchain")
    relative = cargo_target_directory()
    if not relative.is_absolute():
        fail("cargo target directory must resolve to an absolute path")
    previous = os.environ.get("CARGO_TARGET_DIR")
    try:
        os.environ["CARGO_TARGET_DIR"] = "custom-target"
        custom_relative = cargo_target_directory()
        if custom_relative != (ROOT / "custom-target").resolve():
            fail("relative CARGO_TARGET_DIR was not resolved from the checkout")
        os.environ["CARGO_TARGET_DIR"] = "/tmp/podway-custom-target-dir"
        custom_absolute = cargo_target_directory()
        if custom_absolute != Path("/tmp/podway-custom-target-dir").resolve():
            fail("absolute CARGO_TARGET_DIR was not preserved")
    finally:
        if previous is None:
            os.environ.pop("CARGO_TARGET_DIR", None)
        else:
            os.environ["CARGO_TARGET_DIR"] = previous
    return 2


def self_test() -> dict[str, Any]:
    sentinels = 0
    sentinels += self_test_toolchain_and_target_dir()
    sentinels += self_test_path_safety()
    sentinels += self_test_command_escape()
    sentinels += self_test_snapshot_and_clean()
    cli, daemon = build_debug_binaries()
    sentinels += self_test_dual_daemon(cli, daemon)
    return {"mode": "self-test", "ok": True, "sentinels": sentinels}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("daemon", help="build, snapshot, and exec the isolated debug daemon")
    subparsers.add_parser("init", help="initialize the managed disposable Git sandbox")
    run_parser = subparsers.add_parser(
        "run",
        help="invoke the snapshotted CLI against the managed sandbox",
    )
    run_parser.add_argument(
        "podway_args",
        nargs=argparse.REMAINDER,
        help="podway arguments; prefer `--` before the first flag",
    )
    clean_parser = subparsers.add_parser("clean", help="remove the managed runtime root")
    clean_parser.add_argument(
        "--yes",
        action="store_true",
        help="required confirmation that the managed root may be deleted",
    )
    subparsers.add_parser("self-test", help="run focused contributor-runtime sentinels")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    arguments = parser.parse_args(argv)
    try:
        if arguments.command == "daemon":
            return command_daemon()
        if arguments.command == "init":
            return command_init()
        if arguments.command == "run":
            podway_args = list(arguments.podway_args)
            if podway_args and podway_args[0] == "--":
                podway_args = podway_args[1:]
            return command_run(podway_args)
        if arguments.command == "clean":
            return command_clean(yes=arguments.yes)
        if arguments.command == "self-test":
            print(json.dumps(self_test(), sort_keys=True))
            return 0
        fail(f"unsupported command: {arguments.command}")
        return 2
    except DevRuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    except (OSError, subprocess.SubprocessError, release_archive.ReleaseError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
