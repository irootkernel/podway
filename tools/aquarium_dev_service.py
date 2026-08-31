#!/usr/bin/env python3
"""Producer-owned Aquarium controller for Podway's persistent dev daemon."""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import plistlib
import re
import stat
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

PROJECT_ID = "podway"
SERVICE_LABEL = "dev.aquarium.podwayd"
STATE_SCHEMA = "podway.aquarium-service-state/v1"
RECOVERY_SCHEMA = "podway.aquarium-service-recovery/v1"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
TOKEN_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
DEV_VERSION_RE = re.compile(
    r"^v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)-dev\.([0-9a-f]{12})$"
)
MAX_JSON_BYTES = 64 * 1024
READY_TIMEOUT_SECONDS = 30.0


class ControllerError(RuntimeError):
    pass


def _json_bytes(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def _digest_bytes(value: bytes) -> str:
    return f"sha256:{hashlib.sha256(value).hexdigest()}"


def _file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return f"sha256:{digest.hexdigest()}"


def _tree_digest(root: Path, *, exclude: frozenset[str] = frozenset()) -> str:
    digest = hashlib.sha256()
    for path in sorted(
        candidate
        for candidate in root.rglob("*")
        if candidate.is_file() and candidate.relative_to(root).as_posix() not in exclude
    ):
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode):
            raise ControllerError("generation bundles cannot contain symbolic links")
        digest.update(path.relative_to(root).as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(bytes.fromhex(_file_digest(path)[7:]))
        digest.update(b"\n")
    return f"sha256:{digest.hexdigest()}"


def _read_json(path: Path, *, maximum: int = MAX_JSON_BYTES) -> dict[str, Any]:
    metadata = path.lstat()
    if stat.S_ISLNK(metadata.st_mode) or not path.is_file() or metadata.st_size > maximum:
        raise ControllerError(f"unsafe controller file: {path.name}")
    value = json.loads(path.read_bytes())
    if not isinstance(value, dict):
        raise ControllerError(f"invalid controller document: {path.name}")
    return value


def _atomic_write(path: Path, value: bytes, mode: int = 0o600) -> None:
    temporary = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(descriptor, "wb", closefd=True) as stream:
            stream.write(value)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _normal_absolute(path: Path, name: str) -> Path:
    if not path.is_absolute() or path != Path(os.path.normpath(path)):
        raise ControllerError(f"{name} must be absolute and normalized")
    return path


def _runtime_paths(root: Path) -> dict[str, Path]:
    return {
        "root": root,
        "controller": root / "controller",
        "lock": root / "controller/service.lock",
        "state": root / "controller/state.json",
        "recovery": root / "controller/recovery.json",
        "runtime_metadata": root / "runtime.json",
        "plist": root / f"LaunchAgents/{SERVICE_LABEL}.plist",
        "socket": root / "run/podwayd.sock",
    }


def _generation(root: Path) -> dict[str, Any]:
    _normal_absolute(root, "generation root")
    if not SHA_RE.fullmatch(root.name):
        raise ControllerError("generation root basename must be the exact Git SHA")
    canonical = root.resolve(strict=True)
    if canonical != root:
        raise ControllerError("generation root must be canonical")
    external = _read_json(root / ".aquarium-manifest.json")
    bundle = root / "bundle"
    internal = _read_json(bundle / "manifest.json")
    external_fields = {
        "schema", "project_id", "git_sha", "development_version", "artifact_kind",
        "artifact_path", "command_path", "controller_path", "sha256",
    }
    internal_fields = {
        "schema", "project_id", "git_sha", "development_version", "payload_sha256",
        "controller_protocol", "runtime_protocol", "executables",
    }
    development_version = external.get("development_version")
    version_match = (
        DEV_VERSION_RE.fullmatch(development_version)
        if isinstance(development_version, str)
        else None
    )
    if (
        set(external) != external_fields
        or set(internal) != internal_fields
        or external.get("schema") != "aquarium-dev-artifact-manifest/v2"
        or external.get("project_id") != PROJECT_ID
        or external.get("git_sha") != root.name
        or external.get("artifact_kind") != "managed-service"
        or external.get("artifact_path") != "bundle"
        or external.get("command_path") != "bin/podway"
        or external.get("controller_path") != "libexec/aquarium-dev-service"
        or version_match is None
        or version_match.group(1) != root.name[:12]
        or internal.get("schema") != "podway.aquarium-dev-bundle/v1"
        or internal.get("project_id") != PROJECT_ID
        or internal.get("git_sha") != root.name
        or internal.get("development_version") != development_version
        or internal.get("controller_protocol") != "aquarium-dev-service/v1"
        or internal.get("runtime_protocol") != "podway.managed-runtime/v3"
    ):
        raise ControllerError("generation manifest identity is invalid")
    expected_paths = {
        "bin/podway", "libexec/podway", "libexec/podwayd",
        "libexec/aquarium-dev-service", "manifest.json",
    }
    actual_paths = {
        path.relative_to(bundle).as_posix()
        for path in bundle.rglob("*")
        if path.is_file()
    }
    if actual_paths != expected_paths:
        raise ControllerError("generation bundle layout is invalid")
    if external.get("sha256") != _tree_digest(bundle):
        raise ControllerError("generation bundle digest is invalid")
    if internal.get("payload_sha256") != _tree_digest(bundle, exclude=frozenset({"manifest.json"})):
        raise ControllerError("generation payload digest is invalid")
    executables = internal.get("executables")
    if not isinstance(executables, dict) or set(executables) != {
        "launcher", "cli", "daemon", "controller"
    }:
        raise ControllerError("generation executable identities are invalid")
    resolved: dict[str, Path] = {}
    for role, expected in {
        "launcher": "bin/podway",
        "cli": "libexec/podway",
        "daemon": "libexec/podwayd",
        "controller": "libexec/aquarium-dev-service",
    }.items():
        entry = executables.get(role)
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256"} or entry.get("path") != expected:
            raise ControllerError("generation executable layout is invalid")
        path = bundle / expected
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode) or not path.is_file() or metadata.st_mode & 0o111 == 0:
            raise ControllerError("generation executable is unsafe")
        if entry.get("sha256") != _file_digest(path):
            raise ControllerError("generation executable digest is invalid")
        resolved[role] = path
    return {
        "git_sha": root.name,
        "root": root,
        "bundle": bundle,
        "manifest": external,
        **resolved,
    }


