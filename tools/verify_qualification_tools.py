#!/usr/bin/env python3
"""Exercise exact test selection, native refusal and frozen-development guards."""
from __future__ import annotations

import contextlib
import copy
import hashlib
import io
import json
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import time
from unittest import mock

import qualify_development
import qualify_direct_cli
import release_archive
import run_e2e
import run_native_qualification
import verify_quality_contracts

ROOT = Path(__file__).resolve().parents[1]


def rejects(action, exception=SystemExit) -> None:
    try:
        action()
    except exception:
        return
    raise RuntimeError("negative qualification sentinel was accepted")


def exact_output_boundaries() -> None:
    summary = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
    for size in (65536, 70000):
        program = (
            "import os,time; os.write(1,b'x'*60000); time.sleep(.1); "
            f"os.write(1,b'x'*{size - 60000}+b'\\n'); print({summary!r},flush=True)"
        )
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                run_e2e.run_exact([sys.executable, "-c", program], os.environ.copy(), timeout=5)
            except SystemExit as error:
                if size != 70000 or str(error) != "exact test output line exceeds its bound":
                    raise
            else:
                if size != 65536:
                    raise RuntimeError("completed overbound output line was accepted")


def exact_failure_cleanup(root: Path) -> None:
    for mode in ("nonzero", "multiple", "retained-pipe"):
        pid_path = root / (mode + ".pid")
        # Each fixture forks one real descendant into the runner-owned process
        # group. Nonzero exit closes the pipe; the other cases retain it.
        program = (
            "import os,time; from pathlib import Path; pid=os.fork(); "
            f"Path({str(pid_path)!r}).write_text(str(pid)) if pid else None; "
            f"os.close(1) if not pid and {mode == 'nonzero'!r} else None; "
            "time.sleep(30) if not pid else None; "
            f"print('test result: failed',flush=True) if {mode == 'multiple'!r} else None; "
            f"print('test result: failed',flush=True) if {mode == 'multiple'!r} else None; "
            f"os._exit({7 if mode == 'nonzero' else 0})"
        )
        began = time.monotonic()
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                run_e2e.run_exact([sys.executable, "-c", program], os.environ.copy(), timeout=1)
            except SystemExit as error:
                expected = {"nonzero": 7, "multiple": "exact selection produced multiple test results",
                            "retained-pipe": "exact test exceeded its execution deadline"}[mode]
                if error.code != expected:
                    raise
            else:
                raise RuntimeError("failed exact invocation was accepted")
        if time.monotonic() - began > 3:
            raise RuntimeError("exact failure cleanup exceeded its bound")
        pid = int(pid_path.read_text())
        until = time.monotonic() + 2
        while True:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            if time.monotonic() >= until:
                raise RuntimeError("exact failure left a live owned descendant")
            time.sleep(.01)


