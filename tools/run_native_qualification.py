#!/usr/bin/env python3
"""Run the required native Store/Engine qualifications against frozen test binaries."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile

import release_archive
from run_e2e import run_exact

ROOT = Path(__file__).resolve().parents[1]
# This executable matrix is also the quality verifier's ignored-test route registry.
CASES = (
    ("podway-store", "int_v2dfc005_scale::direct_multi_process_g1_g2_l_and_maximum"),
    ("podway-engine", "int_v2dfc006_scale::engine_g1_g2_grown_l_300"),
    ("podway-engine", "int_v2dfc006_scale::engine_g1_g2_grown_l_1024"),
    ("podway-engine", "int_v2dfc006_crashes::engine_write_deadlines_return_busy_without_effect_or_outcome"),
    ("podway-engine", "int_v2dfc006_crashes::sigkill_at_all_five_engine_boundaries_preserves_effect_once_recovery"),
    ("podway-engine", "int_v2dfc006_crashes::random_sigkills_preserve_success_only_lookup_and_exact_retry"),
    ("podway-engine", "int_v2dfc006_exclusion::g5_100_reset_remove_cycles_preserve_all_acknowledged_namespaces_with_four_writers_and_readers"),
    ("podway-engine", "int_v2dfc006_races::g6_rejects_500_state_and_500_artifact_preparation_races_without_stale_commits"),
    ("podway-store", "int_v2dfc005_scale::direct_ten_minute_g9_wal_bound"),
)


def save(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    if sys.flags.optimize:
        raise SystemExit("native qualification requires Python assertions")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-parent", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("native Apple Silicon is required")
    power = subprocess.check_output(["/usr/bin/pmset", "-g", "batt"], text=True)
    if "AC Power" not in power:
        raise SystemExit("AC power is required")
    if not args.evidence_parent.is_absolute() or not args.evidence_parent.is_dir():
        raise SystemExit("an existing absolute owned evidence parent is required")
    for variable in ("PODWAY_DIRECT_SCALE_COUNT", "PODWAY_DIRECT_SCALE_SECONDS", "PODWAY_ENGINE_SCALE_DIAGNOSTIC_COUNT", "PODWAY_DIRECT_G9_OBSERVER_DELAY_MS"):
        if variable in os.environ:
            raise SystemExit(f"required qualification refuses diagnostic override {variable}")
    root = Path(tempfile.mkdtemp(prefix="podway-native-", dir=args.evidence_parent)).resolve()
    print(f"native_qualification_evidence={root}", flush=True)
    snapshot = release_archive.source_snapshot_sha256()
    report = {"schema": "podway.native-qualification/v1", "status": "incomplete", "source_snapshot_sha256": snapshot,
              "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "source_dirty": release_archive.require_clean_tree(True), "power": power,
              "load_average": os.getloadavg(),
              "os": subprocess.check_output(["/usr/bin/sw_vers"], text=True),
              "host": subprocess.check_output(["/usr/sbin/sysctl", "hw.machine", "hw.ncpu", "hw.memsize", "machdep.cpu.brand_string"], text=True),
              "binaries": {}, "cases": []}
    save(root / "registration.json", report)
    environment = os.environ.copy()
    environment["PODWAY_DIRECT_TEST_EVIDENCE_ROOT"] = str(root / "store")
    environment["PODWAY_ENGINE_SCALE_EVIDENCE"] = str(root / "engine")
    environment["PODWAY_ENGINE_G5_EVIDENCE_ROOT"] = str(root / "engine-g5")
    for directory in ("store", "engine", "engine-g5", "binaries"):
        (root / directory).mkdir()
    try:
        for package in ("podway-store", "podway-engine"):
            argv = ["cargo", "test", "--release", "--locked", "-p", package, "--test", "int_suite", "--no-run", "--message-format=json"]
            if package == "podway-store":
                argv += ["--features", "test-support"]
            completed = subprocess.run(argv, cwd=ROOT, env=environment, capture_output=True, text=True, check=True)
            paths = [Path(message["executable"]) for line in completed.stdout.splitlines()
                     if (message := json.loads(line)).get("reason") == "compiler-artifact"
                     and message.get("executable") and message["target"]["name"] == "int_suite"]
            if len(paths) != 1:
                raise RuntimeError(f"{package}: expected one exact Cargo test artifact")
            frozen = root / "binaries" / package
            shutil.copyfile(paths[0], frozen)
            frozen.chmod(0o500)
            report["binaries"][package] = {"path": str(frozen), "sha256": hashlib.sha256(frozen.read_bytes()).hexdigest()}
        if release_archive.source_snapshot_sha256() != snapshot:
            raise RuntimeError("source changed while building qualification binaries")
        for package, test in CASES:
            if release_archive.source_snapshot_sha256() != snapshot:
                raise RuntimeError("source changed before a qualification case")
            case = {"package": package, "test": test, "status": "running"}
            report["cases"].append(case)
            save(root / "report.json", report)
            print(f"native_case={package}::{test}", flush=True)
            try:
                run_exact([str(root / "binaries" / package), "--exact", test, "--ignored", "--nocapture", "--test-threads=1"], environment)
            except (Exception, SystemExit):
                case["status"] = "failed"
                raise
            if release_archive.source_snapshot_sha256() != snapshot:
                raise RuntimeError("source changed during a qualification case")
            case["status"] = "passed"
            save(root / "report.json", report)
        report["status"] = "passed"
    finally:
        save(root / "report.json", report)
    print(json.dumps({"status": report["status"], "evidence": str(root)}), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
