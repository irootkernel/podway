#!/usr/bin/env python3
"""Fail-closed Apple-Silicon G009 qualification runner.

There is intentionally no aggregate command: characterization, human approval, RC freeze,
and unseen holdout remain separate irreversible checkpoints.
"""
from __future__ import annotations
import argparse
import base64
import os
import platform
import resource
import shutil
import subprocess
import sys
import tempfile
import time
import struct
import zipfile
from pathlib import Path
from typing import Any

from g009_common import (EVIDENCE_ROOT, ROOT, TARGET, ARCHIVE_ROOT, QualificationError,
    bounded_bytes, canonical_json, content_addressed_json, fail, host_manifest, load_json,
    require_arm64_host, safe_relative, sha256_bytes, sha256_file)
from g009_performance import SAMPLES, WARMUPS, characterize as calculate_baseline, evaluate_holdout, thresholds
from g009_release import inspect_archive, load_rc, require_bound_file
from g009_release import inspect_archive, require_bound_file, verify_rc_consumption

# User input never supplies executable vectors. These logical gate identifiers are the
# complete subprocess allowlist; profile declarations are checked against this map.
GATES: dict[str, tuple[tuple[str, ...], ...]] = {
    "G009-GATE-FORMAT": (("cargo", "+1.85.0", "fmt", "--all", "--", "--check"),),
    "G009-GATE-CHECK": (("cargo", "+1.85.0", "check", "--workspace", "--all-targets", "--target", TARGET),),
    "G009-GATE-CLIPPY": (("cargo", "+1.85.0", "clippy", "--workspace", "--all-targets", "--all-features", "--target", TARGET, "--", "-D", "warnings"),),
    "G009-GATE-NATIVE-TESTS": (("cargo", "+1.85.0", "test", "--workspace", "--all-targets", "--target", TARGET),),
    "G009-GATE-CONTRACTS": (("python3", "tools/run_verification.py", "--run"),),
    "G009-GATE-G005": (("python3", "tools/run_g005_vertical.py"),),
    "G009-GATE-G008": (("python3", "tools/run_g008_dogfood.py"),),
    "G009-GATE-CRASH": (
        ("python3", "tools/verify_g009_qualification.py", "--crash-registry", "quality/crash-boundaries-v1.json"),
        ("cargo", "+1.85.0", "test", "-p", "podway-store", "--test", "phase2_crash_matrix", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-service", "--test", "phase8_crash_boundaries", "--target", TARGET),
    ),
    "G009-GATE-OBS": (
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase8_observability", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-service", "--test", "phase8_observability", "--target", TARGET),
    ),
    "G009-GATE-SECURITY": (
        ("cargo", "+1.85.0", "test", "-p", "podway-cli", "--test", "phase4_commands", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase4_endpoint", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-daemon", "--test", "phase4_registry", "--target", TARGET),
    ),
    "G009-GATE-MIGRATION": (
        ("cargo", "+1.85.0", "test", "-p", "podway-store", "--test", "phase2_schema_codec", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-store", "--test", "phase2_integrity", "--target", TARGET),
        ("cargo", "+1.85.0", "test", "-p", "podway-store", "--test", "phase2_reset_lifecycle", "--target", TARGET),
    ),
    "G009-GATE-AUDIT": (("cargo", "+1.85.0", "audit", "--deny", "warnings"),),
    "G009-GATE-DENY": (("cargo", "+1.85.0", "deny", "check", "advisories", "bans", "licenses", "sources"),),
    "G009-GATE-COVERAGE": (("cargo", "+1.85.0", "llvm-cov", "report", "--target", TARGET, "--summary-only"),),
    "G009-GATE-FUZZ": (),
}
FUZZ_TARGETS = ("frame_decoder", "request_envelope", "response_additive", "config_procedure", "canonical_json", "selector")
IDENTITY_COMMANDS = (("git", "status", "--porcelain"), ("git", "rev-parse", "HEAD"),
                     ("git", "rev-parse", "HEAD^{tree}"), ("rustc", "+1.85.0", "--version"),
                     ("cargo", "+1.85.0", "--version"))
LABEL = "dev.podway.podwayd"
def resolved_tool_argv(argv: tuple[str, ...]) -> tuple[str, ...]:
    if len(argv) > 1 and argv[0] in {"cargo", "rustc"} and argv[1].startswith("+"):
        rustup = shutil.which("rustup")
        if rustup is None:
            fail("rustup is required for pinned Rust commands")
        return (rustup, "run", argv[1][1:], argv[0], *argv[2:])
    return argv


def run_allowed(
    argv: tuple[str, ...],
    *,
    cwd: Path = ROOT,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        resolved_tool_argv(argv),
        cwd=cwd,
        capture_output=True,
        check=False,
        env=env or {"PATH": os.environ.get("PATH", "")},
        timeout=timeout,
    )

WORKLOAD_ADAPTER_IDS = frozenset({"G009-W01", "G009-W02", "G009-W03", "G009-W04", "G009-W05", "G009-W06", "G009-W07"})
WORKLOAD_COMMANDS = {
    "G009-W01": [["podwayd", "--service"]],
    "G009-W02": [["podway", "status"], ["podway", "next"]],
    "G009-W03": [["podway", "start", "--procedure", ".g009-procedure.yaml", "--task", "G009-linked"]],
    "G009-W04": [["podway", "set", "target-audience", "updated"]],
    "G009-W05": [["podway", "attach", "draft-reference", ".g009-artifact.bin"]],
    "G009-W06": [["podway", "status"]],
    "G009-W07": [["podway", "set", "target-audience", "<65536-byte-string>"]],
}
def _w07_generator(item: dict[str, Any]) -> dict[str, Any]:
    generated = item.get("input_generator")
    if not isinstance(generated, dict) or set(generated) != {"adapter_contract", "algorithm", "code_point", "digest", "utf8_byte_length", "version"}:
        fail("W07 input generator is incomplete")
    contract = generated["adapter_contract"]
    digest = generated["digest"]
    if generated["algorithm"] != "repeat-utf8-code-point" or generated["version"] != 1 or not isinstance(contract, dict) or contract != {"id": "G009-W07", "argv_prefix": ["podway", "set", "target-audience"], "argument_index": 2, "argument_source": "generated_utf8_string"} or not isinstance(digest, dict) or digest.get("algorithm") != "sha256" or digest.get("derivation") != "sha256(generated_utf8_bytes)" or not isinstance(digest.get("hex"), str):
        fail("W07 input generator contract drift")
    code_point = generated["code_point"]
    if not isinstance(code_point, str) or not code_point.startswith("U+"):
        fail("W07 code point is malformed")
    try:
        scalar = chr(int(code_point[2:], 16))
        encoded = scalar.encode("utf-8")
    except (TypeError, ValueError, UnicodeError):
        fail("W07 code point is malformed")
    length = generated["utf8_byte_length"]
    if not isinstance(length, int) or length <= 0 or length % len(encoded):
        fail("W07 byte length is not generated from its UTF-8 code point")
    payload = encoded * (length // len(encoded))
    if sha256_bytes(payload) != digest["hex"]:
        fail("W07 generator digest does not bind generated UTF-8 bytes")
    return {"code_point": code_point, "utf8_byte_length": length, "sha256": digest["hex"], "argument_index": contract["argument_index"], "bytes": payload}



def profile(path: Path) -> dict[str, Any]:
    value = load_json(path)
    required = {"schema", "version", "target", "rust", "release_profile", "minimum_macos", "performance", "workloads", "gates", "archive", "signing_postures", "invalidation", "evidence"}
    if not isinstance(value, dict) or not required.issubset(value): fail("profile is missing required fields")
    target = value["target"]
    if value["schema"] != "podway.g009.qualification/v1" or value["version"] != 1: fail("unsupported profile")
    if not isinstance(target, dict) or target.get("triple") != TARGET or target.get("arch") != "arm64" or target.get("host_arch") != "arm64" or target.get("native_required") is not True or target.get("universal_forbidden") is not True or target.get("x86_64_forbidden") is not True: fail("profile target is not native arm64 only")
    if value["rust"] != {"channel": "1.85.0", "version": "1.85.0"}: fail("profile Rust identity is not 1.85.0")
    if value["release_profile"] != {"codegen-units": 1, "lto": "thin", "panic": "abort", "strip": "symbols"}: fail("release flags drift")
    perf = value["performance"]
    if not isinstance(perf, dict) or perf.get("warmups") != WARMUPS or perf.get("characterization_samples") != SAMPLES or perf.get("holdout_samples") != SAMPLES or perf.get("rounding_permitted") is not False: fail("performance protocol drift")
    workloads = value["workloads"]
    if not isinstance(workloads, list) or len(workloads) != 7 or len({item.get("id") for item in workloads if isinstance(item, dict)}) != 7: fail("profile must define exactly seven unique workloads")
    for item in workloads:
        if not isinstance(item, dict) or not isinstance(item.get("name"), str):
            fail("malformed workload declaration")
        workload_id = item.get("id")
        if workload_id not in WORKLOAD_ADAPTER_IDS or item.get("adapter_id") != workload_id:
            fail("workload has no matching native adapter")
        if item.get("measured_commands") != WORKLOAD_COMMANDS[workload_id]:
            fail("workload command contract differs from its native adapter")
        if workload_id == "G009-W07":
            _w07_generator(item)
        if not isinstance(item.get("hard_bounds"), dict) or not all(isinstance(item["hard_bounds"].get(key), int) and item["hard_bounds"][key] > 0 for key in ("max_completion_ms", "max_rss_mib")): fail("malformed workload hard bounds")
    fuzz = value.get("fuzz")
    if not isinstance(fuzz, dict) or fuzz.get("corpus_root") != "artifacts/g009/fuzz/corpus" or fuzz.get("surfaces") != list(FUZZ_TARGETS):
        fail("fuzz target contract drift")
    if fuzz.get("toolchain") != {"channel": "nightly-2026-07-17", "rustc": "1.99.0-nightly (3d50c25bc 2026-07-16)"}:
        fail("fuzz toolchain contract drift")
    if fuzz.get("sanitizer_env") != {"ASAN_OPTIONS": "quarantine_size_mb=16:thread_local_quarantine_size_kb=64:detect_odr_violation=0"}:
        fail("fuzz sanitizer environment drift")
    if fuzz.get("pre_rc") != {"seconds_per_target": 600} or fuzz.get("change_budget") != {"seconds_per_target": 60} or fuzz.get("rc") != {"rss_limit_mb": 512, "seconds_per_target": 3600, "timeout_seconds": 5}:
        fail("fuzz budget contract drift")
    gates = value.get("gates")
    if not isinstance(gates, list) or {item.get("id") for item in gates if isinstance(item, dict)} != set(GATES) or len(gates) != len(GATES):
        fail("profile gate declarations drift from the runner allowlist")
    for item in gates:
        dispatch = item.get("dispatch") if isinstance(item, dict) else None
        if not isinstance(dispatch, dict) or dispatch != {"command": "full-gates", "only": item["id"], "required_args": ["--rc", "--only"]}:
            fail("profile gate dispatch declaration is not executable")
    checkpoints = value.get("workflow_checkpoints")
    checkpoint_ids = {"G009-GATE-PREFLIGHT", "G009-GATE-PERFORMANCE", "G009-GATE-PACKAGE", "G009-GATE-LIFECYCLE", "G009-GATE-FINAL-001"}
    if not isinstance(checkpoints, list) or {item.get("id") for item in checkpoints if isinstance(item, dict)} != checkpoint_ids:
        fail("workflow checkpoint replacements drift")
    for item in checkpoints:
        dispatch = item.get("dispatch") if isinstance(item, dict) else None
        if not isinstance(dispatch, dict) or not isinstance(dispatch.get("command"), str) or not dispatch["command"] or not isinstance(dispatch.get("required_args"), list) or not dispatch["required_args"]:
            fail("workflow checkpoint replacement is incomplete")
    return value


def identity_manifest(require_clean: bool = True) -> dict[str, Any]:
    outputs: dict[tuple[str, ...], bytes] = {}
    for argv in IDENTITY_COMMANDS:
        result = run_allowed(argv)
        if result.returncode != 0: fail(f"identity command failed: {' '.join(argv)}")
        outputs[argv] = result.stdout
    if require_clean and outputs[("git", "status", "--porcelain")]: fail("source tree is dirty")
    def text(argv: tuple[str, ...]) -> str: return outputs[argv].decode("utf-8", "strict").strip()
    def tool(name: str, argv: tuple[str, ...]) -> dict[str, str]:
        rustup = shutil.which("rustup")
        if rustup is None or "1.85.0" not in text(argv):
            fail(f"{name} is not the pinned 1.85.0 tool")
        located = subprocess.run(
            (rustup, "which", "--toolchain", "1.85.0", name),
            cwd=ROOT,
            capture_output=True,
            check=False,
            env={"PATH": os.environ.get("PATH", "")},
        )
        if located.returncode != 0:
            fail(f"cannot locate pinned {name}")
        binary = Path(located.stdout.decode("utf-8", "strict").strip()).resolve()
        return {"id": name, "version": text(argv), "path": str(binary), "path_sha256": sha256_file(binary)}
    return {"commit": text(("git", "rev-parse", "HEAD")), "tree": text(("git", "rev-parse", "HEAD^{tree}")), "tools": [tool("rustc", ("rustc", "+1.85.0", "--version")), tool("cargo", ("cargo", "+1.85.0", "--version"))]}


def evidence(category: str, value: dict[str, Any]) -> tuple[Path, str]:
    record = dict(value)
    record.setdefault("schema", "podway.g009.checkpoint/v1")
    record.setdefault("host", host_manifest())
    return content_addressed_json(category, record)


def _bound(role: str, path: Path) -> dict[str, str]:
    resolved = path.resolve()
    if not resolved.is_relative_to(ROOT) or not resolved.is_file(): fail(f"bound input {role} is unsafe or missing")
    return {"role": role, "path": str(resolved.relative_to(ROOT)), "sha256": sha256_file(resolved)}


def _input_from_rc(rc: dict[str, Any], role: str) -> Path:
    items = [item for item in rc["inputs"] if item["role"] == role]
    if len(items) != 1: fail(f"RC lacks unique {role} input")
    path = (ROOT / items[0]["path"]).resolve()
    if sha256_file(path) != items[0]["sha256"]: fail(f"stale RC input: {role}")
    return path




def _native_worktree() -> tuple[tempfile.TemporaryDirectory[str], Path]:
    holder = tempfile.TemporaryDirectory(prefix="g009-worktree-")
    path = Path(holder.name) / "workspace"
    result = subprocess.run(("git", "worktree", "add", "--detach", str(path), "HEAD"), cwd=ROOT, capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
    if result.returncode != 0:
        holder.cleanup(); fail("unable to create isolated temporary worktree")
    return holder, path


def _remove_worktree(path: Path, holder: tempfile.TemporaryDirectory[str]) -> None:
    result = subprocess.run(("git", "worktree", "remove", "--force", str(path)), cwd=ROOT, capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
    if result.returncode != 0 or path.exists(): fail("unable to remove isolated workload worktree")
    holder.cleanup()


def _run(argv: tuple[str, ...], cwd: Path, env: dict[str, str], timeout: float = 15) -> subprocess.CompletedProcess[bytes]:
    try: result = subprocess.run(argv, cwd=cwd, capture_output=True, check=False, timeout=timeout, env=env)
    except subprocess.TimeoutExpired: fail(f"native workload command timed out: {argv[0]}")
    if result.returncode != 0: fail(f"native workload command failed ({result.returncode}): {' '.join(argv[:3])}")
    return result

def _socket_paths(env: dict[str, str]) -> tuple[Path, ...]:
    primary = Path(env["TMPDIR"]) / f"podway-{os.getuid()}" / "podwayd.sock"
    fallback = Path("/tmp") / f"podway-{os.getuid()}" / "podwayd.sock"
    return (primary,) if primary == fallback else (primary, fallback)


def _start_daemon(podwayd: Path, cwd: Path, env: dict[str, str]) -> tuple[subprocess.Popen[bytes], Path]:
    candidates = _socket_paths(env)
    if any(candidate.exists() for candidate in candidates):
        fail("refusing a pre-existing Podway socket before workload startup")
    process = subprocess.Popen((str(podwayd), "--service"), cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            _, stderr = process.communicate()
            fail(f"podwayd exited before socket readiness: {sha256_bytes(stderr)}")
        ready = [candidate for candidate in candidates if candidate.exists()]
        if len(ready) == 1:
            return process, ready[0]
        if len(ready) > 1:
            process.terminate()
            process.wait(timeout=5)
            fail("podwayd created multiple socket candidates")
        time.sleep(0.01)
    process.terminate(); process.wait(timeout=5); fail("podwayd did not create its socket")

def _stop_daemon(process: subprocess.Popen[bytes], socket: Path) -> None:
    if process.poll() is None:
        process.terminate()
        try: process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill(); process.wait(timeout=5); fail("podwayd ignored SIGTERM")
    if socket.exists(): fail("podwayd left socket after termination")

def _prepare_task(podway: Path, workspace: Path, env: dict[str, str]) -> Path:
    procedure = workspace / ".g009-procedure.yaml"
    procedure.write_text(
        """schema: podway.procedure/v1
id: g009-benchmark
version: "1"
name: G009 benchmark
description: Deterministic release qualification fixture.
stages:
  - id: benchmark
    title: Benchmark
    items:
      - id: target-audience
        type: text
        prompt: Target audience.
        required: true
        min_length: 1
      - id: draft-reference
        type: artifact
        prompt: Draft reference.
        required: false
rework:
  allow_return_to: any_previous
""",
        encoding="utf-8",
    )
    _run((str(podway), "init"), workspace, env)
    _run(
        (str(podway), "start", "--procedure", procedure.name, "--task", "G009"),
        workspace,
        env,
    )
    return procedure

def _adapter_commands(workload_id: str, podway: Path, workspace: Path, w07: dict[str, Any] | None = None) -> tuple[tuple[str, ...], ...]:
    artifact = workspace / ".g009-artifact.bin"; artifact.write_bytes(b"g009-artifact-v1\n" * 4096)
    if workload_id == "G009-W02": return ((str(podway), "status"), (str(podway), "next"))
    if workload_id == "G009-W03": return ((str(podway), "start", "--procedure", ".g009-procedure.yaml", "--task", "G009-linked"),)
    if workload_id == "G009-W04": return ((str(podway), "set", "target-audience", "updated"),)
    if workload_id == "G009-W05": return ((str(podway), "attach", "draft-reference", artifact.name),)
    if workload_id == "G009-W06": return ((str(podway), "status"),)
    if workload_id == "G009-W07":
        if w07 is None: fail("W07 generator is absent")
        return ((str(podway), "set", "target-audience", w07["bytes"].decode("utf-8", "strict")),)
    fail(f"unknown workload adapter: {workload_id}")

def _measure(argvs: tuple[tuple[str, ...], ...], cwd: Path, env: dict[str, str], bound: dict[str, int], allow_rejection: bool = False, daemon: subprocess.Popen[bytes] | None = None) -> dict[str, Any]:
    started = time.monotonic_ns(); stdout = bytearray(); stderr = bytearray(); exit_code = 0; rss_kib = 0
    for argv in argvs:
        if daemon is not None and daemon.poll() is not None: fail("podwayd exited unexpectedly during workload")
        try: result = subprocess.run(argv, cwd=cwd, capture_output=True, check=False, timeout=bound["max_completion_ms"] / 1000, env=env)
        except subprocess.TimeoutExpired: fail("workload command timed out")
        if result.returncode != 0 and not allow_rejection:
            fail(f"native workload command failed ({result.returncode}): {' '.join(argv[1:3])}")
        if allow_rejection and result.returncode != 0 and (result.returncode != 2 or b"validation" not in result.stderr.lower()):
            fail("maximum-input command did not return the exact validation rejection")
        if daemon is not None:
            sample = subprocess.run(("ps", "-o", "rss=", "-p", str(daemon.pid)), capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
            if sample.returncode != 0 or not sample.stdout.strip().isdigit(): fail("cannot measure live daemon RSS")
            rss_kib = max(rss_kib, int(sample.stdout.strip()))
        exit_code = result.returncode
        stdout.extend(result.stdout); stderr.extend(result.stderr)
    elapsed = time.monotonic_ns() - started
    if elapsed > bound["max_completion_ms"] * 1_000_000 or rss_kib > bound["max_rss_mib"] * 1024: fail("workload exceeded frozen resource bound")
    return {"elapsed_ns": elapsed, "rss_kib": rss_kib, "exit_code": exit_code, "stdout_sha256": sha256_bytes(bytes(stdout)), "stderr_sha256": sha256_bytes(bytes(stderr)), "value": {"numerator": elapsed, "denominator": 1}}
def _collect(profile_data: dict[str, Any], bin_dir: Path, phase: str) -> dict[str, Any]:
    podway, podwayd = (bin_dir / "podway").resolve(), (bin_dir / "podwayd").resolve()
    if not all(path.is_file() and os.access(path, os.X_OK) for path in (podway, podwayd)): fail("prebuilt podway binaries are missing or not executable")
    fixture_digest = sha256_bytes(canonical_json({"schema": "podway.g009.fixture-manifest/v1", "fixture": "g009-safe-synthetic-fixture-v1", "adapters": sorted(WORKLOAD_ADAPTER_IDS)}))
    w07 = _w07_generator(next(item for item in profile_data["workloads"] if item["id"] == "G009-W07"))
    def one(item: dict[str, Any]) -> dict[str, Any]:
        holder, workspace = _native_worktree()
        daemon: subprocess.Popen[bytes] | None = None
        socket: Path | None = None
        try:
            fixture = Path(holder.name) / "fixture"; fixture.mkdir(mode=0o700)
            home, temporary = fixture / "home", fixture / "tmp"
            home.mkdir(mode=0o700)
            temporary.mkdir(mode=0o700)
            for relative in (
                Path("Library/Application Support/Podway"),
                Path("Library/LaunchAgents"),
                Path("Library/Logs/Podway"),
            ):
                directory = home / relative
                directory.mkdir(parents=True, mode=0o700)
                os.chmod(directory, 0o700)
            env = {"HOME": str(home), "TMPDIR": str(temporary), "PATH": os.environ.get("PATH", "")}
            if item["id"] == "G009-W01":
                started = time.monotonic_ns()
                daemon, socket = _start_daemon(podwayd, workspace, env)
                elapsed = time.monotonic_ns() - started
                maximum = int(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)
                rss_kib = maximum // 1024 if platform.system() == "Darwin" else maximum
                if elapsed > item["hard_bounds"]["max_completion_ms"] * 1_000_000 or rss_kib > item["hard_bounds"]["max_rss_mib"] * 1024: fail("cold start exceeded frozen resource bound")
                return {"elapsed_ns": elapsed, "rss_kib": rss_kib, "exit_code": 0, "stdout_sha256": sha256_bytes(b""), "stderr_sha256": sha256_bytes(b""), "value": {"numerator": elapsed, "denominator": 1}}
            daemon, socket = _start_daemon(podwayd, workspace, env)
            procedure = _prepare_task(podway, workspace, env)
            if item["id"] == "G009-W06":
                for index in range(32):
                    _run((str(podway), "set", "target-audience", f"growth-{index}"), workspace, env)
            linked = Path(holder.name) / "linked"
            measured_workspace = workspace
            if item["id"] == "G009-W03":
                _run(("git", "worktree", "add", "--detach", str(linked), "HEAD"), workspace, env)
                _run((str(podway), "init"), linked, env)
                linked_procedure = linked / procedure.name
                linked_procedure.write_bytes(procedure.read_bytes())
                measured_workspace = linked
            return _measure(
                _adapter_commands(item["id"], podway, measured_workspace),
                _adapter_commands(item["id"], podway, measured_workspace, w07),
                measured_workspace,
                env,
                item["hard_bounds"],
                item["id"] == "G009-W07",
                daemon,
            )
        finally:
            try:
                if daemon is not None and socket is not None: _stop_daemon(daemon, socket)
            finally:
                linked_path = Path(holder.name) / "linked"
                if linked_path.exists():
                    removal = subprocess.run(("git", "worktree", "remove", "--force", str(linked_path)), cwd=workspace, capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
                    if removal.returncode != 0: fail("unable to remove linked workload worktree")
                _remove_worktree(workspace, holder)
    workloads: dict[str, Any] = {}
    for item in profile_data["workloads"]:
        warm = [one(item) for _ in range(WARMUPS)]
        measured = [one(item) for _ in range(SAMPLES)]
        workloads[item["id"]] = {
            "kind": "latency",
            "warmups": [entry["value"] for entry in warm],
            "samples": [entry["value"] for entry in measured],
            "resource": {
                "hard_bounds": item["hard_bounds"],
                "warmups": warm,
                "samples": measured,
            },
            "fixture_manifest_sha256": sha256_bytes(
                canonical_json(
                    {
                        "schema": "podway.g009.fixture-manifest/v1",
                        "adapter": item["id"],
                        "commands": item["measured_commands"],
                    }
                )
            ),
            "workload_name": item["name"],
            "adapter_id": item["adapter_id"],
            "measured_commands": item["measured_commands"],
            **(
                {"input_generator": {key: value for key, value in w07.items() if key != "bytes"}}
                if item["id"] == "G009-W07"
                else {}
            ),
        }
    return {"schema": "podway.g009.characterization/v1", "phase": phase, "target": TARGET,
            "warmups": WARMUPS, "samples": SAMPLES, "fixture_sha256": fixture_digest, "workloads": workloads}


def _rc_evidence(rc_path: Path, checkpoint_id: str, **value: Any) -> tuple[Path, str]:
    rc = verify_rc_consumption(rc_path)
    record = {"checkpoint_id": checkpoint_id, "status": "pass", "rc_sha256": sha256_file(rc_path),
              "source": rc["source"], "target": rc["target"], "blockers": [], **value}
    return evidence(checkpoint_id.lower(), record)


def preflight(args: argparse.Namespace) -> None:
    rc_path = Path(args.rc)
    rc = verify_rc_consumption(rc_path)
    profile_path = _input_from_rc(rc, "profile")
    profile(profile_path)
    require_arm64_host(rc["target"])
    out, digest = _rc_evidence(rc_path, "G009-GATE-PREFLIGHT",
                                profile_sha256=sha256_file(profile_path), source_manifest=identity_manifest())
    print(f"{out} {digest}")


def _require_characterization(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("schema") != "podway.g009.characterization/v1" or value.get("target") != TARGET or value.get("warmups") != WARMUPS or value.get("samples") != SAMPLES: fail("invalid characterization")
    calculated = calculate_baseline(value.get("workloads"))
    if value.get("baseline") != calculated: fail("characterization baseline is not mechanically exact")
    return value


def characterize(args: argparse.Namespace) -> None:
    p = profile(Path(args.profile)); require_arm64_host(args.target)
    if args.warmups != WARMUPS or args.samples != SAMPLES: fail("G009 requires exactly 5 warmups and 30 samples")
    measured = _collect(p, Path(args.bin_dir).resolve(), "characterization")
    measured["profile_sha256"] = sha256_file(Path(args.profile)); measured["source"] = identity_manifest()
    measured["baseline"] = calculate_baseline(measured["workloads"])
    out, digest = evidence("performance/characterization", measured)
    print(f"{out} {digest}")


def _verified_approvals(approvals_path: Path, signer_contract_path: Path, characterization: Path, baseline: dict[str, Any], threshold: dict[str, Any]) -> list[dict[str, Any]]:
    contract, bundle = load_json(signer_contract_path), load_json(approvals_path)
    if not isinstance(contract, dict) or contract.get("schema") != "podway.g009.approval-signers/v1" or set(contract) != {"schema", "keyring", "signers"}:
        fail("approval signer contract is not exact")
    keyring = Path(contract["keyring"])
    signers = contract["signers"]
    if not keyring.is_file() or not isinstance(signers, list) or len(signers) != 3:
        fail("approval trust root is unavailable")
    by_role = {item.get("role"): item for item in signers if isinstance(item, dict)}
    if set(by_role) != {"owner", "E", "F"} or any(set(item) != {"role", "signer", "fingerprint"} or not all(isinstance(item[key], str) and item[key] for key in ("signer", "fingerprint")) for item in by_role.values()):
        fail("approval signer roles are incomplete")
    if not isinstance(bundle, dict) or bundle.get("schema") != "podway.g009.approvals/v1" or bundle.get("characterization_sha256") != sha256_file(characterization) or not isinstance(bundle.get("approvals"), list) or len(bundle["approvals"]) != 3:
        fail("approval bundle is stale or incomplete")
    baseline_digest, threshold_digest = sha256_bytes(canonical_json(baseline)), sha256_bytes(canonical_json(threshold))
    seen_roles: set[str] = set(); seen_signers: set[str] = set()
    for approval in bundle["approvals"]:
        if not isinstance(approval, dict) or set(approval) != {"role", "signer", "fingerprint", "characterization_sha256", "baseline_sha256", "thresholds_sha256", "payload", "signature"}:
            fail("approval has mutable or missing fields")
        role = approval["role"]; expected = by_role.get(role)
        if expected is None or approval["signer"] != expected["signer"] or approval["fingerprint"] != expected["fingerprint"] or approval["characterization_sha256"] != sha256_file(characterization) or approval["baseline_sha256"] != baseline_digest or approval["thresholds_sha256"] != threshold_digest:
            fail("approval binding or signer contract mismatch")
        if role in seen_roles or approval["signer"] in seen_signers: fail("approval roles/signers must be distinct")
        payload, signature = Path(approval["payload"]), Path(approval["signature"])
        if not payload.is_file() or not signature.is_file() or payload.is_symlink() or signature.is_symlink():
            fail("approval detached signature inputs are unsafe")
        expected_payload = canonical_json({"role": role, "signer": approval["signer"], "fingerprint": approval["fingerprint"], "characterization_sha256": approval["characterization_sha256"], "baseline_sha256": baseline_digest, "thresholds_sha256": threshold_digest})
        if bounded_bytes(payload) != expected_payload:
            fail("approval payload is not the exact bound statement")
        check = subprocess.run(("gpgv", "--keyring", str(keyring), "--status-fd", "1", str(signature), str(payload)), capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
        if check.returncode != 0 or expected["fingerprint"] not in check.stdout.decode("utf-8", "replace"):
            fail("detached approval signature did not verify against trust root")
        seen_roles.add(role); seen_signers.add(approval["signer"])
    if seen_roles != {"owner", "E", "F"}: fail("missing explicit owner/E/F approvals")
    return bundle["approvals"]

def approve_baseline(args: argparse.Namespace) -> None:
    data = _require_characterization(load_json(Path(args.characterization)))
    if args.roles != "owner,E,F" or not args.approval or not args.signer_contract: fail("exact owner,E,F approvals and signer contract are required")
    approvals_path = Path(args.approval[0])
    if len(args.approval) != 1: fail("approvals must be supplied as one immutable bundle")
    baseline, threshold = data["baseline"], thresholds(data["baseline"])
    approvals = _verified_approvals(approvals_path, Path(args.signer_contract), Path(args.characterization), baseline, threshold)
    for category, value in (("performance/baseline", baseline), ("performance/thresholds", threshold), ("performance/approvals", {"schema": "podway.g009.approvals/v1", "characterization_sha256": sha256_file(Path(args.characterization)), "approvals": approvals})):
        out, digest = evidence(category, value); print(f"{out} {digest}")

def freeze_rc(args: argparse.Namespace) -> None:
    p = profile(Path(args.profile)); require_arm64_host(TARGET); source = identity_manifest()
    baseline, approved = load_json(Path(args.baseline)), load_json(Path(args.thresholds))
    if thresholds(baseline) != approved: fail("thresholds are not mechanically derived")
    approval_path, signer_contract = Path(args.approvals), Path(args.signer_contract)
    _verified_approvals(approval_path, signer_contract, Path(args.characterization), baseline, approved)
    posture = args.signing_posture
    if posture not in p["signing_postures"]: fail("unapproved signing posture")
    if posture == "signed-public": fail("signed-public requires external credentialed signing/notarization")
    signing = {"posture": "unsigned-internal", "codesign": "not_attempted_missing_credentials", "notarization": "not_attempted_missing_credentials", "stapling": "not_applicable_zip", "gatekeeper": "not_claimed"}
    inputs = [_bound("profile", Path(args.profile)), _bound("characterization", Path(args.characterization)), _bound("baseline", Path(args.baseline)), _bound("thresholds", Path(args.thresholds)), _bound("approvals", approval_path), _bound("signer-contract", signer_contract), _bound("lockfile", ROOT / "Cargo.lock")]
    for raw in args.input:
        try: role, supplied = raw.split("=", 1)
        except ValueError: fail("--input must be ROLE=PATH")
        if not role or any(entry["role"] == role for entry in inputs): fail("duplicate RC input role")
        inputs.append(_bound(role, Path(supplied)))
    required = {"profile", "characterization", "baseline", "thresholds", "approvals", "signer-contract", "lockfile", "interfaces", "handoffs", "crash-registry", "fuzz-policy", "observability-policy", "podway", "podwayd"}
    if {entry["role"] for entry in inputs} != required: fail("RC does not bind all invalidation inputs and binaries")
    binaries = {name: {"sha256": next(item["sha256"] for item in inputs if item["role"] == name), "provenance": {"source": source, "target": TARGET, "rust": "1.85.0"}} for name in ("podway", "podwayd")}
    intent = {"schema": "podway.g009.rc-intent/v1", "target": TARGET, "minimum_macos": p["minimum_macos"], "rust": "1.85.0", "source": source, "host": host_manifest(), "inputs": inputs, "signing": signing, "archive_root": ARCHIVE_ROOT, "binaries": binaries}
    out, digest = evidence("rc", intent); print(f"{out} {digest}")


def holdout(args: argparse.Namespace) -> None:
    rc = verify_rc_consumption(Path(args.rc)); require_arm64_host(rc["target"])
    if args.warmups != WARMUPS or args.samples != SAMPLES: fail("holdout requires exactly 5 warmups and 30 samples")
    profile_path = _input_from_rc(rc, "profile"); baseline = load_json(_input_from_rc(rc, "baseline")); approved = load_json(_input_from_rc(rc, "thresholds"))
    p = profile(profile_path)
    measured = _collect(p, Path(args.bin_dir).resolve(), "holdout")
    decision = evaluate_holdout(measured["workloads"], baseline, approved)
    measured["decision"] = decision
    if not decision["passed"]: fail("unseen holdout does not meet frozen thresholds")
    out, digest = _rc_evidence(Path(args.rc), "G009-GATE-PERFORMANCE", holdout=measured)
    print(f"{out} {digest}")


FUZZ_OUTPUT_MAX_BYTES = 1024 * 1024

def _fuzz_output_provenance(stream: str, output: bytes) -> dict[str, Any]:
    if not isinstance(output, bytes) or len(output) > FUZZ_OUTPUT_MAX_BYTES:
        fail(f"fuzz {stream} output cannot be captured within the evidence bound")
    return {
        "bytes": len(output),
        "sha256": sha256_bytes(output),
        "base64": base64.b64encode(output).decode("ascii"),
    }

def _fuzz_provenance(profile_data: dict[str, Any], fuzz_env: dict[str, str]) -> dict[str, Any]:
    toolchain = profile_data["fuzz"]["toolchain"]
    rustup = shutil.which("rustup")
    if rustup is None:
        fail("rustup is required for fuzz provenance")
    tools: list[dict[str, str]] = []
    for name in ("rustc", "cargo"):
        located = subprocess.run(
            (rustup, "which", "--toolchain", toolchain["channel"], name),
            cwd=ROOT,
            capture_output=True,
            check=False,
            env={"PATH": os.environ.get("PATH", "")},
        )
        if located.returncode != 0:
            fail(f"cannot locate fuzz {name}")
        path = Path(located.stdout.decode("utf-8", "strict").strip()).resolve()
        if not path.is_file():
            fail(f"fuzz {name} path is missing")
        tools.append({"id": name, "path": str(path), "sha256": sha256_file(path)})
    cargo_fuzz = shutil.which("cargo-fuzz", path=fuzz_env["PATH"])
    if cargo_fuzz is None:
        fail("cargo-fuzz is required for fuzz provenance")
    cargo_fuzz_path = Path(cargo_fuzz).resolve()
    if not cargo_fuzz_path.is_file():
        fail("cargo-fuzz path is missing")
    tools.append({"id": "cargo-fuzz", "path": str(cargo_fuzz_path), "sha256": sha256_file(cargo_fuzz_path)})
    sources = [ROOT / "Cargo.lock", ROOT / "fuzz" / "Cargo.toml", Path(__file__).resolve()]
    sources.extend(ROOT / "fuzz" / "fuzz_targets" / f"{target}.rs" for target in FUZZ_TARGETS)
    if any(not source.is_file() for source in sources):
        fail("fuzz source binding is absent")
    return {
        "source": identity_manifest(),
        "profile_sha256": sha256_bytes(canonical_json(profile_data)),
        "toolchain": {"channel": toolchain["channel"], "rustc": toolchain["rustc"], "tools": tools},
        "sources": [
            {"path": str(source.relative_to(ROOT)), "sha256": sha256_file(source)}
            for source in sources
        ],
    }

def _run_fuzz_gate(profile_data: dict[str, Any]) -> dict[str, Any]:
    policy = profile_data["fuzz"]["rc"]
    toolchain = profile_data["fuzz"]["toolchain"]
    rustup = shutil.which("rustup")
    if rustup is None:
        fail("rustup is required for fuzz qualification")
    proxy_directory = Path(rustup).resolve().parent
    fuzz_env = {
        "PATH": f"{proxy_directory}{os.pathsep}{os.environ.get('PATH', '')}",
        "RUSTUP_TOOLCHAIN": toolchain["channel"],
        "ASAN_OPTIONS": profile_data["fuzz"]["sanitizer_env"]["ASAN_OPTIONS"],
    }
    rustc = run_allowed(("rustc", "--version"), env=fuzz_env)
    if rustc.returncode != 0 or rustc.stdout.decode("utf-8", "strict").strip() != f"rustc {toolchain['rustc']}":
        fail("installed fuzz toolchain differs from the frozen profile")
    provenance = _fuzz_provenance(profile_data, fuzz_env)
    root = ROOT / profile_data["fuzz"]["corpus_root"]
    root.mkdir(parents=True, exist_ok=True)
    results: list[dict[str, Any]] = []
    for target in FUZZ_TARGETS:
        corpus = Path(tempfile.mkdtemp(prefix=f"{target}-", dir=root))
        argv = ("cargo", "fuzz", "run", target, str(corpus), "--",
                f"-max_total_time={policy['seconds_per_target']}",
                f"-timeout={policy['timeout_seconds']}",
                f"-rss_limit_mb={policy['rss_limit_mb']}")
        try:
            result = run_allowed(argv, cwd=ROOT / "fuzz", env=fuzz_env, timeout=policy["seconds_per_target"] + policy["timeout_seconds"])
        except subprocess.TimeoutExpired:
            fail(f"fuzz target exceeded profile timeout: {target}")
        results.append({
            "target": target,
            "corpus": str(corpus.relative_to(ROOT)),
            "argv": list(argv),
            "exit_code": result.returncode,
            "stdout": _fuzz_output_provenance("stdout", result.stdout),
            "stderr": _fuzz_output_provenance("stderr", result.stderr),
            "status": "pass" if result.returncode == 0 else "fail",
        })
    return {"provenance": provenance, "commands": results}

def run_gate(gate_id: str, profile_data: dict[str, Any]) -> dict[str, Any]:
    commands = GATES.get(gate_id)
    if commands is None: fail(f"gate is not allowlisted: {gate_id}")
    if gate_id == "G009-GATE-FUZZ":
        fuzz = _run_fuzz_gate(profile_data)
        return {
            "gate_id": gate_id,
            "provenance": fuzz["provenance"],
            "commands": fuzz["commands"],
            "status": "pass" if all(item["status"] == "pass" for item in fuzz["commands"]) else "fail",
        }
    results = []
    for argv in commands:
        result = run_allowed(argv)
        results.append({"argv": list(argv), "exit_code": result.returncode, "stdout_sha256": sha256_bytes(result.stdout),
                        "stderr_sha256": sha256_bytes(result.stderr), "status": "pass" if result.returncode == 0 else "fail"})
    return {"gate_id": gate_id, "commands": results, "status": "pass" if all(item["status"] == "pass" for item in results) else "fail"}

def full_gates(args: argparse.Namespace) -> None:
    selected = args.only.split(",") if args.only else list(GATES)
    if not selected or len(set(selected)) != len(selected): fail("gate selection is empty or duplicates a gate")
    unknown = [gate for gate in selected if gate not in GATES]
    if unknown: fail(f"gate is not allowlisted: {unknown[0]}")
    rc = verify_rc_consumption(Path(args.rc)); require_arm64_host(rc["target"])
    profile_data = profile(_input_from_rc(rc, "profile"))
    results = [run_gate(gate, profile_data) for gate in selected]
    if any(item["status"] != "pass" for item in results): fail("one or more real gates failed")
    out, digest = _rc_evidence(Path(args.rc), "G009-GATE-GATES", results=results)
    print(f"{out} {digest}")


def _verify_release_binary(path: Path, name: str) -> None:
    raw = bounded_bytes(path, 1024 * 1024)
    if len(raw) < 32 or raw[:4] != b"\xcf\xfa\xed\xfe":
        fail(f"{name} is not a thin 64-bit Mach-O")
    if struct.unpack_from("<I", raw, 4)[0] != 0x0100000C:
        fail(f"{name} is not arm64 Mach-O")
    commands, offset = struct.unpack_from("<I", raw, 16)[0], 32
    has_macos_target = False
    for _ in range(commands):
        if offset + 8 > len(raw): fail(f"{name} has truncated Mach-O load commands")
        command, size = struct.unpack_from("<II", raw, offset)
        if size < 8 or offset + size > len(raw): fail(f"{name} has invalid Mach-O load command")
        has_macos_target |= command in (0x24, 0x32)
        offset += size
    if not has_macos_target: fail(f"{name} lacks a macOS deployment load command")
    result = _run((str(path), "--version"), ROOT, {"PATH": os.environ.get("PATH", "")})
    if result.stdout.decode("utf-8", "strict").strip() != f"{name} 0.1.0":
        fail(f"{name} version is not 0.1.0")

def _archive_member_bytes(bin_dir: Path) -> dict[str, bytes]:
    podway, podwayd = (bin_dir / "podway").resolve(), (bin_dir / "podwayd").resolve()
    for binary, name in ((podway, "podway"), (podwayd, "podwayd")):
        if not binary.is_file() or not os.access(binary, os.X_OK): fail(f"missing executable {name}")
        _verify_release_binary(binary, name)
    members = {f"{ARCHIVE_ROOT}/bin/podway": bounded_bytes(podway),
               f"{ARCHIVE_ROOT}/bin/podwayd": bounded_bytes(podwayd),
               f"{ARCHIVE_ROOT}/LICENSE": bounded_bytes(ROOT / "sot/LICENSE"),
               f"{ARCHIVE_ROOT}/README.md": bounded_bytes(ROOT / "README.md"),
               f"{ARCHIVE_ROOT}/RELEASE_NOTES.md": bounded_bytes(ROOT / "RELEASE_NOTES.md")}
    with tempfile.TemporaryDirectory(prefix="g009-completions-") as raw:
        directory = Path(raw)
        for shell in ("bash", "zsh", "fish"):
            result = _run((str(podway), "completions", shell), ROOT, {"HOME": str(directory), "TMPDIR": str(directory), "PATH": os.environ.get("PATH", "")})
            if not result.stdout: fail(f"empty {shell} completion output")
            members[f"{ARCHIVE_ROOT}/share/completions/podway.{shell}"] = result.stdout
    for source_root, archive_prefix in ((ROOT / "presets", "share/podway/presets"), (ROOT / "schemas", "share/podway/schemas")):
        if not source_root.is_dir(): fail(f"missing shipped directory: {source_root}")
        for source in sorted(path for path in source_root.rglob("*") if path.is_file()):
            relative = source.relative_to(source_root).as_posix()
            safe_relative(relative)
            members[f"{ARCHIVE_ROOT}/{archive_prefix}/{relative}"] = bounded_bytes(source)
    return members

def _deterministic_zip(path: Path, members: dict[str, bytes]) -> None:
    if path.parent.resolve().is_relative_to(ROOT.resolve()) is False: fail("archive output escapes repository")
    manifest_path = f"{ARCHIVE_ROOT}/payload-digests-v1.json"
    manifest = {"schema": "podway.g009.payload-digests/v1", "members": [
        {"path": name, "sha256": sha256_bytes(data), "size": len(data)}
        for name, data in sorted(members.items())]}
    members = dict(members)
    members[manifest_path] = canonical_json(manifest)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9, strict_timestamps=True) as archive:
            for name in sorted(members):
                safe_relative(name)
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.external_attr = ((0o100755 if name.startswith(f"{ARCHIVE_ROOT}/bin/") else 0o100644) << 16)
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, members[name], compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
        if path.exists():
            if bounded_bytes(path) != bounded_bytes(temporary): fail(f"refusing mismatched existing archive: {path}")
            temporary.unlink()
        else:
            os.replace(temporary, path)
    finally:
        if temporary.exists(): temporary.unlink()

def _write_checksum(path: Path) -> None:
    sidecar = path.with_name(path.name + ".sha256")
    payload = (sha256_file(path) + "\n").encode("ascii")
    if sidecar.exists():
        if bounded_bytes(sidecar, 1024) != payload: fail("refusing mismatched existing detached checksum")
        return
    temporary = sidecar.with_name(f".{sidecar.name}.{os.getpid()}.tmp")
    try:
        temporary.write_bytes(payload)
        os.replace(temporary, sidecar)
    finally:
        if temporary.exists(): temporary.unlink()

def package(args: argparse.Namespace) -> None:
    rc = verify_rc_consumption(Path(args.rc)); require_arm64_host(rc["target"])
    if rc["signing"]["posture"] == "signed-public": fail("package never signs or claims notarization")
    profile_data = profile(_input_from_rc(rc, "profile"))
    archive, bin_dir = Path(args.archive).resolve(), Path(args.bin_dir).resolve()
    for name in ("podway", "podwayd"):
        binary = (bin_dir / name).resolve()
        require_bound_file(rc, name, binary)
        if rc["binaries"][name]["sha256"] != sha256_file(binary): fail(f"package binary differs from RC: {name}")
        _verify_release_binary(binary, name)
    members = _archive_member_bytes(bin_dir)
    required = profile_data["archive"].get("members")
    if not isinstance(required, list) or any(not isinstance(item, str) for item in required): fail("profile archive member declaration is invalid")
    declared_roots = {f"{ARCHIVE_ROOT}/{item}" for item in required}
    actual = set(members) | {f"{ARCHIVE_ROOT}/payload-digests-v1.json"}
    if any(name not in declared_roots and not any(name.startswith(root + "/") for root in declared_roots) for name in actual):
        fail("archive contains an undeclared member")
    if any(root not in actual and not any(name.startswith(root + "/") for name in actual) for root in declared_roots):
        fail("archive omits a declared member or descendant")
    declared = actual
    _deterministic_zip(archive, members)
    _write_checksum(archive)
    report = inspect_archive(archive, declared)
    out, digest = _rc_evidence(Path(args.rc), "G009-GATE-PACKAGE", archive=report, signing=rc["signing"])
    print(f"{out} {digest}")


def _launchctl_absent(uid: int) -> None:
    executable = shutil.which("launchctl")
    if not executable: fail("launchctl is unavailable")
    result = subprocess.run((executable, "print", f"gui/{uid}/{LABEL}"), capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
    if result.returncode == 0: fail(f"refusing pre-existing LaunchAgent {LABEL}")
    if result.returncode not in (3, 113): fail("cannot establish LaunchAgent absence safely")


def _safe_extract_archive(archive: Path, destination: Path) -> Path:
    inspect_archive(archive)
    with zipfile.ZipFile(archive) as bundle:
        for info in bundle.infolist():
            name = info.filename
            if info.is_dir() or not name.startswith(ARCHIVE_ROOT + "/") or ".." in Path(name).parts: fail("unsafe archive member")
            target = (destination / name).resolve()
            if not target.is_relative_to(destination.resolve()): fail("unsafe extraction destination")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(bundle.read(info))
            mode = (info.external_attr >> 16) & 0o777
            os.chmod(target, mode)
            if (target.stat().st_mode & 0o777) != mode: fail("archive mode was not preserved on extraction")
    return destination / ARCHIVE_ROOT


def lifecycle(args: argparse.Namespace) -> None:
    rc = verify_rc_consumption(Path(args.rc)); archive = Path(args.archive); inspect_archive(archive)
    if not args.require_clean_user: fail("lifecycle requires --require-clean-user")
    uid = os.getuid(); _launchctl_absent(uid)
    with tempfile.TemporaryDirectory(prefix="g009-lifecycle-") as raw:
        root = Path(raw); home = root / "home"; home.mkdir(mode=0o700); launch_agents = home / "Library" / "LaunchAgents"; launch_agents.mkdir(parents=True)
        if (launch_agents / f"{LABEL}.plist").exists(): fail("isolated HOME already has Podway label")
        extracted = _safe_extract_archive(archive, root / "extract")
        podway, podwayd = extracted / "bin" / "podway", extracted / "bin" / "podwayd"
        if not os.access(podway, os.X_OK) or not os.access(podwayd, os.X_OK): fail("archive binaries are not executable")
        holder, worktree = _native_worktree(); marker = worktree / ".g009-preserve"; marker.write_text("preserve\n", encoding="utf-8")
        commands = [(str(podway), "daemon", "install", "--daemon-path", str(podwayd)), (str(podway), "daemon", "start"), (str(podway), "daemon", "status"), (str(podway), "daemon", "restart"), (str(podway), "daemon", "logs", "--lines", "1"), (str(podway), "daemon", "uninstall")]
        receipts: list[dict[str, Any]] = []
        installed = False
        uninstalled = False
        environment = {"HOME": str(home), "PATH": os.environ.get("PATH", "")}
        try:
            for argv in commands:
                result = subprocess.run(argv, cwd=worktree, capture_output=True, check=False, timeout=30, env=environment)
                receipts.append({"argv": list(argv), "exit_code": result.returncode, "stdout_sha256": sha256_bytes(result.stdout), "stderr_sha256": sha256_bytes(result.stderr)})
                if result.returncode != 0: fail(f"lifecycle command failed: {' '.join(argv[1:3])}")
                if argv[2] == "install": installed = True
                if argv[2] == "uninstall": uninstalled = True
            if not marker.is_file() or marker.read_text(encoding="utf-8") != "preserve\n": fail("lifecycle did not preserve isolated worktree")
            _launchctl_absent(uid)
        finally:
            if installed and not uninstalled:
                cleanup = subprocess.run((str(podway), "daemon", "uninstall"), cwd=worktree, capture_output=True, check=False, timeout=30, env=environment)
                receipts.append({"argv": [str(podway), "daemon", "uninstall"], "exit_code": cleanup.returncode, "stdout_sha256": sha256_bytes(cleanup.stdout), "stderr_sha256": sha256_bytes(cleanup.stderr), "cleanup": True})
                if cleanup.returncode != 0: fail("lifecycle cleanup uninstall failed")
            _remove_worktree(worktree, holder)
        out, digest = _rc_evidence(Path(args.rc), "G009-GATE-LIFECYCLE", archive_sha256=sha256_file(archive), home_isolated=True, worktree_preserved=True, commands=receipts)
        print(f"{out} {digest}")


def _upstream_gate_ids(trace: dict[str, Any]) -> list[str]:
    if trace.get("schema") != "podway.g009.traceability/v1" or not isinstance(trace.get("rows"), list):
        fail("invalid traceability")
    expected: list[str] = []
    acceptance_ids: list[str] = []
    for row in trace["rows"]:
        if not isinstance(row, dict):
            fail("malformed traceability row")
        gate = row.get("executable_gate")
        row_id = row.get("id")
        if isinstance(row_id, str) and row_id.startswith("ACC-"):
            if row.get("exception_eligible") is not False or row_id in acceptance_ids:
                fail("acceptance exception semantics are not exact")
            acceptance_ids.append(row_id)
        if isinstance(row.get("contract_id"), str) and isinstance(gate, str) and gate != "G009-GATE-FINAL-001":
            if gate in expected:
                fail("upstream traceability gate is duplicated")
            expected.append(gate)
    if not expected or acceptance_ids != [f"ACC-{number:02d}" for number in range(1, 12)]:
        fail("traceability has incomplete upstream acceptance contracts")
    return expected


def acceptance_index(args: argparse.Namespace) -> None:
    rc_path, trace_path, root = Path(args.rc), Path(args.traceability), Path(args.evidence_root)
    rc = verify_rc_consumption(rc_path)
    if not root.resolve().is_relative_to(EVIDENCE_ROOT.resolve()):
        fail("evidence root escapes artifacts/g009")
    expected = _upstream_gate_ids(load_json(trace_path))
    found: dict[str, dict[str, Any]] = {}
    for raw in args.checkpoint:
        artifact = Path(raw).resolve()
        if not artifact.is_relative_to(root.resolve()) or not artifact.is_file() or artifact.is_symlink():
            fail("checkpoint artifact is unsafe or outside evidence root")
        payload = load_json(artifact)
        if not isinstance(payload, dict) or payload.get("status") != "pass" or payload.get("rc_sha256") != sha256_file(rc_path) or payload.get("source") != rc["source"] or payload.get("target") != rc["target"] or payload.get("blockers") != []:
            fail("checkpoint artifact is not a current pass envelope")
        gates = [payload.get("checkpoint_id")]
        if payload.get("checkpoint_id") == "G009-GATE-GATES":
            results = payload.get("results")
            if not isinstance(results, list) or not results:
                fail("gate checkpoint has no results")
            gates = [item.get("gate_id") for item in results if isinstance(item, dict) and item.get("status") == "pass"]
            if len(gates) != len(results):
                fail("gate checkpoint results are incomplete")
        for gate in gates:
            if not isinstance(gate, str) or gate in found:
                fail("checkpoint gate is missing or duplicated")
            found[gate] = {"gate_id": gate, "path": str(artifact.relative_to(root.resolve())), "sha256": sha256_file(artifact),
                           "rc_sha256": sha256_file(rc_path), "target": rc["target"], "source": rc["source"], "blockers": []}
    if list(found) != expected:
        fail("checkpoint artifacts do not exactly match upstream traceability gate order")
    out, digest = _rc_evidence(rc_path, "G009-GATE-ACCEPTANCE-INDEX",
                                traceability_sha256=sha256_file(trace_path), upstream_gate_ids=expected,
                                acceptance_ids=[f"ACC-{number:02d}" for number in range(1, 12)],
                                evidence=[found[gate] for gate in expected])
    print(f"{out} {digest}")


def _review_evidence(index: Path, rc_digest: str, source: dict[str, Any], target: str) -> list[dict[str, Any]]:
    value = load_json(index)
    if not isinstance(value, dict) or value.get("rc_sha256") != rc_digest or value.get("target") != target or value.get("source") != source or value.get("status") != "pass" or value.get("blockers") != []:
        fail("acceptance index is stale or mismatched")
    evidence_rows = value.get("evidence")
    if not isinstance(evidence_rows, list) or not evidence_rows:
        fail("acceptance index has no evidence")
    return evidence_rows


def _verify_reviewer_attestations(args: argparse.Namespace, rc_digest: str, index_digest: str, trace_digest: str) -> list[dict[str, str]]:
    if not args.reviewer_keyring or len(args.attestation) != len(args.reviewer):
        fail("each final reviewer requires one detached attestation and a keyring")
    keyring = Path(args.reviewer_keyring)
    if not keyring.is_file() or keyring.is_symlink():
        fail("reviewer keyring is unsafe or missing")
    verified: list[dict[str, str]] = []
    reviewers = set(args.reviewer)
    for raw in args.attestation:
        try:
            reviewer, payload_raw, signature_raw = raw.split("=", 2)
        except ValueError:
            fail("--attestation must be REVIEWER=PAYLOAD=SIGNATURE")
        if reviewer not in reviewers or any(item["reviewer"] == reviewer for item in verified):
            fail("reviewer attestation is missing or duplicated")
        payload, signature = Path(payload_raw), Path(signature_raw)
        statement = canonical_json({"reviewer": reviewer, "rc_sha256": rc_digest, "acceptance_index_sha256": index_digest, "traceability_sha256": trace_digest})
        if not payload.is_file() or payload.is_symlink() or not signature.is_file() or signature.is_symlink() or bounded_bytes(payload) != statement:
            fail("reviewer attestation is not an exact detached statement")
        result = subprocess.run(("gpgv", "--keyring", str(keyring), str(signature), str(payload)), capture_output=True, check=False)
        if result.returncode != 0:
            fail("reviewer detached attestation did not verify")
        verified.append({"reviewer": reviewer, "payload_sha256": sha256_file(payload), "signature_sha256": sha256_file(signature)})
    if {item["reviewer"] for item in verified} != reviewers:
        fail("reviewer attestations do not cover final reviewers")
    return verified


def final_review(args: argparse.Namespace) -> None:
    rc_path = Path(args.rc); rc = verify_rc_consumption(rc_path); trace_path = Path(args.traceability); index_path = Path(args.index)
    source = rc["source"]
    evidence_rows = _review_evidence(index_path, sha256_file(rc_path), source, rc["target"])
    expected = _upstream_gate_ids(load_json(trace_path))
    if [row.get("gate_id") for row in evidence_rows] != expected:
        fail("acceptance index does not retain exact upstream gate order")
    reviewers = args.reviewer
    if reviewers != ["owner", "E", "F"]:
        fail("final review requires exact ordered owner/E/F reviewers")
    attestations = _verify_reviewer_attestations(args, sha256_file(rc_path), sha256_file(index_path), sha256_file(trace_path))
    review = {"schema": "podway.g009.final-review/v1", "status": "passed", "rc_sha256": sha256_file(rc_path), "acceptance_index_sha256": sha256_file(index_path), "target": rc["target"], "source": source, "traceability_sha256": sha256_file(trace_path), "evidence_count": len(evidence_rows), "reviewers": reviewers, "attestations": attestations, "blockers": [], "signing": rc["signing"]}
    out, digest = _rc_evidence(rc_path, "G009-GATE-FINAL-001", review=review)
    print(f"{out} {digest}")


def verify_final(args: argparse.Namespace) -> None:
    if args.index or args.review:
        if not args.index or not args.review: fail("both final index and review are required")
        index, envelope = load_json(Path(args.index)), load_json(Path(args.review))
        review = envelope.get("review") if isinstance(envelope, dict) else None
        if not isinstance(index, dict) or not isinstance(review, dict) or envelope.get("checkpoint_id") != "G009-GATE-FINAL-001" or review.get("status") != "passed" or review.get("acceptance_index_sha256") != sha256_file(Path(args.index)) or index.get("rc_sha256") != review.get("rc_sha256") or index.get("target") != TARGET or review.get("target") != TARGET or index.get("source") != review.get("source") or review.get("blockers") != [] or review.get("reviewers") != ["owner", "E", "F"]: fail("final index/review validation failed")
        print("final index and review are current and structurally valid"); return
    if not args.rc or not args.archive: fail("verify-final requires RC/archive or index/review")
    rc = verify_rc_consumption(Path(args.rc)); report = inspect_archive(Path(args.archive))
    if rc["signing"]["posture"] == "unsigned-internal" and rc["signing"].get("gatekeeper") != "not_claimed": fail("unsigned RC makes a Gatekeeper claim")
    print(f"RC-bound archive is valid: {report['archive_sha256']}")


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__); sub = p.add_subparsers(dest="command", required=True)
    def command(name: str) -> argparse.ArgumentParser: return sub.add_parser(name)
    x=command("preflight"); x.add_argument("--rc", required=True); x.set_defaults(fn=preflight)
    x=command("characterize"); x.add_argument("--profile", required=True); x.add_argument("--target", required=True); x.add_argument("--warmups", type=int, required=True); x.add_argument("--samples", type=int, required=True); x.add_argument("--bin-dir", required=True); x.set_defaults(fn=characterize)
    x=command("approve-baseline"); x.add_argument("--characterization", required=True); x.add_argument("--roles", required=True); x.add_argument("--approval", action="append"); x.add_argument("--signer-contract", required=True); x.set_defaults(fn=approve_baseline)
    x=command("freeze-rc"); x.add_argument("--profile", required=True); x.add_argument("--baseline", required=True); x.add_argument("--thresholds", required=True); x.add_argument("--characterization", required=True); x.add_argument("--approvals", required=True); x.add_argument("--signer-contract", required=True); x.add_argument("--input", action="append", default=[], metavar="ROLE=PATH"); x.add_argument("--signing-posture", required=True, choices=("unsigned-internal", "signed-public")); x.set_defaults(fn=freeze_rc)
    x=command("holdout"); x.add_argument("--rc", required=True); x.add_argument("--warmups", type=int, required=True); x.add_argument("--samples", type=int, required=True); x.add_argument("--bin-dir", required=True); x.set_defaults(fn=holdout)
    x=command("full-gates"); x.add_argument("--rc", required=True); x.add_argument("--only"); x.set_defaults(fn=full_gates)
    x=command("package"); x.add_argument("--rc", required=True); x.add_argument("--archive", required=True); x.add_argument("--bin-dir", required=True); x.set_defaults(fn=package)
    x=command("lifecycle"); x.add_argument("--rc", required=True); x.add_argument("--archive", required=True); x.add_argument("--require-clean-user", action="store_true"); x.set_defaults(fn=lifecycle)
    x=command("verify-final"); x.add_argument("--rc"); x.add_argument("--archive"); x.add_argument("--index"); x.add_argument("--review"); x.set_defaults(fn=verify_final)
    x=command("acceptance-index"); x.add_argument("--rc", required=True); x.add_argument("--traceability", required=True); x.add_argument("--evidence-root", required=True); x.add_argument("--checkpoint", action="append", required=True); x.set_defaults(fn=acceptance_index)
    x=command("final-review"); x.add_argument("--rc", required=True); x.add_argument("--traceability", required=True); x.add_argument("--index", required=True); x.add_argument("--reviewer", action="append", default=[]); x.add_argument("--reviewer-keyring", required=True); x.add_argument("--attestation", action="append", default=[]); x.set_defaults(fn=final_review)
    return p


def main() -> int:
    try: args = parser().parse_args(); args.fn(args); return 0
    except QualificationError as exc: print(f"G009 qualification failed closed: {exc}", file=sys.stderr); return 2
    except (OSError, subprocess.SubprocessError) as exc: print(f"G009 qualification failed closed: {exc}", file=sys.stderr); return 2
if __name__ == "__main__": raise SystemExit(main())
