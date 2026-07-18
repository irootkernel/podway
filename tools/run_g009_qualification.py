#!/usr/bin/env python3
"""Fail-closed Apple-Silicon G009 qualification runner.

There is intentionally no aggregate command: characterization, human approval, RC freeze,
and unseen holdout remain separate irreversible checkpoints.
"""
from __future__ import annotations
import argparse
import os
import platform
import resource
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
import zipfile
from pathlib import Path
from typing import Any

from g009_common import (EVIDENCE_ROOT, ROOT, TARGET, ARCHIVE_ROOT, QualificationError,
    bounded_bytes, canonical_json, content_addressed_json, fail, host_manifest, load_json,
    require_arm64_host, sha256_bytes, sha256_file)
from g009_performance import SAMPLES, WARMUPS, characterize as calculate_baseline, evaluate_holdout, thresholds
from g009_release import inspect_archive, load_rc

# User input never supplies executable vectors. The profile's frozen workload vectors and these
# gates are the complete subprocess allowlist.
GATES: dict[str, tuple[str, ...]] = {
    "verification": ("python3", "tools/run_verification.py", "--run"),
    "g005-vertical": ("python3", "tools/run_g005_vertical.py"),
    "fmt": ("cargo", "+1.85.0", "fmt", "--check"),
    "check": ("cargo", "+1.85.0", "check", "--workspace", "--locked", "--target", TARGET),
    "clippy": ("cargo", "+1.85.0", "clippy", "--workspace", "--all-targets", "--all-features", "--target", TARGET, "--", "-D", "warnings"),
    "audit": ("cargo", "+1.85.0", "audit", "--deny", "warnings"),
    "deny": ("cargo", "+1.85.0", "deny", "check", "advisories", "bans", "licenses", "sources"),
}
IDENTITY_COMMANDS = (("git", "status", "--porcelain"), ("git", "rev-parse", "HEAD"),
                     ("git", "rev-parse", "HEAD^{tree}"), ("rustc", "+1.85.0", "--version"),
                     ("cargo", "+1.85.0", "--version"))
LABEL = "dev.podway.podwayd"
WORKLOAD_VECTORS = {
    "G009-W01": ("podwayd", "--foreground"),
    "G009-W02": ("podway", "status", "--json"),
    "G009-W03": ("podway", "session", "start", "--json"),
    "G009-W04": ("podway", "item", "set", "--json"),
    "G009-W05": ("podway", "item", "attach", "--json"),
    "G009-W06": ("podway", "status", "--json"),
    "G009-W07": ("podway", "item", "set", "--json"),
}


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
        if not isinstance(item, dict) or not isinstance(item.get("name"), str) or not isinstance(item.get("command_vector"), list) or not item["command_vector"] or not all(isinstance(part, str) for part in item["command_vector"]): fail("malformed workload vector")
        if tuple(item["command_vector"]) != WORKLOAD_VECTORS.get(item.get("id")): fail("workload vector is not allowlisted")
        if not isinstance(item.get("hard_bounds"), dict) or not all(isinstance(item["hard_bounds"].get(key), int) and item["hard_bounds"][key] > 0 for key in ("max_completion_ms", "max_rss_mib")): fail("malformed workload hard bounds")
    return value


def identity_manifest(require_clean: bool = True) -> dict[str, Any]:
    outputs: dict[tuple[str, ...], bytes] = {}
    for argv in IDENTITY_COMMANDS:
        result = subprocess.run(argv, cwd=ROOT, capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
        if result.returncode != 0: fail(f"identity command failed: {' '.join(argv)}")
        outputs[argv] = result.stdout
    if require_clean and outputs[("git", "status", "--porcelain")]: fail("source tree is dirty")
    def text(argv: tuple[str, ...]) -> str: return outputs[argv].decode("utf-8", "strict").strip()
    def tool(name: str, argv: tuple[str, ...]) -> dict[str, str]:
        executable = shutil.which(name)
        if not executable or "1.85.0" not in text(argv): fail(f"{name} is not the pinned 1.85.0 tool")
        binary = Path(executable).resolve()
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


def _command_for_workload(item: dict[str, Any], bin_dir: Path) -> tuple[str, ...]:
    vector = item["command_vector"]
    program = vector[0]
    if program not in ("podway", "podwayd"): fail("profile attempts unallowlisted workload executable")
    binary = (bin_dir / program).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK): fail(f"missing executable for workload: {binary}")
    return (str(binary), *vector[1:])


