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
import threading
import time
from typing import Any, Callable

import release_archive
import release_evidence
from run_g005_vertical import cargo_target_directory


ROOT = Path(__file__).resolve().parents[1]
PINNED_RUST_TOOLCHAIN = "1.97.1"
SCHEMA = "podway.dev-runtime/v1"
V2REL003_QUALIFICATION_SCHEMA = "podway.v2rel003-native-qualification/v1"
IPC_MAX_PAYLOAD_BYTES = 1_048_576
METADATA_NAME = "runtime.json"
DEVELOPMENT_V2_MARKER_NAME = "development-v2.marker"
DEVELOPMENT_V2_MARKER_SCHEMA = "podway.disposable-development-workspace/v1"
DEVELOPMENT_V2_FEATURE = "development-v2-admission"
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
        "development_v2_marker": (
            root / SANDBOX_NAME / ".podway" / "runtime" / DEVELOPMENT_V2_MARKER_NAME
        ),
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


def atomic_write_private_json(
    path: Path, value: dict[str, Any], *, trailing_newline: bool = True
) -> None:
    if path.parent.is_symlink() or not path.parent.is_dir():
        fail(f"metadata directory must be a regular directory: {path.parent}")
    descriptor, temporary_name = tempfile.mkstemp(
        dir=path.parent, prefix=f".{path.name}.", suffix=".tmp"
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as opened:
            payload = release_evidence.canonical_bytes(value)
            opened.write(payload if trailing_newline else payload.removesuffix(b"\n"))
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
            "--features",
            f"podway-daemon/{DEVELOPMENT_V2_FEATURE}",
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
    if (
        release_archive.development_v2_admission_capability(daemon)
        is not release_archive.TestIsolationCapability.ENABLED
    ):
        fail(f"built podwayd lacks {DEVELOPMENT_V2_FEATURE} capability: {daemon}")
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
    if (
        release_archive.development_v2_admission_capability(snapshotted_daemon)
        is not release_archive.TestIsolationCapability.ENABLED
    ):
        fail(f"snapshot podwayd lacks {DEVELOPMENT_V2_FEATURE} capability")
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


def require_qualification_binary(raw: str, *, label: str) -> Path:
    path = Path(raw)
    if not path.is_absolute():
        fail(f"{label} must be an absolute path: {path}")
    reject_dot_components(path, label=label)
    resolved = path.resolve()
    if resolved != path or path.is_symlink():
        fail(f"{label} must be a canonical non-symlink path: {path}")
    metadata = lstat_path(path)
    if not stat.S_ISREG(metadata.st_mode) or not metadata.st_mode & stat.S_IXUSR:
        fail(f"{label} must be an executable regular file: {path}")
    return path


def run_cli_binary(
    cli: Path,
    *,
    worktree: Path,
    account_root: Path,
    dev_home: Path,
    arguments: list[str],
) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        [cli.as_posix(), "--dev", "--json", *arguments],
        cwd=worktree,
        env=isolation_environment(account_root, dev_home),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=COMMAND_TIMEOUT_SECONDS,
    )


def decode_cli_json(
    completed: subprocess.CompletedProcess[bytes],
    *,
    label: str,
    expected_code: int | None = 0,
) -> dict[str, Any]:
    if expected_code is not None and completed.returncode != expected_code:
        fail(
            f"{label} exited {completed.returncode}: "
            f"stdout={completed.stdout.decode('utf-8', errors='replace')[:2000]!r}; "
            f"stderr={completed.stderr.decode('utf-8', errors='replace')[:2000]!r}"
        )
    try:
        value = json.loads(completed.stdout)
    except (UnicodeError, json.JSONDecodeError) as error:
        fail(f"{label} returned invalid JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{label} returned a non-object JSON value")
    return value


def qualification_command(
    cli: Path,
    paths: dict[str, Path],
    arguments: list[str],
    *,
    label: str,
    expected_code: int | None = 0,
) -> dict[str, Any]:
    return decode_cli_json(
        run_cli_binary(
            cli,
            worktree=paths["sandbox"],
            account_root=paths["account_root"],
            dev_home=paths["dev_home"],
            arguments=arguments,
        ),
        label=label,
        expected_code=expected_code,
    )


def require_output_result(
    envelope: dict[str, Any], *, command: str, result_schema: str
) -> dict[str, Any]:
    if envelope.get("command") != command or envelope.get("schema") not in {
        "podway.output/v1",
        "podway.output/v2",
    }:
        fail(f"{command} returned the wrong output envelope")
    result = envelope.get("result")
    if not isinstance(result, dict) or result.get("schema") != result_schema:
        fail(
            f"{command} returned the wrong result schema: "
            f"{result.get('schema') if isinstance(result, dict) else None}"
        )
    return result


def require_error_code(envelope: dict[str, Any], expected: str, *, label: str) -> None:
    if envelope.get("schema") not in {"podway.error/v1", "podway.error/v2"}:
        fail(f"{label} did not return an error envelope")
    observed = envelope.get("code")
    if observed != expected:
        fail(f"{label} returned {observed!r}, expected {expected!r}")


def response_loss_relay(
    proxy_socket: Path, daemon_socket: Path
) -> tuple[threading.Thread, dict[str, bytes | BaseException]]:
    """Forward one request, consume one response, and deliberately discard it."""
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(proxy_socket.as_posix())
    os.chmod(proxy_socket, FILE_MODE)
    listener.listen(1)
    outcome: dict[str, bytes | BaseException] = {}

    def read_single_frame(stream: socket.socket, *, label: str) -> bytes:
        wire = bytearray()
        while len(wire) < 4:
            chunk = stream.recv(4 - len(wire))
            if not chunk:
                fail(f"{label} ended before its frame prefix")
            wire.extend(chunk)
        payload_length = int.from_bytes(wire, "big")
        if payload_length > IPC_MAX_PAYLOAD_BYTES:
            fail(f"{label} exceeds the IPC payload limit: {payload_length}")
        while len(wire) < payload_length + 4:
            chunk = stream.recv(min(64 * 1024, payload_length + 4 - len(wire)))
            if not chunk:
                fail(f"{label} ended before its declared payload")
            wire.extend(chunk)
        if stream.recv(1):
            fail(f"{label} contains trailing bytes after one frame")
        return bytes(wire)

    def relay() -> None:
        try:
            listener.settimeout(10)
            downstream, _ = listener.accept()
            with downstream:
                downstream.settimeout(10)
                request = read_single_frame(downstream, label="response-loss request")
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as upstream:
                    upstream.settimeout(10)
                    upstream.connect(daemon_socket.as_posix())
                    upstream.sendall(request)
                    upstream.shutdown(socket.SHUT_WR)
                    response = read_single_frame(upstream, label="response-loss response")
                outcome["request"] = request
                outcome["response"] = response
                # Closing downstream without forwarding response creates the exact response-loss
                # boundary observed by the CLI.
        except BaseException as error:
            outcome["error"] = error
        finally:
            listener.close()

    thread = threading.Thread(target=relay, name="v2rel003-response-loss", daemon=True)
    thread.start()
    return thread, outcome


