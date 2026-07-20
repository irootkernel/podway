#!/usr/bin/env python3
"""Fail-closed primitives for the local-only G009 qualification evidence."""
from __future__ import annotations

import errno
import enum
import hashlib
import json
import os
import platform
import re
import select
import signal
import stat
import subprocess
import tempfile
import time
import sys
from fractions import Fraction
from pathlib import Path, PurePosixPath
from typing import Any, Callable

CONTROLLER_ROOT = Path(__file__).resolve().parents[1]


class QualificationError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise QualificationError(message)


def candidate_root(*, required: bool = True) -> Path | None:
    raw = os.environ.get("G009_CANDIDATE_ROOT")
    if not raw:
        if required:
            fail("G009_CANDIDATE_ROOT is required; release tools run only from the trusted controller")
        return None
    supplied = Path(raw)
    if not supplied.is_absolute() or supplied.is_symlink() or not supplied.is_dir():
        fail("G009_CANDIDATE_ROOT must name an absolute, non-symlink candidate directory")
    resolved = supplied.resolve()
    if (
        resolved == CONTROLLER_ROOT
        or resolved.is_relative_to(CONTROLLER_ROOT)
        or CONTROLLER_ROOT.is_relative_to(resolved)
    ):
        fail("G009_CANDIDATE_ROOT must be separate and non-overlapping with the controller root")
    return resolved


def require_candidate_root() -> Path:
    root = candidate_root()
    assert root is not None
    return root


# Candidate-scoped commands require G009_CANDIDATE_ROOT explicitly. Controller-only
# policy and final-review commands remain usable without checking out candidate code.
ROOT = candidate_root(required=False) or CONTROLLER_ROOT
EVIDENCE_ROOT = CONTROLLER_ROOT / "artifacts" / "g009"
TARGET = "aarch64-apple-darwin"
TARGET_TUPLES = {
    "aarch64-apple-darwin": {
        "triple": "aarch64-apple-darwin",
        "arch": "arm64",
        "host_arch": "arm64",
        "mach_o_arch": "arm64",
    },
}
MAX_JSON_BYTES = 16 * 1024 * 1024
MAX_FILE_BYTES = 128 * 1024 * 1024
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
PROCESS_READ_CHUNK_BYTES = 64 * 1024
PROCESS_POST_KILL_DRAIN_SECONDS = 2.0

class ProcessGroupState(enum.Enum):
    ABSENT = "absent"
    LIVE = "live"
    UNKNOWN = "unknown"


def _process_group_state(pgid: int) -> ProcessGroupState:
    try:
        os.killpg(pgid, 0)
    except ProcessLookupError:
        return ProcessGroupState.ABSENT
    except OSError as exc:
        if exc.errno == errno.ESRCH:
            return ProcessGroupState.ABSENT
        return ProcessGroupState.UNKNOWN
    return ProcessGroupState.LIVE


