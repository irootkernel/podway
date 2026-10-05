#!/usr/bin/env python3
"""Run the V2DFC-001 feasibility gate and evaluate its pre-registered thresholds.

The driver builds the release binaries, the direct-path example, and the gate harness; runs
the ignored harness checks; measures the shipped daemon path in an isolated
release-qualification runtime against the direct path; and writes raw samples and one
verdict report under an ignored output directory. `V2DFC-007` re-measures G7 and G8 with it
and then deletes it.
"""

from __future__ import annotations

import argparse
import datetime
import json
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from typing import Any, Callable

import dev_runtime as runtime
import release_archive

ROOT = runtime.ROOT
REPORT_SCHEMA = "podway.v2dfc001-gate-report/v1"
L_FIXTURE_BYTES = 43_384_832
GATE_NODES = 300
ITEM_COUNT = 16
FIXTURE_BATCHES_PER_NODE = 6
FIXTURE_ITEM_CHARS = 55_000
MEASURED_ITEM_CHARS = 4_096
HOT_WAL_BYTES = 4 * 1024 * 1024
CHECK_TIMEOUT_SECONDS = 3_600
COMMAND_TIMEOUT_SECONDS = 120
HARNESS_TESTS = {
    "G10": "v2dfc001_g10_legacy_classification",
    "G6": "v2dfc001_g6_preparation_races",
    "G1-G2": "v2dfc001_g1_g2_multi_process_correctness",
    "G2-scale": "v2dfc001_g2_scale_information",
    "G3": "v2dfc001_g3_over_deadline",
    "G4": "v2dfc001_g4_crash_boundaries",
    "G5": "v2dfc001_g5_maintenance_exclusion",
    "G9": "v2dfc001_g9_wal_bound",
}
HARNESS_DIRECTORIES = {
    "G10": "g10",
    "G6": "g6",
    "G1-G2": "g1-g2",
    "G2-scale": "g2-scale",
    "G3": "g3",
    "G4": "g4",
    "G5": "g5",
    "G9": "g9",
}
CHECK_ORDER = ["runtime", "G10", "G6", "G1-G2", "G2-scale", "G3", "G4", "G5", "G9"]
MEASURED_SOURCES = (
    "crates/podway-store/src/test_support.rs",
    "crates/podway-store/src/test_support/v2dfc001.rs",
    "crates/podway-daemon/tests/int_suite.rs",
    "crates/podway-daemon/tests/int_v2dfc001_feasibility.rs",
    "crates/podway-daemon/examples/v2dfc001_direct.rs",
    "tools/v2dfc_gate.py",
)
THRESHOLDS = {
    "g2_wait_p99_us": 250_000,
    "g2_wait_max_us": 1_000_000,
    "g2_hold_p99_us": 50_000,
    "g2_process_ratio": 2.0,
    "g3_bound_us": 600_000,
    "g5_acquisition_p99_us": 2_000_000,
    "g8_warm_p99_us": 100_000,
    "g8_cold_p99_us": 1_000_000,
    "g9_load_max_bytes": 64 * 1024 * 1024,
    "g9_drain_bytes": 4 * 1024 * 1024,
    "g9_drain_ms": 5_000,
}
PROFILES = {
    "evidence": {
        "g7_warmup": 20,
        "g7_measured": 220,
        "g7_managed_control": 20,
        "e1_warmup": 20,
        "e1_measured": 200,
        "g8_warmup": 20,
        "g8_measured": 220,
        "e4_warmup": 3,
        "e4_measured": 20,
    },
    "quick": {
        "g7_warmup": 2,
        "g7_measured": 10,
        "g7_managed_control": 3,
        "e1_warmup": 2,
        "e1_measured": 10,
        "g8_warmup": 2,
        "g8_measured": 10,
        "e4_warmup": 1,
        "e4_measured": 2,
    },
}


class GateError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise GateError(message)


def utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def progress(message: str) -> None:
    print(f"[v2dfc001 {utc_now()}] {message}", flush=True)


def percentile(values: list[float], quantile: float) -> float:
    """Nearest-rank percentile: the smallest value with at least `quantile`% at or below it."""
    if not values:
        fail("a percentile needs at least one sample")
    ordered = sorted(values)
    rank = max(1, math.ceil(quantile / 100 * len(ordered)))
    return ordered[rank - 1]


def statistics(values: list[float]) -> dict[str, Any]:
    return {
        "n": len(values),
        "p50": percentile(values, 50),
        "p95": percentile(values, 95),
        "p99": percentile(values, 99),
        "max": max(values),
    }


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]


def run_command(
    arguments: list[str],
    *,
    cwd: Path,
    environment: dict[str, str] | None = None,
    stdin: bytes | None = None,
    timeout: float = COMMAND_TIMEOUT_SECONDS,
) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        arguments,
        cwd=cwd,
        env=environment,
        input=stdin,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        check=False,
    )


def require_success(completed: subprocess.CompletedProcess[bytes], label: str) -> bytes:
    if completed.returncode != 0:
        fail(
            f"{label} failed with exit {completed.returncode}: "
            f"{completed.stdout.decode('utf-8', 'replace')[-2000:]} "
            f"{completed.stderr.decode('utf-8', 'replace')[-2000:]}"
        )
    return completed.stdout