def publish_development_v2_marker(
    paths: dict[str, Path], metadata: dict[str, Any]
) -> None:
    atomic_write_private_json(
        paths["development_v2_marker"],
        {
            "schema": DEVELOPMENT_V2_MARKER_SCHEMA,
            "feature": DEVELOPMENT_V2_FEATURE,
            "uid": euid(),
            "managed_root": paths["root"].as_posix(),
            "account_root": paths["account_root"].as_posix(),
            "dev_home": paths["dev_home"].as_posix(),
            "workspace_root": paths["sandbox"].as_posix(),
            "socket_path": paths["socket"].as_posix(),
            "state_directory": (paths["dev_home"] / "state").as_posix(),
            "daemon_path": metadata["snapshot"]["podwayd"],
            "daemon_sha256": metadata["snapshot"]["podwayd_sha256"],
        },
        # The daemon verifies this trust-boundary file against podway-core's exact canonical JSON,
        # which deliberately has no record-separating newline.
        trailing_newline=False,
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
    if completed.returncode == 0:
        publish_development_v2_marker(paths, metadata)
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


def write_probe_script(path: Path, *, marker: str, development_v2: bool = False) -> None:
    token = release_archive.ISOLATION_PROBE_TOKEN
    script = (
        "#!/bin/sh\n"
        f"# {marker}\n"
        f'if [ "$1" = "--podway-test-isolation-probe" ] && '
        f'[ "$PODWAY_TEST_ISOLATION_PROBE" = "{token}" ]; then\n'
        f'  printf "%s\\n" "{token}"\n'
        "  exit 0\n"
        "fi\n"
    )
    if development_v2:
        script += (
            f'if [ "$1" = "{release_archive.DEVELOPMENT_V2_PROBE_ARGUMENT}" ] && '
            f'[ "${release_archive.DEVELOPMENT_V2_PROBE_ENV}" = '
            f'"{release_archive.DEVELOPMENT_V2_PROBE_TOKEN}" ]; then\n'
            f'  printf "%s\\n" "{release_archive.DEVELOPMENT_V2_PROBE_TOKEN}"\n'
            "  exit 0\n"
            "fi\n"
        )
    path.write_text(
        script + "exit 1\n",
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


def dogfood_json_command(
    paths: dict[str, Path],
    metadata: dict[str, Any],
    arguments: list[str],
    *,
    command: str,
    output_schema: str,
    result_schema: str,
) -> dict[str, Any]:
    completed = run_snapshotted_cli(paths, metadata, ["--json", *arguments])
    if completed.returncode != 0:
        stdout = completed.stdout.decode("utf-8", errors="replace")[:2000]
        stderr = completed.stderr.decode("utf-8", errors="replace")[:2000]
        fail(
            f"dogfood command failed with exit {completed.returncode}: "
            f"{' '.join(arguments)}; stdout={stdout!r}; stderr={stderr!r}"
        )
    if completed.stderr:
        fail(f"dogfood command emitted stderr: {' '.join(arguments)}")
    try:
        envelope = json.loads(completed.stdout)
    except (UnicodeError, json.JSONDecodeError) as error:
        fail(f"dogfood command returned invalid JSON: {' '.join(arguments)}: {error}")
    if not isinstance(envelope, dict):
        fail(f"dogfood command returned a non-object envelope: {' '.join(arguments)}")
    if envelope.get("schema") != output_schema or envelope.get("command") != command:
        fail(
            f"dogfood command returned the wrong envelope: expected {output_schema}/{command}, "
            f"observed {envelope.get('schema')}/{envelope.get('command')}"
        )
    result = envelope.get("result")
    if not isinstance(result, dict) or result.get("schema") != result_schema:
        fail(
            f"dogfood command returned the wrong result: expected {result_schema}, "
            f"observed {result.get('schema') if isinstance(result, dict) else None}"
        )
    return envelope


def dogfood_v2_status(paths: dict[str, Path], metadata: dict[str, Any]) -> dict[str, Any]:
    return dogfood_json_command(
        paths,
        metadata,
        ["status"],
        command="session.status",
        output_schema="podway.output/v2",
        result_schema="podway.status-result/v2",
    )["result"]


def require_dogfood_node(status: dict[str, Any], expected: str) -> None:
    current = status.get("current")
    observed = None
    if isinstance(current, dict):
        node = current.get("node")
        if isinstance(node, dict):
            observed = node.get("graph_node_id")
    if observed != expected:
        fail(f"dogfood cursor mismatch: expected {expected}, observed {observed}")


def dogfood_v2_mutation(
    paths: dict[str, Path],
    metadata: dict[str, Any],
    arguments: list[str],
    *,
    command: str,
    result_schema: str,
) -> dict[str, Any]:
    result = dogfood_json_command(
        paths,
        metadata,
        arguments,
        command=command,
        output_schema="podway.output/v2",
        result_schema=result_schema,
    )["result"]
    admission = result.get("admission")
    if not isinstance(admission, dict) or admission.get("admitted") is not True:
        fail(f"v2 dogfood mutation was not admitted: {' '.join(arguments)}")
    return result


def dogfood_set_v2(
    paths: dict[str, Path],
    metadata: dict[str, Any],
    item: str,
    value: str,
    key: str,
) -> None:
    dogfood_v2_mutation(
        paths,
        metadata,
        ["set", item, value, "--idempotency-key", key],
        command="item.set",
        result_schema="podway.item-mutation-result/v2",
    )


def dogfood_complete_v2(
    paths: dict[str, Path], metadata: dict[str, Any], key: str
) -> None:
    dogfood_v2_mutation(
        paths,
        metadata,
        ["complete", "--idempotency-key", key],
        command="session.complete",
        result_schema="podway.stage-transition-result/v2",
    )


def self_test_v2_dogfood(cli: Path, daemon: Path) -> dict[str, Any]:
    """Exercise the shipped v2 workflow only inside a disposable managed runtime."""
    checkout = make_synthetic_checkout()
    root = managed_root_for(checkout)
    process: subprocess.Popen[bytes] | None = None
    paths: dict[str, Path] | None = None
    try:
        paths = prepare_synthetic_runtime(checkout)
        metadata = snapshot_pair(paths, cli, daemon, checkout)
        adopt_snapshot_when_idle(paths, metadata)
        snapshotted_daemon = require_trusted_snapshot_binary(
            paths["root"], metadata["snapshot"]["podwayd"], label="snapshot podwayd"
        )
        process = start_isolated_daemon(
            snapshotted_daemon, paths["account_root"], paths["dev_home"]
        )
        initialize_sandbox(paths["sandbox"])
        dogfood_json_command(
            paths,
            metadata,
            ["init"],
            command="workspace.init",
            output_schema="podway.output/v1",
            result_schema="podway.workspace-init-result/v1",
        )
        publish_development_v2_marker(paths, metadata)
        audit_managed_tree(
            paths["root"], expected=paths["root"], uid=euid(), repair_modes=True
        )

        started = dogfood_v2_mutation(
            paths,
            metadata,
            [
                "start",
                "--preset",
                "sw-dev-v2",
                "--task",
                "Dogfood the complete Procedure v2 workflow",
                "--goal",
                "Complete the disposable Procedure v2 dogfood workflow.",
                "--criterion",
                "verified=The complete disposable workflow is recorded.",
                "--actor",
                "V2DOG-005 dogfood",
                "--idempotency-key",
                "v2dog005-start",
            ],
            command="session.start",
            result_schema="podway.session-start-result/v2",
        )
        if started.get("procedure_schema") != "podway.procedure/v2":
            fail("v2 dogfood did not start a Procedure v2 session")
        status = dogfood_v2_status(paths, metadata)
        require_dogfood_node(status, "implement")
        procedure = status.get("procedure")
        if not isinstance(procedure, dict) or procedure.get("id") != "sw-dev-v2":
            fail("v2 dogfood status did not retain the shipped preset identity")

        revised = dogfood_v2_mutation(
            paths,
            metadata,
            [
                "goal",
                "revise",
                "--goal",
                "Complete and restart the disposable Procedure v2 dogfood workflow.",
                "--criterion",
                "verified=The complete workflow survives a daemon restart.",
                "--rework-to",
                "implement",
                "--reason",
                "The dogfood goal now includes restart persistence.",
                "--actor",
                "V2DOG-005 dogfood",
                "--idempotency-key",
                "v2dog005-revise-goal",
            ],
            command="goal.revise",
            result_schema="podway.goal-revision-result/v1",
        )
        if revised.get("goal_revision") != 2 or revised.get("rework_to") != "implement":
            fail("v2 dogfood did not create goal revision two at implement")

        dogfood_set_v2(
            paths,
            metadata,
            "implementation-summary",
            "Implemented the first disposable candidate.",
            "v2dog005-set-implementation-one",
        )
        dogfood_set_v2(
            paths,
            metadata,
            "source-revision",
            "dogfood-revision-one",
            "v2dog005-set-revision-one",
        )
        dogfood_complete_v2(paths, metadata, "v2dog005-complete-implementation-one")
        require_dogfood_node(dogfood_v2_status(paths, metadata), "capture-baseline")

        skipped = dogfood_v2_mutation(
            paths,
            metadata,
            [
                "skip",
                "--reason",
                "No separate baseline is required in the disposable fixture.",
                "--idempotency-key",
                "v2dog005-skip-baseline-one",
            ],
            command="session.skip",
            result_schema="podway.stage-transition-result/v2",
        )
        if skipped.get("transition") != "skip":
            fail("v2 dogfood did not record the skip path")
        require_dogfood_node(dogfood_v2_status(paths, metadata), "test-after-impl")

        retry_from_attempt = dogfood_v2_status(paths, metadata)["current"]["attempt"]["attempt_id"]
        retried = dogfood_v2_mutation(
            paths,
            metadata,
            [
                "retry",
                "--reason",
                "Repeat the disposable verification with the intended environment.",
                "--idempotency-key",
                "v2dog005-retry-test",
            ],
            command="session.retry",
            result_schema="podway.stage-transition-result/v2",
        )
        if (
            retried.get("transition") != "retry"
            or retried.get("from_attempt_id") != retry_from_attempt
            or retried.get("to_attempt_id") == retry_from_attempt
        ):
            fail("v2 dogfood did not record the retry path")
        dogfood_set_v2(
            paths,
            metadata,
            "test-command",
            "make test",
            "v2dog005-set-test-command-one",
        )
        dogfood_set_v2(
            paths,
            metadata,
            "test-exit-status",
            "0",
            "v2dog005-set-test-status-one",
        )
        dogfood_set_v2(
            paths,
            metadata,
            "log-digest",
            "sha256:dogfood-first-pass",
            "v2dog005-set-log-one",
        )
        dogfood_complete_v2(paths, metadata, "v2dog005-complete-test-one")
        require_dogfood_node(dogfood_v2_status(paths, metadata), "decide-after-impl-test")

        failed_decision = dogfood_v2_mutation(
            paths,
            metadata,
            [
                "decide",
                "--option",
                "failed",
                "--reason",
                "The first recorded test evidence requires another implementation attempt.",
                "--actor",
                "V2DOG-005 dogfood",
                "--idempotency-key",
                "v2dog005-decide-rework",
            ],
            command="session.decide",
            result_schema="podway.decision-result/v1",
        )
        if (
            failed_decision.get("effect") != "rework"
            or failed_decision.get("target_graph_node_id") != "implement"
        ):
            fail("v2 dogfood decision did not traverse its declared rework route")
        before_restart = dogfood_v2_status(paths, metadata)
        require_dogfood_node(before_restart, "implement")
        before_restart_attempt = before_restart["current"]["attempt"]["attempt_id"]
        before_restart_session = before_restart["session"]["id"]
        before_restart_goal_revision = before_restart["goal_revision"]
        before_restart_daemon_digest = release_archive.sha256_file(snapshotted_daemon)

        stop_process(process)
        process = start_isolated_daemon(
            snapshotted_daemon, paths["account_root"], paths["dev_home"]
        )
        after_restart = dogfood_v2_status(paths, metadata)
        require_dogfood_node(after_restart, "implement")
        if (
            after_restart["current"]["attempt"]["attempt_id"] != before_restart_attempt
            or after_restart["session"]["id"] != before_restart_session
            or after_restart["goal_revision"] != before_restart_goal_revision
        ):
            fail("v2 dogfood restart changed the active session, attempt, or goal revision")
        if (
            release_archive.sha256_file(snapshotted_daemon) != before_restart_daemon_digest
            or current_snapshot(paths, checkout=checkout)["snapshot"] != metadata["snapshot"]
        ):
            fail("v2 dogfood restart did not retain the adopted daemon snapshot")

        dogfood_set_v2(
            paths,
            metadata,
            "implementation-summary",
            "Implemented the corrected disposable candidate.",
            "v2dog005-set-implementation-two",
        )
        dogfood_set_v2(
            paths,
            metadata,
            "source-revision",
            "dogfood-revision-two",
            "v2dog005-set-revision-two",
        )
        dogfood_complete_v2(paths, metadata, "v2dog005-complete-implementation-two")
        dogfood_v2_mutation(
            paths,
            metadata,
            [
                "skip",
                "--reason",
                "The restarted disposable fixture uses the same bounded environment.",
                "--idempotency-key",
                "v2dog005-skip-baseline-two",
            ],
            command="session.skip",
            result_schema="podway.stage-transition-result/v2",
        )

        for suffix, node in (("impl", "test-after-impl"), ("review", "test-after-review")):
            require_dogfood_node(dogfood_v2_status(paths, metadata), node)
            dogfood_set_v2(
                paths,
                metadata,
                "test-command",
                "make test",
                f"v2dog005-set-test-command-{suffix}",
            )
            dogfood_set_v2(
                paths,
                metadata,
                "test-exit-status",
                "0",
                f"v2dog005-set-test-status-{suffix}",
            )
            dogfood_set_v2(
                paths,
                metadata,
                "log-digest",
                f"sha256:dogfood-{suffix}-pass",
                f"v2dog005-set-log-{suffix}",
            )
            dogfood_complete_v2(paths, metadata, f"v2dog005-complete-test-{suffix}")
            decision_node = (
                "decide-after-impl-test" if suffix == "impl" else "decide-after-review-test"
            )
            require_dogfood_node(dogfood_v2_status(paths, metadata), decision_node)
            dogfood_v2_mutation(
                paths,
                metadata,
                [
                    "decide",
                    "--option",
                    "passed",
                    "--reason",
                    "The recorded disposable verification supports advancement.",
                    "--actor",
                    "V2DOG-005 dogfood",
                    "--idempotency-key",
                    f"v2dog005-decide-{suffix}-passed",
                ],
                command="session.decide",
                result_schema="podway.decision-result/v1",
            )
            if suffix == "impl":
                require_dogfood_node(dogfood_v2_status(paths, metadata), "review-change")
                dogfood_set_v2(
                    paths,
                    metadata,
                    "review-summary",
                    "The corrected disposable candidate has no unresolved finding.",
                    "v2dog005-set-review-summary",
                )
                dogfood_complete_v2(paths, metadata, "v2dog005-complete-review")

        require_dogfood_node(dogfood_v2_status(paths, metadata), "assess-session-goal")
        assessed = dogfood_v2_mutation(
            paths,
            metadata,
            [
                "goal",
                "assess-criterion",
                "verified",
                "--status",
                "satisfied",
                "--reason",
                "The complete disposable workflow survived the daemon restart.",
                "--evidence",
                "test-after-review",
                "--actor",
                "V2DOG-005 dogfood",
                "--idempotency-key",
                "v2dog005-assess-verified",
            ],
            command="goal.assess_criterion",
            result_schema="podway.criterion-assessment-result/v1",
        )
        if assessed.get("complete") is not True or assessed.get("determined_outcome") != "achieved":
            fail("v2 dogfood criterion assessment did not determine the achieved outcome")
        dogfood_v2_mutation(
            paths,
            metadata,
            [
                "decide",
                "--option",
                "achieved",
                "--reason",
                "The recorded criterion assessment supports the achieved route.",
                "--actor",
                "V2DOG-005 dogfood",
                "--idempotency-key",
                "v2dog005-decide-achieved",
            ],
            command="session.decide",
            result_schema="podway.decision-result/v1",
        )
        require_dogfood_node(dogfood_v2_status(paths, metadata), "finish-achieved")
        dogfood_set_v2(
            paths,
            metadata,
            "outcome-note",
            "The disposable v2 workflow completed with restart evidence.",
            "v2dog005-set-outcome",
        )
        dogfood_complete_v2(paths, metadata, "v2dog005-complete-outcome")
        require_dogfood_node(dogfood_v2_status(paths, metadata), "confirm-closeout")
        dogfood_v2_mutation(
            paths,
            metadata,
            [
                "decide",
                "--option",
                "ready",
                "--reason",
                "The recorded outcome is consistent with the achieved assessment.",
                "--actor",
                "V2DOG-005 dogfood",
                "--idempotency-key",
                "v2dog005-decide-closeout",
            ],
            command="session.decide",
            result_schema="podway.decision-result/v1",
        )
        require_dogfood_node(dogfood_v2_status(paths, metadata), "record-closeout")
        dogfood_set_v2(
            paths,
            metadata,
            "closeout-note",
            "V2DOG-005 disposable dogfood complete.",
            "v2dog005-set-closeout",
        )
        dogfood_complete_v2(paths, metadata, "v2dog005-complete-closeout")
        completed = dogfood_v2_status(paths, metadata)
        if completed["session"].get("lifecycle") != "completed" or completed.get("current") is not None:
            fail("v2 dogfood did not reach a completed terminal session")
        if completed.get("latest_goal_outcome") != "achieved" or completed.get("goal_revision") != 2:
            fail("v2 dogfood closeout lost the achieved revised goal")

        manually_reactivated = dogfood_v2_mutation(
            paths,
            metadata,
            [
                "rework",
                "--to",
                "implement",
                "--reason",
                "Exercise the declared completed-session manual reactivation route.",
                "--actor",
                "V2DOG-005 dogfood",
                "--idempotency-key",
                "v2dog005-manual-reactivation",
            ],
            command="session.rework",
            result_schema="podway.rework-result/v1",
        )
        if manually_reactivated.get("reactivated") is not True:
            fail("v2 dogfood completed-session manual rework did not reactivate")
        require_dogfood_node(dogfood_v2_status(paths, metadata), "implement")

        dogfood_v2_mutation(
            paths,
            metadata,
            ["reset", "--yes", "--idempotency-key", "v2dog005-reset-v2"],
            command="session.reset",
            result_schema="podway.stage-transition-result/v2",
        )
        dogfood_json_command(
            paths,
            metadata,
            [
                "start",
                "--preset",
                "sw-dev",
                "--task",
                "Verify the retained Procedure v1 path",
                "--idempotency-key",
                "v2dog005-start-v1",
            ],
            command="session.start",
            output_schema="podway.output/v1",
            result_schema="podway.session-start-result/v1",
        )
        dogfood_json_command(
            paths,
            metadata,
            ["set", "goal", "Retain the v1 lifecycle.", "--idempotency-key", "v2dog005-v1-goal"],
            command="item.set",
            output_schema="podway.output/v1",
            result_schema="podway.item-mutation-result/v1",
        )
        dogfood_json_command(
            paths,
            metadata,
            [
                "add",
                "acceptance-criteria",
                "The v1 session advances unchanged.",
                "--idempotency-key",
                "v2dog005-v1-criterion",
            ],
            command="item.add",
            output_schema="podway.output/v1",
            result_schema="podway.item-mutation-result/v1",
        )
        dogfood_json_command(
            paths,
            metadata,
            ["complete", "--idempotency-key", "v2dog005-v1-complete"],
            command="session.complete",
            output_schema="podway.output/v1",
            result_schema="podway.stage-transition-result/v1",
        )
        legacy = dogfood_json_command(
            paths,
            metadata,
            ["status"],
            command="session.status",
            output_schema="podway.output/v1",
            result_schema="podway.status-result/v1",
        )["result"]
        if (
            legacy["session"].get("lifecycle") != "running"
            or legacy["current"].get("stage_id") != "inspect"
        ):
            fail("v1 dogfood regression did not advance to the second stage")

        return {
            "preset": "sw-dev-v2",
            "success": True,
            "decision_rework": True,
            "goal_revision": 2,
            "retry": True,
            "skip": True,
            "restart": True,
            "closeout": "achieved",
            "manual_reactivation": True,
            "v1_regression": "advanced",
        }
    finally:
        if process is not None:
            stop_process(process)
        cleanup_failures: list[str] = []
        for temporary, label, prefix in (
            (root, "managed runtime", f"podway-dev-{euid()}-"),
            (checkout, "synthetic checkout", "podway-dev-checkout-"),
        ):
            try:
                if temporary.parent != TMP_ROOT or not temporary.name.startswith(prefix):
                    cleanup_failures.append(f"unsafe {label} path: {temporary}")
                    continue
                if temporary.exists():
                    shutil.rmtree(temporary)
                if temporary.exists():
                    cleanup_failures.append(f"{label} remained after cleanup: {temporary}")
            except OSError as error:
                cleanup_failures.append(f"failed to remove {label} {temporary}: {error}")
        if cleanup_failures:
            fail("; ".join(cleanup_failures))


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


V2REL003_PROCEDURE = """\
# Formatting noise is intentional: qualification proves canonical digest stability.

schema: podway.procedure/v2
id: v2rel003-native
version: "1"
name: V2REL-003 native qualification
purpose: Qualify native recovery and admission behavior with a minimal real graph.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Record work
    intent: Record the native qualification evidence.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
        min_length: 1
        max_length: 200
  assess:
    type: decision
    title: Assess goal
    objective: Determine whether the qualification goal is achieved.
    prompt: Is the qualification goal achieved?
    options:
      - id: achieved
        label: Achieved
        criteria: The native evidence supports the criterion.
      - id: not-achieved
        label: Not achieved
        criteria: The native evidence does not support the criterion.
      - id: superseded
        label: Superseded
        criteria: The qualification goal no longer describes the desired outcome.
    reason:
      required: true
      prompt: Explain the decision.
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  close:
    type: action
    title: Close qualification
    intent: Record closeout after the assessed goal.
    items:
      - id: closeout
        type: text
        prompt: Record closeout.
        required: true
        min_length: 1
        max_length: 200
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: assess
    - id: assess
      use: assess
      evidence_from:
        - node: work
          required: true
      routes:
        achieved:
          to: close
          effect: advance
        not-achieved:
          to: work
          effect: rework
        superseded:
          to: close
          effect: advance
    - id: close
      use: close
      terminal: true
manual_rework:
  allowed_targets:
    - work
"""


def command_qualify_v2rel003(
    *, podway: str, podwayd_debug: str, podwayd_release: str
) -> int:
    """Qualify v2 native behavior against three already-built binaries."""
    release_archive.require_native_host()
    cli = require_qualification_binary(podway, label="podway")
    debug_daemon = require_qualification_binary(podwayd_debug, label="podwayd-debug")
    release_daemon = require_qualification_binary(podwayd_release, label="podwayd-release")
    if release_archive.test_isolation_capability(cli) is not release_archive.TestIsolationCapability.ENABLED:
        fail("podway lacks test-isolation capability")
    if release_archive.development_v2_admission_capability(debug_daemon) is not release_archive.TestIsolationCapability.ENABLED:
        fail("podwayd-debug lacks development v2 admission capability")
    if release_archive.development_v2_admission_capability(release_daemon) is not release_archive.TestIsolationCapability.DISABLED:
        fail("podwayd-release unexpectedly exposes development v2 admission")

    checks = {
        "custom_preview_confirmation": False,
        "format_equivalence_restart": False,
        "preset_without_digest": False,
        "next_suggestions": False,
        "detached_replay": False,
        "concurrent_stale_fence": False,
        "sigkill_recovery": False,
        "response_loss_reconciliation": False,
        "completed_manual_reactivation": False,
        "completed_goal_reactivation": False,
        "cancelled_rejection": False,
        "endpoint_isolation": False,
        "release_public_admission": False,
    }
    checkout = make_synthetic_checkout()
    root = managed_root_for(checkout)
    process: subprocess.Popen[bytes] | None = None
    try:
        paths = prepare_synthetic_runtime(checkout)
        metadata = snapshot_pair(paths, cli, debug_daemon, checkout)
        adopt_snapshot_when_idle(paths, metadata)
        snap_cli = Path(metadata["snapshot"]["podway"])
        snap_daemon = Path(metadata["snapshot"]["podwayd"])
        process = start_isolated_daemon(snap_daemon, paths["account_root"], paths["dev_home"])
        initialize_sandbox(paths["sandbox"])
        require_output_result(
            qualification_command(snap_cli, paths, ["init"], label="workspace init"),
            command="workspace.init",
            result_schema="podway.workspace-init-result/v1",
        )
        publish_development_v2_marker(paths, metadata)
        audit_managed_tree(
            paths["root"], expected=paths["root"], uid=euid(), repair_modes=True
        )

        preset = require_output_result(
            dogfood_json_command(
                paths,
                metadata,
                [
                    "start", "--preset", "sw-dev-v2", "--task", "Preset admission",
                    "--goal", "Prove preset admission.",
                    "--criterion", "preset=Preset admission succeeds without a digest.",
                    "--actor", "V2REL-003 qualifier", "--idempotency-key", "v2rel003-preset",
                ],
                command="session.start",
                output_schema="podway.output/v2",
                result_schema="podway.session-start-result/v2",
            ),
            command="session.start",
            result_schema="podway.session-start-result/v2",
        )
        checks["preset_without_digest"] = preset.get("procedure_schema") == "podway.procedure/v2"
        qualification_command(
            snap_cli,
            paths,
            ["--yes", "--idempotency-key", "v2rel003-reset-preset", "reset"],
            label="preset reset",
        )

        procedure_path = paths["sandbox"] / "native-v2.yaml"
        procedure_path.write_text(V2REL003_PROCEDURE, encoding="utf-8")
        os.chmod(procedure_path, FILE_MODE)
        preview = require_output_result(
            qualification_command(
                snap_cli, paths, ["procedure", "preview", procedure_path.name], label="procedure preview"
            ),
            command="procedure.preview",
            result_schema="podway.procedure-preview-result/v1",
        )
        if preview.get("admissible") is not True:
            fail("qualification Procedure v2 preview is not admissible")
        digest = preview.get("procedure_digest")
        suggestion = preview.get("start_suggestion")
        argv = suggestion.get("argv") if isinstance(suggestion, dict) else None
        if not isinstance(digest, str) or not isinstance(argv, list) or argv[:2] != ["podway", "start"]:
            fail("procedure preview omitted its exact start suggestion")

        missing = qualification_command(
            snap_cli,
            paths,
            ["start", "--procedure", procedure_path.name, "--task", "Missing confirmation"],
            label="missing confirmation",
            expected_code=None,
        )
        require_error_code(missing, "DIGEST_CONFIRMATION_REQUIRED", label="missing confirmation")
        mismatch = qualification_command(
            snap_cli,
            paths,
            ["start", "--procedure", procedure_path.name, "--expect-procedure-digest", "sha256:" + "a" * 64, "--task", "Wrong confirmation"],
            label="digest mismatch",
            expected_code=None,
        )
        require_error_code(mismatch, "PROCEDURE_DIGEST_MISMATCH", label="digest mismatch")
        procedure_path.write_text(
            V2REL003_PROCEDURE.replace(
                "Qualify native recovery and admission behavior with a minimal real graph.",
                "Qualify meaningfully edited native behavior with a minimal real graph.",
            ),
            encoding="utf-8",
        )
        semantic_mismatch = qualification_command(
            snap_cli,
            paths,
            [
                "start",
                "--procedure",
                procedure_path.name,
                "--expect-procedure-digest",
                digest,
                "--task",
                "Stale semantic confirmation",
                "--goal",
                "Prove semantic edits invalidate confirmation.",
                "--criterion",
                "semantic=The stale digest is rejected.",
                "--actor",
                "V2REL-003 qualifier",
            ],
            label="semantic edit digest mismatch",
            expected_code=None,
        )
        require_error_code(
            semantic_mismatch,
            "PROCEDURE_DIGEST_MISMATCH",
            label="semantic edit digest mismatch",
        )
        empty_status = qualification_command(
            snap_cli, paths, ["status"], label="status after rejected starts", expected_code=None
        )
        require_error_code(empty_status, "SESSION_NOT_FOUND", label="status after rejected starts")

        procedure_path.write_text(V2REL003_PROCEDURE, encoding="utf-8")
        qualification_command(
            snap_cli,
            paths,
            ["procedure", "format", procedure_path.name, "--write"],
            label="procedure format",
        )
        formatted_preview = require_output_result(
            qualification_command(
                snap_cli, paths, ["procedure", "preview", procedure_path.name], label="formatted preview"
            ),
            command="procedure.preview",
            result_schema="podway.procedure-preview-result/v1",
        )
        if formatted_preview.get("procedure_digest") != digest:
            fail("formatting-equivalent Procedure v2 changed its canonical digest")
        checks["custom_preview_confirmation"] = True

        start_argv = [str(value) for value in argv[1:]]
        start_argv = ["Native qualification" if value == "<task>" else value for value in start_argv]
        start_argv.extend(
            ["--goal", "Prove native recovery.", "--criterion", "native=Native recovery passes.", "--actor", "V2REL-003 qualifier"]
        )
        started = require_output_result(
            qualification_command(snap_cli, paths, start_argv, label="suggested custom start"),
            command="session.start",
            result_schema="podway.session-start-result/v2",
        )
        if started.get("procedure_digest") != digest:
            fail("custom start lost the preview-confirmed digest")

        before_kill = require_output_result(
            qualification_command(snap_cli, paths, ["status"], label="status before SIGKILL"),
            command="session.status",
            result_schema="podway.status-result/v2",
        )
        session_id = before_kill["session"]["id"]
        process.kill()
        process.wait(timeout=5)
        process = start_isolated_daemon(snap_daemon, paths["account_root"], paths["dev_home"])
        after_kill = require_output_result(
            qualification_command(snap_cli, paths, ["status"], label="status after SIGKILL"),
            command="session.status",
            result_schema="podway.status-result/v2",
        )
        if after_kill["session"]["id"] != session_id or after_kill["procedure"]["digest"] != digest:
            fail("SIGKILL recovery changed session or Procedure identity")
        if current_snapshot(paths, checkout=checkout)["snapshot"] != metadata["snapshot"]:
            fail("SIGKILL recovery changed the adopted CLI/daemon snapshot identity")
        checks["sigkill_recovery"] = True
        checks["format_equivalence_restart"] = True

        next_result = require_output_result(
            qualification_command(snap_cli, paths, ["next"], label="next action suggestion"),
            command="session.next",
            result_schema="podway.next-result/v2",
        )
        set_suggestion = next(
            (entry for entry in next_result.get("suggestions", []) if entry.get("command") == "item.set"),
            None,
        )
        if not isinstance(set_suggestion, dict):
            fail("next omitted the required action item suggestion")
        if next_result.get("allowed_actions") != [
            "item.set",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.revise",
        ]:
            fail(f"action next returned unexpected legal actions: {next_result.get('allowed_actions')}")
        set_argv = [str(value) for value in set_suggestion["argv"][1:]]
        set_argv = ["native evidence" if value == "<text>" else value for value in set_argv]
        qualification_command(
            snap_cli, paths, ["--idempotency-key", "v2rel003-next-set", *set_argv], label="execute next suggestion"
        )

        status_envelope = qualification_command(
            snap_cli, paths, ["status"], label="status before stale race"
        )
        status = require_output_result(
            status_envelope,
            command="session.status",
            result_schema="podway.status-result/v2",
        )
        fence = [
            "--if-workspace-uuid", status_envelope["workspace"]["uuid"],
            "--if-session-id", status["session"]["id"],
            "--if-session-revision", str(status["session"]["revision"]),
            "--if-attempt", status["current"]["attempt"]["attempt_id"],
        ]
        racers = []
        for suffix in ("a", "b"):
            racers.append(
                subprocess.Popen(
                    [
                        snap_cli.as_posix(), "--dev", "--json", *fence,
                        "--idempotency-key", f"v2rel003-race-{suffix}",
                        "retry", "--reason", f"Concurrent stale-fence contender {suffix}.",
                    ],
                    cwd=paths["sandbox"],
                    env=isolation_environment(paths["account_root"], paths["dev_home"]),
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                )
            )
        race_results = [process.communicate(timeout=COMMAND_TIMEOUT_SECONDS) + (process.returncode,) for process in racers]
        race_json = [json.loads(stdout) for stdout, _stderr, _code in race_results]
        race_codes = sorted(value.get("code", "OK") for value in race_json)
        if race_codes != ["OK", "SESSION_REVISION_CONFLICT"]:
            fail(f"stale-fenced concurrent mutations did not produce one winner: {race_codes}")
        checks["concurrent_stale_fence"] = True

        detached = require_output_result(
            qualification_command(
                snap_cli,
                paths,
                [
                    "--detach", "--idempotency-key", "v2rel003-detached", "retry",
                    "--reason", "Exercise detached admission and immutable replay.",
                ],
                label="detached mutation",
            ),
            command="session.retry",
            result_schema="podway.detached-admission-result/v2",
        )
        job_id = detached["admission"]["job_id"]
        waited = require_output_result(
            qualification_command(snap_cli, paths, ["job", "wait", job_id], label="detached wait"),
            command="job.wait",
            result_schema="podway.job-result/v2",
        )
        detached_status = require_output_result(
            qualification_command(snap_cli, paths, ["job", "status", job_id], label="detached status"),
            command="job.status",
            result_schema="podway.job-result/v2",
        )
        if detached_status.get("job") != waited.get("job"):
            fail("terminal detached job.status changed the waited durable job receipt")
        lookup = require_output_result(
            qualification_command(
                snap_cli, paths, ["--idempotency-key", "v2rel003-detached", "job", "lookup"], label="detached lookup"
            ),
            command="job.lookup",
            result_schema="podway.job-lookup-result/v2",
        )
        if (
            lookup.get("job", {}).get("terminal_response") != waited.get("job")
            or waited.get("job", {}).get("job", {}).get("id") != job_id
        ):
            fail("detached terminal lookup was not an exact replay")
        checks["detached_replay"] = True

        loss_key = "v2rel003-response-loss"
        loss_status_envelope = qualification_command(
            snap_cli, paths, ["status"], label="response-loss precondition status"
        )
        loss_status = require_output_result(
            loss_status_envelope,
            command="session.status",
            result_schema="podway.status-result/v2",
        )
        proxy = paths["dev_home"] / "run" / "response-loss.sock"
        relay, relay_outcome = response_loss_relay(proxy, paths["socket"])
        loss_arguments = [
            "--if-workspace-uuid", loss_status_envelope["workspace"]["uuid"],
            "--if-session-id", loss_status["session"]["id"],
            "--if-session-revision", str(loss_status["session"]["revision"]),
            "--if-attempt", loss_status["current"]["attempt"]["attempt_id"],
            "--idempotency-key", loss_key, "retry", "--reason",
            "Exercise a daemon response that is consumed but not delivered.",
        ]
        lost = subprocess.run(
            [
                snap_cli.as_posix(), "--json", "--socket", proxy.as_posix(),
                "--worktree", paths["sandbox"].as_posix(), "--timeout", "10s",
                *loss_arguments,
            ],
            cwd=paths["sandbox"],
            env=isolation_environment(paths["account_root"], paths["dev_home"]),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=COMMAND_TIMEOUT_SECONDS,
        )
        relay.join(timeout=15)
        if relay.is_alive():
            fail("response-loss relay did not finish")
        if "error" in relay_outcome:
            fail(f"response-loss relay failed: {relay_outcome['error']}")
        lost_json = decode_cli_json(lost, label="discarded response mutation", expected_code=None)
        if lost_json.get("code") != "MUTATION_OUTCOME_UNKNOWN":
            captured = relay_outcome.get("response")
            decoded = None
            if isinstance(captured, bytes) and len(captured) >= 4:
                try:
                    decoded = json.loads(captured[4:])
                except (UnicodeError, json.JSONDecodeError):
                    decoded = captured[:500].hex()
            fail(
                f"discarded response mutation returned unexpected envelope: {lost_json}; "
                f"daemon_response={decoded}"
            )
        require_error_code(lost_json, "MUTATION_OUTCOME_UNKNOWN", label="discarded response mutation")
        if lost.returncode != 4 or lost_json.get("retryable") is not True:
            fail("discarded response did not produce retryable unknown outcome")
        response_wire = relay_outcome.get("response")
        if not isinstance(response_wire, bytes) or len(response_wire) < 5:
            fail("response-loss relay did not capture a complete daemon response")
        length = int.from_bytes(response_wire[:4], "big")
        if len(response_wire) != length + 4:
            fail("response-loss relay captured a malformed response frame")
        try:
            discarded_envelope = json.loads(response_wire[4:])
        except (UnicodeError, json.JSONDecodeError) as error:
            fail(f"discarded daemon response was not JSON: {error}")
        loss_lookup = require_output_result(
            qualification_command(
                snap_cli, paths, ["--idempotency-key", loss_key, "job", "lookup"], label="response-loss lookup"
            ),
            command="job.lookup",
            result_schema="podway.job-lookup-result/v2",
        )
        terminal_response = loss_lookup.get("job", {}).get("terminal_response")
        if terminal_response != discarded_envelope:
            fail("response-loss lookup did not preserve the discarded daemon response exactly")
        replayed = qualification_command(
            snap_cli,
            paths,
            loss_arguments,
            label="response-loss exact replay",
        )
        replay_request_id = replayed.pop("request_id", None)
        discarded_request_id = discarded_envelope.pop("request_id", None)
        if (
            not isinstance(replay_request_id, str)
            or not isinstance(discarded_request_id, str)
            or replay_request_id == discarded_request_id
            or replayed != discarded_envelope
        ):
            fail(
                "response-loss replay did not preserve the frozen response apart from its "
                f"current request correlation: replay={replayed}; discarded={discarded_envelope}"
            )
        checks["response_loss_reconciliation"] = True

        qualification_command(
            snap_cli,
            paths,
            ["--idempotency-key", "v2rel003-post-detached-set", "set", "result", "detached evidence"],
            label="restore required item after detached retry",
        )

        complete_next = require_output_result(
            qualification_command(snap_cli, paths, ["next"], label="next complete suggestion"),
            command="session.next",
            result_schema="podway.next-result/v2",
        )
        complete_suggestions = [
            entry for entry in complete_next.get("suggestions", []) if entry.get("command") == "session.complete"
        ]
        if len(complete_suggestions) != 1 or complete_suggestions[0].get("argv") != ["podway", "complete"]:
            fail("next did not return the exact placeholder-free complete suggestion")
        qualification_command(
            snap_cli,
            paths,
            ["--idempotency-key", "v2rel003-complete-work", *complete_suggestions[0]["argv"][1:]],
            label="execute complete suggestion",
        )
        decision_next = require_output_result(
            qualification_command(snap_cli, paths, ["next"], label="next decision suggestion"),
            command="session.next",
            result_schema="podway.next-result/v2",
        )
        assessment_suggestions = [
            entry for entry in decision_next.get("suggestions", []) if entry.get("command") == "goal.assess_criterion"
        ]
        if len(assessment_suggestions) != 1 or any(
            entry.get("command") == "session.decide" for entry in decision_next.get("suggestions", [])
        ):
            fail("assessment next did not expose exactly one unassessed criterion")
        assessment_argv = [str(value) for value in assessment_suggestions[0]["argv"][1:]]
        assessment_argv = [
            "satisfied" if value == "<status>" else "Native evidence passed." if value == "<reason>" else value
            for value in assessment_argv
        ]
        qualification_command(
            snap_cli,
            paths,
            ["--idempotency-key", "v2rel003-assess", *assessment_argv, "--evidence", "work", "--actor", "V2REL-003 qualifier"],
            label="execute assessment suggestion",
        )
        assessed_next = require_output_result(
            qualification_command(snap_cli, paths, ["next"], label="next assessed decision"),
            command="session.next",
            result_schema="podway.next-result/v2",
        )
        decision_suggestions = [
            entry for entry in assessed_next.get("suggestions", []) if entry.get("command") == "session.decide"
        ]
        allowed_options = [
            option.get("option_id")
            for option in assessed_next.get("options", [])
            if isinstance(option, dict)
        ]
        suggested_options = [
            entry.get("argv", [None] * 4)[3]
            for entry in decision_suggestions
            if len(entry.get("argv", [])) >= 4
        ]
        if (
            not isinstance(allowed_options, list)
            or allowed_options != ["achieved"]
            or suggested_options != allowed_options
        ):
            fail(
                "assessed decision next did not expose exactly one suggestion per option: "
                f"allowed={allowed_options}; "
                f"suggestions={assessed_next.get('suggestions')}"
            )
        achieved_suggestion = next(
            entry for entry in decision_suggestions if "achieved" in entry.get("argv", [])
        )
        achieved_argv = [
            "Native evidence supports achievement." if value == "<reason>" else str(value)
            for value in achieved_suggestion["argv"][1:]
        ]
        qualification_command(
            snap_cli,
            paths,
            ["--idempotency-key", "v2rel003-decide", *achieved_argv, "--actor", "V2REL-003 qualifier"],
            label="execute decision suggestion",
        )
        close_next = require_output_result(
            qualification_command(snap_cli, paths, ["next"], label="next close suggestions"),
            command="session.next",
            result_schema="podway.next-result/v2",
        )
        close_set = next(entry for entry in close_next["suggestions"] if entry.get("command") == "item.set")
        close_argv = [
            "Native closeout recorded." if value == "<text>" else str(value)
            for value in close_set["argv"][1:]
        ]
        qualification_command(
            snap_cli, paths, ["--idempotency-key", "v2rel003-closeout", *close_argv], label="execute close suggestion"
        )
        close_ready = require_output_result(
            qualification_command(snap_cli, paths, ["next"], label="next close complete"),
            command="session.next",
            result_schema="podway.next-result/v2",
        )
        close_complete = next(
            entry for entry in close_ready["suggestions"] if entry.get("command") == "session.complete"
        )
        qualification_command(
            snap_cli,
            paths,
            ["--idempotency-key", "v2rel003-complete-close", *close_complete["argv"][1:]],
            label="execute close complete suggestion",
        )
        checks["next_suggestions"] = True
        completed_status = require_output_result(
            qualification_command(snap_cli, paths, ["status"], label="completed status"),
            command="session.status",
            result_schema="podway.status-result/v2",
        )
        if completed_status["session"]["lifecycle"] != "completed":
            fail("qualification session did not complete")

        revised = require_output_result(
            qualification_command(
                snap_cli,
                paths,
                ["--idempotency-key", "v2rel003-goal-reactivate", "goal", "revise", "--goal", "Prove native recovery after reactivation.", "--criterion", "native=Native reactivation passes.", "--rework-to", "work", "--reason", "Exercise completed goal reactivation.", "--actor", "V2REL-003 qualifier", "--reactivate"],
                label="goal reactivation",
            ),
            command="goal.revise",
            result_schema="podway.goal-revision-result/v1",
        )
        checks["completed_goal_reactivation"] = revised.get("reactivated") is True
        qualification_command(
            snap_cli, paths, ["--idempotency-key", "v2rel003-cancel", "cancel", "--reason", "Exercise cancelled terminal rejection."], label="cancel session"
        )
        cancelled_rework = qualification_command(
            snap_cli,
            paths,
            ["--idempotency-key", "v2rel003-cancelled-rework", "rework", "--to", "work", "--reason", "Must be rejected."],
            label="cancelled rework",
            expected_code=None,
        )
        require_error_code(cancelled_rework, "SESSION_CANCELLED", label="cancelled rework")
        checks["cancelled_rejection"] = True

        # The same completed fixture's declared manual target was exercised by the production
        # reactivation path above; verify the independent native manual route with the established
        # end-to-end dogfood fixture rather than synthesizing store state.
        manual = self_test_v2_dogfood(cli, debug_daemon)
        checks["completed_manual_reactivation"] = manual.get("manual_reactivation") is True

        checks["endpoint_isolation"] = self_test_dual_daemon(cli, debug_daemon) == 1
    finally:
        if process is not None:
            stop_process(process)
        if root.exists():
            shutil.rmtree(root, ignore_errors=True)
        shutil.rmtree(checkout, ignore_errors=True)

    release_checkout = make_synthetic_checkout()
    release_root = managed_root_for(release_checkout)
    release_process: subprocess.Popen[bytes] | None = None
    try:
        release_paths = prepare_synthetic_runtime(release_checkout)
        release_process = start_isolated_daemon(
            release_daemon, release_paths["account_root"], release_paths["dev_home"]
        )
        initialize_sandbox(release_paths["sandbox"])
        qualification_command(cli, release_paths, ["init"], label="release-profile init")
        release_started_envelope = qualification_command(
            cli,
            release_paths,
            [
                "--idempotency-key", "v2rel003-release-public-admission",
                "start", "--preset", "sw-dev-v2", "--task", "Release admission",
                "--goal", "Prove public Procedure v2 admission in the release profile.",
                "--criterion", "admission=Release profile admits Procedure v2 publicly.",
                "--actor", "V2REL-003 qualifier",
            ],
            label="release-profile public v2 admission",
        )
        release_started = require_output_result(
            release_started_envelope,
            command="session.start",
            result_schema="podway.session-start-result/v2",
        )
        if release_started_envelope.get("schema") != "podway.output/v2":
            fail("release-profile public v2 start did not use the v2 output envelope")
        if (
            release_started.get("procedure_schema") != "podway.procedure/v2"
            or release_started.get("admission", {}).get("admitted") is not True
        ):
            fail("release-profile public v2 start was not durably admitted")
        release_status = require_output_result(
            qualification_command(
                cli,
                release_paths,
                ["status"],
                label="release-profile status after public v2 admission",
            ),
            command="session.status",
            result_schema="podway.status-result/v2",
        )
        if release_status.get("procedure", {}).get("schema") != "podway.procedure/v2":
            fail("release-profile status did not retain the admitted Procedure v2 session")
        checks["release_public_admission"] = True
    finally:
        if release_process is not None:
            stop_process(release_process)
        if release_root.exists():
            shutil.rmtree(release_root, ignore_errors=True)
        shutil.rmtree(release_checkout, ignore_errors=True)

    missing = sorted(name for name, passed in checks.items() if not passed)
    if missing:
        fail(f"V2REL-003 native qualification checks failed: {', '.join(missing)}")
    print(
        json.dumps(
            {"schema": V2REL003_QUALIFICATION_SCHEMA, "ok": True, "checks": checks},
            sort_keys=True,
            separators=(",", ":"),
        )
    )
    return 0


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
        write_probe_script(daemon_source, marker="daemon-probe", development_v2=True)
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
        ensure_private_directory(paths["sandbox"] / ".podway", uid=euid())
        ensure_private_directory(paths["sandbox"] / ".podway" / "runtime", uid=euid())
        publish_development_v2_marker(paths, metadata)
        require_owned_regular_file(
            paths["development_v2_marker"],
            label="development-v2 marker",
            uid=euid(),
            mode=FILE_MODE,
        )
        marker = json.loads(paths["development_v2_marker"].read_text(encoding="utf-8"))
        if (
            marker.get("schema") != DEVELOPMENT_V2_MARKER_SCHEMA
            or marker.get("feature") != DEVELOPMENT_V2_FEATURE
            or marker.get("workspace_root") != paths["sandbox"].as_posix()
            or marker.get("daemon_path") != snapshot["podwayd"]
            or marker.get("daemon_sha256") != daemon_digest
        ):
            fail("development-v2 marker lost its exact runtime or snapshot binding")
        sentinels += 1
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
    dogfood = self_test_v2_dogfood(cli, daemon)
    sentinels += 9
    return {"mode": "self-test", "ok": True, "sentinels": sentinels, "dogfood": dogfood}


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
    qualifier = subparsers.add_parser(
        "qualify-v2rel003",
        help="qualify native v2 daemon, recovery, and release-admission behavior",
    )
    qualifier.add_argument("--podway", required=True, help="absolute prebuilt CLI path")
    qualifier.add_argument(
        "--podwayd-debug", required=True, help="absolute feature-enabled debug daemon path"
    )
    qualifier.add_argument(
        "--podwayd-release", required=True, help="absolute feature-requested release daemon path"
    )
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
        if arguments.command == "qualify-v2rel003":
            return command_qualify_v2rel003(
                podway=arguments.podway,
                podwayd_debug=arguments.podwayd_debug,
                podwayd_release=arguments.podwayd_release,
            )
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
