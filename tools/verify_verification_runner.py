#!/usr/bin/env python3
"""Exercise verification-runner fail-closed sentinels without external test dependencies."""

from __future__ import annotations

from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
from typing import Any, Callable, Iterator

TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parent
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import phase0_receipts
import run_verification as runner


FIXTURE = ROOT / "tests/fixtures/phase0/verification-runner-sentinels.json"
MAX_FIXTURE_BYTES = 1024 * 1024


@contextmanager
def patched(module: Any, **changes: Any) -> Iterator[None]:
    previous = {name: getattr(module, name) for name in changes}
    try:
        for name, value in changes.items():
            setattr(module, name, value)
        yield
    finally:
        for name, value in previous.items():
            setattr(module, name, value)


def capture_runner_failure(action: Callable[[], Any]) -> str:
    try:
        action()
    except runner.VerificationError as error:
        return error.code
    raise AssertionError("expected verification runner rejection, but the action succeeded")


def capture_receipt_failure(action: Callable[[], Any]) -> str:
    try:
        action()
    except phase0_receipts.ContractError as error:
        return error.code
    raise AssertionError("expected receipt rejection, but the action succeeded")


def sentinel_environment(root: Path) -> dict[str, Any]:
    variables = runner.safe_environment(root)
    config_inputs = [
        {
            "origin": origin,
            "path": candidate.as_posix(),
            "resolved_path": candidate.resolve(strict=False).as_posix(),
            "state": "absent",
        }
        for origin, candidate in runner.expected_config_inputs(root, variables)
    ]
    return {
        "architecture": "sentinel",
        "cargo_v": "sentinel",
        "config_inputs": config_inputs,
        "executables": [],
        "platform": "sentinel",
        "python": "sentinel",
        "rustc_vv": "sentinel",
        "variables": variables,
    }


def report_for(root: Path, run_id: str, started_at_utc: str | None = None) -> dict[str, Any]:
    return runner.build_report(
        run_id=run_id,
        started_at_utc=started_at_utc or runner.current_utc_timestamp(),
        environment=sentinel_environment(root),
        source_manifest_digest="0" * 64,
        source_file_count=0,
        commands=[],
        status="failed",
    )


def write_report(root: Path, report: dict[str, Any], canonical: bool, pointer: dict[str, Any] | None = None) -> None:
    artifact = root / report["report_artifact"]["path"]
    artifact.parent.mkdir(parents=True, exist_ok=True)
    content = runner.canonical_json(report)
    artifact.write_bytes(content)
    if canonical:
        canonical_path = root / runner.REPORT_RELATIVE
        canonical_path.parent.mkdir(parents=True, exist_ok=True)
        canonical_path.write_bytes(content)
    if pointer is not None:
        pointer_path = root / runner.REPORT_POINTER_RELATIVE
        pointer_path.parent.mkdir(parents=True, exist_ok=True)
        pointer_path.write_bytes(runner.canonical_json(pointer))


def publish_test_report(root: Path, report: dict[str, Any]) -> None:
    content = runner.canonical_json(report)
    report_digest = runner.digest_bytes(content)
    write_report(root, report, canonical=True, pointer=runner.build_report_pointer(report, report_digest))


def test_stale_report() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-stale-") as temporary_name:
        root = Path(temporary_name).resolve()
        timestamp = (datetime.now(timezone.utc) - timedelta(seconds=runner.MAX_REPORT_AGE_SECONDS + 1)).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        )
        return [capture_runner_failure(lambda: runner.validate_report_shape(report_for(root, "1" * 32, timestamp), root))]


def test_future_report() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-future-") as temporary_name:
        root = Path(temporary_name).resolve()
        timestamp = (datetime.now(timezone.utc) + timedelta(seconds=60)).strftime("%Y-%m-%dT%H:%M:%SZ")
        return [capture_runner_failure(lambda: runner.validate_report_shape(report_for(root, "2" * 32, timestamp), root))]