def host_facts() -> dict[str, Any]:
    def text(arguments: list[str]) -> str:
        completed = run_command(arguments, cwd=ROOT)
        return completed.stdout.decode("utf-8", "replace").strip()

    power = text(["pmset", "-g", "batt"])
    settings = text(["pmset", "-g"])
    return {
        "cpu": text(["sysctl", "-n", "machdep.cpu.brand_string"]),
        "logical_cpus": int(text(["sysctl", "-n", "hw.ncpu"])),
        "memory_bytes": int(text(["sysctl", "-n", "hw.memsize"])),
        "macos": text(["sw_vers", "-productVersion"]),
        "macos_build": text(["sw_vers", "-buildVersion"]),
        "ac_power": "AC Power" in power,
        "low_power_mode": any(
            line.split()[-1] == "1"
            for line in settings.splitlines()
            if line.strip().startswith("lowpowermode")
        ),
        "load_average": os.getloadavg(),
    }


def production_daemons() -> list[dict[str, Any]]:
    """Read-only observation of installed service daemons, which the gate never touches."""
    completed = run_command(["ps", "-Ao", "pid=,%cpu=,command="], cwd=ROOT)
    daemons = []
    for line in completed.stdout.decode("utf-8", "replace").splitlines():
        fields = line.split(None, 2)
        if len(fields) == 3 and fields[2].split(" ", 1)[0].endswith("/podwayd") and " --service" in fields[2]:
            daemons.append({"pid": int(fields[0]), "cpu_percent": float(fields[1])})
    return daemons


def candidate_identity() -> dict[str, Any]:
    def git(arguments: list[str]) -> str:
        return require_success(run_command(["git", *arguments], cwd=ROOT), "git").decode().strip()

    sources = {}
    for relative in MEASURED_SOURCES:
        sources[relative] = "sha256:" + release_archive.sha256_file(ROOT / relative)
    return {
        "base_head": git(["rev-parse", "HEAD"]),
        "index_tree": git(["write-tree"]),
        "measured_sources": sources,
        "unstaged_measured_changes": git(["diff", "--name-only", "--", *MEASURED_SOURCES]).split(),
    }


def build(output: Path) -> dict[str, Path]:
    cargo = runtime.pinned_cargo()
    environment = dict(os.environ)
    environment["PATH"] = f"{cargo.parent}:{environment.get('PATH', '/usr/bin:/bin')}"
    environment["CARGO_INCREMENTAL"] = "0"
    target = runtime.cargo_target_directory() / "release"
    steps = [
        ["build", "--release", "--locked", "-p", "podway-cli", "--bin", "podway",
         "-p", "podway-daemon", "--bin", "podwayd"],
        ["build", "--release", "--locked", "-p", "podway-daemon", "--example", "v2dfc001_direct"],
    ]
    for step in steps:
        progress(f"cargo {' '.join(step)}")
        require_success(
            run_command(
                [cargo.as_posix(), *step],
                cwd=ROOT,
                environment=environment,
                timeout=CHECK_TIMEOUT_SECONDS,
            ),
            f"cargo {step[0]}",
        )
    progress("cargo test --release --no-run (gate harness)")
    completed = run_command(
        [
            cargo.as_posix(), "test", "--release", "--locked", "-p", "podway-daemon",
            "--test", "int_suite", "--no-run", "--message-format=json",
        ],
        cwd=ROOT,
        environment=environment,
        timeout=CHECK_TIMEOUT_SECONDS,
    )
    stdout = require_success(completed, "cargo test --no-run")
    harness = None
    for line in stdout.decode("utf-8").splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        target_info = message.get("target", {})
        if (
            message.get("reason") == "compiler-artifact"
            and target_info.get("name") == "int_suite"
            and "test" in target_info.get("kind", [])
            and isinstance(message.get("executable"), str)
            and "podway-daemon" in message.get("package_id", "")
        ):
            harness = Path(message["executable"])
    if harness is None:
        fail("cargo did not report the podway-daemon int_suite executable")
    binaries = {
        "cli": target / "podway",
        "daemon": target / "podwayd",
        "direct": target / "examples" / "v2dfc001_direct",
        "harness": harness,
    }
    for name, path in binaries.items():
        if not path.is_file():
            fail(f"release {name} binary is missing: {path}")
    write_json(
        output / "build.json",
        {
            name: {"path": path.name, "sha256": "sha256:" + release_archive.sha256_file(path)}
            for name, path in binaries.items()
        },
    )
    return binaries


def procedure_text(nodes: int) -> str:
    items = "".join(
        f"      - id: i{index:02}\n        type: text\n        prompt: Record payload {index}.\n"
        "        required: false\n        max_length: 65536\n"
        for index in range(1, ITEM_COUNT + 1)
    )
    placements = "".join(
        f"    - id: n{node:04}\n      use: work\n"
        + ("      terminal: true\n" if node == nodes else f"      next: n{node + 1:04}\n")
        for node in range(1, nodes + 1)
    )
    return (
        "schema: podway.procedure/v2\nid: v2dfc001-gate\nversion: \"1\"\nname: V2DFC-001 gate\n"
        "purpose: Measure the direct-write model.\nnode_definitions:\n  work:\n    type: action\n"
        "    title: Record payloads\n    intent: Record gate payloads.\n    items:\n"
        f"{items}graph:\n  entry: n0001\n  nodes:\n{placements}"
    )


def payload(tag: str, length: int) -> str:
    value = (tag + "-")[:length]
    filler = "".join(chr(ord("a") + index % 26) for index in range(length))
    return (value + filler[len(value):])[:length]


def envelope_result(stdout: bytes, command: str) -> tuple[dict[str, Any], dict[str, Any]]:
    envelope = json.loads(stdout)
    if envelope.get("schema") != "podway.output/v3" or envelope.get("command") != command:
        fail(f"{command} returned an unexpected envelope: {stdout[:400]!r}")
    return envelope, envelope["result"]


