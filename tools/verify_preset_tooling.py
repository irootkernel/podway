#!/usr/bin/env python3
"""Exercise contributor preset creation and import against the real Podway validator."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
TOOL = ROOT / "tools/manage_presets.py"
COMMAND_TIMEOUT_SECONDS = 45


def run_tool(podway: Path, *arguments: str) -> tuple[int, dict[str, Any]]:
    completed = subprocess.run(
        [sys.executable, TOOL.as_posix(), *arguments, "--podway", podway.as_posix()],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        timeout=COMMAND_TIMEOUT_SECONDS,
    )
    try:
        result = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise AssertionError(f"preset tool returned invalid JSON: {completed.stdout!r}; {error}") from error
    return completed.returncode, result


def validate_with_podway(podway: Path, source: Path) -> dict[str, Any]:
    completed = subprocess.run(
        [podway.as_posix(), "--json", "procedure", "validate", source.as_posix()],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        timeout=COMMAND_TIMEOUT_SECONDS,
    )
    if completed.returncode != 0:
        raise AssertionError(completed.stdout or completed.stderr)
    return json.loads(completed.stdout)


def expect_failure(podway: Path, *arguments: str) -> dict[str, Any]:
    code, result = run_tool(podway, *arguments)
    if code == 0 or result.get("ok") is not False:
        raise AssertionError(f"preset tool unexpectedly succeeded: {arguments}; {result}")
    return result


def verify(podway: Path) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="podway-preset-tool-") as temporary_name:
        root = Path(temporary_name)
        created = root / "created"
        imported = root / "imported"
        rejected = root / "rejected"
        created.mkdir()
        imported.mkdir()
        rejected.mkdir()

        code, receipt = run_tool(
            podway,
            "create",
            "--id",
            "release-check",
            "--name",
            "Release Check",
            "--description",
            "Repository release preparation and verification.",
            "--output-dir",
            created.as_posix(),
        )
        if code != 0 or receipt.get("ok") is not True:
            raise AssertionError(receipt)
        created_file = created / "release-check.yaml"
        validated = validate_with_podway(podway, created_file)
        procedure = validated["result"]["procedure"]
        if procedure["id"] != "release-check" or len(procedure["stages"]) != 2:
            raise AssertionError("created scaffold did not preserve its identity and two-stage baseline")
        original = created_file.read_bytes()
        expect_failure(
            podway,
            "create",
            "--id",
            "release-check",
            "--name",
            "Replacement",
            "--description",
            "Must not replace existing bytes.",
            "--output-dir",
            created.as_posix(),
        )
        if created_file.read_bytes() != original:
            raise AssertionError("duplicate create replaced the existing preset")
        expect_failure(
            podway,
            "create",
            "--id",
            "../escape",
            "--name",
            "Escape",
            "--description",
            "Must be rejected.",
            "--output-dir",
            created.as_posix(),
        )

        source = ROOT / "presets/docs-only.yaml"
        code, receipt = run_tool(
            podway,
            "import",
            "--source",
            source.as_posix(),
            "--output-dir",
            imported.as_posix(),
        )
        imported_file = imported / "docs-only.yaml"
        if code != 0 or receipt.get("ok") is not True or imported_file.read_bytes() != source.read_bytes():
            raise AssertionError(f"valid import did not preserve exact source bytes: {receipt}")
        expect_failure(
            podway,
            "import",
            "--source",
            source.as_posix(),
            "--output-dir",
            imported.as_posix(),
        )

        malformed = root / "malformed.yaml"
        malformed.write_text("schema: podway.procedure/v1\nid: malformed\n", encoding="utf-8")
        expect_failure(
            podway,
            "import",
            "--source",
            malformed.as_posix(),
            "--output-dir",
            rejected.as_posix(),
        )
        symlink = root / "linked.yaml"
        symlink.symlink_to(source)
        expect_failure(
            podway,
            "import",
            "--source",
            symlink.as_posix(),
            "--output-dir",
            rejected.as_posix(),
        )
        oversized = root / "oversized.yaml"
        with oversized.open("wb") as handle:
            handle.truncate(1_048_577)
        expect_failure(
            podway,
            "import",
            "--source",
            oversized.as_posix(),
            "--output-dir",
            rejected.as_posix(),
        )
        if list(rejected.iterdir()):
            raise AssertionError("rejected imports left output or staging files")
        staging = [path.name for directory in (created, imported) for path in directory.iterdir() if path.name.startswith(".preset-")]
        if staging:
            raise AssertionError(f"preset operations left staging files: {staging}")
        return {
            "created": created_file.name,
            "imported": imported_file.name,
            "rejections": ["duplicate", "unsafe-id", "malformed", "symlink", "oversized"],
        }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--podway", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = verify(arguments.podway.resolve(strict=True))
        print(json.dumps({"ok": True, **result}, sort_keys=True, separators=(",", ":")))
        return 0
    except (AssertionError, OSError, KeyError, TypeError, ValueError) as error:
        print(json.dumps({"error": {"message": str(error)}, "ok": False}, sort_keys=True, separators=(",", ":")))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
