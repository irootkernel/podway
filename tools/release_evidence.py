#!/usr/bin/env python3
"""Canonical Podway release-evidence shapes and atomic publication helpers."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import sys
import tempfile
import time
from typing import Any


PRODUCT = "podway"
PROVENANCE_SCHEMA = "podway.release-provenance/v3"
PATCH_PROVENANCE_SCHEMA = "podway.release-provenance/v4"
HANDOFF_SCHEMA = "podway.dolgorae-compatibility-handoff/v4"
PATCH_HANDOFF_SCHEMA = "podway.dolgorae-compatibility-handoff/v5"
RELEASE_GATE = "make test + fuzzing: passed"
PATCH_RELEASE_GATE = "make dist-patch: passed"
DEVELOPMENT_GATE = "make test + make release-prepare + make test-fuzzing: passed"
PASSED = "passed"
PENDING = "pending"
PACKAGED_CONFORMANCE_SCENARIOS = [
    "e2e_v2dfc007_native_conformance::compact_status_keeps_only_bounded_metadata_and_current_identity",
    "e2e_v2dfc007_native_conformance::procedure_workspace_and_session_conflicts_leave_no_mutation_or_outcome",
    "e2e_v2dfc007_native_conformance::public_goal_flow_preserves_rework_and_stale_attempt_history_across_processes",
    "e2e_v2dfc007_native_conformance::destructive_lifecycle_preserves_user_files_and_config_and_reinitializes_identity",
    "e2e_v2dfc007_direct_cli::a_sqlite_writer_bounds_contention_without_recording_a_failed_outcome",
    "e2e_v2dfc007_direct_cli::sigkill_after_commit_before_stdout_preserves_the_original_outcome_and_exact_replay",
    "e2e_v2dfc007_direct_cli::record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight",
    "e2e_v2dfc007_direct_cli::delegation_stdin_acceptance_receipt_and_completion_preserve_the_real_attempt",
    "e2e_v2dfc007_direct_cli::every_legacy_generation_refuses_without_new_files_then_explicitly_resets",
    "e2e_v2dfc007_runtime_cli::generated_item_recipes_preserve_all_types_clearing_and_attachment_evidence",
    "e2e_v2dfc007_runtime_cli::generated_cursor_and_goal_recipes_require_only_declared_placeholder_values",
    "e2e_v2dfc007_runtime_cli::evidence_continuations_preserve_unicode_pages_and_rework_invalidates_old_tokens",
    "e2e_v2dfc007_runtime_cli::bare_terminal_archive_uses_no_active_attempt_and_replays_after_current_session_removal",
    "e2e_v2dfc007_retirement::retired_surface_returns_valid_guidance_without_reading_open_stdin_or_worktree",
    "e2e_v2sdg008_qualification::supervised_delegation_preserves_documented_example_hosts_and_recovery",
    "e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance",
    "e2e_v2_native_qualification::release_binary_ignores_every_debug_maintenance_hook",
]

RELEASE_STATUS = {"notarization": "not-attempted", "signing": "unsigned"}
SHA256 = re.compile(r"[0-9a-f]{64}")
IDENTITY_SHA256 = re.compile(r"sha256:[0-9a-f]{64}")
GIT_OBJECT = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})")

PROVENANCE_KEYS = {
    "archive",
    "artifact_class",
    "binaries",
    "build_identity",
    "cargo_lock_sha256",
    "contract_manifest_digest",
    "contract_manifest_schema",
    "packaged_conformance",
    "product",
    "release_gate",
    "release_gate_result",
    "release_status",
    "schema",
    "source_commit",
    "source_dirty",
    "source_tree",
    "source_snapshot_sha256",
    "target",
    "toolchain",
    "version",
}
HANDOFF_KEYS = {
    "artifact_class",
    "adapter",
    "adapter_catalog",
    "artifact",
    "binaries",
    "build_identity",
    "contract",
    "packaged_conformance",
    "product",
    "provenance",
    "release_gate",
    "release_gate_result",
    "release_status",
    "schema",
    "source",
    "target",
    "toolchain",
    "version",
}
PATCH_PROVENANCE_KEYS = (PROVENANCE_KEYS - {"packaged_conformance"}) | {"patch_gate"}
PATCH_HANDOFF_KEYS = (HANDOFF_KEYS - {"packaged_conformance"}) | {"patch_gate"}


class EvidenceError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise EvidenceError(message)


def canonical_bytes(value: dict[str, Any]) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_object(path: Path, label: str) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file: {path}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"{label} is not readable JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def scratch_parent() -> Path:
    volume = Path("/Volumes/RootKernel")
    if os.path.ismount(volume) and os.access(volume, os.W_OK):
        parent = volume / "tmp"
        if parent.is_symlink():
            fail("scratch parent must not be a symlink")
        parent.mkdir(exist_ok=True)
        return parent
    return Path(tempfile.gettempdir())


def require_process_group_gone(group: int) -> None:
    deadline = time.monotonic() + 3
    while True:
        completed = subprocess.run(["ps", "-axo", "pid=,pgid=,stat="],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=5, text=True)
        if completed.returncode != 0:
            fail(f"cannot prove cleanup of owned process group {group}")
        remaining = []
        for line in completed.stdout.splitlines():
            fields = line.split()
            if len(fields) == 3 and fields[1] == str(group) and not fields[2].startswith("Z"):
                remaining.append(fields[0])
        if not remaining:
            return
        if time.monotonic() >= deadline:
            fail(f"owned process group {group} still has live children: {remaining}")
        time.sleep(0.01)


def run_bounded(arguments: list[str], *, cwd: Path, environment: dict[str, str] | None = None,
    timeout_seconds: float = 300, label: str) -> subprocess.CompletedProcess[bytes]:
    process = subprocess.Popen(arguments, cwd=cwd, env=environment,
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    deadline = time.monotonic() + timeout_seconds
    buffers = {"stdout": bytearray(), "stderr": bytearray()}
    selector = selectors.DefaultSelector()
    try:
        for name, stream in (("stdout", process.stdout), ("stderr", process.stderr)):
            assert stream is not None
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, name)
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                fail(f"{label} timed out after {timeout_seconds:g}s")
            for key, _events in selector.select(min(remaining, 0.2)):
                data = os.read(key.fileobj.fileno(), 65536)
                if not data:
                    selector.unregister(key.fileobj)
                    continue
                buffers[key.data].extend(data)
                if len(buffers[key.data]) > 8 * 1024 * 1024:
                    fail(f"{label} exceeded its output budget")
        try:
            process.wait(timeout=max(0.001, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            fail(f"{label} timed out after {timeout_seconds:g}s")
    finally:
        selector.close()
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            fail(f"{label} cannot reap owned child {process.pid}")
        for stream in (process.stdout, process.stderr):
            if stream is not None:
                stream.close()
        require_process_group_gone(process.pid)
    return subprocess.CompletedProcess(arguments, process.returncode, bytes(buffers["stdout"]), bytes(buffers["stderr"]))


def atomic_write_bytes(path: Path, value: bytes) -> None:
    if path.parent.is_symlink() or not path.parent.is_dir():
        fail(f"output directory must be a regular directory: {path.parent}")
    descriptor, temporary_name = tempfile.mkstemp(
        dir=path.parent, prefix=f".{path.name}.", suffix=".tmp"
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as opened:
            opened.write(value)
            opened.flush()
            os.fsync(opened.fileno())
        temporary.chmod(0o644)
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def atomic_write_json(path: Path, value: dict[str, Any]) -> None:
    atomic_write_bytes(path, canonical_bytes(value))


def _exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    if set(value) != expected:
        fail(
            f"{label} fields mismatch: missing={sorted(expected - set(value))}, "
            f"unknown={sorted(set(value) - expected)}"
        )


def _exact_object(value: Any, expected: dict[str, Any], label: str) -> None:
    if json.dumps(value, sort_keys=True, separators=(",", ":")) != json.dumps(
        expected, sort_keys=True, separators=(",", ":")
    ):
        fail(f"{label} must equal {expected!r}")


def _digest(value: Any, label: str, *, prefixed: bool = False) -> str:
    pattern = IDENTITY_SHA256 if prefixed else SHA256
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        fail(f"{label} must be a lowercase SHA-256 identity")
    return value


def _string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{label} must be a non-empty string")
    return value


def validate_packaged_conformance(value: Any, expected_result: str) -> None:
    if expected_result not in {PENDING, PASSED}:
        fail(f"unsupported expected packaged-conformance result: {expected_result}")
    _exact_object(
        value,
        {"result": expected_result, "scenarios": PACKAGED_CONFORMANCE_SCENARIOS},
        "packaged_conformance",
    )


def validate_release_status(value: Any) -> None:
    _exact_object(value, RELEASE_STATUS, "release_status")


def validate_patch_gate(value: Any) -> None:
    if not isinstance(value, dict):
        fail("patch_gate must be an object")
    _exact_keys(
        value,
        {"prior_make_test_passed", "tested_baseline_commit"},
        "patch_gate",
    )
    if value["prior_make_test_passed"] is not True:
        fail("patch_gate prior_make_test_passed must be the boolean true")
    baseline = value["tested_baseline_commit"]
    if not isinstance(baseline, str) or GIT_OBJECT.fullmatch(baseline) is None:
        fail("patch_gate tested_baseline_commit must be a full Git identity")


def validate_provenance(
    value: dict[str, Any],
    *,
    version: str,
    target: str,
    commit: str,
    tree: str,
    conformance_result: str | None,
) -> None:
    schema = value.get("schema")
    patch = schema == PATCH_PROVENANCE_SCHEMA
    if schema not in {PROVENANCE_SCHEMA, PATCH_PROVENANCE_SCHEMA}:
        fail(f"unsupported provenance schema: {schema!r}")
    _exact_keys(value, PATCH_PROVENANCE_KEYS if patch else PROVENANCE_KEYS, "provenance")
    if patch and conformance_result is not None:
        fail("patch provenance cannot claim packaged conformance")
    if not patch and conformance_result is None:
        fail("full provenance requires packaged conformance")
    development = value.get("artifact_class") == "development-qualification"
    if development and patch:
        fail("development qualification cannot use the patch gate")
    expected = {
        "artifact_class": "development-qualification" if development else "distribution",
        "product": PRODUCT,
        "release_gate": DEVELOPMENT_GATE if development else (PATCH_RELEASE_GATE if patch else RELEASE_GATE),
        "release_gate_result": PASSED,
        "schema": PATCH_PROVENANCE_SCHEMA if patch else PROVENANCE_SCHEMA,
        "source_commit": commit,
        "source_dirty": value.get("source_dirty") if development else False,
        "source_tree": tree,
        "target": target,
        "version": version,
    }
    mismatches = {
        key: {"expected": expected_value, "actual": value.get(key)}
        for key, expected_value in expected.items()
        if value.get(key) != expected_value
    }
    if mismatches:
        fail(f"provenance identity mismatch: {mismatches}")
    if not isinstance(value["source_dirty"], bool) or (not development and value["source_dirty"]):
        fail("provenance source_dirty must be a boolean, and false for distribution")
    _digest(value["source_snapshot_sha256"], "provenance source snapshot digest")
    if GIT_OBJECT.fullmatch(commit) is None or GIT_OBJECT.fullmatch(tree) is None:
        fail("qualified Git commit and tree must be lowercase object identities")
    archive = value["archive"]
    if not isinstance(archive, dict):
        fail("provenance archive must be an object")
    _exact_keys(archive, {"name", "sha256"}, "provenance archive")
    _string(archive["name"], "provenance archive name")
    _digest(archive["sha256"], "provenance archive digest")
    binaries = value["binaries"]
    if not isinstance(binaries, dict):
        fail("provenance binaries must be an object")
    _exact_keys(binaries, {"podway"}, "provenance binaries")
    for role in ("podway",):
        _digest(binaries[role], f"provenance {role} digest")
    _digest(value["build_identity"], "provenance build identity", prefixed=True)
    _digest(value["cargo_lock_sha256"], "provenance Cargo.lock digest")
    _digest(value["contract_manifest_digest"], "provenance manifest digest", prefixed=True)
    if value["contract_manifest_schema"] != "podway.contract-manifest/v2":
        fail("provenance contract manifest schema is invalid")
    _string(value["toolchain"], "provenance toolchain")
    validate_release_status(value["release_status"])
    if patch:
        validate_patch_gate(value["patch_gate"])
    else:
        assert conformance_result is not None
        validate_packaged_conformance(value["packaged_conformance"], conformance_result)


def handoff_from_provenance(
    provenance: dict[str, Any],
    provenance_name: str,
    provenance_sha256: str,
    adapter: dict[str, Any],
    adapter_catalog_sha256: str,
) -> dict[str, Any]:
    patch = provenance.get("schema") == PATCH_PROVENANCE_SCHEMA
    handoff = {
        "artifact_class": provenance["artifact_class"],
        "adapter": adapter,
        "adapter_catalog": {
            "path": "release/dolgorae-adapter-contract-v3.json",
            "sha256": adapter_catalog_sha256,
        },
        "artifact": provenance["archive"],
        "binaries": provenance["binaries"],
        "build_identity": provenance["build_identity"],
        "contract": {
            "digest": provenance["contract_manifest_digest"],
            "schema": provenance["contract_manifest_schema"],
        },
        "product": provenance["product"],
        "provenance": {"name": provenance_name, "sha256": provenance_sha256},
        "release_gate": provenance["release_gate"],
        "release_gate_result": provenance["release_gate_result"],
        "release_status": provenance["release_status"],
        "schema": PATCH_HANDOFF_SCHEMA if patch else HANDOFF_SCHEMA,
        "source": {
            "clean": not provenance["source_dirty"],
            "commit": provenance["source_commit"],
            "tree": provenance["source_tree"],
            "snapshot_sha256": provenance["source_snapshot_sha256"],
        },
        "target": provenance["target"],
        "toolchain": {
            "cargo_lock_sha256": provenance["cargo_lock_sha256"],
            "rustc": provenance["toolchain"],
        },
        "version": provenance["version"],
    }
    if patch:
        handoff["patch_gate"] = provenance["patch_gate"]
    else:
        handoff["packaged_conformance"] = provenance["packaged_conformance"]
    return handoff


def validate_handoff(
    value: dict[str, Any],
    provenance: dict[str, Any],
    provenance_name: str,
    provenance_sha256: str,
    adapter: dict[str, Any],
    adapter_catalog_sha256: str,
) -> None:
    patch = provenance.get("schema") == PATCH_PROVENANCE_SCHEMA
    _exact_keys(value, PATCH_HANDOFF_KEYS if patch else HANDOFF_KEYS, "handoff")
    expected = handoff_from_provenance(
        provenance,
        provenance_name,
        provenance_sha256,
        adapter,
        adapter_catalog_sha256,
    )
    if canonical_bytes(value) != canonical_bytes(expected):
        fail("handoff does not exactly and bidirectionally repeat provenance identities")


def mark_packaged_conformance_passed(path: Path, provenance: dict[str, Any]) -> dict[str, Any]:
    updated = dict(provenance)
    updated["packaged_conformance"] = {
        "result": PASSED,
        "scenarios": PACKAGED_CONFORMANCE_SCENARIOS,
    }
    atomic_write_json(path, updated)
    return updated


def self_test() -> dict[str, Any]:
    if len(PACKAGED_CONFORMANCE_SCENARIOS) != len(set(PACKAGED_CONFORMANCE_SCENARIOS)):
        fail("packaged conformance scenarios must be unique")
    if any("::" not in name for name in PACKAGED_CONFORMANCE_SCENARIOS):
        fail("packaged conformance scenarios must be acceptance tests")
    with tempfile.TemporaryDirectory(prefix="podway-evidence-self-test-", dir=scratch_parent()) as name:
        root = Path(name)
        target = root / "evidence.json"
        target.write_text("original\n", encoding="utf-8")
        atomic_write_json(target, {"stable": True})
        if target.read_bytes() != canonical_bytes({"stable": True}):
            fail("atomic evidence publication produced unexpected bytes")
        victim = root / "victim"
        victim.write_text("unchanged\n", encoding="utf-8")
        target.unlink()
        target.symlink_to(victim)
        atomic_write_json(target, {"replacement": True})
        if victim.read_text(encoding="utf-8") != "unchanged\n" or target.is_symlink():
            fail("atomic evidence publication followed an output symlink")
    return {"mode": "self-test", "ok": True, "scenarios": len(PACKAGED_CONFORMANCE_SCENARIOS)}


def main() -> int:
    if sys.argv[1:] != ["self-test"]:
        print("usage: release_evidence.py self-test", file=sys.stderr)
        return 2
    try:
        result = self_test()
    except (EvidenceError, OSError, UnicodeError) as error:
        print(json.dumps({"error": str(error), "mode": "self-test", "ok": False}), file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