def _validate_state_record(value: Any, *, resolve_generation: bool = True) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ControllerError("service state is invalid")
    required = {"schema", "active_git_sha", "generation_root", "bundle_sha256", "plist_sha256"}
    if set(value) != required or value.get("schema") != STATE_SCHEMA:
        raise ControllerError("service state is invalid")
    if not SHA_RE.fullmatch(str(value.get("active_git_sha", ""))):
        raise ControllerError("service state generation is invalid")
    generation_root = value.get("generation_root")
    if not isinstance(generation_root, str):
        raise ControllerError("service state generation root is invalid")
    _normal_absolute(Path(generation_root), "service state generation root")
    if not TOKEN_RE.fullmatch(str(value.get("bundle_sha256", ""))) or not TOKEN_RE.fullmatch(
        str(value.get("plist_sha256", ""))
    ):
        raise ControllerError("service state digest is invalid")
    if resolve_generation:
        generation = _generation(Path(generation_root))
        if (
            generation["git_sha"] != value["active_git_sha"]
            or generation["manifest"]["sha256"] != value["bundle_sha256"]
        ):
            raise ControllerError("service state does not match its generation")
    return value


def _state_with_generation(
    paths: dict[str, Path],
) -> tuple[dict[str, Any], dict[str, Any]] | None:
    try:
        value = _validate_state_record(_read_json(paths["state"]), resolve_generation=False)
    except FileNotFoundError:
        return None
    generation = _generation(Path(value["generation_root"]))
    if (
        generation["git_sha"] != value["active_git_sha"]
        or generation["manifest"]["sha256"] != value["bundle_sha256"]
    ):
        raise ControllerError("service state does not match its generation")
    if _file_digest(paths["plist"]) != value["plist_sha256"]:
        raise ControllerError("service state does not match its LaunchAgent")
    if paths["runtime_metadata"].read_bytes() != _runtime_metadata(paths, generation):
        raise ControllerError("service state does not match its runtime metadata")
    return value, generation


