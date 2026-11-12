#!/usr/bin/env python3
"""Read the repository's exact build toolchain without selecting a global default."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import tomllib


ROOT = Path(__file__).resolve().parents[1]


class ToolchainError(RuntimeError):
    pass


def pinned_version(root: Path = ROOT) -> str:
    path = root / "rust-toolchain.toml"
    try:
        value = tomllib.loads(path.read_text(encoding="utf-8"))["toolchain"]["channel"]
    except (OSError, UnicodeError, tomllib.TOMLDecodeError, KeyError, TypeError) as error:
        raise ToolchainError(f"cannot read the pinned Rust version from {path}: {error}") from error
    if not isinstance(value, str) or re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value) is None:
        raise ToolchainError(f"{path} must specify an exact stable toolchain.channel version")
    return value


def require_version(output: str, root: Path = ROOT) -> None:
    expected = pinned_version(root)
    if not output.startswith(f"rustc {expected} "):
        raise ToolchainError(f"release archive requires rustc {expected}, observed {output}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("version",))
    parser.parse_args()
    try:
        print(pinned_version())
    except ToolchainError as error:
        raise SystemExit(str(error)) from error


if __name__ == "__main__":
    main()
