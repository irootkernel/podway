#!/usr/bin/env python3
"""Measure V2DFC-007 G7/G8 through one frozen real CLI in owned Git fixtures."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
G7_BASELINE = {
    "S": {"observe": (684.4, 1088.8), "set": (2163.7, 3518.2),
          "record": (1659.3, 2669.5), "complete": (2210.2, 3609.9)},
    "M": {"observe": (806.7, 1290.0), "set": (2605.7, 4027.0),
          "record": (1961.8, 3545.5), "complete": (2631.0, 4422.4)},
    "L": {"observe": (1562.9, 1977.0), "set": (5075.6, 7659.0),
          "record": (3808.6, 5194.8), "complete": (5121.5, 7054.9)},
}
HELPER_TEST = "int_v2dfc007_cli_qualification::qualified_reader_and_output_validator"
FILES = ("state.sqlite3", "state.sqlite3-wal", "state.sqlite3-shm")
L_BYTES = 43_384_832


def sha(path: Path) -> str:
    with path.open("rb") as source:
        return "sha256:" + hashlib.file_digest(source, "sha256").hexdigest()


def save(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def bounded(argv: list[str], *, cwd: Path, environment: dict[str, str],
            data: bytes | None = None, timeout: float = 30) -> tuple[subprocess.CompletedProcess, int]:
    started = time.perf_counter_ns()
    child = subprocess.Popen(argv, cwd=cwd, env=environment, stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             start_new_session=True)
    try:
        stdout, stderr = child.communicate(data, timeout=timeout)
        elapsed = (time.perf_counter_ns() - started + 999) // 1000
        if len(stdout) > 2_097_152 or len(stderr) > 65_536:
            raise RuntimeError("qualification output exceeded its bound")
        return subprocess.CompletedProcess(argv, child.returncode, stdout, stderr), elapsed
    finally:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        if child.poll() is None:
            child.wait(timeout=5)
        for stream in (child.stdin, child.stdout, child.stderr):
            if stream is not None:
                stream.close()


def statistics(values: list[int]) -> dict[str, float | int]:
    assert len(values) == 220
    values = sorted(values)
    return {"samples": len(values), **{
        name: values[math.ceil(fraction * len(values)) - 1]
        for name, fraction in (("p50_us", .5), ("p95_us", .95), ("p99_us", .99), ("max_us", 1))
    }}


def procedure_text(nodes: int) -> str:
    # Preserve the V2DFC-001 Procedure and payload recipes without its runtime.
    items = "".join(
        f"      - id: i{index:02}\n        type: text\n        prompt: Record payload {index}.\n"
        "        required: false\n        max_length: 65536\n"
        for index in range(1, 17)
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


class Fences:
    def __init__(self, envelope: dict[str, Any]):
        result = envelope["result"]
        status = result["status"]
        assert status["session"]["lifecycle"] == "running"
        self.workspace = envelope["workspace"]["uuid"]
        self.session = status["session"]["id"]
        self.revision = status["session"]["revision"]
        self.attempt = status["current"]["attempt"]["attempt_id"]
        self.node = status["current"]["node"]["graph_node_id"]
        self.items = {item["item_id"]: item["revision"] for item in result["active_items"]}
        assert set(self.items) == {f"i{i:02}" for i in range(1, 17)}

    def complete(self, key: str) -> list[str]:
        return ["complete", "--if-workspace-uuid", self.workspace, "--if-session-id", self.session,
                "--if-session-revision", str(self.revision), "--if-attempt", self.attempt,
                "--idempotency-key", key]

    def record(self, key: str, values: dict[str, str]) -> bytes:
        data = json.dumps({"schema": "podway.item-record-many-input/v1",
            "workspace_uuid": self.workspace, "session_id": self.session,
            "session_revision": self.revision, "attempt_id": self.attempt,
            "idempotency_key": key, "operations": [
                {"item_id": item, "expected_item_revision": self.items[item],
                 "record": {"type": "text", "value": value}}
                for item, value in values.items()]}, separators=(",", ":")).encode()
        assert len(data) <= 1_048_576
        return data


class Qualification:
    def __init__(self, output: Path, cli: Path, helper: Path):
        self.root = output
        self.environment = {"PATH": "/usr/bin:/bin"}
        self.calls = 0
        self.samples: list[dict[str, Any]] = []
        self.g8_samples: list[dict[str, Any]] = []
        self.children: list[subprocess.Popen] = []
        self.procedure, self.payload = procedure_text, payload
        self.cli = self.freeze(cli, "podway")
        self.helper = self.freeze(helper, "engine-qualification")
        (self.root / "calls").mkdir()
        self.index = self.root / "output-index.jsonl"
        self.index.touch()

    def freeze(self, source: Path, name: str) -> Path:
        assert source.is_absolute() and source.is_file() and not source.is_symlink()
        destination = self.root / name
        shutil.copyfile(source, destination)
        destination.chmod(0o500)
        assert sha(source) == sha(destination)
        return destination

    def invoke(self, worktree: Path, arguments: list[str], command: str,
               data: bytes | None = None) -> tuple[dict[str, Any], int]:
        assert worktree.parent == self.root and worktree.name in ("s", "m", "l")
        self.calls += 1
        identifier = f"{self.calls:06}"
        argv = [str(self.cli), "--json", *arguments]
        result, elapsed = bounded(argv, cwd=worktree, environment=self.environment, data=data)
        base = self.root / "calls" / identifier
        base.with_suffix(".stdout.json").write_bytes(result.stdout)
        base.with_suffix(".stderr").write_bytes(result.stderr)
        if data is not None:
            base.with_suffix(".stdin.json").write_bytes(data)
        save(base.with_suffix(".call.json"), {"argv": argv, "cwd": str(worktree),
            "stdin": str(base.with_suffix(".stdin.json")) if data is not None else None,
            "exit_code": result.returncode, "elapsed_us": elapsed,
            "output_sha256": sha(base.with_suffix(".stdout.json"))})
        assert result.returncode == 0 and not result.stderr, (identifier, command, result)
        envelope = json.loads(result.stdout)
        assert envelope["schema"] == "podway.output/v4" and envelope["command"] == command
        with self.index.open("a") as index:
            index.write(json.dumps({"output": str(base.with_suffix(".stdout.json").relative_to(self.root)),
                                    "command": command}) + "\n")
        return envelope, elapsed

    def observe(self, worktree: Path) -> Fences:
        return Fences(self.invoke(worktree, ["observe"], "session.observe")[0])

    def image(self, worktree: Path, label: str, *, shm: bool = True) -> Path:
        destination = self.root / "images" / label
        destination.mkdir(parents=True)
        runtime = worktree / ".podway/runtime"
        for name in FILES if shm else FILES[:2]:
            source = runtime / name
            if source.exists():
                assert source.is_file() and not source.is_symlink()
                shutil.copyfile(source, destination / name)
        assert (destination / FILES[0]).is_file()
        save(destination / "image.json", {"files": self.image_hashes(destination),
            "config": sha(worktree / ".podway/config.yaml"), "procedure": sha(worktree / "gate.yaml")})
        return destination

    @staticmethod
    def image_hashes(directory: Path) -> dict[str, str]:
        return {name: sha(directory / name) for name in FILES if (directory / name).exists()}

    def restore(self, worktree: Path, image: Path) -> None:
        assert not any(child.poll() is None for child in self.children)
        runtime = worktree / ".podway/runtime"
        for name in FILES:
            source = image / name
            if source.exists():
                shutil.copyfile(source, runtime / name)
            else:
                (runtime / name).unlink(missing_ok=True)
        expected = json.loads((image / "image.json").read_text())
        assert self.image_hashes(runtime) == expected["files"]
        assert sha(worktree / ".podway/config.yaml") == expected["config"]
        assert sha(worktree / "gate.yaml") == expected["procedure"]

    def fill(self, worktree: Path, name: str, node: int) -> None:
        for batch in range(6):
            fences = self.observe(worktree)
            values = {f"i{i:02}": self.payload(f"{name}-n{node}-b{batch}-i{i}", 55_000)
                      for i in range(1, 17)}
            self.invoke(worktree, ["record", "--stdin"], "item.record_many",
                        fences.record(f"fill-{name}-{node}-{batch}", values))
        fences = self.observe(worktree)
        self.invoke(worktree, fences.complete(f"fill-{name}-{node}-complete"), "session.complete")

    def fixtures(self) -> dict[str, dict[str, Any]]:
        fixtures = {}
        for name in ("S", "M", "L"):
            worktree = self.root / name.lower()
            worktree.mkdir()
            result, _ = bounded(["/usr/bin/git", "init", "--quiet"], cwd=worktree,
                                environment=self.environment)
            assert result.returncode == 0
            (worktree / "gate.yaml").write_text(self.procedure(300))
            self.invoke(worktree, ["init", "--idempotency-key", f"fixture-{name}-init"], "workspace.init")
            preview = self.invoke(worktree, ["procedure", "preview", "gate.yaml"], "procedure.preview")[0]
            self.invoke(worktree, ["start", "--procedure", "gate.yaml", "--expect-procedure-digest",
                preview["result"]["procedure_digest"], "--task", f"V2DFC-001 fixture {name}",
                "--idempotency-key", f"fixture-{name}-start"], "session.start")
            self.invoke(worktree, ["begin", "--idempotency-key", f"fixture-{name}-begin"], "session.begin")
            filled = 0
            if name != "S":
                while True:
                    self.fill(worktree, name, filled)
                    filled += 1
                    assert filled <= 59, "L growth leaves insufficient 240-iteration headroom"
                    size = (worktree / ".podway/runtime/state.sqlite3").stat().st_size
                    if name == "M" or size >= L_BYTES:
                        break
            fences = self.observe(worktree)
            assert int(fences.node[1:]) == filled + 1
            image = self.image(worktree, f"{name}-base")
            fixtures[name] = {"worktree": worktree, "filled_nodes": filled, "base": image}
            save(self.root / f"fixture-{name}.json", {"filled_nodes": filled,
                "main_database_bytes": (image / FILES[0]).stat().st_size,
                "workspace": fences.workspace, "session": fences.session,
                "revision": fences.revision, "node": fences.node})
        assert (fixtures["L"]["base"] / FILES[0]).stat().st_size >= L_BYTES
        return fixtures

    def iterations(self, worktree: Path, name: str, prefix: str, count: int,
                   warmup: int, measured: bool) -> None:
        for iteration in range(count):
            key = f"{prefix}-{name}-{iteration}"
            observed, elapsed = self.invoke(worktree, ["observe"], "session.observe")
            fences = Fences(observed)
            times = {"observe": elapsed}
            calls = {"observe": self.calls}
            value = self.payload(f"{key}-set", 4096)
            result, times["set"] = self.invoke(worktree, ["set", "i01", value,
                "--if-workspace-uuid", fences.workspace, "--if-session-id", fences.session,
                "--if-attempt", fences.attempt, "--if-item-revision", str(fences.items["i01"]),
                "--idempotency-key", f"{key}-set"], "item.set")
            calls["set"] = self.calls
            fences.revision = result["result"]["revision"]
            fences.items["i01"] += 1
            values = {f"i{i:02}": self.payload(f"{key}-r{i}", 4096) for i in range(1, 17)}
            result, times["record"] = self.invoke(worktree, ["record", "--stdin"], "item.record_many",
                                                 fences.record(f"{key}-record", values))
            calls["record"] = self.calls
            fences.revision = result["result"]["revision"]
            result, times["complete"] = self.invoke(worktree, fences.complete(f"{key}-complete"),
                                                   "session.complete")
            calls["complete"] = self.calls
            assert result["result"]["session_state"] == "running"
            if measured:
                for command, elapsed in times.items():
                    self.samples.append({"fixture": name, "command": command,
                        "call": f"{calls[command]:06}", "iteration": iteration,
                        "warmup": iteration < warmup, "elapsed_us": elapsed})
            if iteration % 20 == 19:
                print(f"{prefix} {name}: {iteration + 1}/{count}", flush=True)

    def helper_process(self, document: dict[str, Any], label: str) -> subprocess.Popen:
        path = self.root / f"{label}-input.json"
        save(path, document)
        environment = {**self.environment, "PODWAY_CLI_QUALIFICATION_INPUT": str(path)}
        child = subprocess.Popen([str(self.helper), "--exact", HELPER_TEST, "--ignored", "--nocapture"],
            cwd=self.root, env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, start_new_session=True)
        self.children.append(child)
        return child

    def reap(self, child: subprocess.Popen, label: str, *, kill: bool) -> int:
        if kill:
            assert child.poll() is None, "an already exited child is not a counted kill"
            os.killpg(child.pid, signal.SIGKILL)
        timed_out = False
        try:
            stdout, stderr = child.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(child.pid, signal.SIGKILL)
            stdout, stderr = child.communicate(timeout=5)
        finally:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            if child.poll() is None:
                child.wait(timeout=5)
        (self.root / f"{label}-stdout").write_bytes(stdout)
        (self.root / f"{label}-stderr").write_bytes(stderr)
        save(self.root / f"{label}-exit.json", {"pid": child.pid, "exit": child.returncode})
        assert not timed_out, "qualification helper exceeded its bounded lifetime"
        assert len(stdout) <= 2_097_152 and len(stderr) <= 65_536
        if kill:
            assert child.returncode == -signal.SIGKILL
        return child.returncode

    def hot_wal(self, worktree: Path, name: str) -> tuple[Path, Path, list[str], dict[str, Any], int]:
        fences = self.observe(worktree)
        repair_args = ["workspace", "repair", "--idempotency-key", f"g8-{name}-repair"]
        original = self.invoke(worktree, repair_args, "workspace.repair")[0]
        assert self.invoke(worktree, repair_args, "workspace.repair")[0] == original
        show = self.invoke(worktree, ["workspace", "show"], "workspace.show")[0]
        ready = self.root / f"{name}-reader-ready"
        reader = self.helper_process({"mode": "reader", "evidence_root": str(self.root),
            "worktree": str(worktree), "workspace_uuid": fences.workspace,
            "manifest": show["result"]["contract_manifest_digest"], "ready": str(ready)}, f"{name}-reader")
        expires = time.monotonic() + 10
        while not ready.exists():
            assert reader.poll() is None and time.monotonic() < expires, "qualified reader did not become ready"
            time.sleep(.001)
        pinned_sequence = int(ready.read_text())
        last_key = ""
        for batch in range(64):
            fences = self.observe(worktree)
            last_key = f"g8-{name}-hot-{batch}"
            values = {f"i{i:02}": self.payload(f"{last_key}-{i}", 4096) for i in range(1, 17)}
            self.invoke(worktree, ["record", "--stdin"], "item.record_many", fences.record(last_key, values))
            if (worktree / ".podway/runtime/state.sqlite3-wal").stat().st_size >= 4_194_304:
                break
        else:
            raise RuntimeError("supported CLI writes did not produce the required hot WAL")
        before = self.invoke(worktree, ["observe"], "session.observe")[0]
        fences = Fences(before)
        killed_key = f"g8-{name}-killed"
        values = {f"i{i:02}": self.payload(f"{killed_key}-{i}", 4096) for i in range(1, 17)}
        killed_input = fences.record(killed_key, values)
        (self.root / f"{name}-killed-input.json").write_bytes(killed_input)
        writer = subprocess.Popen([str(self.cli), "--json", "record", "--stdin"], cwd=worktree,
            env=self.environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            start_new_session=True)
        self.children.append(writer)
        save(self.root / f"{name}-killed-writer-request.json", {
            "argv": writer.args, "cwd": str(worktree), "pid": writer.pid,
            "cli_sha256": sha(self.cli), "stdin_sha256": sha(self.root / f"{name}-killed-input.json"),
            "idempotency_key": killed_key, "registered_delay_seconds": .001,
            "boundary": "natural owned SIGKILL; stdin delivery may be interrupted; no release failpoint"})
        # Feed stdin with the same bounded subprocess path; no blocking pipe
        # write may outlive the owned child. A completed child is not a kill.
        try:
            stdout, stderr = writer.communicate(killed_input, timeout=.001)
        except subprocess.TimeoutExpired:
            pass
        else:
            (self.root / f"{name}-unaccounted-writer-stdout").write_bytes(stdout)
            (self.root / f"{name}-unaccounted-writer-stderr").write_bytes(stderr)
            raise RuntimeError("writer exited before the registered SIGKILL delay; setup incomplete")
        self.reap(writer, f"{name}-killed-writer", kill=True)
        lookup = self.invoke(worktree, ["outcome", "lookup", "--idempotency-key", killed_key], "outcome.lookup")[0]
        after = self.invoke(worktree, ["observe"], "session.observe")[0]
        committed = int(lookup["result"]["found"])
        assert after["workspace"]["latest_workspace_sequence"] == before["workspace"]["latest_workspace_sequence"] + committed
        assert Fences(after).revision == fences.revision + committed
        assert Fences(after).attempt == fences.attempt
        acknowledged = self.invoke(worktree, ["outcome", "lookup", "--idempotency-key", last_key], "outcome.lookup")[0]
        assert acknowledged["result"]["found"]
        self.reap(reader, f"{name}-pinned-reader", kill=True)
        cold = self.image(worktree, f"{name}-cold", shm=False)
        assert (cold / FILES[1]).stat().st_size >= 4_194_304
        copy = self.root / f"{name}-main-only.sqlite3"
        shutil.copyfile(cold / FILES[0], copy)
        with sqlite3.connect(copy.as_uri() + "?mode=ro&immutable=1", uri=True) as connection:
            main_sequence = connection.execute("SELECT next_workspace_sequence FROM workspace_state WHERE singleton=1").fetchone()[0]
            missing = connection.execute("SELECT count(*) FROM mutation_outcomes WHERE idempotency_key=?", (last_key,)).fetchone()[0]
        assert missing == 0 and main_sequence <= pinned_sequence
        assert after["workspace"]["latest_workspace_sequence"] > main_sequence
        save(self.root / f"{name}-hot-wal-proof.json", {"pinned_sequence": pinned_sequence,
            "main_only_sequence": main_sequence, "coherent_sequence": after["workspace"]["latest_workspace_sequence"],
            "acknowledged_key_absent_from_main_only": last_key, "setup_record_calls": batch + 1,
            "killed_key_committed": bool(committed), "reader_and_writer_signal": 9})
        self.restore(worktree, cold)
        assert self.invoke(worktree, repair_args, "workspace.repair")[0] == original
        runtime = worktree / ".podway/runtime"
        assert all(sha(runtime / item) == sha(cold / item) for item in FILES[:2])
        assert (runtime / FILES[2]).stat().st_size >= 32_768
        warm = self.image(worktree, f"{name}-warm")
        return cold, warm, repair_args, original, after["workspace"]["latest_workspace_sequence"]

    def g8(self, fixtures: dict[str, dict[str, Any]]) -> dict[str, Any]:
        result = {}
        for name in ("S", "M", "L"):
            fixture = fixtures[name]
            worktree = fixture["worktree"]
            if name == "L":
                self.restore(worktree, fixture["base"])
                self.iterations(worktree, name, "e1", 220, 20, False)
            save(self.root / f"{name}-g8-prehistory.json", self.invoke(worktree, ["observe"], "session.observe")[0])
            cold, warm, arguments, replay, sequence = self.hot_wal(worktree, name)
            save(self.root / f"{name}-g8-mode-registration.json", {
                "coherent_sequence": sequence, "workspace_uuid": replay["workspace"]["uuid"],
                "cold_image": str(cold.relative_to(self.root)), "warm_image": str(warm.relative_to(self.root)),
                "mutation_argv": arguments, "original_replay": replay})
            modes = {}
            for mode, image in (("ro-warm", warm), ("ro-cold", cold), ("rw-warm", warm), ("rw-cold", cold)):
                samples = []
                for iteration in range(240):
                    self.restore(worktree, image)
                    if mode.startswith("ro"):
                        response, elapsed = self.invoke(worktree, ["workspace", "show"], "workspace.show")
                        assert response["workspace"]["uuid"] == replay["workspace"]["uuid"]
                        assert response["workspace"]["latest_workspace_sequence"] == sequence
                    else:
                        response, elapsed = self.invoke(worktree, arguments, "workspace.repair")
                        assert response == replay
                    runtime = worktree / ".podway/runtime"
                    assert all(sha(runtime / item) == sha(image / item) for item in FILES[:2])
                    expected = json.loads((image / "image.json").read_text())
                    assert sha(worktree / ".podway/config.yaml") == expected["config"]
                    assert sha(worktree / "gate.yaml") == expected["procedure"]
                    self.g8_samples.append({"fixture": name, "mode": mode,
                        "call": f"{self.calls:06}", "iteration": iteration,
                        "warmup": iteration < 20, "elapsed_us": elapsed,
                        "image": str(image.relative_to(self.root)),
                        "shm_before": expected["files"].get(FILES[2]),
                        "shm_after": sha(runtime / FILES[2]) if (runtime / FILES[2]).exists() else None})
                    if iteration >= 20:
                        samples.append(elapsed)
                stats = statistics(samples)
                threshold = 100_000 if mode.endswith("warm") else 1_000_000
                modes[mode] = {**stats, "metric": "full_invocation_open_upper_bound_us",
                    "threshold_us": threshold if name == "L" else None,
                    "verdict": "pass" if name != "L" or stats["p99_us"] <= threshold else "unverified-by-upper-bound"}
                save(self.root / "g8-progress.json", {**result, name: modes})
                save(self.root / "g8-samples.json", self.g8_samples)
                print(f"G8 {name}/{mode}: {stats}", flush=True)
            result[name] = modes
        return result

    def validate(self) -> None:
        child = self.helper_process({"mode": "validate-output", "evidence_root": str(self.root),
                                     "index": str(self.index)}, "validate-output")
        assert self.reap(child, "validate-output", kill=False) == 0

    def l_lifecycle(self, fixture: dict[str, Any]) -> dict[str, Any]:
        """Measure lifecycle commands on the same retained L active history."""
        worktree = fixture["worktree"]
        digest = self.invoke(worktree, ["procedure", "preview", "gate.yaml"], "procedure.preview")[0]["result"]["procedure_digest"]
        samples = {name: [] for name in ("reset", "archive", "start")}
        for iteration in range(240):
            self.restore(worktree, fixture["base"])
            _, elapsed = self.invoke(worktree, ["reset", "--yes", "--progress-summary",
                "Owned L lifecycle qualification.", "--idempotency-key", f"l-reset-{iteration}"], "session.reset")
            if iteration >= 20:
                samples["reset"].append(elapsed)
            self.restore(worktree, fixture["base"])
            self.invoke(worktree, ["cancel", "--reason", "Owned archive qualification.",
                "--idempotency-key", f"l-cancel-{iteration}"], "session.cancel")
            self.invoke(worktree, ["disposition", "not-required", "--reason", "Owned fixture needs no handoff.",
                "--idempotency-key", f"l-dispose-{iteration}"], "session.terminal_disposition")
            _, elapsed = self.invoke(worktree, ["archive", "--idempotency-key", f"l-archive-{iteration}"], "session.archive")
            if iteration >= 20:
                samples["archive"].append(elapsed)
            _, elapsed = self.invoke(worktree, ["start", "--procedure", "gate.yaml", "--expect-procedure-digest", digest,
                "--task", "Owned L lifecycle qualification", "--idempotency-key", f"l-start-{iteration}"], "session.start")
            if iteration >= 20:
                samples["start"].append(elapsed)
            save(self.root / "l-lifecycle-samples.json", samples)
        # G7 has no historical comparator for these commands. Do not invent a
        # daemon baseline or apply G8 open-cost limits to whole invocations.
        return {name: {**statistics(values), "metric": "full_invocation_us", "verdict": "informational",
                       "threshold": None} for name, values in samples.items()}

    def cleanup(self) -> None:
        for child in self.children:
            if child.poll() is None:
                self.reap(child, f"cleanup-{child.pid}", kill=True)
        assert all(child.poll() is not None for child in self.children)


def main() -> int:
    if sys.flags.optimize:
        raise SystemExit("native qualification requires Python assertions; optimization is unsupported")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--helper", type=Path, required=True,
                        help="release Engine int_suite containing the fixture-only qualification helper")
    parser.add_argument("--evidence-parent", type=Path, required=True)
    arguments = parser.parse_args()
    assert platform.system() == "Darwin" and platform.machine() == "arm64"
    assert arguments.evidence_parent.is_absolute() and arguments.evidence_parent.is_dir()
    power = subprocess.check_output(["/usr/bin/pmset", "-g", "batt"]).decode()
    assert "AC Power" in power
    output = Path(tempfile.mkdtemp(prefix="v2dfc007-real-gates-", dir=arguments.evidence_parent)).resolve()
    print(f"qualification_evidence={output}", flush=True)
    run = Qualification(output, arguments.cli, arguments.helper)
    source_diff = output / "candidate.diff"
    source_diff.write_bytes(subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=ROOT))
    untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=ROOT)
    source_files = {os.fsdecode(path): sha(ROOT / os.fsdecode(path)) for path in untracked.split(b"\0") if path}
    save(output / "untracked-source.json", source_files)
    version, _ = bounded([str(run.cli), "version", "--json", "--identity"],
                         cwd=output, environment=run.environment)
    assert version.returncode == 0 and not version.stderr
    (output / "version-identity.json").write_bytes(version.stdout)
    identity = {"cli_sha256": sha(run.cli), "helper_sha256": sha(run.helper),
        "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip(),
        "source_status": subprocess.check_output(["git", "status", "--short"], cwd=ROOT).decode(),
        "source_diff_sha256": sha(source_diff), "untracked_source": source_files,
        "version_identity_sha256": sha(output / "version-identity.json"),
        "host": subprocess.check_output(["/usr/sbin/sysctl", "hw.machine", "hw.ncpu", "hw.memsize", "machdep.cpu.brand_string"]).decode(),
        "os": subprocess.check_output(["/usr/bin/sw_vers"]).decode(), "power": power,
        "load": subprocess.check_output(["/bin/ps", "-axo", "pid,pcpu,comm"]).decode(),
        "G7_recipe": "300 nodes; S/M/L; 16 optional text items; six 55000-character batches per growth node; 20+220 iterations",
        "G8_endpoints": ["workspace.show", "exact workspace.repair replay"],
        "G8_method": "20+220 matched tuples per cell; full invocation is a conservative open-cost upper bound; no bound relaxation",
        "G8_shm": "volatile SHM changes are recorded per sample; every sample restores its genuine original mode tuple",
        "G8_L_prehistory": "original L base restored, then E1 20+200 iterations",
        "comparator": "recorded V2DFC-001 daemon columns; no daemon execution"}
    save(output / "registration.json", identity)
    report: dict[str, Any] = {"schema": "podway.real-cli-qualification/v1", "status": "incomplete"}
    try:
        fixtures = run.fixtures()
        for name in ("S", "M", "L"):
            run.restore(fixtures[name]["worktree"], fixtures[name]["base"])
            run.iterations(fixtures[name]["worktree"], name, "g7-direct", 240, 20, True)
            save(output / "g7-samples.json", run.samples)
        g7 = {}
        for name in ("S", "M", "L"):
            g7[name] = {}
            for command, (p50, p99) in G7_BASELINE[name].items():
                stats = statistics([s["elapsed_us"] for s in run.samples
                    if s["fixture"] == name and s["command"] == command and not s["warmup"]])
                g7[name][command] = {**stats, "daemon_p50_us": p50 * 1000, "daemon_p99_us": p99 * 1000,
                    "verdict": "pass" if stats["p50_us"] <= p50 * 1000 and stats["p99_us"] <= p99 * 1000 else "fail"}
        report["G7"] = g7
        save(output / "report.json", report)
        if not all(cell["verdict"] == "pass" for fixture in g7.values() for cell in fixture.values()):
            report["status"] = "G7-failed"
            raise RuntimeError("G7 failed; do not retry without owner direction")
        report["G8"] = run.g8(fixtures)
        report["L_lifecycle"] = run.l_lifecycle(fixtures["L"])
        run.validate()
        assert sha(run.cli) == identity["cli_sha256"] and sha(run.helper) == identity["helper_sha256"]
        report["output_validation"] = "passed canonical shipped validator for every successful workload response; version identity metadata retained separately"
        report["status"] = "passed" if (
            all(cell["verdict"] == "pass" for fixture in report["G7"].values() for cell in fixture.values())
            and all(cell["verdict"] == "pass" for cell in report["G8"]["L"].values())
        ) else "G8-unverified-by-upper-bound"
    finally:
        run.cleanup()
        report["owned_children_remaining"] = 0
        report["fixtures_retained"] = True
        save(output / "report.json", report)
    print(json.dumps({"evidence": str(output), "status": report["status"]}), flush=True)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