def _state(paths: dict[str, Path]) -> dict[str, Any] | None:
    resolved = _state_with_generation(paths)
    return resolved[0] if resolved is not None else None


def _recovery(paths: dict[str, Path]) -> dict[str, Any] | None:
    try:
        value = _read_json(paths["recovery"])
    except FileNotFoundError:
        return None
    if value.get("schema") != RECOVERY_SCHEMA:
        raise ControllerError("service recovery state is invalid")
    if set(value) != {"schema", "target_git_sha", "target_generation_root", "previous", "phase", "debt"}:
        raise ControllerError("service recovery state shape is invalid")
    if not SHA_RE.fullmatch(str(value.get("target_git_sha", ""))):
        raise ControllerError("service recovery target is invalid")
    if not isinstance(value.get("target_generation_root"), str):
        raise ControllerError("service recovery target root is invalid")
    target_root = _normal_absolute(
        Path(value["target_generation_root"]), "service recovery target root"
    )
    if target_root.name != value["target_git_sha"]:
        raise ControllerError("service recovery target identity is invalid")
    if value.get("previous") is not None:
        _validate_state_record(value["previous"], resolve_generation=False)
    if value.get("phase") not in {"prepared", "prior-stopped", "recovery-required"}:
        raise ControllerError("service recovery phase is invalid")
    debt = value.get("debt")
    if not isinstance(debt, list) or len(debt) > 8 or any(
        item not in {"target-activation-failed", "prior-restoration-unproven"} for item in debt
    ):
        raise ControllerError("service recovery debt is invalid")
    return value


def _recovery_required(paths: dict[str, Path]) -> bool:
    return _recovery(paths) is not None


def _run(
    arguments: list[str],
    *,
    environment: dict[str, str] | None = None,
    timeout: float = 35.0,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        arguments,
        env=environment,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="strict",
        timeout=timeout,
    )


def _daemon_status(
    paths: dict[str, Path], generation: dict[str, Any], *, timeout: float = 35.0
) -> dict[str, Any] | None:
    environment = {
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        "PODWAY_DEV_HOME": os.fspath(paths["root"]),
    }
    result = _run(
        [os.fspath(generation["cli"]), "--dev", "--json", "daemon", "status"],
        environment=environment,
        timeout=timeout,
    )
    if result.returncode != 0:
        return None
    try:
        output = json.loads(result.stdout)
    except json.JSONDecodeError:
        return None
    daemon = output.get("result") if isinstance(output, dict) else None
    if not isinstance(daemon, dict):
        return None
    if daemon.get("reachable") is False:
        return None
    return daemon


def _daemon_matches(daemon: dict[str, Any], generation: dict[str, Any]) -> bool:
    return (
        daemon.get("mode") == "dev"
        and daemon.get("executable_path") == os.fspath(generation["daemon"])
        and daemon.get("source_commit") == generation["git_sha"]
    )


def _daemon_busy(daemon: dict[str, Any]) -> bool:
    return bool(
        not isinstance(daemon.get("queued_job_count"), int)
        or not isinstance(daemon.get("running_job_count"), int)
        or not isinstance(daemon.get("in_flight_client_count"), int)
        or not isinstance(daemon.get("maintenance_operation_count"), int)
        or daemon["queued_job_count"] > 0
        or daemon["running_job_count"] > 0
        or daemon["in_flight_client_count"] > 0
        or daemon["maintenance_operation_count"] > 0
        or daemon.get("readiness_state") != "ready"
        or (daemon.get("worktree_recovery") or {}).get("failed", 0) > 0
    )


def observe(root: Path) -> dict[str, Any]:
    paths = _runtime_paths(_normal_absolute(root, "runtime root"))
    try:
        recovery = _recovery_required(paths)
        resolved = _state_with_generation(paths)
    except (ControllerError, OSError, json.JSONDecodeError):
        return {"state": "absent", "active": None, "busy": False, "recovery": True}
    if resolved is None:
        return {"state": "absent", "active": None, "busy": False, "recovery": recovery}
    state, generation = resolved
    try:
        daemon = _daemon_status(paths, generation)
    except (ControllerError, OSError, subprocess.SubprocessError):
        daemon = None
    if recovery or (daemon is not None and not _daemon_matches(daemon, generation)):
        phase = "broken"
    elif daemon is None:
        phase = "stopped"
    elif daemon.get("readiness_state") != "ready":
        phase = "starting"
    else:
        phase = "ready"
    busy = daemon is not None and _daemon_busy(daemon)
    if busy:
        phase = "busy"
    return {"state": phase, "active": state["active_git_sha"], "busy": busy, "recovery": recovery}


