#!/usr/bin/env python3
"""Generate and verify the deterministic Podway contract manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import tempfile
import tomllib
from typing import Any

import repository_assets


ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = Path("contracts/contract-manifest-v1.json")
MANIFEST_SCHEMA = "podway.contract-manifest/v1"
PRODUCT = "podway"
SUPPORTED_IPC_IDS = ["podway.ipc/v1"]
STATIC_ASSETS = {
    "contracts/command-routes.json": "catalog",
    "quality/v2-acceptance-matrix-v1.json": "catalog",
    "quality/v2-compatibility-matrix-v1.json": "catalog",
    "quality/v2-payload-matrix-v1.json": "catalog",
    "release/v2-release-gate-matrix-v1.json": "catalog",
    "spec/authoring-diagnostics.json": "catalog",
    "spec/command-catalog.yaml": "catalog",
    "spec/error-codes.json": "catalog",
    "spec/state-transition-matrix.csv": "transition_matrix",
    "spec/canonicalization-v1.json": "canonicalization_rules",
}


class ManifestError(Exception):
    """The checked contract manifest is missing, malformed, or stale."""


def fail(message: str) -> None:
    raise ManifestError(message)


def reject_noncanonical_numbers(value: Any) -> None:
    if isinstance(value, float):
        fail("cannot canonicalize JSON: numbers must be signed integers")
    if isinstance(value, dict):
        for item in value.values():
            reject_noncanonical_numbers(item)
    elif isinstance(value, (list, tuple)):
        for item in value:
            reject_noncanonical_numbers(item)


def canonical_bytes(value: Any) -> bytes:
    reject_noncanonical_numbers(value)
    try:
        return json.dumps(
            value,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        fail(f"cannot canonicalize JSON: {error}")


def sha256_bytes(value: bytes) -> str:
    return f"sha256:{hashlib.sha256(value).hexdigest()}"


def normalized_path(value: Any, label: str) -> Path:
    if not isinstance(value, str):
        fail(f"{label} must be a string")
    candidate = PurePosixPath(value)
    if (
        not value
        or "\\" in value
        or candidate.is_absolute()
        or any(part in {"", ".", ".."} for part in candidate.parts)
    ):
        fail(f"{label} is not a normalized relative path: {value!r}")
    return Path(*candidate.parts)


def checked_file(root: Path, relative: Path, label: str) -> Path:
    root = root.resolve()
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            fail(f"{label} contains a symlink: {relative.as_posix()}")
    try:
        current.resolve(strict=False).relative_to(root)
    except ValueError:
        fail(f"{label} escapes the repository root: {relative.as_posix()}")
    if not current.is_file():
        fail(f"{label} is missing or not a regular file: {relative.as_posix()}")
    return current


def matching_files(root: Path, directory: str, suffix: str) -> list[str]:
    relative = normalized_path(directory, "asset directory")
    base = root
    for part in relative.parts:
        base /= part
        if base.is_symlink() or not base.is_dir():
            fail(f"asset directory is missing or invalid: {directory}")

    paths: list[str] = []

    def visit(current: Path) -> None:
        with os.scandir(current) as entries:
            for entry in sorted(entries, key=lambda item: item.name):
                path = Path(entry.path)
                displayed = path.relative_to(root).as_posix()
                if entry.is_symlink():
                    fail(f"asset directory contains a symlink: {displayed}")
                if entry.is_dir(follow_symlinks=False):
                    visit(path)
                elif entry.is_file(follow_symlinks=False):
                    if entry.name.endswith(suffix):
                        paths.append(displayed)
                else:
                    fail(f"asset directory contains a non-regular entry: {displayed}")

    visit(base)
    if not paths:
        fail(f"asset directory contains no {suffix} files: {directory}")
    return sorted(paths)


def expected_asset_kinds(root: Path) -> dict[str, str]:
    assets = dict(STATIC_ASSETS)
    schema_paths = matching_files(root, "assets/schemas", ".schema.json")
    for path in schema_paths:
        relative = Path(path).relative_to("assets/schemas").as_posix()
        if re.fullmatch(r".+-v[1-9][0-9]*\.schema\.json", relative) is None:
            fail(f"schema asset is not version-named: {path}")
        assets[f"schemas/{relative}"] = "schema"
    for path in matching_files(root, "docs/examples/json", ".json"):
        assets[path] = "known_answer_fixture"
    for path in matching_files(root, "tests/fixtures/contract", ".json"):
        assets[path] = "known_answer_fixture"
    for path in matching_files(root, "tests/fixtures/v2", ""):
        assets[path] = "known_answer_fixture"
    return assets


def product_version(root: Path) -> str:
    path = checked_file(root, Path("Cargo.toml"), "workspace manifest")
    try:
        with path.open("rb") as handle:
            version = tomllib.load(handle)["workspace"]["package"]["version"]
    except (OSError, tomllib.TOMLDecodeError, KeyError, TypeError) as error:
        fail(f"cannot read workspace product version: {error}")
    if not isinstance(version, str) or not version:
        fail("workspace product version must be a non-empty string")
    return version


def verify_known_answers(root: Path) -> int:
    path = checked_file(
        root,
        Path("tests/fixtures/contract/canonicalization-v1.json"),
        "canonicalization fixture",
    )
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse canonicalization fixture: {error}")
    if not isinstance(value, dict) or set(value) != {"schema_version", "cases"}:
        fail("canonicalization fixture has unexpected or missing fields")
    if value["schema_version"] != "podway.canonicalization-known-answers/v1":
        fail("canonicalization fixture schema version is invalid")
    cases = value["cases"]
    if not isinstance(cases, list) or not cases:
        fail("canonicalization fixture must contain cases")
    identifiers: set[str] = set()
    for index, case in enumerate(cases):
        if not isinstance(case, dict) or set(case) != {"id", "input", "canonical", "sha256"}:
            fail(f"canonicalization fixture case {index} has unexpected or missing fields")
        identifier = case["id"]
        if not isinstance(identifier, str) or not identifier or identifier in identifiers:
            fail("canonicalization fixture IDs must be unique non-empty strings")
        identifiers.add(identifier)
        observed = canonical_bytes(case["input"])
        if case["canonical"] != observed.decode("utf-8"):
            fail(f"canonicalization fixture bytes drift: {identifier}")
        if case["sha256"] != sha256_bytes(observed):
            fail(f"canonicalization fixture digest drift: {identifier}")
    return len(cases)


def build_manifest(root: Path = ROOT) -> dict[str, Any]:
    assets = []
    for relative_name, kind in sorted(expected_asset_kinds(root).items()):
        relative = normalized_path(relative_name, "contract asset")
        path = checked_file(root, repository_assets.logical_source(relative), "contract asset")
        assets.append({"kind": kind, "path": relative_name, "sha256": sha256_bytes(path.read_bytes())})
    manifest: dict[str, Any] = {
        "schema_version": MANIFEST_SCHEMA,
        "product": PRODUCT,
        "product_version": product_version(root),
        "supported_ipc_ids": SUPPORTED_IPC_IDS,
        "assets": assets,
    }
    manifest["digest"] = sha256_bytes(canonical_bytes(manifest))
    return manifest


def validate_shape(value: Any) -> dict[str, Any]:
    expected_keys = {
        "schema_version", "product", "product_version", "supported_ipc_ids", "assets", "digest"
    }
    if not isinstance(value, dict) or set(value) != expected_keys:
        fail("contract manifest has unexpected or missing top-level fields")
    if value["schema_version"] != MANIFEST_SCHEMA or value["product"] != PRODUCT:
        fail("contract manifest identity is invalid")
    if value["supported_ipc_ids"] != SUPPORTED_IPC_IDS:
        fail("contract manifest supported IPC IDs drift")
    assets = value["assets"]
    if not isinstance(assets, list) or not assets:
        fail("contract manifest must contain assets")
    paths = []
    for index, asset in enumerate(assets):
        if not isinstance(asset, dict) or set(asset) != {"kind", "path", "sha256"}:
            fail(f"contract manifest asset {index} has unexpected or missing fields")
        if not isinstance(asset["kind"], str):
            fail(f"contract manifest asset {index} kind must be a string")
        paths.append(normalized_path(asset["path"], f"contract manifest asset {index}").as_posix())
        digest = asset["sha256"]
        if not isinstance(digest, str) or len(digest) != 71 or not digest.startswith("sha256:"):
            fail(f"contract manifest asset {index} digest is invalid")
        try:
            int(digest[7:], 16)
        except ValueError:
            fail(f"contract manifest asset {index} digest is invalid")
    if paths != sorted(set(paths)):
        fail("contract manifest asset paths must be sorted and unique")
    unsigned = {key: item for key, item in value.items() if key != "digest"}
    if value["digest"] != sha256_bytes(canonical_bytes(unsigned)):
        fail("contract manifest self digest is invalid")
    return value


def check(root: Path = ROOT) -> int:
    verify_known_answers(root)
    path = checked_file(root, MANIFEST_PATH, "contract manifest")
    try:
        observed = validate_shape(json.loads(path.read_text(encoding="utf-8")))
    except json.JSONDecodeError as error:
        fail(f"cannot parse contract manifest: {error}")
    expected = build_manifest(root)
    if observed != expected:
        fail("contract manifest is stale; run tools/contract_manifest.py --write")
    return len(expected["assets"])


def write(root: Path = ROOT) -> tuple[int, bool]:
    verify_known_answers(root)
    manifest = build_manifest(root)
    destination = root / MANIFEST_PATH
    content = (json.dumps(manifest, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8")
    if destination.is_file() and destination.read_bytes() == content:
        return len(manifest["assets"]), False
    destination.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary_name, destination)
    except BaseException:
        Path(temporary_name).unlink(missing_ok=True)
        raise
    return len(manifest["assets"]), True


def self_test(root: Path = ROOT) -> list[str]:
    completed = []
    for identifier, value in (
        ("finite_float", 1.25),
        ("nested_float", {"items": [1, 2.5]}),
        ("nan", float("nan")),
        ("infinity", float("inf")),
    ):
        try:
            canonical_bytes(value)
        except ManifestError:
            completed.append(f"{identifier}_rejected")
        else:
            fail(f"{identifier} canonicalization sentinel did not fail")

    with tempfile.TemporaryDirectory(prefix="podway-contract-manifest-") as temporary_name:
        fixture = Path(temporary_name)
        shutil.copy2(root / "Cargo.toml", fixture / "Cargo.toml")
        for relative_name in expected_asset_kinds(root):
            source_relative = repository_assets.logical_source(relative_name)
            source = root / source_relative
            destination = fixture / source_relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
        write(fixture)
        check(fixture)
        completed.append("deterministic_generation")

        nested_schema = fixture / "assets/schemas/nested/example-v2.schema.json"
        nested_schema.parent.mkdir(parents=True, exist_ok=True)
        nested_schema.write_text(
            '{"$schema":"https://json-schema.org/draft/2020-12/schema",'
            '"$id":"urn:podway:schema:example:v2"}\n',
            encoding="utf-8",
        )
        write(fixture)
        check(fixture)
        if "schemas/nested/example-v2.schema.json" not in expected_asset_kinds(fixture):
            fail("nested versioned schema discovery sentinel did not pass")
        completed.append("nested_versioned_schema")

        unversioned_schema = fixture / "assets/schemas/unversioned.schema.json"
        unversioned_schema.write_text("{}\n", encoding="utf-8")
        try:
            expected_asset_kinds(fixture)
        except ManifestError:
            completed.append("unversioned_schema_rejected")
        else:
            fail("unversioned schema sentinel did not fail")
        unversioned_schema.unlink()

        nested_examples = fixture / "docs/examples/json/nested/example.json"
        nested_contract = fixture / "tests/fixtures/contract/nested/fixture.json"
        for path in (nested_examples, nested_contract):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("{}\n", encoding="utf-8")
        nested_assets = expected_asset_kinds(fixture)
        if not all(path.relative_to(fixture).as_posix() in nested_assets for path in (nested_examples, nested_contract)):
            fail("nested known-answer discovery sentinel did not pass")
        completed.append("nested_known_answers")

        nested_v2 = fixture / "tests/fixtures/v2/nested/fixture.bin"
        nested_v2.parent.mkdir(parents=True, exist_ok=True)
        nested_v2.write_bytes(b"\x00v2-fixture\xff")
        if nested_v2.relative_to(fixture).as_posix() not in expected_asset_kinds(fixture):
            fail("non-JSON v2 fixture discovery sentinel did not pass")
        completed.append("v2_all_regular_files")

        v2_file_link = fixture / "tests/fixtures/v2/nested/fixture-link.bin"
        v2_file_link.symlink_to(nested_v2)
        try:
            expected_asset_kinds(fixture)
        except ManifestError:
            completed.append("v2_file_symlink_rejected")
        else:
            fail("v2 fixture symlink sentinel did not fail")
        v2_file_link.unlink()

        file_link = fixture / "docs/examples/json/fixture-link"
        file_link.symlink_to(nested_examples)
        try:
            expected_asset_kinds(fixture)
        except ManifestError:
            completed.append("file_symlink_rejected")
        else:
            fail("known-answer file symlink sentinel did not fail")
        file_link.unlink()

        directory_link = fixture / "tests/fixtures/contract/directory-link"
        directory_link.symlink_to(nested_contract.parent, target_is_directory=True)
        try:
            expected_asset_kinds(fixture)
        except ManifestError:
            completed.append("directory_symlink_rejected")
        else:
            fail("known-answer directory symlink sentinel did not fail")
        directory_link.unlink()

        write(fixture)
        check(fixture)

        first_asset = fixture / repository_assets.logical_source(
            sorted(expected_asset_kinds(fixture))[0]
        )
        first_asset.write_bytes(first_asset.read_bytes() + b"\n")
        try:
            check(fixture)
        except ManifestError:
            completed.append("asset_tamper")
        else:
            fail("asset tamper sentinel did not fail")

        write(fixture)
        manifest_path = fixture / MANIFEST_PATH
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["digest"] = "sha256:" + "0" * 64
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        try:
            check(fixture)
        except ManifestError:
            completed.append("self_digest_tamper")
        else:
            fail("manifest self-digest sentinel did not fail")
    return completed


def receipt(mode: str, ok: bool, **details: Any) -> None:
    print(json.dumps({"mode": mode, "ok": ok, **details}, sort_keys=True, separators=(",", ":")))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true", help="generate the checked contract manifest")
    mode.add_argument("--check", action="store_true", help="verify the checked contract manifest")
    mode.add_argument("--self-test", action="store_true", help="run isolated known-fail controls")
    arguments = parser.parse_args()
    selected = "write" if arguments.write else "check" if arguments.check else "self-test"
    try:
        if arguments.write:
            assets, changed = write()
            receipt(selected, True, assets=assets, changed=changed)
        elif arguments.check:
            receipt(selected, True, assets=check())
        else:
            receipt(selected, True, sentinels=self_test())
    except (ManifestError, OSError, tomllib.TOMLDecodeError) as error:
        receipt(selected, False, error={"code": "contract_manifest_invalid", "message": str(error)})
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