def test_canonical_report_replacement() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-canonical-") as temporary_name:
        root = Path(temporary_name).resolve()
        first = report_for(root, "3" * 32)
        second = report_for(root, "4" * 32)
        publish_test_report(root, first)
        write_report(root, second, canonical=True)
        return [capture_runner_failure(lambda: runner.read_published_report(root))]


def test_immutable_report_replacement() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-immutable-") as temporary_name:
        root = Path(temporary_name).resolve()
        first = report_for(root, "5" * 32)
        second = report_for(root, "6" * 32)
        publish_test_report(root, first)
        replacement = root / first["report_artifact"]["path"]
        replacement.write_bytes(runner.canonical_json(second))
        return [capture_runner_failure(lambda: runner.read_published_report(root))]


def test_pointer_replacement() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-pointer-") as temporary_name:
        root = Path(temporary_name).resolve()
        first = report_for(root, "7" * 32)
        second = report_for(root, "8" * 32)
        publish_test_report(root, first)
        write_report(root, second, canonical=False)
        pointer_path = root / runner.REPORT_POINTER_RELATIVE
        pointer_path.write_bytes(runner.canonical_json(runner.build_report_pointer(second, runner.digest_bytes(runner.canonical_json(second)))))
        return [capture_runner_failure(lambda: runner.read_published_report(root))]


def test_report_self_digest_tamper() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-self-digest-") as temporary_name:
        root = Path(temporary_name).resolve()
        report = report_for(root, "9" * 32)
        report["report_identity"]["digest"] = "0" * 64
        return [capture_runner_failure(lambda: runner.validate_report_shape(report, root))]


def test_log_tamper() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-log-") as temporary_name:
        root = Path(temporary_name).resolve()
        relative = Path("logs/sentinel.log")
        path = root / relative
        path.parent.mkdir(parents=True)
        path.write_bytes(b"trusted")
        metadata = runner.log_metadata(root, relative, "sentinel log")
        path.write_bytes(b"tampered")
        return [
            capture_runner_failure(
                lambda: runner.validate_log_metadata(metadata, "sentinel log", relative, root, runner.MAX_GATE_STDOUT_BYTES)
            )
        ]