def failed_g7_status(root: Path) -> None:
    binary = root / "g7-status-binary"
    binary.write_bytes(b"status-only fixture, not a qualified product")

    class FailedMeasurements:
        def __init__(self, output, cli, helper):
            self.cli, self.helper, self.environment = cli, helper, {}
            self.samples = []
            self.cleaned = False

        def fixtures(self):
            return {name: {"worktree": root, "base": []} for name in ("S", "M", "L")}

        def restore(self, *args):
            pass

        def iterations(self, worktree, name, *args):
            for command, (_, p99) in qualify_direct_cli.G7_BASELINE[name].items():
                self.samples.extend({"fixture": name, "command": command, "warmup": False,
                                     "elapsed_us": p99 * 2000} for _ in range(220))

        def g8(self, fixtures):
            raise RuntimeError("G7 failure did not stop before G8")

        def cleanup(self):
            self.cleaned = True

    runs = []

    def create(*args):
        run = FailedMeasurements(*args)
        runs.append(run)
        return run

    argv = ["qualify_direct_cli.py", "--cli", str(binary), "--helper", str(binary),
            "--evidence-parent", str(root)]
    with mock.patch.object(sys, "argv", argv), \
            mock.patch.object(qualify_direct_cli, "Qualification", create), \
            mock.patch.object(qualify_direct_cli.platform, "system", return_value="Darwin"), \
            mock.patch.object(qualify_direct_cli.platform, "machine", return_value="arm64"), \
            mock.patch.object(qualify_direct_cli.subprocess, "check_output", side_effect=lambda argv, **kwargs: b"" if argv[:2] == ["git", "ls-files"] else b"AC Power"), \
            mock.patch.object(qualify_direct_cli, "bounded", return_value=(subprocess.CompletedProcess([], 0, b"{}", b""), 1)), \
            contextlib.redirect_stdout(io.StringIO()):
        try:
            qualify_direct_cli.main()
        except RuntimeError as error:
            if str(error) != "G7 failed; do not retry without owner direction":
                raise
        else:
            raise RuntimeError("failed G7 measurements produced success")
    reports = list(root.glob("v2dfc007-real-gates-*/report.json"))
    if len(reports) != 1 or json.loads(reports[0].read_text())["status"] != "G7-failed" or not runs[0].cleaned:
        raise RuntimeError("failed G7 did not retain failure status and cleanup")


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="podway-qualification-tools-") as name:
        root = Path(name)
        source = root / "selection.rs"
        source.write_text('#[test] fn first() {}\n#[test] fn second() {}\n')
        binary = root / "selection"
        subprocess.run(["rustc", "--test", str(source), "-o", str(binary)], check=True)
        with contextlib.redirect_stdout(io.StringIO()):
            rejects(lambda: run_e2e.run_exact([str(binary), "--exact", "missing"], os.environ.copy(), timeout=10))
            rejects(lambda: run_e2e.run_exact([str(binary)], os.environ.copy(), timeout=10))
            run_e2e.run_exact([str(binary), "--exact", "first"], os.environ.copy(), timeout=10)
        # EOF must not turn the remaining process wait into an unbounded wait.
        started = time.monotonic()
        rejects(lambda: run_e2e.run_exact(
            [sys.executable, "-c", "import os,time; os.close(1); time.sleep(30)"],
            os.environ.copy(), timeout=0.1,
        ), (SystemExit, subprocess.TimeoutExpired))
        if time.monotonic() - started > 2:
            raise RuntimeError("exact test deadline did not bound the wait after EOF")
        exact_output_boundaries()
        with tempfile.TemporaryDirectory(prefix="exact-cleanup-", dir=root) as cleanup_root:
            exact_failure_cleanup(Path(cleanup_root))
        with tempfile.TemporaryDirectory(prefix="g7-status-", dir=root) as status_root:
            failed_g7_status(Path(status_root))
        for tool in ("qualify_direct_cli.py", "run_native_qualification.py", "qualify_development.py"):
            output = root / "must-not-exist"
            result = subprocess.run([sys.executable, "-O", str(ROOT / "tools" / tool), "--evidence-parent", str(output)], capture_output=True, text=True, check=False)
            if result.returncode == 0 or "assertions" not in result.stderr or output.exists():
                raise RuntimeError("optimized qualification did not refuse before evidence effects")
        original_cases = run_native_qualification.CASES
        try:
            run_native_qualification.CASES = ()
            rejects(lambda: verify_quality_contracts.require_test_route(
                ROOT / "crates/podway-engine/tests/int_v2dfc006_crashes.rs",
                "sigkill_at_all_five_engine_boundaries_preserves_effect_once_recovery",
            ), verify_quality_contracts.ContractError)
        finally:
            run_native_qualification.CASES = original_cases
        original_root = release_archive.ROOT
        source.unlink()
        binary.unlink()
        try:
            release_archive.ROOT = root
            subprocess.run(["git", "init", "--quiet", str(root)], check=True)
            (root / "tools").mkdir()
            sentinel = root / "tools/sentinel.py"
            sentinel.write_text("original\n")
            subprocess.run(["git", "-C", str(root), "add", "tools/sentinel.py"], check=True)
            snapshot = release_archive.source_snapshot_sha256()
            # A source mutation between a completed check and the next stage
            # must invalidate the original snapshot rather than replace it.
            subprocess.run([sys.executable, "-c", "from pathlib import Path; Path('tools/sentinel.py').write_text('changed\\n')"], cwd=root, check=True)
            rejects(lambda: qualify_development.require_frozen(snapshot), release_archive.ReleaseError)
        finally:
            release_archive.ROOT = original_root
        binary.write_bytes(b"checked build fixture")
        receipt = {"schema": "podway.development-checks/v1", "source_snapshot_sha256": "a" * 64,
                   "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                   "checks": [{"argv": list(argv), "exit_code": 0, "source_before": "a" * 64, "source_after": "a" * 64}
                              for argv in qualify_development.STEPS]}
        qualify_development.validate_checks(receipt, "a" * 64, binary)
        invalid = copy.deepcopy(receipt)
        invalid["checks"][0]["source_before"] = "b" * 64
        rejects(lambda: qualify_development.validate_checks(invalid, "a" * 64, binary), release_archive.ReleaseError)
        invalid = copy.deepcopy(receipt)
        invalid["checks"].pop()
        rejects(lambda: qualify_development.validate_checks(invalid, "a" * 64, binary), release_archive.ReleaseError)
        binary.write_bytes(b"different build")
        rejects(lambda: qualify_development.validate_checks(receipt, "a" * 64, binary), release_archive.ReleaseError)
    print(json.dumps({"ok": True, "mode": "qualification-tool-checks"}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
