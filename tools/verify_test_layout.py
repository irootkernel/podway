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
DAEMON_LIFECYCLE_VERBS = ("install", "uninstall", "start", "stop", "restart")
DAEMON_ISOLATION_MARKERS = ("PODWAY_TEST_ACCOUNT_ROOT", "PODWAY_TEST_LAUNCHCTL")
RUST_STRING_LITERAL_RE = re.compile(r'"((?:[^"\\]|\\.)*)"')
ARGV_SEPARATOR_RE = re.compile(r"[\s,]*")


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


def spawns_daemon_lifecycle(source: str) -> bool:
    """Report whether a test source builds a daemon lifecycle argument vector.

    Shape: the literal ``"daemon"`` immediately followed by one of ``install``, ``uninstall``,
    ``start``, ``stop``, or ``restart`` as the next Rust string literal, with nothing but
    whitespace and commas between them. That covers every way rustfmt lays an argument vector out
    (``&["daemon", "install"]``, ``vec!["--json", "daemon", "uninstall", "--yes"]``, and the
    one-literal-per-line form) while rejecting prose and code that merely mentions both words,
    such as ``"daemon" => arguments.get(index + 1) == Some(&"install")``.

    Limits: it is textual, so it cannot see an argument vector assembled from variables, from a
    constant defined in another file, or from a formatted string, and it reads commented-out code
    as live. It is deliberately conservative in that direction: a miss leaves the pre-existing
    review burden in place, whereas a false positive only asks a suite to declare isolation it can
    always declare safely.
    """
    previous = None
    for literal in RUST_STRING_LITERAL_RE.finditer(source):
        if (
            previous is not None
            and previous.group(1) == "daemon"
            and literal.group(1) in DAEMON_LIFECYCLE_VERBS
            and ARGV_SEPARATOR_RE.fullmatch(source[previous.end() : literal.start()])
        ):
            return True
        previous = literal
    return False


def validate_daemon_isolation(sources: dict[Path, str]) -> int:
    """Require both debug overrides in every suite that spawns a daemon lifecycle route.

    A test that runs ``podway daemon install|uninstall|start|stop|restart`` operates on real
    account state unless it overrides both axes. ``HOME`` does not detach it: account resolution
    reads the operating-system account database (ADR-0012), so ``PODWAY_TEST_ACCOUNT_ROOT`` is what
    redirects the published launch agent and metadata index, and ``PODWAY_TEST_LAUNCHCTL`` is what
    keeps the fixed ``dev.podway.podwayd`` label out of the developer's live ``launchd`` domain.
    Either override alone still reaches live state, so both are required.

    Only sources that also name a product binary marker are scanned, so a suite that never spawns
    the product is left alone. The markers are declared once per file rather than per spawn: this
    is a static gate that makes an unisolated lifecycle suite impossible to add unnoticed, not a
    proof that every individual spawn in a scanned file carries them.
    """
    scanned = {
        path: source
        for path, source in sources.items()
        if any(marker in source for marker in PRODUCT_BINARY_MARKERS)
        and spawns_daemon_lifecycle(source)
    }
    unguarded = sorted(
        path.relative_to(ROOT).as_posix()
        for path, source in scanned.items()
        if not all(marker in source for marker in DAEMON_ISOLATION_MARKERS)
    )
    if unguarded:
        raise LayoutError(
            "test sources spawning a daemon lifecycle route must set both "
            f"{list(DAEMON_ISOLATION_MARKERS)}: {unguarded}"
        )
    return len(scanned)


def test_source_texts() -> dict[Path, str]:
    sources: dict[Path, str] = {}
    for tests in sorted(ROOT.glob("crates/*/tests")):
        for path in sorted(tests.glob("*.rs")):
            sources[path.resolve()] = path.read_text(encoding="utf-8")
    return sources


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
    lifecycle_sources = validate_daemon_isolation(test_source_texts())
    return {
        "targets": target_counts,
        "sources": source_counts,
        "daemon_lifecycle_sources": lifecycle_sources,
    }


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
    assert spawns_daemon_lifecycle('&["daemon", "install"]')
    assert spawns_daemon_lifecycle('vec![\n    "daemon",\n    "uninstall",\n    "--yes",\n]')
    assert not spawns_daemon_lifecycle('"daemon" => arguments.get(index + 1) == Some(&"install")')
    assert not spawns_daemon_lifecycle('&["daemon", "status"]')
    isolated = (
        'env!("CARGO_BIN_EXE_podway") &["daemon", "install"]'
        ' PODWAY_TEST_ACCOUNT_ROOT PODWAY_TEST_LAUNCHCTL'
    )
    assert validate_daemon_isolation({ROOT / "int_isolated.rs": isolated}) == 1
    unscanned = 'env!("CARGO_BIN_EXE_podway") &["daemon", "status"]'
    assert validate_daemon_isolation({ROOT / "int_unscanned.rs": unscanned}) == 0
    unguarded_sources = (
        'env!("CARGO_BIN_EXE_podway") vec!["--json", "daemon", "install"]',
        'env!("CARGO_BIN_EXE_podway") &["daemon", "uninstall"] PODWAY_TEST_ACCOUNT_ROOT',
        'env!("CARGO_BIN_EXE_podway") &["daemon", "restart"] PODWAY_TEST_LAUNCHCTL',
    )
    for source in unguarded_sources:
        try:
            validate_daemon_isolation({ROOT / "int_unguarded.rs": source})
        except LayoutError:
            continue
        raise LayoutError("self-test daemon-isolation sentinel unexpectedly passed")


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
            receipt(selected, True, sentinels=9)
        return 0
    except (LayoutError, json.JSONDecodeError, KeyError, OSError, TypeError) as error:
        receipt(selected, False, error=str(error))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
