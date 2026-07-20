#!/usr/bin/env python3
"""Generate the G036 trusted-verifier-replay test report.

This generator emits ``artifacts/g036/g036-test-report.json`` in exactly the
form that ``verify_g009_qualification.validate_g036_test_report`` recomputes and
compares. To make drift impossible it imports and reuses the verifier's own
helpers rather than reimplementing any of them: the validated matrix, the
product source tree, the exact Cargo command set, the trusted toolchain, the
per-invocation replay identity, and the per-command hermetic replay.

Authenticity model (fail-closed, never fabricated)
--------------------------------------------------
A G036 report is only valid when every one of the 50 exact commands runs
successfully under the hermetic sandbox with exactly one passing test and zero
ignored tests -- the validator enforces ``exitCode == 0``, ``testCount == 1``
and ``ignoredCount == 0`` for every receipt. Those are therefore the only
values a passing receipt can hold. This generator builds each receipt with
that unique passing contract and then calls the verifier's own
``_trusted_replay_g036_command`` to *prove* it by real execution: if any real
command exits non-zero, or does not run exactly its named test, the helper
raises and generation aborts without writing anything. The recorded exit codes
and counts are thus authenticated by execution, not asserted.

Cost
----
``--plan`` performs only the cheap preflight: it validates the matrix, computes
every binding and the 50-command plan, and prints the commands. It runs no
Cargo command and does not require the native host.

Running without ``--plan`` executes the full 50-command hermetic replay
(each command prebuilds and runs an exact test binary under sandbox-exec) and
takes tens of minutes on the native Apple-Silicon host. The supervisor drives
that separately.
"""

from __future__ import annotations

import argparse
import contextlib
from datetime import datetime, timezone
from pathlib import Path
import secrets
import shlex
import shutil
import sys
import tempfile
from typing import Any

from g009_common import canonical_json, sha256_bytes, sha256_file
from verify_g009_qualification import (
    G036_CRITERION_COUNT,
    G036_EXACT_COMMAND_COUNT,
    G036_MATRIX_PATH,
    G036_MATRIX_SHA256,
    G036_REPORT_PATH,
    G036_TARGET,
    QualificationError,
    _g036_product_source_tree,
    _g036_replay_identity,
    _g036_toolchain,
    _matrix_cargo_commands,
    _trusted_replay_g036_command,
    validate_product_acceptance_matrix,
)

ROOT = Path(__file__).resolve().parents[1]
POLICY_PATH = ROOT / "release/g009-release-policy-v1.json"
VERIFIER_PATH = ROOT / "tools/verify_g009_qualification.py"