def status_document(root: Path) -> dict[str, Any]:
    observed = observe(root)
    return {
        "schema": "aquarium-dev-service-status/v1",
        "project_id": PROJECT_ID,
        "state": observed["state"],
        "active_git_sha": observed["active"],
        "busy": observed["busy"],
        "recovery_required": observed["recovery"],
    }


def _plan_token(
    action: str, root: Path, observed: dict[str, Any], target: dict[str, Any]
) -> str:
    bound = {
        "schema": "podway.aquarium-service-plan-token/v1",
        "action": action,
        "runtime_root": os.fspath(root),
        "active_git_sha": observed["active"],
        "busy": observed["busy"],
        "recovery_required": observed["recovery"],
        "target_git_sha": target["git_sha"],
        "target_bundle_sha256": target["manifest"]["sha256"],
    }
    return _digest_bytes(_json_bytes(bound))


def _plan_document(root: Path, target: dict[str, Any]) -> dict[str, Any]:
    recovery = _recovery(_runtime_paths(root))
    if recovery is not None and (
        recovery["target_git_sha"] != target["git_sha"]
        or recovery["target_generation_root"] != os.fspath(target["root"])
    ):
        try:
            _generation(Path(recovery["target_generation_root"]))
        except (ControllerError, FileNotFoundError, OSError, json.JSONDecodeError):
            pass
        else:
            raise ControllerError("the recovery target does not match the selected generation")
    observed = observe(root)
    if observed["busy"]:
        action = "defer"
    elif observed["recovery"]:
        action = "repair"
    elif observed["active"] is None:
        action = "install"
    elif observed["active"] == target["git_sha"] and observed["state"] in {"ready", "busy"}:
        action = "no-change"
    elif observed["active"] == target["git_sha"]:
        action = "repair"
    else:
        action = "activate"
    return {
        "schema": "aquarium-dev-service-plan/v1",
        "project_id": PROJECT_ID,
        "target_git_sha": target["git_sha"],
        "action": action,
        "active_git_sha": observed["active"],
        "busy": observed["busy"],
        "plan_token": None if action == "defer" else _plan_token(action, root, observed, target),
    }


def plan_document(root: Path, generation_root: Path) -> dict[str, Any]:
    root = _normal_absolute(root, "runtime root")
    return _plan_document(root, _generation(generation_root))


def _prepare_root(paths: dict[str, Path]) -> None:
    paths["root"].mkdir(mode=0o700, parents=True, exist_ok=True)
    if paths["root"].is_symlink() or paths["root"].resolve() != paths["root"]:
        raise ControllerError("runtime root is unsafe")
    paths["root"].chmod(0o700)
    for directory in (
        paths["controller"],
        paths["plist"].parent,
        paths["socket"].parent,
        paths["root"] / "state",
        paths["root"] / "logs",
    ):
        directory.mkdir(mode=0o700, exist_ok=True)
        metadata = directory.lstat()
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
            raise ControllerError("runtime directories must not be symbolic links")
        if directory.resolve() != directory:
            raise ControllerError("runtime directories must be canonical")
        directory.chmod(0o700)
    try:
        lock_metadata = paths["lock"].lstat()
    except FileNotFoundError:
        pass
    else:
        if stat.S_ISLNK(lock_metadata.st_mode) or not stat.S_ISREG(lock_metadata.st_mode):
            raise ControllerError("the service lock is unsafe")