class FencesV1:
    """Fences derived from one observation of the active attempt."""

    def __init__(self, envelope: dict[str, Any], result: dict[str, Any]) -> None:
        status = result["status"]
        self.workspace_uuid = envelope["workspace"]["uuid"]
        self.session_id = status["session"]["id"]
        self.session_revision = status["session"]["revision"]
        self.attempt_id = status["current"]["attempt"]["attempt_id"]
        self.item_revisions = {
            item["item_id"]: item["revision"] for item in result.get("active_items", [])
        }
        for index in range(1, ITEM_COUNT + 1):
            self.item_revisions.setdefault(f"i{index:02}", 0)


def executable_identity(path: Path) -> dict[str, str]:
    return {"path": str(path), "sha256": "sha256:" + release_archive.sha256_file(path)}


class IsolatedRuntimeV1:
    """One owner-private release-qualification runtime with snapshot executables."""

    def __init__(self, cli: Path, daemon: Path) -> None:
        self.root = Path(
            tempfile.mkdtemp(prefix=f"podway-release-qa-{os.geteuid()}-", dir="/private/tmp")
        )
        self.root.chmod(0o700)
        self.account = self.root / "account"
        self.sandbox = self.root / "sandbox"
        snapshot = self.root / "snapshots" / release_archive.sha256_file(daemon)[:16]
        for directory in (
            self.account, self.account / ".podway", self.account / ".podway/run", self.sandbox,
            self.root / "snapshots", snapshot, self.root / "run", self.root / "state", self.root / "logs",
        ):
            directory.mkdir(mode=0o700)
        self.cli = release_archive.snapshot_executable(cli, snapshot / "podway", "podway")
        self.daemon = release_archive.snapshot_executable(daemon, snapshot / "podwayd", "podwayd")
        metadata = {
            "schema": "podway.managed-runtime/v3",
            "metadata_version": 3,
            "purpose": "release-qualification",
            "euid": os.geteuid(),
            "canonical_root": str(self.root),
            "mode": runtime.RELEASE_QA_MODE,
            "paths": {
                "lock": str(self.root / "run/podwayd.lock"),
                "socket": str(self.root / "run/podwayd.sock"),
                "service_state": str(self.root / "state/service.json"),
                "registry": str(self.root / "state/workspaces.json"),
                "recovery": str(self.root / "state/recovery.json"),
                "log": str(self.root / "logs/podwayd.log"),
                "bootstrap_log": str(self.root / "logs/podwayd-bootstrap.log"),
            },
            "sandbox_root": str(self.sandbox),
            "executables": {
                "cli": executable_identity(self.cli),
                "daemon": executable_identity(self.daemon),
                "controller": None,
            },
            "generation": None,
        }
        metadata_path = self.root / "runtime.json"
        release_archive.write_json(metadata_path, metadata)
        metadata_path.chmod(0o600)
        self.socket = self.root / "run/podwayd.sock"
        self.process: subprocess.Popen[bytes] | None = None

    def start(self) -> None:
        self.process = runtime.start_isolated_daemon(
            self.daemon, self.account, self.root, cli=self.cli,
            label="V2DFC-001 baseline daemon", mode=runtime.RELEASE_QA_MODE,
        )

    def stop(self) -> None:
        if self.process is not None:
            runtime.stop_process(self.process)
            code = self.process.returncode
            self.process = None
            if code != 0:
                fail(f"the baseline daemon exited with {code}")
        if self.socket.exists():
            fail(f"the baseline daemon left its socket behind: {self.socket}")

    def managed(self, worktree: Path, arguments: list[str], stdin: bytes | None = None) -> bytes:
        completed = run_command(
            [self.cli.as_posix(), "--mode", runtime.RELEASE_QA_MODE, "--json", *arguments],
            cwd=worktree,
            environment=runtime.isolation_environment(self.account, self.root),
            stdin=stdin,
        )
        return require_success(completed, f"podway {' '.join(arguments[:2])}")

    def socket_command(self) -> list[str]:
        return [self.cli.as_posix(), "--socket", self.socket.as_posix(), "--json"]

    def socket_environment(self) -> dict[str, str]:
        return {"PATH": "/usr/bin:/bin", "HOME": self.account.as_posix()}

    def cleanup(self) -> dict[str, Any]:
        self.stop()
        remaining_sockets = [str(path) for path in self.root.glob("**/podwayd.sock")]
        processes = run_command(["ps", "-Ao", "command="], cwd=ROOT)
        if processes.returncode != 0:
            fail("cannot prove release-qualification process cleanup")
        expected = f"{self.daemon} --mode {runtime.RELEASE_QA_MODE}"
        remaining_processes = [
            line.strip()
            for line in processes.stdout.decode("utf-8", "replace").splitlines()
            if line.strip() == expected
        ]
        if remaining_sockets or remaining_processes:
            fail(
                f"isolated runtime cleanup left state behind; recoverable root: {self.root}; "
                f"sockets={remaining_sockets}; processes={remaining_processes}"
            )
        shutil.rmtree(self.root)
        if self.root.exists():
            fail(f"isolated runtime root remains: {self.root}")
        return {"sockets_remaining": 0, "processes_remaining": 0, "root_removed": True}