def _wait_for_process_group(
    process: subprocess.Popen[bytes], deadline: float,
) -> ProcessGroupState:
    state = _process_group_state(process.pid)
    while state is not ProcessGroupState.ABSENT and time.monotonic() < deadline:
        process.poll()
        time.sleep(0.01)
        state = _process_group_state(process.pid)
    if process.poll() is None:
        try:
            process.wait(timeout=max(0, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            return ProcessGroupState.UNKNOWN
    return _process_group_state(process.pid)


def _signal_process_group(process: subprocess.Popen[bytes], sig: signal.Signals) -> ProcessGroupState:
    try:
        os.killpg(process.pid, sig)
    except ProcessLookupError:
        return ProcessGroupState.ABSENT
    except OSError as exc:
        if exc.errno == errno.ESRCH:
            return ProcessGroupState.ABSENT
        return ProcessGroupState.UNKNOWN
    return _process_group_state(process.pid)


def _containment_holders(leash_path: Path) -> set[int]:
    """Return processes holding the inherited containment file on Darwin."""
    lsof = Path("/usr/sbin/lsof")
    if platform.system() != "Darwin" or not lsof.is_file():
        fail("bounded process containment requires Darwin lsof")
    try:
        probe = subprocess.run(
            (str(lsof), "-n", "-t", str(leash_path)),
            check=False, capture_output=True, cwd="/", env={"PATH": "/usr/bin:/bin"},
            timeout=2,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        fail(f"bounded process containment observation failed: {exc}")
    if probe.returncode != 0 or probe.stderr:
        fail("bounded process containment observation could not enumerate file holders")
    try:
        holders = {int(line) for line in probe.stdout.decode("ascii", "strict").splitlines() if line}
    except (UnicodeDecodeError, ValueError):
        fail("bounded process containment observation received malformed holder data")
    if os.getpid() not in holders:
        fail("bounded process containment observation cannot see its own containment file")
    return holders


def _containment_escapes(
    process: subprocess.Popen[bytes], leash_path: Path,
) -> set[int]:
    """Find inherited containment holders that left the launched process group."""
    escapes: set[int] = set()
    for pid in _containment_holders(leash_path):
        if pid == os.getpid():
            continue
        try:
            pgid = os.getpgid(pid)
        except ProcessLookupError:
            continue
        except OSError as exc:
            if exc.errno == errno.ESRCH:
                continue
            fail(f"bounded process containment cannot determine holder group: {exc}")
        if pgid != process.pid:
            escapes.add(pid)
    return escapes


def _descendant_pids(root_pid: int) -> set[int]:
    """Return the currently observable descendants before group termination."""
    probe = subprocess.run(
        ("/bin/ps", "-axo", "pid=,ppid="), check=False, capture_output=True,
        cwd="/", env={"PATH": "/usr/bin:/bin"}, timeout=2,
    )
    if probe.returncode != 0 or probe.stderr:
        fail("bounded process cleanup could not enumerate descendants")
    parents: dict[int, set[int]] = {}
    try:
        for line in probe.stdout.decode("ascii", "strict").splitlines():
            pid_text, parent_text = line.split()
            pid, parent = int(pid_text), int(parent_text)
            parents.setdefault(parent, set()).add(pid)
    except (UnicodeDecodeError, ValueError):
        fail("bounded process cleanup received malformed process table")
    pending, descendants = [root_pid], set()
    while pending:
        parent = pending.pop()
        for child in parents.get(parent, set()):
            if child not in descendants:
                descendants.add(child)
                pending.append(child)
    return descendants


def _stabilized_descendants(process: subprocess.Popen[bytes]) -> set[int]:
    """Capture a stable descendant tree before terminating its process group."""
    observed: set[int] = set()
    stable_rounds = 0
    deadline = time.monotonic() + PROCESS_POST_KILL_DRAIN_SECONDS
    while time.monotonic() < deadline:
        current = _descendant_pids(process.pid)
        if current.issubset(observed):
            stable_rounds += 1
            if stable_rounds >= 2:
                return observed
        else:
            observed.update(current)
            stable_rounds = 0
        time.sleep(0.02)
    fail("bounded process cleanup could not stabilize descendant capture")

def _terminate_descendants(
    process: subprocess.Popen[bytes], observed: set[int] | None = None,
) -> set[int]:
    descendants = set() if observed is None else set(observed)
    descendants.update(_stabilized_descendants(process))
    for pid in descendants:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        except OSError as exc:
            fail(f"bounded process cleanup could not signal descendant {pid}: {exc}")
    return descendants

def _kill_remaining_descendants(process: subprocess.Popen[bytes], descendants: set[int]) -> None:
    deadline = time.monotonic() + PROCESS_POST_KILL_DRAIN_SECONDS
    stable_rounds = 0
    while time.monotonic() < deadline:
        descendants.update(_descendant_pids(process.pid))
        live = {pid for pid in descendants if _pid_live(pid)}
        for pid in live:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except OSError as exc:
                fail(f"bounded process cleanup could not kill escaped descendant {pid}: {exc}")
        remaining = {pid for pid in descendants if _pid_live(pid)}
        if not remaining:
            stable_rounds += 1
            if stable_rounds >= 2:
                return
        else:
            stable_rounds = 0
        time.sleep(0.02)
    fail("bounded process cleanup could not prove escaped descendant termination")


def _pid_live(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except OSError as exc:
        if exc.errno == errno.ESRCH:
            return False
        fail(f"bounded process cleanup cannot determine descendant state: {exc}")
    probe = subprocess.run(
        ("/bin/ps", "-o", "stat=", "-p", str(pid)),
        check=False,
        capture_output=True,
        cwd="/",
        env={"PATH": "/usr/bin:/bin"},
        timeout=2,
    )
    if probe.stderr or probe.returncode not in {0, 1}:
        fail("bounded process cleanup cannot inspect descendant state")
    try:
        state = probe.stdout.decode("ascii", "strict").strip()
    except UnicodeDecodeError:
        fail("bounded process cleanup received malformed descendant state")
    return bool(state) and not state.startswith("Z")


def _kill_process_group(
    process: subprocess.Popen[bytes],
    leash_path: Path,
    observed_descendants: set[int] | None = None,
) -> None:
    descendants = set() if observed_descendants is None else set(observed_descendants)
    descendants.update(_containment_holders(leash_path))
    descendants.discard(os.getpid())
    escapes = _containment_escapes(process, leash_path)
    descendants.update(escapes)
    descendants.update(_terminate_descendants(process, descendants))
    state = _signal_process_group(process, signal.SIGTERM)
    if state is not ProcessGroupState.ABSENT:
        state = _wait_for_process_group(process, time.monotonic() + 1)
    if state is not ProcessGroupState.ABSENT:
        state = _signal_process_group(process, signal.SIGKILL)
        if state is not ProcessGroupState.ABSENT:
            state = _wait_for_process_group(
                process, time.monotonic() + PROCESS_POST_KILL_DRAIN_SECONDS,
            )
    if state is not ProcessGroupState.ABSENT:
        fail(
            "bounded process cleanup could not prove process-group termination "
            "(permission denied or process group state unknown)"
        )
    if process.poll() is None:
        try:
            process.wait(timeout=PROCESS_POST_KILL_DRAIN_SECONDS)
        except subprocess.TimeoutExpired:
            fail("bounded process direct child survived process-group termination")
    _kill_remaining_descendants(process, descendants)
    if escapes:
        fail("bounded process containment observed a process-group escape")

def _close_streams(streams: dict[int, tuple[str, Any]]) -> None:
    for _, stream in streams.values():
        try:
            stream.close()
        except BaseException:
            pass
    streams.clear()


def _cleanup_bounded_process(
    process: subprocess.Popen[bytes],
    streams: dict[int, tuple[str, Any]],
    leash_path: Path,
    observed_descendants: set[int] | None = None,
) -> None:
    cleanup_error: BaseException | None = None
    try:
        _kill_process_group(process, leash_path, observed_descendants)
    except BaseException as exc:
        cleanup_error = exc
    finally:
        _close_streams(streams)
        for stream in (process.stdout, process.stderr):
            if stream is not None:
                try:
                    stream.close()
                except BaseException:
                    pass
    if cleanup_error is not None:
        if isinstance(cleanup_error, QualificationError):
            raise cleanup_error
        raise QualificationError(f"bounded process cleanup failed: {cleanup_error}") from cleanup_error


def _contained_process_argv(
    argv: tuple[str, ...], *, allow_descendants: bool,
) -> tuple[str, ...]:
    if platform.system() != "Darwin":
        fail("bounded process containment requires Darwin sandbox-exec")
    sandbox = Path("/usr/bin/sandbox-exec")
    if not sandbox.is_file():
        fail("bounded process containment requires sandbox-exec")
    process_rules = "" if allow_descendants else "(deny process-fork)"
    if Path(argv[0]) == sandbox:
        if len(argv) < 4 or argv[1] != "-p":
            fail("pre-sandboxed bounded process must use an inline profile")
        return (argv[0], argv[1], f"{argv[2]}{process_rules}", *argv[3:])
    return (
        str(sandbox),
        "-p",
        f"(version 1)(allow default){process_rules}",
        *argv,
    )


def bounded_process(
    argv: tuple[str, ...],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: float,
    stream_limit: int,
    aggregate_limit: int,
    observer: Callable[[int], None] | None = None,
    allow_descendants: bool = False,
) -> dict[str, Any]:
    """Run one isolated process group with bounded, concurrently drained output."""
    if timeout <= 0 or stream_limit < 0 or aggregate_limit < 0:
        fail("invalid bounded process limits")
    leash_fd, leash_name = tempfile.mkstemp(prefix="g009-bounded-containment-")
    leash_path = Path(leash_name)
    try:
        process = subprocess.Popen(
            _contained_process_argv(argv, allow_descendants=allow_descendants),
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            pass_fds=(leash_fd,),
        )
    except BaseException:
        os.close(leash_fd)
        leash_path.unlink(missing_ok=True)
        raise
    streams: dict[int, tuple[str, Any]] = {}
    observed_descendants: set[int] = set()
    try:
        if process.stdout is None or process.stderr is None:
            fail("bounded process pipe setup failed")
        streams = {
            process.stdout.fileno(): ("stdout", process.stdout),
            process.stderr.fileno(): ("stderr", process.stderr),
        }
        captured = {"stdout": bytearray(), "stderr": bytearray()}
        overflow = {"stdout": False, "stderr": False}
        reason = "completed"
        deadline = time.monotonic() + timeout
        started_ns = time.monotonic_ns()
        drain_deadline: float | None = None
        while streams:
            observed_descendants.update(_descendant_pids(process.pid))
            now = time.monotonic()
            if observer is not None:
                observer(process.pid)
            if reason == "completed" and now >= deadline:
                reason = "timeout"
                _kill_process_group(process, leash_path, observed_descendants)
                drain_deadline = time.monotonic() + PROCESS_POST_KILL_DRAIN_SECONDS
            if drain_deadline is not None and now >= drain_deadline:
                reason = "post_kill_drain_timeout"
                _close_streams(streams)
                break
            wait_until = drain_deadline if drain_deadline is not None else deadline
            readable, _, _ = select.select(
                list(streams), [], [], max(0, min(wait_until - now, 0.1)),
            )
            for fd in readable:
                name, stream = streams[fd]
                chunk = os.read(fd, PROCESS_READ_CHUNK_BYTES)
                if not chunk:
                    streams.pop(fd)
                    stream.close()
                    continue
                aggregate = len(captured["stdout"]) + len(captured["stderr"])
                permitted = min(stream_limit - len(captured[name]), aggregate_limit - aggregate)
                if permitted > 0:
                    captured[name].extend(chunk[:permitted])
                if len(chunk) > permitted:
                    overflow[name] = True
                    if reason == "completed":
                        reason = "output_overflow"
                        _kill_process_group(process, leash_path, observed_descendants)
                        drain_deadline = time.monotonic() + PROCESS_POST_KILL_DRAIN_SECONDS
        observed_descendants.update(_descendant_pids(process.pid))
        _kill_process_group(process, leash_path, observed_descendants)
        returncode = process.returncode
        if returncode is None:
            fail("bounded process direct child was not reaped")
        terminal = (
            "success" if reason == "completed" and returncode == 0 else
            "nonzero_exit" if reason == "completed" and returncode > 0 else
            "signal" if reason == "completed" and returncode < 0 else reason
        )
        return {
            "stdout": bytes(captured["stdout"]), "stderr": bytes(captured["stderr"]),
            "stdout_overflow": overflow["stdout"], "stderr_overflow": overflow["stderr"],
            "termination_reason": reason, "terminal_mode": terminal,
            "exit_code": returncode if returncode >= 0 else None,
            "signal": -returncode if returncode < 0 else None,
            "timeout": reason == "timeout",
            "elapsed_ns": time.monotonic_ns() - started_ns,
        }
    except BaseException as original:
        try:
            _cleanup_bounded_process(process, streams, leash_path, observed_descendants)
        except QualificationError as cleanup_error:
            raise cleanup_error from original
        raise
    finally:
        os.close(leash_fd)
        leash_path.unlink(missing_ok=True)
def bounded_process_self_test() -> dict[str, str]:
    """Exercise every bounded-process terminal mode without external dependencies."""
    environment = {"PATH": os.environ.get("PATH", "")}
    base = (sys.executable, "-c")
    cases = {
        "success": ("import sys; print('ok')", 1, 1024, 2048, "success"),
        "nonzero": ("import sys; sys.exit(7)", 1, 1024, 2048, "nonzero_exit"),
        "signal": ("import os, signal; os.kill(os.getpid(), signal.SIGTERM)", 1, 1024, 2048, "signal"),
        "timeout": ("import time; time.sleep(10)", 0.05, 1024, 2048, "timeout"),
        "stdout_overflow": ("import sys; sys.stdout.write('x' * 32)", 1, 16, 64, "output_overflow"),
        "stderr_overflow": ("import sys; sys.stderr.write('x' * 32)", 1, 16, 64, "output_overflow"),
        "simultaneous_pipes": (
            "import os; os.write(1, b'o' * 32); os.write(2, b'e' * 32)",
            1, 64, 128, "success",
        ),
    }
    observed: dict[str, str] = {}
    with tempfile.TemporaryDirectory(prefix="g009-bounded-process-") as raw:
        cwd = Path(raw)
        for name, (program, timeout, stream_limit, aggregate_limit, expected) in cases.items():
            result = bounded_process(
                (*base, program), cwd=cwd, env=environment, timeout=timeout,
                stream_limit=stream_limit, aggregate_limit=aggregate_limit,
            )
            if result["terminal_mode"] != expected:
                fail(f"bounded process self-test {name} returned {result['terminal_mode']}")
            if name == "nonzero" and result["exit_code"] != 7:
                fail("bounded process self-test lost nonzero exit status")
            if name == "signal" and result["signal"] != signal.SIGTERM:
                fail("bounded process self-test lost signal status")
            if name == "stdout_overflow" and not result["stdout_overflow"]:
                fail("bounded process self-test missed stdout overflow")
            if name == "stderr_overflow" and not result["stderr_overflow"]:
                fail("bounded process self-test missed stderr overflow")
            if name == "simultaneous_pipes" and (result["stdout"] != b"o" * 32 or result["stderr"] != b"e" * 32):
                fail("bounded process self-test did not drain simultaneous pipes")
            observed[name] = expected
        escaped_pid_path = cwd / "immediate-double-fork.pid"
        escaped = (
            "import os; os.fork(); "
            f"open({str(escaped_pid_path)!r}, 'w').write(str(os.getpid()))"
        )
        result = bounded_process(
            (*base, escaped), cwd=cwd, env=environment, timeout=1,
            stream_limit=1024, aggregate_limit=2048,
        )
        if result["terminal_mode"] != "nonzero_exit":
            fail("bounded process self-test did not deny descendant creation")
        if escaped_pid_path.exists():
            fail("bounded process self-test executed after denied descendant creation")
        observed["immediate_double_fork_escape"] = "denied_before_creation"
        contained_pid_path = cwd / "allowed-descendant.pid"
        contained = (
            "import os,time; "
            "pid=os.fork(); "
            "(os._exit(0) if pid else None); "
            "os.close(1); os.close(2); "
            f"open({str(contained_pid_path)!r}, 'w').write(str(os.getpid())); "
            "time.sleep(10)"
        )
        result = bounded_process(
            (*base, contained),
            cwd=cwd,
            env=environment,
            timeout=1,
            stream_limit=1024,
            aggregate_limit=2048,
            allow_descendants=True,
        )
        if result["terminal_mode"] != "success":
            fail("bounded process self-test changed the direct-child terminal result")
        if not contained_pid_path.is_file():
            fail("bounded process self-test did not create the allowed descendant fixture")
        contained_pid = int(contained_pid_path.read_text())
        if _pid_live(contained_pid):
            fail("bounded process self-test leaked a closed-pipe descendant")
        observed["allowed_descendant_cleanup"] = "reaped"
        observer_pid: list[int] = []

        def failing_observer(pid: int) -> None:
            observer_pid.append(pid)
            raise RuntimeError("injected observer failure")

        try:
            bounded_process(
                (*base, "import time; time.sleep(10)"), cwd=cwd, env=environment,
                timeout=1, stream_limit=1024, aggregate_limit=2048,
                observer=failing_observer,
            )
        except RuntimeError as exc:
            if str(exc) != "injected observer failure":
                fail(f"bounded process self-test observer failure changed: {exc}")
        else:
            fail("bounded process self-test observer failure was not raised")
        if len(observer_pid) != 1 or _process_group_state(observer_pid[0]) is not ProcessGroupState.ABSENT:
            fail("bounded process self-test observer failure leaked a process group")
        observed["observer_failure_cleanup"] = "reaped"

        original_killpg = os.killpg
        probe_pid: list[int] = []

        def eperm_probe(pgid: int, sig: int | signal.Signals) -> None:
            raise PermissionError(errno.EPERM, "injected process-group permission denial")

        try:
            os.killpg = eperm_probe
            try:
                bounded_process(
                    (*base, "import time; time.sleep(10)"), cwd=cwd, env=environment,
                    timeout=0.05, stream_limit=1024, aggregate_limit=2048,
                    observer=lambda pid: probe_pid.append(pid),
                )
            except QualificationError as exc:
                if "could not prove process-group termination" not in str(exc):
                    fail(f"bounded process self-test EPERM cleanup misclassified: {exc}")
            else:
                fail("bounded process self-test EPERM probe reported successful cleanup")
        finally:
            os.killpg = original_killpg
        if not probe_pid:
            fail("bounded process self-test EPERM probe did not observe the child")
        denied_pid = probe_pid[0]
        try:
            original_killpg(denied_pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            os.waitpid(denied_pid, 0)
        except ChildProcessError:
            pass
        if _process_group_state(denied_pid) is not ProcessGroupState.ABSENT:
            fail("bounded process self-test EPERM cleanup fixture survived explicit cleanup")
        observed["eperm_probe_cleanup"] = "fail_closed"
    return observed

def _no_duplicate_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON key: {key}")
        result[key] = value
    return result

def _reject_constant(value: str) -> None:
    fail(f"non-finite JSON value: {value}")

def bounded_bytes(path: Path, limit: int = MAX_FILE_BYTES) -> bytes:
    """Read one regular, non-symlink file through a no-follow descriptor."""
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    except OSError as exc:
        fail(f"cannot safely open {path}: {exc}")
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode):
            fail(f"input is not a regular file: {path}")
        if metadata.st_size > limit:
            fail(f"read exceeds {limit} byte limit: {path}")
        with os.fdopen(fd, "rb", closefd=False) as handle:
            data = handle.read(limit + 1)
    except OSError as exc:
        fail(f"cannot read {path}: {exc}")
    finally:
        os.close(fd)
    if len(data) > limit:
        fail(f"read exceeds {limit} byte limit: {path}")
    return data
def bounded_regular_tree(
    root: Path,
    *,
    member_limit: int,
    path_depth: int,
    path_length: int,
    label: str,
) -> list[tuple[str, Path, int]]:
    """Enumerate one regular-file tree incrementally without following links."""
    if (
        member_limit < 1
        or path_depth < 1
        or path_length < 1
        or root.is_symlink()
        or not root.is_dir()
    ):
        fail(f"{label} root or limits are unsafe")
    pending: list[tuple[Path, tuple[str, ...]]] = [(root, ())]
    files: list[tuple[str, Path, int]] = []
    entry_count = 0
    while pending:
        directory, prefix = pending.pop()
        bounded_entries: list[os.DirEntry[str]] = []
        try:
            with os.scandir(directory) as iterator:
                for entry in iterator:
                    entry_count += 1
                    if entry_count > member_limit:
                        fail(f"{label} member count exceeds frozen limit")
                    bounded_entries.append(entry)
        except OSError as exc:
            fail(f"{label} directory cannot be enumerated: {exc}")
        for entry in sorted(bounded_entries, key=lambda item: item.name, reverse=True):
            parts = (*prefix, entry.name)
            relative = Path(*parts).as_posix()
            if len(parts) > path_depth or len(relative) > path_length:
                fail(f"{label} member path exceeds frozen limit")
            try:
                metadata = entry.stat(follow_symlinks=False)
            except OSError as exc:
                fail(f"{label} member cannot be inspected: {exc}")
            path = Path(entry.path)
            if entry.is_symlink():
                fail(f"{label} contains a symlink")
            if stat.S_ISDIR(metadata.st_mode):
                pending.append((path, parts))
            elif stat.S_ISREG(metadata.st_mode):
                files.append((relative, path, metadata.st_size))
            else:
                fail(f"{label} contains a non-regular member")
    return sorted(files, key=lambda item: item[0])

def load_json_bytes(raw: bytes, label: str = "<bytes>", limit: int = MAX_JSON_BYTES) -> Any:
    if len(raw) > limit:
        fail(f"read exceeds {limit} byte limit: {label}")
    try:
        return json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=_no_duplicate_object,
            parse_constant=_reject_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(f"invalid JSON {label}: {exc}")


def load_json(path: Path, limit: int = MAX_JSON_BYTES) -> Any:
    return load_json_bytes(bounded_bytes(path, limit), str(path), limit)

def _canonical(value: Any) -> str:
    if value is None: return "null"
    if value is True: return "true"
    if value is False: return "false"
    if isinstance(value, int): return str(value)
    if isinstance(value, float): fail("floating point values are forbidden in authoritative JSON")
    if isinstance(value, str): return json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    if isinstance(value, list): return "[" + ",".join(_canonical(item) for item in value) + "]"
    if isinstance(value, dict):
        if not all(isinstance(key, str) for key in value): fail("JSON object key is not a string")
        return "{" + ",".join(_canonical(key) + ":" + _canonical(value[key]) for key in sorted(value)) + "}"
    fail(f"non-JSON value: {type(value).__name__}")

def canonical_json(value: Any) -> bytes:
    return _canonical(value).encode("utf-8")

def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()

def sha256_file(path: Path) -> str:
    return sha256_bytes(bounded_bytes(path))

def fraction_value(value: Any, label: str) -> Fraction:
    if not isinstance(value, dict) or set(value) != {"numerator", "denominator"}:
        fail(f"{label} must be {{numerator,denominator}}")
    numerator, denominator = value["numerator"], value["denominator"]
    if not isinstance(numerator, int) or not isinstance(denominator, int) or denominator <= 0:
        fail(f"{label} has invalid fraction")
    return Fraction(numerator, denominator)

def fraction_json(value: Fraction) -> dict[str, int]:
    return {"numerator": value.numerator, "denominator": value.denominator}

def safe_relative(path: str) -> PurePosixPath:
    candidate = PurePosixPath(path)
    if not path or candidate.is_absolute() or ".." in candidate.parts or any(part in ("", ".") for part in candidate.parts):
        fail(f"unsafe relative path: {path!r}")
    return candidate

def archive_root(target: str) -> str:
    target_tuple(target)
    return f"podway-0.1.0-{target}"


def target_tuple(target: str) -> dict[str, str]:
    if not isinstance(target, str):
        fail(f"unsupported native target: {target!r}")
    result = TARGET_TUPLES.get(target)
    if result is None:
        fail(f"unsupported native target: {target}")
    return result


def profile_target_tuple(value: Any) -> dict[str, str]:
    if not isinstance(value, dict):
        fail("profile target must be an object")
    triple = value.get("triple")
    expected = target_tuple(triple) if isinstance(triple, str) else None
    if expected is None or value != {
        **expected,
        "native_required": True,
        "universal_forbidden": True,
    }:
        fail("profile target is not an exact native target tuple")
    return expected


def safe_extract_member(name: str, target: str) -> PurePosixPath:
    candidate = safe_relative(name)
    if candidate.parts[0] != archive_root(target):
        fail(f"archive member outside required root: {name}")
    return candidate

def atomic_immutable_json(path: Path, value: Any) -> str:
    root = EVIDENCE_ROOT.resolve()
    try:
        relative = path.relative_to(EVIDENCE_ROOT)
    except ValueError:
        fail(f"evidence path outside {EVIDENCE_ROOT}: {path}")
    if any(part in ("", ".", "..") for part in relative.parts):
        fail("unsafe evidence path")
    current = EVIDENCE_ROOT
    if current.is_symlink():
        fail("evidence root may not be a symlink")
    for part in relative.parts[:-1]:
        current = current / part
        if current.exists() and (current.is_symlink() or not current.is_dir()):
            fail(f"unsafe evidence directory: {current}")
        if not current.exists():
            current.mkdir(mode=0o755)
    if path.exists() and path.is_symlink():
        fail(f"evidence target may not be a symlink: {path}")
    if path.parent.resolve() != (root / relative.parent).resolve() or not path.parent.resolve().is_relative_to(root):
        fail("evidence path traversal")
    payload = canonical_json(value)
    digest = sha256_bytes(payload)
    if path.exists():
        if not path.is_file() or bounded_bytes(path) != payload: fail(f"immutable evidence already differs: {path}")
        return digest
    fd, temp_name = tempfile.mkstemp(prefix=".g009-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(payload); handle.flush(); os.fsync(handle.fileno())
        os.chmod(temp_name, stat.S_IRUSR | stat.S_IRGRP | stat.S_IROTH)
        try: os.link(temp_name, path)
        except FileExistsError:
            if path.is_symlink() or not path.is_file() or bounded_bytes(path) != payload: fail(f"immutable evidence race differs: {path}")
        finally: os.unlink(temp_name)
    finally:
        if os.path.exists(temp_name): os.unlink(temp_name)
    return digest

def content_addressed_json(category: str, value: Any) -> tuple[Path, str]:
    payload = canonical_json(value); digest = sha256_bytes(payload)
    path = EVIDENCE_ROOT / category / f"{digest}.json"
    atomic_immutable_json(path, value)
    return path, digest

def host_manifest() -> dict[str, str]:
    return {"machine": platform.machine(), "system": platform.system(), "platform": platform.platform()}

class TranslationState(enum.Enum):
    UNTRANSLATED = "untranslated"
    TRANSLATED = "translated"
    MISSING = "missing"
    UNKNOWN = "unknown"


_MISSING_TRANSLATION_OID = re.compile(
    rb"sysctl:\s*unknown oid ['\"]sysctl\.proc_translated['\"]\s*"
)


def _translation_state(probe: subprocess.CompletedProcess[bytes]) -> TranslationState:
    """Classify the exact sysctl.proc_translated probe outcomes we trust."""
    if (
        not isinstance(probe.returncode, int)
        or not isinstance(probe.stdout, bytes)
        or not isinstance(probe.stderr, bytes)
    ):
        return TranslationState.UNKNOWN
    if probe.returncode == 0 and not probe.stderr:
        if probe.stdout.strip() == b"0":
            return TranslationState.UNTRANSLATED
        if probe.stdout.strip() == b"1":
            return TranslationState.TRANSLATED
    if (
        probe.returncode == 1
        and not probe.stdout
        and _MISSING_TRANSLATION_OID.fullmatch(probe.stderr)
    ):
        return TranslationState.MISSING
    return TranslationState.UNKNOWN


def _translation_probe() -> TranslationState:
    try:
        probe = subprocess.run(
            ("/usr/sbin/sysctl", "-in", "sysctl.proc_translated"),
            check=False,
            capture_output=True,
            cwd=CONTROLLER_ROOT,
            env={"PATH": os.environ.get("PATH", "")},
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return TranslationState.UNKNOWN
    return _translation_state(probe)


def native_host_self_test() -> dict[str, str]:
    """Exercise translation-probe classification without querying the host."""
    cases = {
        "untranslated": (
            subprocess.CompletedProcess((), 0, b"0\n", b""),
            TranslationState.UNTRANSLATED,
        ),
        "translated": (
            subprocess.CompletedProcess((), 0, b"1\n", b""),
            TranslationState.TRANSLATED,
        ),
        "missing": (
            subprocess.CompletedProcess(
                (), 1, b"", b"sysctl: unknown oid 'sysctl.proc_translated'\n",
            ),
            TranslationState.MISSING,
        ),
        "malformed": (
            subprocess.CompletedProcess((), 1, b"0\n", b""),
            TranslationState.UNKNOWN,
        ),
        "wrong_exit": (
            subprocess.CompletedProcess(
                (), 2, b"", b"sysctl: unknown oid 'sysctl.proc_translated'\n",
            ),
            TranslationState.UNKNOWN,
        ),
    }
    observed: dict[str, str] = {}
    for name, (probe, expected) in cases.items():
        actual = _translation_state(probe)
        if actual is not expected:
            fail(f"native host self-test misclassified {name}")
        observed[name] = actual.value

    original_host_manifest = host_manifest
    original_translation_probe = _translation_probe
    try:
        globals()["host_manifest"] = lambda: {
            "system": "Darwin", "machine": "arm64", "platform": "test",
        }
        globals()["_translation_probe"] = lambda: TranslationState.UNTRANSLATED
        require_native_host("aarch64-apple-darwin")
        globals()["_translation_probe"] = lambda: TranslationState.TRANSLATED
        try:
            require_native_host("aarch64-apple-darwin")
        except QualificationError:
            observed["translated_arm64"] = "rejected"
        else:
            fail("native host self-test accepted translated arm64")
        globals()["_translation_probe"] = lambda: TranslationState.MISSING
        try:
            require_native_host("aarch64-apple-darwin")
        except QualificationError:
            observed["missing_arm64"] = "rejected"
        else:
            fail("native host self-test accepted missing arm64 translation probe")
        try:
            require_native_host("x86_64-apple-darwin")
        except QualificationError:
            observed["unsupported_x86_64"] = "rejected"
        else:
            fail("native host self-test accepted unsupported x86_64")
        try:
            target_tuple(None)
        except QualificationError:
            observed["malformed_target"] = "rejected"
        else:
            fail("native host self-test accepted a malformed target")
    finally:
        globals()["host_manifest"] = original_host_manifest
        globals()["_translation_probe"] = original_translation_probe
    return observed


def require_native_host(target: str) -> dict[str, str]:
    expected = target_tuple(target)
    host = host_manifest()
    if host["system"] != "Darwin" or host["machine"] != expected["host_arch"]:
        fail(
            f"requires native Darwin {expected['host_arch']} host, got "
            f"{host['system']} {host['machine']}"
        )
    state = _translation_probe()
    if state is TranslationState.UNTRANSLATED:
        return expected
    fail(f"requires an untranslated native {expected['host_arch']} process")



def require_digest(value: Any, label: str) -> str:
    if not isinstance(value, str) or not SHA256_RE.fullmatch(value): fail(f"invalid {label} SHA-256")
    return value

def require_object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict): fail(f"{label} must be an object")
    return value