def _relative(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def _binding(path: Path) -> dict[str, str]:
    if path.is_symlink() or not path.is_file():
        raise QualificationError(f"G036 source binding is absent or unsafe: {path}")
    return {"path": _relative(path), "sha256": sha256_file(path)}


def _source_block(matrix: dict[str, Any]) -> dict[str, Any]:
    """Recompute the report ``source`` block exactly as the validator expects."""
    matrix_binding = _binding(G036_MATRIX_PATH)
    if matrix_binding["sha256"] != G036_MATRIX_SHA256:
        raise QualificationError("G036 matrix digest does not match the frozen constant")
    return {
        "cargoLock": _binding(ROOT / "Cargo.lock"),
        "testSources": matrix["source_files"],
        "matrix": matrix_binding,
        "policy": _binding(POLICY_PATH),
        "verifier": _binding(VERIFIER_PATH),
        "productSourceTree": _g036_product_source_tree(),
    }


def _scope_block(matrix: dict[str, Any]) -> dict[str, int]:
    """Recompute scope counts the way the validator derives them."""
    return {
        "criterionCount": G036_CRITERION_COUNT,
        "cargoCriterionCount": sum(
            item["proof"]["kind"] in {"cargo-test", "cargo-test-set"} for item in matrix["criteria"]
        ),
        "artifactCriterionCount": sum(
            item["proof"]["kind"] == "artifact" for item in matrix["criteria"]
        ),
        "exactCommandCount": G036_EXACT_COMMAND_COUNT,
    }


def _criteria_block(matrix: dict[str, Any]) -> list[dict[str, Any]]:
    return [{"id": criterion["id"], "proof": criterion["proof"]} for criterion in matrix["criteria"]]


def _authenticated_commands(
    matrix: dict[str, Any], persistent_root: Path | None
) -> list[dict[str, Any]]:
    """Run the full 50-command hermetic replay and return authenticated receipts.

    Each receipt is built with the only values a passing exact-test replay can
    hold and then authenticated by the verifier's own per-command replay, which
    fails closed on any real drift. This is the expensive part.

    ``persistent_root`` reuses one replay root across invocations so retries
    keep the built daemon and Cargo cache; authenticity is unaffected because
    every command still runs for real under the sandbox.
    """
    expected_commands = _matrix_cargo_commands(matrix)
    if len(expected_commands) != G036_EXACT_COMMAND_COUNT:
        raise QualificationError("G036 exact command count drift")
    toolchain = _g036_toolchain()
    receipts: list[dict[str, Any]] = []
    if persistent_root is not None:
        persistent_root.mkdir(parents=True, exist_ok=True)
        raw_context: Any = contextlib.nullcontext(str(persistent_root))
    else:
        raw_context = tempfile.TemporaryDirectory(prefix="p36-", dir="/private/tmp")
    with raw_context as raw:
        replay_root = Path(raw)
        target_dir = replay_root / "target"
        # The daemon receipt snapshot is one-shot (mkdir exist_ok=False), so a
        # reused root keeps only the Cargo cache and gets a fresh receipt
        # directory name every invocation; stale ones from prior retries are
        # removed first.
        receipts_dir = replay_root / "receipts"
        if persistent_root is not None:
            for stale in replay_root.glob("receipts*"):
                if stale.is_dir() and not stale.is_symlink():
                    shutil.rmtree(stale)
            receipts_dir = replay_root / f"receipts-{secrets.token_hex(8)}"
        identity = _g036_replay_identity(toolchain, target_dir, receipts_dir)
        host_toolchain_sha256 = sha256_bytes(
            canonical_json({"host": identity["host"], "toolchain": identity["toolchain"]})
        )
        input_tree_sha256 = identity["inputTree"]["sha256"]
        for index, (command, descriptor) in enumerate(expected_commands.items(), 1):
            sys.stderr.write(f"[g036] replay {index}/{len(expected_commands)}: {command}\n")
            sys.stderr.flush()
            receipt = {
                "command": command,
                "argv": shlex.split(command),
                "semanticBindings": descriptor["semanticBindings"],
                "inputTreeSha256": input_tree_sha256,
                "hostToolchainSha256": host_toolchain_sha256,
                "exitCode": 0,
                "testCount": 1,
                "ignoredCount": 0,
            }
            # Fail-closed authentication: raises unless the real hermetic run
            # exits 0 with exactly its one named passing test and zero ignored.
            _trusted_replay_g036_command(receipt, descriptor, identity, target_dir)
            receipts.append(receipt)
    return receipts


def build_report(matrix: dict[str, Any], command_receipts: list[dict[str, Any]]) -> dict[str, Any]:
    status = "passed" if all(
        receipt["exitCode"] == 0 and receipt["testCount"] == 1 and receipt["ignoredCount"] == 0
        for receipt in command_receipts
    ) else "failed"
    report = {
        "schemaVersion": 6,
        "kind": "api-package-test-report",
        "storyId": "G036",
        "generatedAt": datetime.now(timezone.utc).isoformat(),
        "target": dict(G036_TARGET),
        "source": _source_block(matrix),
        "scope": _scope_block(matrix),
        "criteria": _criteria_block(matrix),
        "commands": command_receipts,
        "artifactProofs": [],
        "replay": {"kind": "trusted-verifier-replay", "requireSingleCurrentInvocation": True},
        "result": {"status": status},
    }
    _structural_selfcheck(report)
    return report


def _structural_selfcheck(report: dict[str, Any]) -> None:
    """Cheap assembly guard (no replay); the verifier remains the authority."""
    required = {
        "schemaVersion", "kind", "storyId", "generatedAt", "target", "source",
        "scope", "criteria", "commands", "artifactProofs", "replay", "result",
    }
    if set(report) != required:
        raise QualificationError("G036 generated report key set drift")
    if len(report["criteria"]) != G036_CRITERION_COUNT:
        raise QualificationError("G036 generated report criterion count drift")
    if len(report["commands"]) != G036_EXACT_COMMAND_COUNT:
        raise QualificationError("G036 generated report command count drift")
    if report["result"] != {"status": "passed"}:
        raise QualificationError("G036 generated report is not a passing replay")


def _print_plan(matrix: dict[str, Any]) -> None:
    source = _source_block(matrix)
    scope = _scope_block(matrix)
    commands = _matrix_cargo_commands(matrix)
    print(f"G036 report output: {G036_REPORT_PATH}")
    print("G036 source bindings:")
    for field in ("cargoLock", "matrix", "policy", "verifier"):
        print(f"  {field}: {source[field]['path']} {source[field]['sha256']}")
    print(f"  testSources: {len(source['testSources'])} files")
    print(
        "  productSourceTree: "
        f"{len(source['productSourceTree']['paths'])} paths sha256={source['productSourceTree']['sha256']}"
    )
    print(f"G036 scope: {scope}")
    print(f"G036 criteria: {len(matrix['criteria'])}")
    print(f"G036 exact command plan ({len(commands)} commands):")
    for index, command in enumerate(commands, 1):
        print(f"  {index:2d}. {command}")
    if len(commands) != G036_EXACT_COMMAND_COUNT:
        raise QualificationError("G036 exact command count drift")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--plan",
        action="store_true",
        help="cheap preflight: validate the matrix and print the 50-command plan without running it",
    )
    parser.add_argument(
        "--replay-root",
        type=Path,
        default=None,
        help="persistent replay root reused across retries (default: fresh temporary root)",
    )
    args = parser.parse_args()
    try:
        matrix = validate_product_acceptance_matrix()
        if args.plan:
            _print_plan(matrix)
            return 0
        command_receipts = _authenticated_commands(matrix, args.replay_root)
        report = build_report(matrix, command_receipts)
        G036_REPORT_PATH.parent.mkdir(parents=True, exist_ok=True)
        payload = canonical_json(report) + b"\n"
        G036_REPORT_PATH.write_bytes(payload)
        digest = sha256_file(G036_REPORT_PATH)
        print(f"{G036_REPORT_PATH} {digest} {report['result']['status']}")
        return 0
    except QualificationError as exc:
        print(f"G036 report generation failed closed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