def record_document(fences: FencesV1, key: str, values: dict[str, str]) -> bytes:
    document = {
        "schema": "podway.item-record-many-input/v1",
        "workspace_uuid": fences.workspace_uuid,
        "session_id": fences.session_id,
        "session_revision": fences.session_revision,
        "attempt_id": fences.attempt_id,
        "idempotency_key": key,
        "operations": [
            {
                "item_id": item,
                "expected_item_revision": fences.item_revisions[item],
                "record": {"type": "text", "value": value},
            }
            for item, value in values.items()
        ],
    }
    return json.dumps(document, separators=(",", ":")).encode("utf-8")


def managed_observe(rt: IsolatedRuntimeV1, worktree: Path) -> FencesV1:
    envelope, result = envelope_result(rt.managed(worktree, ["observe"]), "session.observe")
    return FencesV1(envelope, result)


def build_fixtures(rt: IsolatedRuntimeV1, output: Path) -> dict[str, dict[str, Any]]:
    fixtures: dict[str, dict[str, Any]] = {}
    rt.start()
    for name in ("S", "M", "L"):
        worktree = rt.sandbox / name.lower()
        worktree.mkdir(mode=0o700)
        runtime.initialize_sandbox(worktree)
        (worktree / "gate.yaml").write_text(procedure_text(GATE_NODES), encoding="utf-8")
        rt.managed(worktree, ["init"])
        preview_output = rt.managed(worktree, ["procedure", "preview", "gate.yaml"])
        _, preview = envelope_result(preview_output, "procedure.preview")
        rt.managed(
            worktree,
            ["start", "--procedure", "gate.yaml", "--expect-procedure-digest", preview["procedure_digest"],
             "--task", f"V2DFC-001 fixture {name}", "--idempotency-key", f"fixture-{name}-start"],
        )
        rt.managed(worktree, ["begin", "--idempotency-key", f"fixture-{name}-begin"])
        nodes = 0
        if name != "S":
            while True:
                fill_node(rt, worktree, name, nodes)
                nodes += 1
                if name == "M":
                    break
                if nodes % 7 == 0:
                    rt.stop()
                    size = (worktree / ".podway/runtime/state.sqlite3").stat().st_size
                    rt.start()
                    progress(f"fixture L main database {size} bytes after {nodes} nodes")
                    if size >= L_FIXTURE_BYTES:
                        break
        fixtures[name] = {"worktree": worktree, "filled_nodes": nodes}
        progress(f"fixture {name} ready after {nodes} filled nodes")
    rt.stop()
    snapshots = output / "fixture-snapshots"
    snapshots.mkdir()
    for name, fixture in fixtures.items():
        runtime_directory = fixture["worktree"] / ".podway/runtime"
        for suffix in ("-wal", "-shm"):
            if (runtime_directory / f"state.sqlite3{suffix}").exists():
                fail(f"fixture {name} kept a {suffix} file after a graceful daemon stop")
        snapshot = snapshots / f"{name}.sqlite3"
        shutil.copy2(runtime_directory / "state.sqlite3", snapshot)
        fixture["snapshot"] = snapshot
        fixture["main_database_bytes"] = snapshot.stat().st_size
    if fixtures["L"]["main_database_bytes"] < L_FIXTURE_BYTES:
        fail("fixture L is smaller than the recorded largest Store")
    return fixtures


def fill_node(rt: IsolatedRuntimeV1, worktree: Path, name: str, node: int) -> None:
    for batch in range(FIXTURE_BATCHES_PER_NODE):
        fences = managed_observe(rt, worktree)
        values = {
            f"i{index:02}": payload(f"{name}-n{node}-b{batch}-i{index}", FIXTURE_ITEM_CHARS)
            for index in range(1, ITEM_COUNT + 1)
        }
        document = record_document(fences, f"fill-{name}-{node}-{batch}", values)
        rt.managed(worktree, ["record", "--stdin"], stdin=document)
    fences = managed_observe(rt, worktree)
    rt.managed(
        worktree,
        ["complete", "--if-workspace-uuid", fences.workspace_uuid, "--if-session-id", fences.session_id,
         "--if-session-revision", str(fences.session_revision), "--if-attempt", fences.attempt_id,
         "--idempotency-key", f"fill-{name}-{node}-complete"],
    )


def restore_fixture(fixture: dict[str, Any]) -> None:
    runtime_directory = fixture["worktree"] / ".podway/runtime"
    for suffix in ("-wal", "-shm"):
        (runtime_directory / f"state.sqlite3{suffix}").unlink(missing_ok=True)
    shutil.copy2(fixture["snapshot"], runtime_directory / "state.sqlite3")


