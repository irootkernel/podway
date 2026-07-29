#!/usr/bin/env python3
"""Verify that Cargo integration-test targets have an explicit test layer."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
PREFIXES = ("arch_", "int_", "e2e_")
PRODUCT_BINARY_MARKERS = (
    "CARGO_BIN_EXE_podway",
    "CARGO_BIN_EXE_podwayd",
    "PODWAYD_TEST_BINARY",
)
PATH_ATTRIBUTE_RE = re.compile(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]')


class LayoutError(Exception):
    """A Cargo test target violates the repository test-layer contract."""


def classify(name: str, source: str) -> str:
    layers = [prefix.removesuffix("_") for prefix in PREFIXES if name.startswith(prefix)]
    if len(layers) != 1:
        raise LayoutError(f"integration-test target must start with exactly one of {PREFIXES}: {name}")
    layer = layers[0]
    uses_product_binary = any(marker in source for marker in PRODUCT_BINARY_MARKERS)
    if layer == "e2e" and not uses_product_binary:
        raise LayoutError(f"e2e target does not use a Podway product binary: {name}")
    if layer == "arch" and uses_product_binary:
        raise LayoutError(f"architecture target must not execute a Podway product binary: {name}")
    return layer


def cargo_test_targets() -> list[tuple[str, Path]]:
    completed = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise LayoutError(completed.stderr.strip() or "cargo metadata failed")
    metadata: dict[str, Any] = json.loads(completed.stdout)
    targets: list[tuple[str, Path]] = []
    for package in metadata["packages"]:
        for target in package["targets"]:
            if "test" in target["kind"]:
                targets.append((target["name"], Path(target["src_path"])))
    return sorted(targets, key=lambda item: (item[0], item[1].as_posix()))


def target_sources(root: Path) -> dict[Path, str]:
    pending = [root.resolve()]
    sources: dict[Path, str] = {}
    while pending:
        path = pending.pop()
        if path in sources:
            continue
        if not path.is_relative_to(ROOT) or path.is_symlink() or not path.is_file():
            raise LayoutError(f"test target references an unsafe or missing source: {path}")
        try:
            source = path.read_text(encoding="utf-8")
        except OSError as error:
            raise LayoutError(f"cannot read test target source {path}: {error}") from error
        sources[path] = source
        for relative in PATH_ATTRIBUTE_RE.findall(source):
            pending.append((path.parent / relative).resolve())
    return sources


def source_layer(path: Path) -> str | None:
    if path.stem.endswith("_suite"):
        return None
    layers = [prefix.removesuffix("_") for prefix in PREFIXES if path.name.startswith(prefix)]
    if not layers:
        return None
    if len(layers) != 1:
        raise LayoutError(f"test source has an ambiguous layer prefix: {path}")
    return layers[0]


def expected_test_sources() -> dict[Path, str]:
    expected: dict[Path, str] = {}
    for tests in sorted(ROOT.glob("crates/*/tests")):
        for path in sorted(tests.glob("*.rs")):
            layer = source_layer(path)
            if layer is not None:
                expected[path.resolve()] = layer
    return expected


def validate_registrations(
    expected: dict[Path, str],
    registrations: dict[Path, list[tuple[str, str]]],
) -> None:
    missing = sorted(path for path in expected if path not in registrations)
    duplicates = sorted(path for path, owners in registrations.items() if len(owners) != 1)
    mismatches = sorted(
        path
        for path, owners in registrations.items()
        if path in expected and any(layer != expected[path] for layer, _target in owners)
    )
    unexpected = sorted(path for path in registrations if path not in expected)
    if missing or duplicates or mismatches or unexpected:
        details = {
            "missing": [path.as_posix() for path in missing],
            "duplicates": [path.as_posix() for path in duplicates],
            "mismatches": [path.as_posix() for path in mismatches],
            "unexpected": [path.as_posix() for path in unexpected],
        }
        raise LayoutError(f"test source registration is incomplete or ambiguous: {details}")


def verify() -> dict[str, dict[str, int]]:
    target_counts = {"arch": 0, "int": 0, "e2e": 0}
    source_counts = {"arch": 0, "int": 0, "e2e": 0}
    expected = expected_test_sources()
    registrations: dict[Path, list[tuple[str, str]]] = {}
    targets = cargo_test_targets()
    if not targets:
        raise LayoutError("workspace contains no Cargo integration-test targets")
    for name, path in targets:
        sources = target_sources(path)
        layer = classify(name, "\n".join(sources.values()))
        target_counts[layer] += 1
        for source_path in sources:
            if source_path in expected:
                registrations.setdefault(source_path, []).append((layer, name))
    validate_registrations(expected, registrations)
    for layer in expected.values():
        source_counts[layer] += 1
    if any(count == 0 for count in target_counts.values()):
        raise LayoutError(f"every test layer must contain at least one target: {target_counts}")
    return {"targets": target_counts, "sources": source_counts}


def self_test() -> None:
    assert classify("arch_contract", "fn main() {}") == "arch"
    assert classify("int_scenario", "fn main() {}") == "int"
    assert classify("int_cli", 'env!("CARGO_BIN_EXE_podway")') == "int"
    assert classify("e2e_cli", 'env!("CARGO_BIN_EXE_podway")') == "e2e"
    invalid = (
        ("scenario", "fn main() {}"),
        ("arch_cli", 'env!("CARGO_BIN_EXE_podway")'),
        ("e2e_fake", "fn main() {}"),
    )
    for name, source in invalid:
        try:
            classify(name, source)
        except LayoutError:
            continue
        raise LayoutError(f"self-test sentinel unexpectedly passed: {name}")
    expected = {Path("int_a.rs"): "int", Path("int_b.rs"): "int"}
    invalid_registrations = (
        {Path("int_a.rs"): [("int", "int_suite")]},
        {
            Path("int_a.rs"): [("int", "int_suite"), ("int", "int_other")],
            Path("int_b.rs"): [("int", "int_suite")],
        },
        {
            Path("int_a.rs"): [("e2e", "e2e_suite")],
            Path("int_b.rs"): [("int", "int_suite")],
        },
    )
    for registrations in invalid_registrations:
        try:
            validate_registrations(expected, registrations)
        except LayoutError:
            continue
        raise LayoutError("self-test source-registration sentinel unexpectedly passed")


def receipt(mode: str, ok: bool, **details: Any) -> None:
    print(json.dumps({"mode": mode, "ok": ok, **details}, separators=(",", ":"), sort_keys=True))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    selected = "check" if arguments.check else "self-test"
    try:
        if arguments.check:
            receipt(selected, True, **verify())
        else:
            self_test()
            receipt(selected, True, sentinels=6)
        return 0
    except (LayoutError, json.JSONDecodeError, KeyError, OSError, TypeError) as error:
        receipt(selected, False, error=str(error))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