def _native_worktree() -> tuple[tempfile.TemporaryDirectory[str], Path]:
    holder = tempfile.TemporaryDirectory(prefix="g009-worktree-")
    path = Path(holder.name) / "workspace"
    result = subprocess.run(("git", "worktree", "add", "--detach", str(path), "HEAD"), cwd=ROOT, capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
    if result.returncode != 0:
        holder.cleanup(); fail("unable to create isolated temporary worktree")
    return holder, path


def _remove_worktree(path: Path, holder: tempfile.TemporaryDirectory[str]) -> None:
    subprocess.run(("git", "worktree", "remove", "--force", str(path)), cwd=ROOT, capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
    holder.cleanup()


def _collect(profile_data: dict[str, Any], bin_dir: Path, phase: str) -> dict[str, Any]:
    holder, workspace = _native_worktree()
    try:
        fixture = workspace / ".g009-fixture"
        fixture.mkdir(mode=0o700)
        (fixture / "input.txt").write_bytes(b"g009-safe-synthetic-fixture-v1\n")
        home = fixture / "home"; home.mkdir(mode=0o700)
        workloads: dict[str, Any] = {}
        for item in profile_data["workloads"]:
            argv = _command_for_workload(item, bin_dir)
            bound = item["hard_bounds"]
            def once() -> dict[str, Any]:
                started = time.monotonic_ns()
                try:
                    result = subprocess.run(argv, cwd=workspace, capture_output=True, check=False,
                                            timeout=bound["max_completion_ms"] / 1000,
                                            env={"HOME": str(home), "PATH": os.environ.get("PATH", ""), "G009_FIXTURE": str(fixture)})
                except subprocess.TimeoutExpired: fail(f"workload {item['id']} exceeded completion bound")
                elapsed = time.monotonic_ns() - started
                maximum = int(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)
                rss_kib = maximum // 1024 if platform.system() == "Darwin" else maximum
                if result.returncode != 0: fail(f"workload {item['id']} returned {result.returncode}")
                if elapsed > bound["max_completion_ms"] * 1_000_000: fail(f"workload {item['id']} exceeded completion bound")
                if rss_kib > bound["max_rss_mib"] * 1024: fail(f"workload {item['id']} exceeded RSS bound")
                return {"elapsed_ns": elapsed, "rss_kib": rss_kib, "exit_code": result.returncode,
                        "stdout_sha256": sha256_bytes(result.stdout), "stderr_sha256": sha256_bytes(result.stderr),
                        "value": {"numerator": elapsed, "denominator": 1}}
            warm = [once() for _ in range(WARMUPS)]
            measured = [once() for _ in range(SAMPLES)]
            workloads[item["id"]] = {"kind": "latency", "warmups": [entry["value"] for entry in warm],
                "samples": [entry["value"] for entry in measured], "resource": {"hard_bounds": bound, "warmups": warm, "samples": measured},
                "workload_name": item["name"], "command_vector": list(item["command_vector"])}
        return {"schema": "podway.g009.characterization/v1", "phase": phase, "target": TARGET,
                "warmups": WARMUPS, "samples": SAMPLES, "fixture_sha256": sha256_file(fixture / "input.txt"),
                "workloads": workloads}
    finally:
        _remove_worktree(workspace, holder)


def preflight(args: argparse.Namespace) -> None:
    profile(Path(args.profile)); require_arm64_host(args.target)
    source = identity_manifest()
    out, digest = evidence("preflight", {"checkpoint_id": "Q0", "target": TARGET, "profile_sha256": sha256_file(Path(args.profile)), "source": source, "status": "pass"})
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


def approve_baseline(args: argparse.Namespace) -> None:
    data = _require_characterization(load_json(Path(args.characterization)))
    roles = args.roles.split(",")
    if roles != ["owner", "E", "F"] or len(set(roles)) != 3 or not args.approval: fail("exactly owner,E,F detached approvals are required")
    baseline = data["baseline"]; threshold = thresholds(baseline)
    baseline_digest = sha256_bytes(canonical_json(baseline)); threshold_digest = sha256_bytes(canonical_json(threshold))
    approvals: list[dict[str, Any]] = []; seen: set[str] = set()
    for raw in args.approval:
        item = load_json(Path(raw))
        if not isinstance(item, dict) or item.get("role") not in roles or item["role"] in seen or item.get("baseline_sha256") != baseline_digest or item.get("thresholds_sha256") != threshold_digest or not isinstance(item.get("detached_signature"), str) or not item["detached_signature"]: fail("invalid, stale, or duplicate detached approval")
        seen.add(item["role"]); approvals.append(item)
    if seen != set(roles): fail("missing approval role")
    for category, value in (("performance/baseline", baseline), ("performance/thresholds", threshold), ("performance/approvals", {"schema": "podway.g009.approvals/v1", "characterization_sha256": sha256_file(Path(args.characterization)), "approvals": approvals})):
        out, digest = evidence(category, value); print(f"{out} {digest}")


def freeze_rc(args: argparse.Namespace) -> None:
    p = profile(Path(args.profile)); require_arm64_host(TARGET); source = identity_manifest()
    baseline, approved = load_json(Path(args.baseline)), load_json(Path(args.thresholds))
    if thresholds(baseline) != approved: fail("thresholds are not mechanically derived")
    approval_path = Path(args.approvals) if args.approvals else Path(args.baseline).parent / "approvals.json"
    approvals = load_json(approval_path)
    if not isinstance(approvals, dict) or len(approvals.get("approvals", [])) != 3: fail("missing approvals")
    posture = args.signing_posture
    if posture not in p["signing_postures"]: fail("unapproved signing posture")
    if posture == "signed-public" and (not args.developer_id or not args.notary_profile): fail("signed-public requires credentials")
    signing = {"posture": posture, "codesign": "not_attempted_missing_credentials", "notarization": "not_attempted_missing_credentials", "stapling": "not_applicable_zip", "gatekeeper": "not_claimed"} if posture == "unsigned-internal" else {"posture": posture, "codesign": "intended", "notarization": "intended", "stapling": "not_applicable_zip", "gatekeeper": "pending"}
    inputs = [_bound("profile", Path(args.profile)), _bound("baseline", Path(args.baseline)), _bound("thresholds", Path(args.thresholds)), _bound("approvals", approval_path), _bound("lockfile", ROOT / "Cargo.lock")]
    for raw in args.input:
        try: role, supplied = raw.split("=", 1)
        except ValueError: fail("--input must be ROLE=PATH")
        if not role or any(entry["role"] == role for entry in inputs): fail("duplicate RC input role")
        inputs.append(_bound(role, Path(supplied)))
    required = {"profile", "baseline", "thresholds", "approvals", "lockfile", "interfaces", "handoffs", "crash-registry", "fuzz-policy", "observability-policy"}
    if {entry["role"] for entry in inputs} != required: fail("RC does not bind all invalidation inputs")
    intent = {"schema": "podway.g009.rc-intent/v1", "target": TARGET, "minimum_macos": p["minimum_macos"], "rust": "1.85.0", "source": source, "host": host_manifest(), "inputs": inputs, "signing": signing, "archive_root": ARCHIVE_ROOT}
    out, digest = evidence("rc", intent); print(f"{out} {digest}")


def holdout(args: argparse.Namespace) -> None:
    rc = load_rc(Path(args.rc)); require_arm64_host(rc["target"])
    if args.warmups != WARMUPS or args.samples != SAMPLES: fail("holdout requires exactly 5 warmups and 30 samples")
    profile_path = _input_from_rc(rc, "profile"); baseline = load_json(_input_from_rc(rc, "baseline")); approved = load_json(_input_from_rc(rc, "thresholds"))
    p = profile(profile_path)
    measured = _collect(p, Path(args.bin_dir).resolve(), "holdout")
    measured["rc_sha256"] = sha256_file(Path(args.rc)); measured["source"] = identity_manifest()
    decision = evaluate_holdout(measured["workloads"], baseline, approved)
    measured["decision"] = decision
    if not decision["passed"]: fail("unseen holdout does not meet frozen thresholds")
    out, digest = evidence("performance/holdout", measured); print(f"{out} {digest}")


def run_gate(gate_id: str) -> dict[str, Any]:
    argv = GATES.get(gate_id)
    if argv is None: fail(f"gate is not allowlisted: {gate_id}")
    result = subprocess.run(argv, cwd=ROOT, capture_output=True, check=False, env={"PATH": os.environ.get("PATH", "")})
    return {"gate_id": gate_id, "argv": list(argv), "exit_code": result.returncode, "stdout_sha256": sha256_bytes(result.stdout), "stderr_sha256": sha256_bytes(result.stderr), "status": "pass" if result.returncode == 0 else "fail"}


def full_gates(args: argparse.Namespace) -> None:
    rc = load_rc(Path(args.rc)); require_arm64_host(rc["target"])
    selected = args.only.split(",") if args.only else list(GATES)
    results = [run_gate(gate) for gate in selected]
    if any(item["status"] != "pass" for item in results): fail("one or more real gates failed")
    out, digest = evidence("gates", {"checkpoint_id": "Q5", "rc_sha256": sha256_file(Path(args.rc)), "source": identity_manifest(), "results": results, "blockers": []})
    print(f"{out} {digest}")


def package(args: argparse.Namespace) -> None:
    rc = load_rc(Path(args.rc)); archive = Path(args.archive)
    if not archive.with_name(archive.name + ".sha256").is_file(): fail("missing final archive checksum")
    report = inspect_archive(archive)
    out, digest = evidence("release/package", {"checkpoint_id": "Q6", "rc_sha256": sha256_file(Path(args.rc)), "source": identity_manifest(), "archive": report, "signing": rc["signing"], "blockers": []})
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
    return destination / ARCHIVE_ROOT


def lifecycle(args: argparse.Namespace) -> None:
    rc = load_rc(Path(args.rc)); archive = Path(args.archive); inspect_archive(archive)
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
        out, digest = evidence("release/lifecycle", {"checkpoint_id": "Q7", "rc_sha256": sha256_file(Path(args.rc)), "archive_sha256": sha256_file(archive), "home_isolated": True, "worktree_preserved": True, "commands": receipts, "blockers": []})
        print(f"{out} {digest}")


def _review_evidence(root: Path, rc_digest: str, source: dict[str, Any], target: str) -> list[dict[str, Any]]:
    if not root.resolve().is_relative_to(EVIDENCE_ROOT.resolve()): fail("evidence root escapes artifacts/g009")
    index = root / "final" / "acceptance-index.json"
    value = load_json(index)
    if not isinstance(value, dict) or value.get("rc_sha256") != rc_digest or value.get("target") != target or value.get("source") != source: fail("acceptance index is stale or mismatched")
    evidence_rows = value.get("evidence")
    if not isinstance(evidence_rows, list) or not evidence_rows: fail("acceptance index has no evidence")
    for row in evidence_rows:
        if not isinstance(row, dict) or not isinstance(row.get("path"), str) or not isinstance(row.get("sha256"), str): fail("malformed acceptance evidence")
        path = (root / row["path"]).resolve()
        if not path.is_relative_to(root.resolve()) or not path.is_file() or sha256_file(path) != row["sha256"]: fail("stale acceptance evidence")
    return evidence_rows


def final_review(args: argparse.Namespace) -> None:
    rc_path = Path(args.rc); rc = load_rc(rc_path); trace = load_json(Path(args.traceability)); root = Path(args.evidence_root)
    if not isinstance(trace, dict) or trace.get("schema") != "podway.g009.traceability/v1" or not isinstance(trace.get("rows"), list): fail("invalid traceability")
    rows = trace["rows"]; ids = [row.get("id") for row in rows if isinstance(row, dict)]
    if len(ids) != len(set(ids)) or (args.require_final_001 and "FINAL-001" not in ids): fail("traceability IDs incomplete")
    source = rc.get("source");
    if not isinstance(source, dict): fail("RC source identity missing")
    evidence_rows = _review_evidence(root, sha256_file(rc_path), source, TARGET)
    required = {row.get("executable_gate") for row in rows if isinstance(row, dict) and isinstance(row.get("executable_gate"), str)}
    found = {row.get("gate_id") for row in evidence_rows}
    if not required.issubset(found): fail("traceability evidence is incomplete")
    blockers = [row.get("blockers") for row in evidence_rows]
    if any(not isinstance(item, list) or item for item in blockers): fail("acceptance evidence contains blockers")
    reviewers = args.reviewer
    if len(reviewers) < 2 or len(set(reviewers)) != len(reviewers) or any(not name.strip() for name in reviewers): fail("final review requires named distinct reviewers")
    review = {"schema": "podway.g009.final-review/v1", "status": "passed", "rc_sha256": sha256_file(rc_path), "target": TARGET, "source": source, "traceability_sha256": sha256_file(Path(args.traceability)), "evidence_count": len(evidence_rows), "reviewers": reviewers, "blockers": [], "signing": rc["signing"]}
    out, digest = evidence("final/review", review); print(f"{out} {digest}")


def verify_final(args: argparse.Namespace) -> None:
    if args.index or args.review:
        if not args.index or not args.review: fail("both final index and review are required")
        index, review = load_json(Path(args.index)), load_json(Path(args.review))
        if not isinstance(index, dict) or not isinstance(review, dict) or review.get("status") != "passed" or index.get("rc_sha256") != review.get("rc_sha256") or index.get("target") != TARGET or review.get("target") != TARGET or index.get("source") != review.get("source") or review.get("blockers") != [] or len(set(review.get("reviewers", []))) != len(review.get("reviewers", [])): fail("final index/review validation failed")
        print("final index and review are current and structurally valid"); return
    if not args.rc or not args.archive: fail("verify-final requires RC/archive or index/review")
    rc = load_rc(Path(args.rc)); report = inspect_archive(Path(args.archive))
    if rc["signing"]["posture"] == "unsigned-internal" and rc["signing"].get("gatekeeper") != "not_claimed": fail("unsigned RC makes a Gatekeeper claim")
    print(f"RC-bound archive is valid: {report['archive_sha256']}")


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__); sub = p.add_subparsers(dest="command", required=True)
    def command(name: str) -> argparse.ArgumentParser: return sub.add_parser(name)
    x=command("preflight"); x.add_argument("--profile", required=True); x.add_argument("--target", required=True); x.set_defaults(fn=preflight)
    x=command("characterize"); x.add_argument("--profile", required=True); x.add_argument("--target", required=True); x.add_argument("--warmups", type=int, required=True); x.add_argument("--samples", type=int, required=True); x.add_argument("--bin-dir", required=True); x.set_defaults(fn=characterize)
    x=command("approve-baseline"); x.add_argument("--characterization", required=True); x.add_argument("--roles", required=True); x.add_argument("--approval", action="append"); x.set_defaults(fn=approve_baseline)
    x=command("freeze-rc"); x.add_argument("--profile", required=True); x.add_argument("--baseline", required=True); x.add_argument("--thresholds", required=True); x.add_argument("--approvals"); x.add_argument("--input", action="append", default=[], metavar="ROLE=PATH"); x.add_argument("--signing-posture", required=True, choices=("unsigned-internal", "signed-public")); x.add_argument("--developer-id"); x.add_argument("--notary-profile"); x.set_defaults(fn=freeze_rc)
    x=command("holdout"); x.add_argument("--rc", required=True); x.add_argument("--warmups", type=int, required=True); x.add_argument("--samples", type=int, required=True); x.add_argument("--bin-dir", required=True); x.set_defaults(fn=holdout)
    x=command("full-gates"); x.add_argument("--rc", required=True); x.add_argument("--only"); x.set_defaults(fn=full_gates)
    x=command("package"); x.add_argument("--rc", required=True); x.add_argument("--archive", required=True); x.set_defaults(fn=package)
    x=command("lifecycle"); x.add_argument("--rc", required=True); x.add_argument("--archive", required=True); x.add_argument("--require-clean-user", action="store_true"); x.set_defaults(fn=lifecycle)
    x=command("verify-final"); x.add_argument("--rc"); x.add_argument("--archive"); x.add_argument("--index"); x.add_argument("--review"); x.set_defaults(fn=verify_final)
    x=command("final-review"); x.add_argument("--rc", required=True); x.add_argument("--traceability", required=True); x.add_argument("--evidence-root", required=True); x.add_argument("--reviewer", action="append", default=[]); x.add_argument("--require-final-001", action="store_true"); x.set_defaults(fn=final_review)
    return p


def main() -> int:
    try: args = parser().parse_args(); args.fn(args); return 0
    except QualificationError as exc: print(f"G009 qualification failed closed: {exc}", file=sys.stderr); return 2
    except (OSError, subprocess.SubprocessError) as exc: print(f"G009 qualification failed closed: {exc}", file=sys.stderr); return 2
if __name__ == "__main__": raise SystemExit(main())