def measure_iterations(
    invoke: Callable[[list[str], bytes | None], tuple[int, bytes]],
    *,
    fixture: str,
    path: str,
    warmup: int,
    measured: int,
    key_prefix: str,
) -> list[dict[str, Any]]:
    """Runs observe, set, record, and complete per iteration, deriving fences from outputs."""
    samples = []
    for iteration in range(warmup + measured):
        elapsed, stdout = invoke(["observe"], None)
        envelope, result = envelope_result(stdout, "session.observe")
        fences = FencesV1(envelope, result)
        timings = {"observe": elapsed}
        key = f"{key_prefix}-{iteration}"
        elapsed, stdout = invoke(
            ["set", "i01", payload(f"{key}-set", MEASURED_ITEM_CHARS),
             "--if-workspace-uuid", fences.workspace_uuid, "--if-session-id", fences.session_id,
             "--if-attempt", fences.attempt_id, "--if-item-revision", str(fences.item_revisions["i01"]),
             "--idempotency-key", f"{key}-set"],
            None,
        )
        _, set_result = envelope_result(stdout, "item.set")
        timings["set"] = elapsed
        fences.session_revision = set_result["revision"]
        fences.item_revisions["i01"] += 1
        values = {
            f"i{index:02}": payload(f"{key}-r{index}", MEASURED_ITEM_CHARS)
            for index in range(1, ITEM_COUNT + 1)
        }
        elapsed, stdout = invoke(["record", "--stdin"], record_document(fences, f"{key}-record", values))
        _, record_result = envelope_result(stdout, "item.record_many")
        timings["record"] = elapsed
        elapsed, stdout = invoke(
            ["complete", "--if-workspace-uuid", fences.workspace_uuid, "--if-session-id", fences.session_id,
             "--if-session-revision", str(record_result["revision"]), "--if-attempt", fences.attempt_id,
             "--idempotency-key", f"{key}-complete"],
            None,
        )
        _, complete_result = envelope_result(stdout, "session.complete")
        if complete_result.get("session_state") != "running":
            fail(f"fixture {fixture} reached the end of its chain")
        timings["complete"] = elapsed
        for command, elapsed_us in timings.items():
            samples.append({
                "fixture": fixture, "path": path, "command": command, "iteration": iteration,
                "warmup": iteration < warmup, "elapsed_us": elapsed_us,
            })
    return samples


def timed(
    arguments: list[str], cwd: Path, environment: dict[str, str], stdin: bytes | None
) -> tuple[int, bytes]:
    started = time.perf_counter_ns()
    completed = run_command(arguments, cwd=cwd, environment=environment, stdin=stdin)
    elapsed = (time.perf_counter_ns() - started) // 1_000
    return elapsed, require_success(completed, " ".join(arguments[-2:]))


class DirectPathV1:
    """The release direct-path example, run from a fixture worktree like the CLI."""

    def __init__(self, executable: Path, account: Path) -> None:
        self.executable = executable
        self.environment = {"PATH": "/usr/bin:/bin", "HOME": account.as_posix()}

    def json(self, worktree: Path, arguments: list[str]) -> dict[str, Any]:
        completed = run_command(
            [self.executable.as_posix(), *arguments],
            cwd=worktree,
            environment=self.environment,
            timeout=CHECK_TIMEOUT_SECONDS,
        )
        return json.loads(require_success(completed, arguments[0]))

    def convert(self, fixture: dict[str, Any]) -> None:
        restore_fixture(fixture)
        self.json(fixture["worktree"], ["convert"])

    def invoke(self, worktree: Path) -> Callable[[list[str], bytes | None], tuple[int, bytes]]:
        return lambda arguments, stdin: timed(
            [self.executable.as_posix(), "--json", *arguments], worktree, self.environment, stdin
        )