def _runtime_metadata(paths: dict[str, Path], generation: dict[str, Any]) -> bytes:
    root = paths["root"]
    return _json_bytes({
        "schema": "podway.managed-runtime/v3",
        "metadata_version": 3,
        "purpose": "aquarium-development",
        "euid": os.geteuid(),
        "canonical_root": os.fspath(root),
        "mode": "dev",
        "paths": {
            "lock": os.fspath(root / "run/podwayd.lock"),
            "socket": os.fspath(root / "run/podwayd.sock"),
            "service_state": os.fspath(root / "state/service.json"),
            "registry": os.fspath(root / "state/workspaces.json"),
            "recovery": os.fspath(root / "state/recovery.json"),
            "log": os.fspath(root / "logs/podwayd.log"),
            "bootstrap_log": os.fspath(root / "logs/podwayd-bootstrap.log"),
        },
        "sandbox_root": None,
        "executables": {
            "cli": {"path": os.fspath(generation["cli"]), "sha256": _file_digest(generation["cli"])},
            "daemon": {"path": os.fspath(generation["daemon"]), "sha256": _file_digest(generation["daemon"])},
            "controller": {"path": os.fspath(generation["controller"]), "sha256": _file_digest(generation["controller"])},
        },
        "generation": generation["git_sha"],
    })


def _plist(paths: dict[str, Path], generation: dict[str, Any]) -> bytes:
    document = {
        "Label": SERVICE_LABEL,
        "ProgramArguments": [os.fspath(generation["daemon"]), "--dev"],
        "EnvironmentVariables": {"PODWAY_DEV_HOME": os.fspath(paths["root"])},
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},
        "ThrottleInterval": 5,
        "ProcessType": "Background",
        "StandardOutPath": "/dev/null",
        "StandardErrorPath": os.fspath(paths["root"] / "logs/podwayd-bootstrap.log"),
    }
    return plistlib.dumps(document, fmt=plistlib.FMT_XML, sort_keys=True)


def _launchctl(*arguments: str) -> subprocess.CompletedProcess[str]:
    return _run(["/bin/launchctl", *arguments])


def _bootout() -> None:
    result = _launchctl("bootout", f"gui/{os.geteuid()}/{SERVICE_LABEL}")
    if result.returncode not in {0, 3, 113} and "Could not find service" not in result.stderr:
        raise ControllerError("cannot stop the prior Aquarium Podway service")


def _bootstrap(plist: Path) -> None:
    result = _launchctl("bootstrap", f"gui/{os.geteuid()}", os.fspath(plist))
    if result.returncode != 0:
        raise ControllerError("cannot start the Aquarium Podway service")


def _wait_ready(paths: dict[str, Path], generation: dict[str, Any]) -> None:
    deadline = time.monotonic() + READY_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        try:
            state = _validate_state_record(
                _read_json(paths["state"]), resolve_generation=False
            )
        except (ControllerError, FileNotFoundError, OSError, json.JSONDecodeError):
            state = None
        remaining = max(0.01, deadline - time.monotonic())
        daemon = (
            _daemon_status(paths, generation, timeout=min(1.0, remaining))
            if state is not None
            else None
        )
        if (
            state is not None
            and state["active_git_sha"] == generation["git_sha"]
            and state["generation_root"] == os.fspath(generation["root"])
            and state["bundle_sha256"] == generation["manifest"]["sha256"]
            and daemon is not None
            and _daemon_matches(daemon, generation)
            and daemon.get("readiness_state") == "ready"
        ):
            verified = _state(paths)
            if verified is not None and verified["active_git_sha"] == generation["git_sha"]:
                return
        time.sleep(0.1)
    raise ControllerError("the selected Aquarium Podway generation did not become ready")


def _install(paths: dict[str, Path], generation: dict[str, Any]) -> None:
    metadata = _runtime_metadata(paths, generation)
    plist = _plist(paths, generation)
    _atomic_write(paths["runtime_metadata"], metadata)
    _atomic_write(paths["plist"], plist)
    state = {
        "schema": STATE_SCHEMA,
        "active_git_sha": generation["git_sha"],
        "generation_root": os.fspath(generation["root"]),
        "bundle_sha256": generation["manifest"]["sha256"],
        "plist_sha256": _digest_bytes(plist),
    }
    _atomic_write(paths["state"], _json_bytes(state))
    _bootstrap(paths["plist"])
    _wait_ready(paths, generation)