def manifest_root(root: Path, include_race_file: bool) -> dict[str, Any]:
    (root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    (root / "Cargo.lock").write_text("version = 3\n", encoding="utf-8")
    (root / "Makefile").write_text("test:\n\t@true\n", encoding="utf-8")
    (root / "deny.toml").write_text("[advisories]\n", encoding="utf-8")
    tools = root / "tools"
    tools.mkdir()
    race_file = tools / "race.py"
    if include_race_file:
        race_file.write_text("sentinel\n", encoding="utf-8")
    return runner.build_source_manifest(root)


def publish_manifest(root: Path, manifest: dict[str, Any], run_id: str) -> dict[str, Any]:
    content = runner.source_manifest_bytes(manifest)
    path = root / runner.expected_source_manifest_relative(run_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    return {
        "run_id": run_id,
        "source_manifest": {"file_count": len(manifest["inputs"]), "path": path.relative_to(root).as_posix(), "sha256": runner.digest_bytes(content)},
    }


def test_manifest_add_race() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-manifest-add-") as temporary_name:
        root = Path(temporary_name).resolve()
        report = publish_manifest(root, manifest_root(root, include_race_file=False), "a" * 32)
        (root / "tools/race.py").write_text("added\n", encoding="utf-8")
        return [capture_runner_failure(lambda: runner.validate_source_manifest(root, report))]


def test_manifest_delete_race() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-manifest-delete-") as temporary_name:
        root = Path(temporary_name).resolve()
        report = publish_manifest(root, manifest_root(root, include_race_file=True), "b" * 32)
        (root / "tools/race.py").unlink()
        return [capture_runner_failure(lambda: runner.validate_source_manifest(root, report))]


def test_executable_drift() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-executable-") as temporary_name:
        root = Path(temporary_name).resolve()
        executable = root / "runner-sentinel-tool"
        executable.write_bytes(b"first")
        executable.chmod(stat.S_IRUSR | stat.S_IWUSR | stat.S_IXUSR)
        environment = {"PATH": root.as_posix()}
        first, error = runner.collect_executable_identity(root, "runner-sentinel-tool", environment)
        if error is not None or first is None:
            raise AssertionError(error or "sentinel executable was not collected")
        executable.write_bytes(b"second")
        second, error = runner.collect_executable_identity(root, "runner-sentinel-tool", environment)
        if error is not None or second is None or first["sha256"] == second["sha256"]:
            raise AssertionError(error or "sentinel executable digest did not change")
        with patched(runner, collect_environment=lambda *_: ({"executables": [second]}, None)):
            return [capture_runner_failure(lambda: runner.validate_environment(root, {"environment": {"executables": [first]}}))]


def test_config_drift() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-config-") as temporary_name:
        root = Path(temporary_name).resolve()
        config = root / ".cargo/config.toml"
        config.parent.mkdir()
        config.write_text("[build]\ntarget-dir = 'one'\n", encoding="utf-8")
        environment = runner.safe_environment(root)
        first, error = runner.collect_config_inputs(root, environment)
        if error is not None:
            raise AssertionError(error)
        config.write_text("[build]\ntarget-dir = 'two'\n", encoding="utf-8")
        second, error = runner.collect_config_inputs(root, environment)
        if error is not None or first == second:
            raise AssertionError(error or "sentinel configuration identity did not change")
        with patched(runner, collect_environment=lambda *_: ({"config_inputs": second}, None)):
            return [capture_runner_failure(lambda: runner.validate_environment(root, {"environment": {"config_inputs": first}}))]


def test_gate_timeout() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-timeout-") as temporary_name:
        root = Path(temporary_name).resolve()
        run_id = runner.create_run_directory(root)
        with patched(
            runner,
            GATES=(("sentinel-timeout", (sys.executable, "-c", "import time; time.sleep(60)")),),
            GATE_TIMEOUT_SECONDS=(0,),
            GATE_TERMINATION_GRACE_SECONDS=1,
        ):
            result = runner.run_gate(root, run_id, 0, dict(os.environ))
        if result["status"] != "failed":
            raise AssertionError(f"timeout sentinel did not fail closed: {result}")
        return [result["termination"]]


def test_gate_output_overflow() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-overflow-") as temporary_name:
        root = Path(temporary_name).resolve()
        run_id = runner.create_run_directory(root)
        with patched(
            runner,
            GATES=(("sentinel-overflow", (sys.executable, "-c", "import sys; sys.stdout.write('x' * 4096)")),),
            GATE_TIMEOUT_SECONDS=(5,),
            GATE_TERMINATION_GRACE_SECONDS=1,
            MAX_GATE_STDOUT_BYTES=128,
            MAX_GATE_STDERR_BYTES=128,
            MAX_GATE_TOTAL_OUTPUT_BYTES=128,
        ):
            result = runner.run_gate(root, run_id, 0, dict(os.environ))
        if result["status"] != "failed":
            raise AssertionError(f"output sentinel did not fail closed: {result}")
        return [result["termination"]]


def test_probe_timeout() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-probe-timeout-") as temporary_name:
        root = Path(temporary_name).resolve()
        with patched(
            runner,
            PROBE_TIMEOUT_SECONDS=0,
            PROBE_TERMINATION_GRACE_SECONDS=1,
        ):
            result = runner.run_environment_probe(
                root, (sys.executable, "-c", "import time; time.sleep(60)"), dict(os.environ)
            )
        if result["failure"] is None or result["exit_code"] != 124:
            raise AssertionError(f"probe timeout sentinel did not fail closed: {result}")
        return [result["failure"]]


def test_probe_output_overflow() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-probe-overflow-") as temporary_name:
        root = Path(temporary_name).resolve()
        with patched(
            runner,
            PROBE_TIMEOUT_SECONDS=5,
            PROBE_TERMINATION_GRACE_SECONDS=1,
            MAX_PROBE_STDOUT_BYTES=128,
            MAX_PROBE_STDERR_BYTES=128,
            MAX_PROBE_TOTAL_OUTPUT_BYTES=128,
        ):
            result = runner.run_environment_probe(
                root, (sys.executable, "-c", "import sys; sys.stdout.write('x' * 4096)"), dict(os.environ)
            )
        if (
            result["failure"] is None
            or result["exit_code"] != 125
            or len(result["stdout"]) > 128
            or len(result["stderr"]) > 128
        ):
            raise AssertionError(f"probe overflow sentinel did not fail closed: {result}")
        return [result["failure"]]


def test_json_bounds() -> list[str]:
    with tempfile.TemporaryDirectory(prefix="podway-runner-json-") as temporary_name:
        root = Path(temporary_name).resolve()
        path = root / "oversized.json"
        path.write_bytes(b"{}")
        with patched(runner, MAX_REPORT_JSON_BYTES=1):
            runner_code = capture_runner_failure(
                lambda: runner.read_json_no_duplicates(path, "oversized report", runner.MAX_REPORT_JSON_BYTES)
            )
        with patched(phase0_receipts, MAX_JSON_BYTES=1):
            receipt_code = capture_receipt_failure(
                lambda: phase0_receipts.load_bounded_json(path, "oversized receipt", "invalid_json")
            )
        return [runner_code, receipt_code]


def test_manifest_bounds() -> list[str]:
    manifest = {
        "schema_version": runner.SOURCE_MANIFEST_SCHEMA_VERSION,
        "inputs": [{"path": "Cargo.toml", "sha256": "0" * 64, "size": 1}],
    }
    with patched(runner, MAX_SOURCE_MANIFEST_FILES=0):
        file_count_code = capture_runner_failure(lambda: runner.validate_source_manifest_shape(manifest))
    with patched(runner, MAX_SOURCE_INPUT_BYTES=0):
        total_bytes_code = capture_runner_failure(lambda: runner.validate_source_manifest_shape(manifest))
    with patched(runner, MAX_SOURCE_MANIFEST_JSON_BYTES=1):
        json_bytes_code = capture_runner_failure(lambda: runner.source_manifest_bytes(manifest))
    return list(dict.fromkeys((file_count_code, total_bytes_code, json_bytes_code)))


EXPECTED_REJECTION_CODES: dict[str, tuple[str, ...]] = {
    "stale-report": ("report_stale",),
    "future-report": ("report_from_future",),
    "canonical-report-replacement": ("canonical_pointer_replaced",),
    "immutable-report-replacement": ("canonical_pointer_replaced",),
    "pointer-replacement": ("canonical_pointer_replaced",),
    "report-self-digest-tamper": ("report_tampered",),
    "log-tamper": ("log_tampered",),
    "manifest-add-race": ("source_stale",),
    "manifest-delete-race": ("source_stale",),
    "executable-drift": ("environment_drift",),
    "config-drift": ("environment_drift",),
    "gate-timeout": ("deadline_exceeded",),
    "gate-output-overflow": ("output_limit_exceeded",),
    "probe-timeout": ("probe_timeout",),
    "probe-output-overflow": ("probe_output_overflow",),
    "json-bounds": ("resource_limit", "json_too_large"),
    "manifest-bounds": ("source_manifest_limit",),
}

TESTS: dict[str, Callable[[], list[str]]] = {
    "stale-report": test_stale_report,
    "future-report": test_future_report,
    "canonical-report-replacement": test_canonical_report_replacement,
    "immutable-report-replacement": test_immutable_report_replacement,
    "pointer-replacement": test_pointer_replacement,
    "report-self-digest-tamper": test_report_self_digest_tamper,
    "log-tamper": test_log_tamper,
    "manifest-add-race": test_manifest_add_race,
    "manifest-delete-race": test_manifest_delete_race,
    "executable-drift": test_executable_drift,
    "config-drift": test_config_drift,
    "gate-timeout": test_gate_timeout,
    "gate-output-overflow": test_gate_output_overflow,
    "probe-timeout": test_probe_timeout,
    "probe-output-overflow": test_probe_output_overflow,
    "json-bounds": test_json_bounds,
    "manifest-bounds": test_manifest_bounds,
}


def fixture_expected_codes(value: Any, label: str) -> tuple[str, ...]:
    if isinstance(value, str) and value:
        return (value,)
    if (
        isinstance(value, list)
        and len(value) > 1
        and all(isinstance(code, str) and code for code in value)
        and len(set(value)) == len(value)
    ):
        return tuple(value)
    raise AssertionError(f"{label} must be a non-empty code or a unique multi-code array")


def fixture_expected_value(codes: tuple[str, ...]) -> str | list[str]:
    return codes[0] if len(codes) == 1 else list(codes)


def load_fixture() -> list[dict[str, Any]]:
    raw = FIXTURE.read_bytes()
    if len(raw) > MAX_FIXTURE_BYTES:
        raise AssertionError(f"sentinel fixture exceeds {MAX_FIXTURE_BYTES} bytes")
    value = json.loads(raw.decode("utf-8"))
    if not isinstance(value, dict) or set(value) != {"schemaVersion", "kind", "cases"}:
        raise AssertionError("sentinel fixture has an invalid shape")
    if value["schemaVersion"] != 1 or value["kind"] != "podway.verification-runner-sentinels/v1":
        raise AssertionError("sentinel fixture identity drift")
    cases = value["cases"]
    if not isinstance(cases, list):
        raise AssertionError("sentinel fixture cases must be a list")
    normalized: list[dict[str, Any]] = []
    for index, case in enumerate(cases):
        if not isinstance(case, dict) or set(case) != {"id", "expected"}:
            raise AssertionError(f"sentinel fixture case {index} has an invalid shape")
        identifier = case["id"]
        if not isinstance(identifier, str) or not identifier:
            raise AssertionError(f"sentinel fixture case {index} has an invalid identifier")
        normalized.append(
            {
                "id": identifier,
                "expected": fixture_expected_codes(case["expected"], f"sentinel fixture case {index} expected"),
            }
        )
    if [case["id"] for case in normalized] != list(EXPECTED_REJECTION_CODES) or list(TESTS) != list(EXPECTED_REJECTION_CODES):
        raise AssertionError("sentinel fixture case order or coverage drift")
    for case in normalized:
        if case["expected"] != EXPECTED_REJECTION_CODES[case["id"]]:
            raise AssertionError(f"sentinel fixture expectation drift for {case['id']}")
    return normalized


def main() -> int:
    try:
        cases = load_fixture()
        results: list[dict[str, Any]] = []
        for case in cases:
            observed = TESTS[case["id"]]()
            expected = case["expected"]
            if observed != list(expected):
                raise AssertionError(f"sentinel {case['id']} expected {list(expected)}, observed {observed}")
            results.append(
                {
                    "expected": fixture_expected_value(expected),
                    "id": case["id"],
                    "observed": observed,
                    "result": "rejected",
                }
            )
        print(json.dumps({"fixture": FIXTURE.relative_to(ROOT).as_posix(), "ok": True, "results": results}, sort_keys=True, separators=(",", ":")))
        return 0
    except (
        AssertionError,
        runner.VerificationError,
        phase0_receipts.ContractError,
        OSError,
        ValueError,
        TypeError,
        json.JSONDecodeError,
    ) as error:
        print(json.dumps({"error": {"message": str(error)}, "ok": False}, sort_keys=True, separators=(",", ":")))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
