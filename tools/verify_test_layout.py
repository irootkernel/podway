#!/usr/bin/env python3
"""Verify that Cargo integration-test targets have an explicit test layer."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
PREFIXES = ("arch_", "int_", "e2e_")
PRODUCT_BINARY_MARKERS = (
    "CARGO_BIN_EXE_podway",
    "CARGO_BIN_EXE_podwayd",
    "PODWAYD_TEST_BINARY",
)


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
    if layer != "e2e" and uses_product_binary:
        raise LayoutError(f"{layer} target uses a Podway product binary and must be e2e: {name}")
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


def verify() -> dict[str, int]:
    counts = {"arch": 0, "int": 0, "e2e": 0}
    targets = cargo_test_targets()
    if not targets:
        raise LayoutError("workspace contains no Cargo integration-test targets")
    for name, path in targets:
        try:
            source = path.read_text(encoding="utf-8")
        except OSError as error:
            raise LayoutError(f"cannot read integration-test target {path}: {error}") from error
        counts[classify(name, source)] += 1
    if any(count == 0 for count in counts.values()):
        raise LayoutError(f"every test layer must contain at least one target: {counts}")
    return counts


def self_test() -> None:
    assert classify("arch_contract", "fn main() {}") == "arch"
    assert classify("int_scenario", "fn main() {}") == "int"
    assert classify("e2e_cli", 'env!("CARGO_BIN_EXE_podway")') == "e2e"
    invalid = (
        ("scenario", "fn main() {}"),
        ("int_cli", 'env!("CARGO_BIN_EXE_podway")'),
        ("e2e_fake", "fn main() {}"),
    )
    for name, source in invalid:
        try:
            classify(name, source)
        except LayoutError:
            continue
        raise LayoutError(f"self-test sentinel unexpectedly passed: {name}")


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
            receipt(selected, True, targets=verify())
        else:
            self_test()
            receipt(selected, True, sentinels=3)
        return 0
    except (LayoutError, json.JSONDecodeError, KeyError, OSError, TypeError) as error:
        receipt(selected, False, error=str(error))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