def daemon_blocks(
    rt: IsolatedRuntimeV1, fixtures: dict[str, dict[str, Any]], profile: dict[str, int]
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Measures every fixture through the shipped CLI and daemon, restarting the daemon each time."""
    samples: list[dict[str, Any]] = []
    control: list[dict[str, Any]] = []
    managed_environment = runtime.isolation_environment(rt.account, rt.root)
    managed_command = [rt.cli.as_posix(), "--mode", runtime.RELEASE_QA_MODE, "--json"]
    for name, fixture in fixtures.items():
        restore_fixture(fixture)
        rt.start()
        progress(f"G7 daemon block on {name}")
        worktree = fixture["worktree"]
        samples += measure_iterations(
            lambda arguments, stdin: timed(
                rt.socket_command() + arguments, worktree, rt.socket_environment(), stdin
            ),
            fixture=name,
            path="daemon",
            warmup=profile["g7_warmup"],
            measured=profile["g7_measured"],
            key_prefix=f"g7-daemon-{name}",
        )
        if name == "S":
            control = measure_iterations(
                lambda arguments, stdin: timed(
                    managed_command + arguments, worktree, managed_environment, stdin
                ),
                fixture=name,
                path="managed-control",
                warmup=0,
                measured=profile["g7_managed_control"],
                key_prefix=f"g7-control-{name}",
            )
        rt.stop()
    return samples, control


def direct_blocks(
    direct: DirectPathV1, fixtures: dict[str, dict[str, Any]], profile: dict[str, int]
) -> list[dict[str, Any]]:
    """Measures every fixture through the direct path from the same restored snapshot."""
    samples: list[dict[str, Any]] = []
    for name, fixture in fixtures.items():
        direct.convert(fixture)
        progress(f"G7 direct block on {name}")
        samples += measure_iterations(
            direct.invoke(fixture["worktree"]),
            fixture=name,
            path="direct",
            warmup=profile["g7_warmup"],
            measured=profile["g7_measured"],
            key_prefix=f"g7-direct-{name}",
        )
    return samples


def open_costs(
    direct: DirectPathV1, fixtures: dict[str, dict[str, Any]], output: Path, profile: dict[str, int]
) -> dict[str, Any]:
    """G8: cheap-open cost on every fixture, warm and from a restored hot WAL."""
    g8: dict[str, Any] = {}
    for name, fixture in fixtures.items():
        progress(f"G8 open cost on {name}")
        worktree = fixture["worktree"]
        snapshot = (output / "g8" / name).as_posix()
        hot = direct.json(
            worktree, ["hot-wal", "--snapshot", snapshot, "--min-wal-bytes", str(HOT_WAL_BYTES)]
        )
        g8[name] = {"hot_wal_bytes": hot["wal_bytes"], "database_bytes": hot["database_bytes"]}
        for mode in ("ro", "rw"):
            for state in ("warm", "cold"):
                arguments = [
                    "open-bench", "--mode", mode, "--state", state,
                    "--warmup", str(profile["g8_warmup"]), "--runs", str(profile["g8_measured"]),
                ]
                if state == "cold":
                    arguments += ["--snapshot", snapshot]
                bench = direct.json(worktree, arguments)
                g8[name][f"{mode}_{state}"] = statistics(bench["samples_us"])
    return g8


def runtime_checks(
    binaries: dict[str, Path], output: Path, profile: dict[str, int]
) -> dict[str, Any]:
    """G7, E1, G8, and E4 in one isolated runtime, with the daemon stopped before conversion."""
    results: dict[str, Any] = {}
    rt = IsolatedRuntimeV1(binaries["cli"], binaries["daemon"])
    direct = DirectPathV1(binaries["direct"], rt.account)
    try:
        fixtures = build_fixtures(rt, output)
        results["fixtures"] = {
            name: {
                "main_database_bytes": fixture["main_database_bytes"],
                "filled_nodes": fixture["filled_nodes"],
            }
            for name, fixture in fixtures.items()
        }
        samples, control = daemon_blocks(rt, fixtures, profile)
        samples += direct_blocks(direct, fixtures, profile)
        (output / "g7").mkdir()
        with (output / "g7" / "samples.jsonl").open("w", encoding="utf-8") as handle:
            for sample in samples + control:
                handle.write(json.dumps(sample, sort_keys=True) + "\n")
        results["G7"] = evaluate_g7(samples, control)

        large = fixtures["L"]
        direct.convert(large)
        progress("E1 phase timings on L")
        phases = phase_records(direct, large["worktree"], profile)
        results["E1"] = evaluate_e1(phases)
        write_json(output / "e1.json", {"samples": phases})

        g8 = open_costs(direct, fixtures, output, profile)
        write_json(output / "g8.json", g8)
        results["G8"] = evaluate_g8(g8)

        progress("E4 integrity cost on L")
        e4 = direct.json(
            large["worktree"],
            ["integrity-bench", "--warmup", str(profile["e4_warmup"]),
             "--runs", str(profile["e4_measured"])],
        )
        results["E4"] = {
            "verdict": "information",
            "database_bytes": e4["database_bytes"],
            "quick_check_us": statistics(e4["quick_check_us"]),
            "integrity_check_us": statistics(e4["integrity_check_us"]),
        }
    except BaseException:
        results["runtime_root_preserved"] = str(rt.root)
        try:
            rt.stop()
        except GateError as error:
            results["runtime_stop_error"] = str(error)
        raise
    results["cleanup"] = rt.cleanup()
    return results


def phase_records(
    direct: DirectPathV1, worktree: Path, profile: dict[str, int]
) -> list[dict[str, Any]]:
    """E1: one iteration per pass with `--phase-timings`, keeping only measured passes."""
    records: list[dict[str, Any]] = []
    for iteration in range(profile["e1_warmup"] + profile["e1_measured"]):
        measured = iteration >= profile["e1_warmup"]

        def invoke(arguments: list[str], stdin: bytes | None) -> tuple[int, bytes]:
            command = [direct.executable.as_posix(), "--phase-timings", "--json", *arguments]
            started = time.perf_counter_ns()
            completed = run_command(
                command, cwd=worktree, environment=direct.environment, stdin=stdin
            )
            elapsed = (time.perf_counter_ns() - started) // 1_000
            stdout = require_success(completed, "phase timing")
            for line in completed.stderr.decode("utf-8").splitlines():
                if measured and line.startswith("V2DFC001_PHASES "):
                    phases = json.loads(line.split(" ", 1)[1])
                    records.append({"command": arguments[0], "wall_us": elapsed, "phases": phases})
            return elapsed, stdout

        measure_iterations(
            invoke, fixture="L", path="direct-phases", warmup=0, measured=1,
            key_prefix=f"e1-{iteration}",
        )
    return records


def evaluate_g7(samples: list[dict[str, Any]], control: list[dict[str, Any]]) -> dict[str, Any]:
    rows = []
    passed = True
    for fixture in ("S", "M", "L"):
        for command in ("observe", "set", "record", "complete"):
            stats = {}
            for path in ("daemon", "direct"):
                values = [
                    sample["elapsed_us"] for sample in samples
                    if sample["fixture"] == fixture and sample["command"] == command
                    and sample["path"] == path and not sample["warmup"]
                ]
                stats[path] = statistics(values)
            direct, daemon = stats["direct"], stats["daemon"]
            ok = direct["p50"] <= daemon["p50"] and direct["p99"] <= daemon["p99"]
            passed = passed and ok
            verdict = "pass" if ok else "fail"
            rows.append({"fixture": fixture, "command": command, **stats, "verdict": verdict})
    managed = {
        command: statistics([sample["elapsed_us"] for sample in control if sample["command"] == command])
        for command in ("observe", "set", "record", "complete")
    } if control else {}
    return {"verdict": "pass" if passed else "fail", "rows": rows, "managed_control": managed}


def evaluate_e1(records: list[dict[str, Any]]) -> dict[str, Any]:
    summary: dict[str, Any] = {}
    for command in ("observe", "set", "record", "complete"):
        phases: dict[str, list[float]] = {}
        for record in records:
            if record["command"] != command:
                continue
            for name, value in record["phases"].items():
                phases.setdefault(name, []).append(value)
        summary[command] = {
            name: {"p50": percentile(values, 50), "p95": percentile(values, 95)}
            for name, values in sorted(phases.items())
        }
    return {"verdict": "information", "phases_us": summary}


def evaluate_g8(g8: dict[str, Any]) -> dict[str, Any]:
    large = g8["L"]
    warm = max(large["ro_warm"]["p99"], large["rw_warm"]["p99"])
    cold = max(large["ro_cold"]["p99"], large["rw_cold"]["p99"])
    ok = warm <= THRESHOLDS["g8_warm_p99_us"] and cold <= THRESHOLDS["g8_cold_p99_us"]
    return {"verdict": "pass" if ok else "fail", "warm_p99_us": warm, "cold_p99_us": cold, "fixtures": g8}


def evaluate_harness(check: str, directory: Path, exit_code: int) -> dict[str, Any]:
    summary_path = directory / "summary.json"
    if not summary_path.exists():
        return {"verdict": "fail", "exit_code": exit_code, "reason": "the check wrote no summary"}
    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    samples = read_jsonl(directory / "samples.jsonl") if (directory / "samples.jsonl").exists() else []
    result: dict[str, Any] = {"exit_code": exit_code, "summary": summary}
    ok = exit_code == 0
    if check in ("G1-G2", "G2-scale"):
        waits = [sample["wait_us"] for sample in samples]
        holds = [sample["hold_us"] for sample in samples]
        elapsed = summary["writer_elapsed_us"]
        ratio = max(elapsed) / min(elapsed)
        result["g2"] = {"wait_us": statistics(waits), "hold_us": statistics(holds), "process_ratio": ratio}
        g1_ok = ok
        g2_ok = (
            percentile(waits, 99) <= THRESHOLDS["g2_wait_p99_us"]
            and max(waits) <= THRESHOLDS["g2_wait_max_us"]
            and percentile(holds, 99) <= THRESHOLDS["g2_hold_p99_us"]
            and ratio <= THRESHOLDS["g2_process_ratio"]
        )
        if check == "G2-scale":
            result["verdict"] = "information" if g1_ok else "fail"
            return result
        result["g1_verdict"] = "pass" if g1_ok else "fail"
        result["g2_verdict"] = "pass" if g2_ok else "fail"
        ok = g1_ok and g2_ok
    elif check == "G3":
        measured = [sample for sample in samples if not sample["warmup"]]
        result["elapsed_us"] = statistics([sample["elapsed_us"] for sample in measured])
        ok = ok and all(
            sample["code"] == "WORKSPACE_BUSY" and sample["elapsed_us"] <= THRESHOLDS["g3_bound_us"]
            and not sample["outcome_recorded"] and sample["state_unchanged"]
            for sample in measured
        )
    elif check == "G5":
        acquisitions = [sample["acquisition_us"] for sample in samples]
        result["acquisition_us"] = statistics(acquisitions)
        ok = ok and percentile(acquisitions, 99) <= THRESHOLDS["g5_acquisition_p99_us"]
    elif check == "G9":
        load = [sample["wal_bytes"] for sample in samples if sample["phase"] == "load"]
        drain = [sample for sample in samples if sample["phase"] == "drain"]
        settled = [sample for sample in drain if sample["elapsed_ms"] >= THRESHOLDS["g9_drain_ms"]]
        drained = settled[0]["wal_bytes"] if settled else None
        result["wal_bytes"] = {
            "load_max": max(load), "after_drain_window": drained, "load_samples": len(load),
        }
        ok = (
            ok
            and max(load) <= THRESHOLDS["g9_load_max_bytes"]
            and drained is not None
            and drained <= THRESHOLDS["g9_drain_bytes"]
        )
    result["verdict"] = "pass" if ok else "fail"
    return result


def run_harness(check: str, harness: Path, output: Path, profile_name: str) -> dict[str, Any]:
    logs = output / "logs"
    logs.mkdir(exist_ok=True)
    environment = dict(os.environ)
    environment["V2DFC001_PROFILE"] = profile_name
    environment["V2DFC001_OUTPUT_DIR"] = (output / "harness").as_posix()
    started = time.monotonic()
    with (logs / f"{check}.log").open("wb") as log:
        completed = subprocess.run(
            [harness.as_posix(), "--exact", f"int_v2dfc001_feasibility::{HARNESS_TESTS[check]}",
             "--ignored", "--nocapture", "--test-threads=1"],
            cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT,
            timeout=CHECK_TIMEOUT_SECONDS, check=False,
        )
    result = evaluate_harness(check, output / "harness" / HARNESS_DIRECTORIES[check], completed.returncode)
    result["duration_seconds"] = round(time.monotonic() - started, 1)
    return result


def command_run(arguments: argparse.Namespace) -> int:
    profile_name = "quick" if arguments.quick else "evidence"
    profile = PROFILES[profile_name]
    selected = arguments.checks.split(",") if arguments.checks else list(CHECK_ORDER)
    unknown = sorted(set(selected) - set(CHECK_ORDER))
    if unknown:
        fail(f"unknown checks: {unknown}")
    output = Path(arguments.output_dir).resolve()
    output.mkdir(parents=True, exist_ok=False)
    report: dict[str, Any] = {
        "schema": REPORT_SCHEMA,
        "profile": profile_name,
        "evidence": profile_name == "evidence" and selected == CHECK_ORDER,
        "started_at": utc_now(),
        "host": host_facts(),
        "candidate": candidate_identity(),
        "parameters": profile,
        "thresholds": THRESHOLDS,
        "checks_selected": selected,
        "production_daemons_before": production_daemons(),
        "checks": {},
    }
    write_json(output / "report.json", report)
    status = 0
    try:
        binaries = build(output)
        for check in [check for check in CHECK_ORDER if check in selected]:
            progress(f"running {check}")
            if check == "runtime":
                results = runtime_checks(binaries, output, profile)
                report["checks"].update({name: results[name] for name in ("G7", "G8", "E1", "E4")})
                report["runtime"] = {
                    name: results[name] for name in ("fixtures", "cleanup") if name in results
                }
                failed = [name for name in ("G7", "G8") if report["checks"][name]["verdict"] != "pass"]
            else:
                report["checks"][check] = run_harness(check, binaries["harness"], output, profile_name)
                failed = [] if report["checks"][check]["verdict"] in ("pass", "information") else [check]
            write_json(output / "report.json", report)
            if failed:
                report["stopped_after"] = check
                report["failed_checks"] = failed
                status = 1
                break
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
        status = 1
        raise
    finally:
        report["finished_at"] = utc_now()
        report["host_after"] = {"load_average": os.getloadavg()}
        report["production_daemons_after"] = production_daemons()
        before = {daemon["pid"] for daemon in report["production_daemons_before"]}
        after = {daemon["pid"] for daemon in report["production_daemons_after"]}
        report["production_daemons_unchanged"] = before == after
        write_json(output / "report.json", report)
        verdicts = {name: check.get("verdict") for name, check in report["checks"].items()}
        print(json.dumps(verdicts, sort_keys=True))
    return status


def command_inventory(arguments: argparse.Namespace) -> int:
    """E2: list every Rust test by crate, target, and top-level module."""
    cargo = runtime.pinned_cargo()
    environment = dict(os.environ)
    environment["PATH"] = f"{cargo.parent}:{environment.get('PATH', '/usr/bin:/bin')}"
    completed = run_command(
        [cargo.as_posix(), "test", "--workspace", "--locked", "--no-run", "--message-format=json"],
        cwd=ROOT, environment=environment, timeout=CHECK_TIMEOUT_SECONDS,
    )
    stdout = require_success(completed, "cargo test --no-run")
    inventory: dict[str, int] = {}
    for line in stdout.decode("utf-8").splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            message.get("reason") != "compiler-artifact"
            or not message.get("executable")
            or not message.get("profile", {}).get("test")
        ):
            continue
        package = Path(message["manifest_path"]).parent.name
        target = message["target"]["name"]
        listed = require_success(
            run_command([message["executable"], "--list", "--format", "terse"], cwd=ROOT),
            f"{target} --list",
        )
        for entry in listed.decode("utf-8").splitlines():
            if not entry.endswith(": test"):
                continue
            name = entry[: -len(": test")]
            module = name.split("::", 1)[0] if "::" in name else "(root)"
            key = f"{package}|{target}|{module}"
            inventory[key] = inventory.get(key, 0) + 1
    rows = [
        {"crate": key.split("|")[0], "target": key.split("|")[1], "module": key.split("|")[2], "tests": count}
        for key, count in sorted(inventory.items())
    ]
    write_json(
        Path(arguments.output).resolve(),
        {
            "base_head": candidate_identity()["base_head"],
            "rows": rows,
            "total": sum(inventory.values()),
        },
    )
    print(json.dumps({"modules": len(rows), "tests": sum(inventory.values())}))
    return 0


def self_test() -> None:
    values = list(range(1, 101))
    assert percentile(values, 50) == 50
    assert percentile(values, 95) == 95
    assert percentile(values, 99) == 99
    assert percentile(list(range(1, 11)), 99) == 10
    assert percentile([7], 99) == 7
    assert percentile(list(range(1, 221)), 99) == 218
    stats = statistics([3, 1, 2])
    assert stats == {"n": 3, "p50": 2, "p95": 3, "p99": 3, "max": 3}
    sample = lambda fixture, command, path, value: {
        "fixture": fixture, "command": command, "path": path, "warmup": False, "elapsed_us": value,
    }
    rows = []
    for fixture in ("S", "M", "L"):
        for command in ("observe", "set", "record", "complete"):
            rows.append(sample(fixture, command, "daemon", 10))
            rows.append(sample(fixture, command, "direct", 10))
    assert evaluate_g7(rows, [])["verdict"] == "pass"
    rows[-1]["elapsed_us"] = 11
    assert evaluate_g7(rows, [])["verdict"] == "fail"
    assert payload("tag", 8) == "tag-efgh"
    assert len(payload("x" * 70, 64)) == 64
    procedure = procedure_text(3)
    assert "next: n0002" in procedure and "terminal: true" in procedure and procedure.count("- id: n") == 3


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="run the gate and write the verdict report")
    run.add_argument("--output-dir", required=True)
    run.add_argument("--quick", action="store_true", help="exploratory parameters; never evidence")
    run.add_argument("--checks", help=f"comma-separated subset of {','.join(CHECK_ORDER)}")
    inventory = commands.add_parser("inventory", help="E2 test inventory")
    inventory.add_argument("--output", required=True)
    commands.add_parser("self-test", help="check the percentile and verdict rules")
    arguments = parser.parse_args(argv)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    try:
        if arguments.command == "self-test":
            self_test()
            print(json.dumps({"ok": True, "mode": "self-test"}))
            return 0
        if arguments.command == "inventory":
            return command_inventory(arguments)
        return command_run(arguments)
    except (GateError, runtime.DevRuntimeError, release_archive.ReleaseError) as error:
        print(json.dumps({"ok": False, "error": str(error)}), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
