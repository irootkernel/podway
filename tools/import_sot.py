#!/usr/bin/env python3
"""Fail-closed, one-way import of immutable SOT assets into the repository."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
from pathlib import PurePosixPath
import shutil
import stat
import tempfile
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
CONTRACT_PATH = Path("contracts/canonical-import.json")
SOURCE_DIRECTORIES = ("schemas", "spec", "presets")
CONTRACT_VERSION = "podway.canonical-import/v1"
GENERATOR_VERSION = "phase-0a-v1"
MAPPING_KEYS = {
    "source",
    "destination",
    "source_sha256",
    "copy_mode",
    "owner",
    "generator_version",
}


class ContractError(Exception):
    """A contract or controlled-file invariant was violated."""


def fail(message: str) -> None:
    raise ContractError(message)


def digest_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def relative_path(value: Any, label: str) -> Path:
    if not isinstance(value, str) or not value:
        fail(f"{label} must be a non-empty string")
    if "\\" in value:
        fail(f"{label} must use POSIX separators")
    raw_parts = value.split("/")
    if any(part in {"", ".", ".."} for part in raw_parts):
        fail(f"{label} is not a normalized relative path: {value!r}")
    candidate = PurePosixPath(value)
    if candidate.is_absolute():
        fail(f"{label} must be relative: {value!r}")
    return Path(*candidate.parts)


def is_under(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def checked_path(root: Path, relative: Path, label: str) -> Path:
    root = root.resolve()
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            fail(f"{label} contains a symlink: {relative.as_posix()}")
    resolved = current.resolve(strict=False)
    if not is_under(resolved, root):
        fail(f"{label} escapes repository root: {relative.as_posix()}")
    return current


def regular_files(root: Path, relative_directory: str, required: bool) -> set[str]:
    root = root.resolve()
    directory = checked_path(root, relative_path(relative_directory, "directory"), "directory")
    if not directory.exists():
        if required:
            fail(f"required directory is missing: {relative_directory}")
        return set()
    if not directory.is_dir():
        fail(f"directory is not a directory: {relative_directory}")

    files: set[str] = set()
    for current_name, child_directories, child_files in os.walk(directory, followlinks=False):
        current = Path(current_name)
        for child_name in sorted(child_directories):
            child = current / child_name
            if child.is_symlink():
                fail(f"directory tree contains a symlink: {child.relative_to(root).as_posix()}")
        for child_name in sorted(child_files):
            child = current / child_name
            if child.is_symlink():
                fail(f"directory tree contains a symlink: {child.relative_to(root).as_posix()}")
            if not child.is_file():
                fail(f"directory tree contains a non-regular file: {child.relative_to(root).as_posix()}")
            files.add(child.relative_to(root).as_posix())
    return files


def load_contract(root: Path = ROOT) -> dict[str, Any]:
    path = checked_path(root, CONTRACT_PATH, "canonical import contract")
    if not path.is_file():
        fail(f"canonical import contract is missing: {CONTRACT_PATH.as_posix()}")
    try:
        with path.open(encoding="utf-8") as handle:
            contract = json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse canonical import contract: {error}")
    if not isinstance(contract, dict):
        fail("canonical import contract must be an object")
    return contract


def validate_contract(root: Path = ROOT) -> tuple[dict[str, Any], list[dict[str, str]]]:
    contract = load_contract(root)
    expected_contract_keys = {
        "contract_version",
        "generator_version",
        "owner",
        "copy_mode",
        "imports",
    }
    if set(contract) != expected_contract_keys:
        fail("canonical import contract has unexpected or missing top-level fields")
    if contract["contract_version"] != CONTRACT_VERSION:
        fail("canonical import contract version is not v1")
    if contract["generator_version"] != GENERATOR_VERSION:
        fail("canonical import generator version is not phase-0a-v1")
    if contract["owner"] != "sot" or contract["copy_mode"] != "exact":
        fail("canonical import contract must be owned by sot in exact copy mode")
    raw_mappings = contract["imports"]
    if not isinstance(raw_mappings, list) or not raw_mappings:
        fail("canonical import contract imports must be a non-empty list")

    mappings: list[dict[str, str]] = []
    sources: set[str] = set()
    destinations: set[str] = set()
    for index, raw_mapping in enumerate(raw_mappings):
        if not isinstance(raw_mapping, dict) or set(raw_mapping) != MAPPING_KEYS:
            fail(f"import mapping {index} has unexpected or missing fields")
        mapping = {key: raw_mapping[key] for key in MAPPING_KEYS}
        if not all(isinstance(value, str) for value in mapping.values()):
            fail(f"import mapping {index} values must all be strings")
        source = relative_path(mapping["source"], f"mapping {index} source")
        destination = relative_path(mapping["destination"], f"mapping {index} destination")
        source_parts = source.parts
        if len(source_parts) < 3 or source_parts[0] != "sot" or source_parts[1] not in SOURCE_DIRECTORIES:
            fail(f"mapping {index} source is outside imported SOT directories")
        if destination.parts != source_parts[1:]:
            fail(f"mapping {index} destination must exactly mirror its SOT-relative path")
        if mapping["copy_mode"] != "exact" or mapping["owner"] != "sot":
            fail(f"mapping {index} must be an exact SOT-owned copy")
        if mapping["generator_version"] != GENERATOR_VERSION:
            fail(f"mapping {index} has an unsupported generator version")
        if not len(mapping["source_sha256"]) == 64 or any(
            character not in "0123456789abcdef" for character in mapping["source_sha256"]
        ):
            fail(f"mapping {index} source_sha256 is not a lowercase SHA-256 digest")
        source_name = source.as_posix()
        destination_name = destination.as_posix()
        if source_name in sources or destination_name in destinations:
            fail(f"mapping {index} duplicates a source or destination")
        sources.add(source_name)
        destinations.add(destination_name)
        mappings.append(mapping)

    shipped = set()
    for directory in SOURCE_DIRECTORIES:
        shipped.update(regular_files(root, f"sot/{directory}", required=True))
    if sources != shipped:
        missing = sorted(shipped - sources)
        extra = sorted(sources - shipped)
        fail(f"canonical mappings do not exactly cover shipped SOT files; missing={missing}, extra={extra}")
    return contract, mappings


def validate_sources(root: Path, mappings: list[dict[str, str]]) -> None:
    for mapping in mappings:
        source_relative = relative_path(mapping["source"], "mapping source")
        source = checked_path(root, source_relative, "SOT source")
        if not source.is_file():
            fail(f"SOT source is not a regular file: {mapping['source']}")
        actual_digest = digest_file(source)
        if actual_digest != mapping["source_sha256"]:
            fail(f"SOT source digest drift: {mapping['source']}")


def validate_destination_tree(root: Path, mappings: list[dict[str, str]], require_content: bool) -> None:
    expected = {mapping["destination"] for mapping in mappings}
    actual: set[str] = set()
    for directory in SOURCE_DIRECTORIES:
        actual.update(regular_files(root, directory, required=False))
    extras = sorted(actual - expected)
    if extras:
        fail(f"unmapped destination extras: {extras}")
    if not require_content:
        return
    missing = sorted(expected - actual)
    if missing:
        fail(f"imported destination files are missing: {missing}")
    for mapping in mappings:
        destination = checked_path(root, relative_path(mapping["destination"], "mapping destination"), "destination")
        if not destination.is_file():
            fail(f"destination is not a regular file: {mapping['destination']}")
        if digest_file(destination) != mapping["source_sha256"]:
            fail(f"destination digest drift: {mapping['destination']}")


def atomic_copy(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as target, source.open("rb") as origin:
            shutil.copyfileobj(origin, target, length=1024 * 1024)
            target.flush()
            os.fsync(target.fileno())
        os.chmod(temporary, stat.S_IMODE(source.stat().st_mode))
        os.replace(temporary, destination)
        directory_descriptor = os.open(destination.parent, os.O_RDONLY)
        try:
            os.fsync(directory_descriptor)
        finally:
            os.close(directory_descriptor)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def check(root: Path = ROOT) -> int:
    _, mappings = validate_contract(root)
    validate_sources(root, mappings)
    validate_destination_tree(root, mappings, require_content=True)
    return len(mappings)


def write(root: Path = ROOT) -> tuple[int, int]:
    _, mappings = validate_contract(root)
    validate_sources(root, mappings)
    validate_destination_tree(root, mappings, require_content=False)
    changed = 0
    for mapping in mappings:
        source = checked_path(root, relative_path(mapping["source"], "mapping source"), "SOT source")
        destination_relative = relative_path(mapping["destination"], "mapping destination")
        destination = checked_path(root, destination_relative, "destination")
        if destination.exists() and not destination.is_file():
            fail(f"destination is not a regular file: {mapping['destination']}")
        if destination.exists() and digest_file(destination) == mapping["source_sha256"]:
            continue
        atomic_copy(source, destination)
        changed += 1
    validate_destination_tree(root, mappings, require_content=True)
    return len(mappings), changed


def receipt(mode: str, ok: bool, **details: Any) -> None:
    print(json.dumps({"mode": mode, "ok": ok, **details}, sort_keys=True, separators=(",", ":")))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="verify imported assets without writing")
    mode.add_argument("--write", action="store_true", help="atomically import SOT assets into destination directories")
    arguments = parser.parse_args()
    selected_mode = "check" if arguments.check else "write"
    try:
        if arguments.check:
            receipt(selected_mode, True, imports_checked=check())
        else:
            checked, changed = write()
            receipt(selected_mode, True, imports_checked=checked, files_changed=changed)
    except (ContractError, OSError) as error:
        receipt(selected_mode, False, error={"code": "canonical_import_failed", "message": str(error)})
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