def apply(root: Path, generation_root: Path, token: str) -> dict[str, Any]:
    root = _normal_absolute(root, "runtime root")
    paths = _runtime_paths(root)
    target = _generation(generation_root)
    preliminary = _plan_document(root, target)
    if (
        preliminary["action"] == "defer"
        or preliminary["plan_token"] != token
        or not TOKEN_RE.fullmatch(token)
    ):
        raise ControllerError("the service plan token is absent or stale")
    _prepare_root(paths)
    lock_descriptor = os.open(paths["lock"], os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(lock_descriptor, fcntl.LOCK_EX)
        target = _generation(generation_root)
        planned = _plan_document(root, target)
        if planned["action"] == "defer" or planned["plan_token"] != token or not TOKEN_RE.fullmatch(token):
            raise ControllerError("the service plan token is absent or stale")
        if planned["action"] == "no-change":
            return {
                "schema": "aquarium-dev-service-result/v1",
                "project_id": PROJECT_ID,
                "status": "no-change",
                "active_git_sha": target["git_sha"],
                "recovery_required": False,
            }
        existing_recovery = _recovery(paths)
        try:
            current = _state(paths)
        except (ControllerError, OSError, json.JSONDecodeError):
            current = None
        prior = existing_recovery["previous"] if existing_recovery is not None else current
        live = _daemon_status(paths, target)
        if live is not None and _daemon_busy(live):
            raise ControllerError("the active service became busy")
        recovery = existing_recovery or {
            "schema": RECOVERY_SCHEMA,
            "target_git_sha": target["git_sha"],
            "target_generation_root": os.fspath(target["root"]),
            "previous": prior,
            "phase": "prepared",
            "debt": [],
        }
        recovery["target_git_sha"] = target["git_sha"]
        recovery["target_generation_root"] = os.fspath(target["root"])
        recovery["phase"] = "prepared"
        recovery["debt"] = []
        _atomic_write(paths["recovery"], _json_bytes(recovery))
        try:
            if live is not None or current is not None or existing_recovery is not None:
                _bootout()
            recovery["phase"] = "prior-stopped"
            _atomic_write(paths["recovery"], _json_bytes(recovery))
            _install(paths, target)
            paths["recovery"].unlink()
            return {
                "schema": "aquarium-dev-service-result/v1",
                "project_id": PROJECT_ID,
                "status": "repaired" if planned["action"] == "repair" else "activated",
                "active_git_sha": target["git_sha"],
                "recovery_required": False,
            }
        except Exception as activation_error:
            rollback_error: Exception | None = None
            if prior is not None:
                try:
                    _bootout()
                    previous_generation = _generation(Path(prior["generation_root"]))
                    _install(paths, previous_generation)
                    paths["recovery"].unlink()
                except Exception as error:
                    rollback_error = error
            if rollback_error is not None or prior is None:
                recovery["phase"] = "recovery-required"
                recovery["debt"] = ["target-activation-failed"] + (["prior-restoration-unproven"] if rollback_error else [])
                _atomic_write(paths["recovery"], _json_bytes(recovery))
            raise ControllerError("service activation failed; prior restoration was " + ("not proven" if rollback_error or prior is None else "proven")) from activation_error
    finally:
        os.close(lock_descriptor)


def main(arguments: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=("status", "plan", "apply"))
    parser.add_argument("--json", action="store_true", required=True)
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--generation-root", type=Path)
    parser.add_argument("--plan-token")
    options = parser.parse_args(arguments)
    try:
        if options.operation == "status":
            if options.generation_root is not None or options.plan_token is not None:
                raise ControllerError("status accepts only the runtime root")
            result = status_document(options.runtime_root)
        elif options.operation == "plan":
            if options.generation_root is None or options.plan_token is not None:
                raise ControllerError("plan requires exactly one generation root")
            result = plan_document(options.runtime_root, options.generation_root)
        else:
            if options.generation_root is None or options.plan_token is None:
                raise ControllerError("apply requires the generation root and exact token")
            result = apply(options.runtime_root, options.generation_root, options.plan_token)
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0
    except (
        ControllerError,
        OSError,
        UnicodeError,
        json.JSONDecodeError,
        subprocess.SubprocessError,
    ) as error:
        print(f"Aquarium Podway service controller failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
