#!/usr/bin/env python3
"""Create or import canonical built-in preset source files for Podway development."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import uuid
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PRESET_DIRECTORY = ROOT / "assets/presets"
MAX_PROCEDURE_BYTES = 1_048_576
MAX_VALIDATOR_OUTPUT_BYTES = 4 * 1_048_576
VALIDATOR_TIMEOUT_SECONDS = 30
ID_PATTERN = re.compile(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$")


class PresetToolError(Exception):
    """A contributor preset operation failed closed."""


def fail(message: str) -> None:
    raise PresetToolError(message)


def object_no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, child in pairs:
        if key in value:
            fail(f"validator JSON repeats field: {key}")
        value[key] = child
    return value


def validate_identifier(identifier: str) -> str:
    if not isinstance(identifier, str):
        fail("preset id must be a string")
    if len(identifier.encode("utf-8")) > 64 or ID_PATTERN.fullmatch(identifier) is None:
        fail("preset id must match ^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$ and be at most 64 bytes")
    return identifier


def validate_text(value: str, label: str, maximum: int) -> str:
    if not isinstance(value, str):
        fail(f"{label} must be a string")
    if not value or len(value.encode("utf-8")) > maximum:
        fail(f"{label} must be non-empty and at most {maximum} UTF-8 bytes")
    return value


def regular_executable(path: Path) -> Path:
    candidate = path.resolve(strict=True)
    metadata = candidate.stat()
    if path.is_symlink() or not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o111 == 0:
        fail(f"podway validator must be an executable regular non-symlink file: {path}")
    return candidate


def output_directory(path: Path) -> Path:
    if path.is_symlink():
        fail(f"preset output directory must not be a symlink: {path}")
    candidate = path.resolve(strict=True)
    if not candidate.is_dir():
        fail(f"preset output directory is not a directory: {path}")
    return candidate


def read_bounded_regular_file(path: Path) -> bytes:
    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        fail(f"cannot open preset source safely: {error}")
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            fail("preset source must be a regular file")
        if metadata.st_size > MAX_PROCEDURE_BYTES:
            fail(f"preset source exceeds {MAX_PROCEDURE_BYTES} bytes")
        chunks: list[bytes] = []
        remaining = MAX_PROCEDURE_BYTES + 1
        while remaining:
            chunk = os.read(descriptor, min(65_536, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        content = b"".join(chunks)
        if len(content) > MAX_PROCEDURE_BYTES:
            fail(f"preset source exceeds {MAX_PROCEDURE_BYTES} bytes")
        final_metadata = os.fstat(descriptor)
        identity = (metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns)
        final_identity = (
            final_metadata.st_dev,
            final_metadata.st_ino,
            final_metadata.st_size,
            final_metadata.st_mtime_ns,
        )
        if final_identity != identity or len(content) != final_metadata.st_size:
            fail("preset source changed while it was being read")
        return content
    finally:
        os.close(descriptor)


def create_staged_file(directory: Path, content: bytes) -> Path:
    name = f".preset-{os.getpid()}-{uuid.uuid4().hex}.yaml"
    staged = directory / name
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(staged, flags, 0o600)
    try:
        view = memoryview(content)
        while view:
            written = os.write(descriptor, view)
            view = view[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    return staged


def validator_result(podway: Path, staged: Path) -> dict[str, Any]:
    try:
        completed = subprocess.run(
        [podway.as_posix(), "--json", "procedure", "validate", staged.as_posix()],
            cwd=ROOT,
            check=False,
            capture_output=True,
            timeout=VALIDATOR_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired:
        fail(f"podway validator exceeded {VALIDATOR_TIMEOUT_SECONDS} seconds")
    if len(completed.stdout) > MAX_VALIDATOR_OUTPUT_BYTES or len(completed.stderr) > MAX_VALIDATOR_OUTPUT_BYTES:
        fail("podway validator output exceeded its contributor-tool bound")
    if completed.returncode != 0:
        detail = completed.stdout or completed.stderr
        message = detail.decode("utf-8", errors="replace").strip()
        fail(f"preset failed Podway procedure validation: {message}")
    try:
        envelope = json.loads(completed.stdout.decode("utf-8"), object_pairs_hook=object_no_duplicates)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"podway validator did not return valid JSON: {error}")
    if not isinstance(envelope, dict) or envelope.get("schema") != "podway.output/v3" or envelope.get("command") != "procedure.validate":
        fail("podway validator returned an unexpected envelope")
    result = envelope.get("result")
    if not isinstance(result, dict):
        fail("podway validator result is missing")
    if result.get("valid") is not True or result.get("procedure_schema") != "podway.procedure/v2":
        fail("podway validator did not admit the Procedure v2 source")
    return result


def top_level_metadata(source: bytes) -> dict[str, str]:
    try:
        text = source.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"validated preset source is not UTF-8: {error}")
    stripped = text.lstrip()
    if stripped.startswith("{"):
        try:
            value = json.loads(text, object_pairs_hook=object_no_duplicates)
        except json.JSONDecodeError as error:
            fail(f"validated preset JSON cannot be read for metadata: {error}")
        if not isinstance(value, dict):
            fail("validated preset JSON root is not an object")
        return {field: value.get(field, "") for field in ("id", "version", "name", "purpose", "description")}

    metadata: dict[str, str] = {}
    for line in text.splitlines():
        if not line or line[0].isspace() or line.startswith("#") or ":" not in line:
            continue
        key, raw = line.split(":", 1)
        if key not in {"id", "version", "name", "purpose", "description"}:
            continue
        raw = raw.strip()
        if raw.startswith('"'):
            try:
                value = json.loads(raw)
            except json.JSONDecodeError as error:
                fail(f"validated preset metadata {key} is not a JSON-compatible scalar: {error}")
        else:
            value = raw
        if not isinstance(value, str):
            fail(f"validated preset metadata {key} is not text")
        metadata[key] = value
    return metadata


def admitted_metadata(
    result: dict[str, Any], source: bytes, expected: tuple[str, str, str] | None
) -> tuple[str, str, str, str]:
    metadata = top_level_metadata(source)
    identifier = validate_identifier(metadata.get("id", ""))
    version = validate_text(metadata.get("version", ""), "preset version", 64)
    purpose = validate_text(metadata.get("purpose", ""), "preset purpose", 4_000)
    if expected is None:
        return identifier, version, identifier, purpose
    expected_identifier, name, description = expected
    if identifier != expected_identifier:
        fail("generated preset identity did not round-trip through Podway preview")
    return (
        identifier,
        version,
        validate_text(name, "preset name", 120),
        validate_text(description, "preset description", 4_000),
    )


def publish_staged(staged: Path, destination: Path) -> None:
    if destination.exists() or destination.is_symlink():
        fail(f"preset destination already exists: {destination}")
    os.chmod(staged, 0o644)
    try:
        os.link(staged, destination, follow_symlinks=False)
    except FileExistsError:
        fail(f"preset destination already exists: {destination}")
    directory_descriptor = os.open(destination.parent, os.O_RDONLY)
    try:
        os.fsync(directory_descriptor)
    finally:
        os.close(directory_descriptor)


def scaffold(identifier: str, name: str, description: str) -> bytes:
    quoted_name = json.dumps(name, ensure_ascii=False)
    quoted_description = json.dumps(description, ensure_ascii=False)
    return (
        "schema: podway.procedure/v2\n"
        f"id: {identifier}\n"
        'version: "1"\n'
        f"name: {quoted_name}\n"
        f"purpose: {quoted_description}\n"
        f"description: {quoted_description}\n"
        "node_definitions:\n"
        "  prepare:\n"
        "    type: action\n"
        "    title: Prepare\n"
        "    intent: Define the work and its completion conditions.\n"
        "    instructions:\n"
        "      - Define the work and its completion conditions.\n"
        "    items:\n"
        "      - id: preparation-complete\n"
        "        type: confirm\n"
        "        prompt: Preparation is complete.\n"
        "        required: true\n"
        "  finish:\n"
        "    type: action\n"
        "    title: Finish\n"
        "    intent: Verify the result and record that it is ready.\n"
        "    instructions:\n"
        "      - Verify the result and record that it is ready.\n"
        "    items:\n"
        "      - id: result-ready\n"
        "        type: confirm\n"
        "        prompt: The result is verified and ready.\n"
        "        required: true\n"
        "graph:\n"
        "  entry: prepare\n"
        "  nodes:\n"
        "    - id: prepare\n"
        "      use: prepare\n"
        "      next: finish\n"
        "    - id: finish\n"
        "      use: finish\n"
        "      terminal: true\n"
        "manual_rework:\n"
        "  allowed_targets:\n"
        "    - prepare\n"
    ).encode("utf-8")


def install_content(content: bytes, directory: Path, podway: Path, expected: tuple[str, str, str] | None) -> dict[str, Any]:
    if not content or len(content) > MAX_PROCEDURE_BYTES:
        fail(f"preset content must contain 1 through {MAX_PROCEDURE_BYTES} bytes")
    staged = create_staged_file(directory, content)
    try:
        result = validator_result(podway, staged)
        identifier, version, name, description = admitted_metadata(result, content, expected)
        destination = directory / f"{identifier}.yaml"
        publish_staged(staged, destination)
        return {
            "description": description,
            "digest": result.get("digest"),
            "id": identifier,
            "name": name,
            "path": destination.as_posix(),
            "version": version,
            "warnings": result.get("warnings", []),
        }
    finally:
        try:
            staged.unlink()
        except FileNotFoundError:
            pass


def command_create(arguments: argparse.Namespace) -> dict[str, Any]:
    identifier = validate_identifier(arguments.identifier)
    name = validate_text(arguments.name, "preset name", 120)
    description = validate_text(arguments.description, "preset description", 4_000)
    directory = output_directory(arguments.output_dir)
    podway = regular_executable(arguments.podway)
    result = install_content(scaffold(identifier, name, description), directory, podway, (identifier, name, description))
    return {"mode": "create", "ok": True, "preset": result}


def command_import(arguments: argparse.Namespace) -> dict[str, Any]:
    source = arguments.source
    if source.suffix != ".yaml":
        fail("preset source must use the .yaml extension")
    content = read_bounded_regular_file(source)
    directory = output_directory(arguments.output_dir)
    podway = regular_executable(arguments.podway)
    result = install_content(content, directory, podway, None)
    return {"mode": "import", "ok": True, "preset": result, "source": source.as_posix()}


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(description=__doc__)
    subcommands = value.add_subparsers(dest="command", required=True)
    create = subcommands.add_parser("create", help="create a validated built-in preset scaffold")
    create.add_argument("--id", dest="identifier", required=True)
    create.add_argument("--name", required=True)
    create.add_argument("--description", required=True)
    create.add_argument("--output-dir", type=Path, default=DEFAULT_PRESET_DIRECTORY)
    create.add_argument("--podway", type=Path, required=True)
    create.set_defaults(action=command_create)
    import_command = subcommands.add_parser("import", help="validate and import a built-in preset YAML file")
    import_command.add_argument("--source", type=Path, required=True)
    import_command.add_argument("--output-dir", type=Path, default=DEFAULT_PRESET_DIRECTORY)
    import_command.add_argument("--podway", type=Path, required=True)
    import_command.set_defaults(action=command_import)
    return value


def main() -> int:
    try:
        arguments = parser().parse_args()
        print(json.dumps(arguments.action(arguments), sort_keys=True, separators=(",", ":")))
        return 0
    except (PresetToolError, OSError, TypeError, ValueError) as error:
        print(json.dumps({"error": {"message": str(error)}, "ok": False}, sort_keys=True, separators=(",", ":")))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
