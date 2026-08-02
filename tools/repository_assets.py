#!/usr/bin/env python3
"""Resolve and validate Podway's canonical repository assets."""

from __future__ import annotations

import os
from pathlib import Path, PurePosixPath
from typing import Any


ASSET_DIRECTORIES = {
    "presets": "assets/presets",
    "schemas": "assets/schemas",
    "spec": "assets/specifications",
}
RETIRED_DIRECTORIES = (
    "docs/presets",
    "docs/schemas",
    "docs/spec",
    "presets",
    "schemas",
    "spec",
)


class AssetError(RuntimeError):
    """A canonical asset path or tree invariant was violated."""


def fail(message: str) -> None:
    raise AssetError(message)


def normalized_path(value: Any, label: str) -> Path:
    if not isinstance(value, str) or not value:
        fail(f"{label} must be a non-empty string")
    candidate = PurePosixPath(value)
    if (
        "\\" in value
        or candidate.is_absolute()
        or any(part in {"", ".", ".."} for part in candidate.parts)
    ):
        fail(f"{label} is not a normalized relative path: {value!r}")
    return Path(*candidate.parts)


def checked_path(root: Path, relative: Path, label: str) -> Path:
    root = root.resolve()
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            fail(f"{label} contains a symlink: {relative.as_posix()}")
    try:
        current.resolve(strict=False).relative_to(root)
    except ValueError:
        fail(f"{label} escapes repository root: {relative.as_posix()}")
    return current


def logical_source(relative: Path | str) -> Path:
    logical = normalized_path(relative.as_posix() if isinstance(relative, Path) else relative, "asset path")
    prefix = logical.parts[0]
    source_directory = ASSET_DIRECTORIES.get(prefix)
    if source_directory is None:
        return logical
    return Path(source_directory, *logical.parts[1:])


def regular_files(root: Path, relative_directory: str, required: bool = True) -> set[str]:
    directory = checked_path(root, normalized_path(relative_directory, "asset directory"), "asset directory")
    if not directory.exists():
        if required:
            fail(f"required asset directory is missing: {relative_directory}")
        return set()
    if not directory.is_dir():
        fail(f"asset directory is not a directory: {relative_directory}")

    files: set[str] = set()
    for current_name, child_directories, child_files in os.walk(directory, followlinks=False):
        current = Path(current_name)
        for child_name in child_directories:
            child = current / child_name
            if child.is_symlink():
                fail(f"asset tree contains a symlink: {child.relative_to(root).as_posix()}")
        for child_name in child_files:
            child = current / child_name
            if child.is_symlink() or not child.is_file():
                fail(f"asset tree contains a non-regular file: {child.relative_to(root).as_posix()}")
            files.add(child.relative_to(root).as_posix())
    return files


def validate_layout(root: Path) -> int:
    files = set()
    for source_directory in ASSET_DIRECTORIES.values():
        files.update(regular_files(root, source_directory))
    for retired in RETIRED_DIRECTORIES:
        if (root / retired).exists():
            fail(f"retired duplicate asset directory still exists: {retired}")
    return len(files)
