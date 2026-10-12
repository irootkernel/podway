#!/usr/bin/env python3
"""Build and inspect the deterministic Podway Apple Silicon release archive."""

from __future__ import annotations

import argparse
from enum import Enum
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
from typing import Any, Callable

import repository_assets
import release_evidence
import verify_patch_release


ROOT = Path(__file__).resolve().parents[1]
PRODUCT_VERSION = "0.2.13"
TARGET = "aarch64-apple-darwin"
ARCHIVE_ROOT = f"podway-{PRODUCT_VERSION}-{TARGET}"
MACHO_64_LITTLE_ENDIAN = b"\xcf\xfa\xed\xfe"
CPU_TYPE_ARM64 = 0x0100000C
COMPLETION_NAMES = {
    "bash": "podway.bash",
    "fish": "podway.fish",
    "zsh": "podway.zsh",
}
ISOLATION_PROBE_ENV = "PODWAY_TEST_ISOLATION_PROBE"
ISOLATION_PROBE_TOKEN = "podway-test-isolation-v1"
ISOLATION_PROBE_TIMEOUT_SECONDS = 5


class ReleaseError(RuntimeError):
    pass


class TestIsolationCapability(Enum):
    ENABLED = "enabled"
    DISABLED = "disabled"
    INDETERMINATE = "indeterminate"


def fail(message: str) -> None:
    raise ReleaseError(message)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run(arguments: list[str], *, label: str) -> bytes:
    try:
        completed = release_evidence.run_bounded(arguments, cwd=ROOT, label=label)
    except release_evidence.EvidenceError as error:
        fail(str(error))
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        fail(f"{label} failed with exit {completed.returncode}: {detail}")
    return completed.stdout


def require_regular_file(path: Path, label: str) -> Path:
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file: {path}")
    return path.resolve()


def snapshot_file(source: Path, destination: Path, label: str, *, executable: bool = False) -> Path:
    flags = os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(source, flags)
    except OSError as error:
        fail(f"cannot open {label} as a regular non-symlink file: {source}: {error}")
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            fail(f"{label} must be a regular non-symlink file: {source}")
        if metadata.st_size > 512 * 1024 * 1024 or (executable and metadata.st_size > 128 * 1024 * 1024):
            fail(f"{label} exceeds its snapshot byte budget: {source}")
        if executable and metadata.st_mode & stat.S_IXUSR == 0:
            fail(f"{label} is not executable: {source}")
        destination.parent.mkdir(parents=True, exist_ok=True)
        with os.fdopen(descriptor, "rb", closefd=False) as opened_source:
            with destination.open("xb") as opened_destination:
                shutil.copyfileobj(opened_source, opened_destination)
                opened_destination.flush()
                os.fsync(opened_destination.fileno())
        after = os.fstat(descriptor)
        if (metadata.st_size, metadata.st_mtime_ns, metadata.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            fail(f"{label} changed while being snapshotted: {source}")
        os.chmod(destination, 0o500 if executable else 0o400)
    finally:
        os.close(descriptor)
    return destination.resolve()


def snapshot_executable(source: Path, destination: Path, label: str) -> Path:
    return snapshot_file(source, destination, label, executable=True)


def require_native_binary(path: Path, expected_name: str) -> Path:
    path = require_regular_file(path, expected_name)
    mode = path.stat().st_mode
    if mode & stat.S_IXUSR == 0:
        fail(f"{expected_name} is not executable: {path}")
    with path.open("rb") as binary:
        header = binary.read(8)
    if len(header) != 8 or header[:4] != MACHO_64_LITTLE_ENDIAN:
        fail(f"{expected_name} is not a thin 64-bit little-endian Mach-O binary")
    if int.from_bytes(header[4:8], "little") != CPU_TYPE_ARM64:
        fail(f"{expected_name} is not an arm64 Mach-O binary")
    version_arguments = ["version"]
    version = run([str(path), *version_arguments], label=f"{expected_name} version probe")
    version_prefix = "v" if expected_name == "podway" else ""
    expected_version = f"{expected_name} {version_prefix}{PRODUCT_VERSION}\n".encode()
    if version != expected_version:
        fail(f"{expected_name} version output does not equal {expected_version!r}")
    return path


def verify_release_contract(
    contract_root: Path,
    podway: Path,
    source_commit: str,
) -> dict[str, Any]:
    payload = run(
        [
            "cargo",
            "run",
            "--quiet",
            "--offline",
            "--locked",
            "-p",
            "podway-protocol",
            "--features",
            "release-contract-verifier",
            "--bin",
            "podway-contract-verifier",
            "--",
            "--contract-root",
            str(contract_root),
            "--podway",
            str(podway),
            "--expected-target",
            TARGET,
            "--expected-source-commit",
            source_commit,
        ],
        label="authoritative Rust release contract verifier",
    )
    try:
        receipt = json.loads(payload)
    except json.JSONDecodeError as error:
        fail(f"Rust release contract verifier returned invalid JSON: {error}")
    if not isinstance(receipt, dict) or receipt.get("schema") != "podway.contract-verification/v2":
        fail("Rust release contract verifier returned an invalid receipt")
    return receipt


def run_test_isolation_probe(path: Path, token: str) -> tuple[int, bytes, bytes] | None:
    environment = {
        "PATH": "/usr/bin:/bin",
        ISOLATION_PROBE_ENV: token,
    }
    try:
        completed = subprocess.run(
            [str(path), "--podway-test-isolation-probe"],
            cwd=ROOT,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=ISOLATION_PROBE_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return completed.returncode, completed.stdout, completed.stderr


def test_isolation_capability(path: Path) -> TestIsolationCapability:
    enabled_probe = run_test_isolation_probe(path, ISOLATION_PROBE_TOKEN)
    disabled_probe = run_test_isolation_probe(path, f"{ISOLATION_PROBE_TOKEN}-disabled")
    if enabled_probe == (0, f"{ISOLATION_PROBE_TOKEN}\n".encode(), b""):
        return TestIsolationCapability.ENABLED
    if (
        enabled_probe is not None
        and disabled_probe is not None
        and enabled_probe == disabled_probe
        and enabled_probe[0] != 0
    ):
        return TestIsolationCapability.DISABLED
    return TestIsolationCapability.INDETERMINATE


def validate_package_mode(artifact_class: str, allow_dirty: bool) -> None:
    if artifact_class not in {"distribution", "development-qualification"}:
        fail("unsupported artifact class")
    if artifact_class == "distribution" and allow_dirty:
        fail("distribution archives cannot use --allow-dirty")


def verify_artifact_class(
    binaries: dict[str, Path],
    artifact_class: str,
    capability_probe: Callable[[Path], TestIsolationCapability] = test_isolation_capability,
) -> None:
    capabilities = {role: capability_probe(path) for role, path in binaries.items()}
    indeterminate = {
        role: capability.value
        for role, capability in capabilities.items()
        if capability is TestIsolationCapability.INDETERMINATE
    }
    if indeterminate:
        fail(f"binary isolation probe was indeterminate: {indeterminate}")
    expected = TestIsolationCapability.DISABLED
    mismatches = {
        name: {"expected_test_isolation": expected.value, "actual": actual.value}
        for name, actual in capabilities.items()
        if actual != expected
    }
    if mismatches:
        fail(f"binary isolation does not match --artifact-class {artifact_class}: {mismatches}")


def self_test() -> dict[str, Any]:
    global ROOT
    cli = Path("/fixture/cli/product")
    sentinels = 0
    for artifact_class in ("distribution", "development-qualification"):
        verify_artifact_class({"podway": cli}, artifact_class, lambda _: TestIsolationCapability.DISABLED)
        sentinels += 1
        for capability in (TestIsolationCapability.ENABLED, TestIsolationCapability.INDETERMINATE):
            try:
                verify_artifact_class({"podway": cli}, artifact_class, lambda _, capability=capability: capability)
            except ReleaseError:
                pass
            else:
                fail("artifact-class sentinel accepted enabled or ambiguous hooks")
            sentinels += 1
    try:
        validate_package_mode("distribution", True)
    except ReleaseError:
        pass
    else:
        fail("package-mode sentinel accepted a dirty distribution")
    sentinels += 1
    validate_package_mode("development-qualification", True)
    with tempfile.TemporaryDirectory(prefix="podway-release-self-test-", dir=scratch_parent()) as name:
        root = Path(name)
        source = root / "source"
        source.write_bytes(b"verified bytes")
        source.chmod(0o700)
        snapshot = snapshot_executable(source, root / "snapshot", "sentinel")
        source.write_bytes(b"replacement bytes")
        if snapshot.read_bytes() != b"verified bytes":
            fail("executable snapshot changed after source replacement")
        symlink = root / "symlink"
        symlink.symlink_to(source)
        try:
            snapshot_executable(symlink, root / "rejected", "sentinel")
        except ReleaseError:
            pass
        else:
            fail("executable snapshot followed a symlink")
        sentinels += 2
        staging = root / "staging"
        manifest_target = staging / "share/podway/contracts/contract-manifest-v2.json"
        manifest_target.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / "contracts/contract-manifest-v2.json", manifest_target)
        for relative in packaged_preset_manifest_paths():
            target = staging / "share/podway" / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / repository_assets.logical_source(relative), target)
        archive_path = root / "presets.tar.gz"
        write_archive(staging, archive_path)
        with tarfile.open(archive_path, mode="r:gz") as archive:
            verify_packaged_preset_identity(archive)
        tampered = staging / "share/podway/presets/bug-fix-v2.yaml"
        tampered.write_bytes(tampered.read_bytes() + b"# tampered\n")
        write_archive(staging, archive_path)
        try:
            with tarfile.open(archive_path, mode="r:gz") as archive:
                verify_packaged_preset_identity(archive)
        except ReleaseError as error:
            if "preset bytes differ" not in str(error):
                fail("packaged preset tamper sentinel returned the wrong error")
        else:
            fail("packaged preset tamper sentinel was accepted")
        sentinels += 2
    version = f"v{PRODUCT_VERSION}"
    open_changelog = f"# Changelog\n\n## {version} - Unreleased\n"
    published_changelog = f"# Changelog\n\n## {version} - 2026-09-02\n"
    validate_changelog_release_status(open_changelog, "development-qualification")
    validate_changelog_release_status(published_changelog, "distribution")
    try:
        validate_changelog_release_status(open_changelog, "distribution")
    except ReleaseError:
        pass
    else:
        fail("open distribution changelog sentinel was accepted")
    sentinels += 3
    major, minor, patch = (int(part) for part in PRODUCT_VERSION.split("."))
    next_version = f"v{major}.{minor}.{patch + 1}"
    planned_changelog = f"# Changelog\n\n## {next_version} - Unreleased\n\n## {version} - 2026-09-02\n"
    validate_changelog_release_status(planned_changelog, "development-qualification")
    sentinels += 1
    for label, document, artifact_class in (
        ("planned distribution", planned_changelog, "distribution"),
        ("wrong distribution version", planned_changelog.replace("Unreleased", "2026-09-03"), "distribution"),
        ("published future development", planned_changelog.replace("Unreleased", "2026-09-03"), "development-qualification"),
        ("missing current baseline", f"# Changelog\n\n## {next_version} - Unreleased\n", "development-qualification"),
        ("older planned version", f"# Changelog\n\n## v0.0.0 - Unreleased\n\n## {version} - 2026-09-02\n", "development-qualification"),
        ("multiple open versions", planned_changelog.replace("2026-09-02", "Unreleased"), "development-qualification"),
    ):
        try:
            validate_changelog_release_status(document, artifact_class)
        except ReleaseError:
            pass
        else:
            fail(f"{label} changelog sentinel was accepted")
        sentinels += 1
    with tempfile.TemporaryDirectory(prefix="podway-source-snapshot-", dir=scratch_parent()) as name:
        original_root = ROOT
        ROOT = Path(name)
        try:
            run(["git", "init", "--quiet"], label="source snapshot fixture init")
            (ROOT / "tools").mkdir()
            source = ROOT / "tools/sentinel.py"
            source.write_bytes(b"original\n")
            source.chmod(0o644)
            (ROOT / ".gitignore").write_text(".private\n", encoding="utf-8")
            run(["git", "add", "tools/sentinel.py", ".gitignore"], label="source snapshot fixture index")
            baseline = source_snapshot_sha256()
            source.write_bytes(b"changed\n")
            if source_snapshot_sha256() == baseline:
                fail("source snapshot accepted tracked byte drift")
            source.write_bytes(b"original\n")
            source.chmod(0o755)
            if source_snapshot_sha256() == baseline:
                fail("source snapshot accepted executable mode drift")
            source.chmod(0o644)
            source.unlink()
            if source_snapshot_sha256() == baseline:
                fail("source snapshot accepted tracked deletion")
            source.write_bytes(b"original\n")
            source.chmod(0o644)
            extra = ROOT / "tools/extra.py"
            extra.write_bytes(b"new\n")
            if source_snapshot_sha256() == baseline:
                fail("source snapshot omitted bounded untracked source")
            extra.unlink()
            private = ROOT / ".private"
            private.write_bytes(b"private bytes\n")
            if source_snapshot_sha256() != baseline:
                fail("source snapshot inspected ignored private state")
            outside = ROOT / "credentials.txt"
            outside.write_bytes(b"must not be read\n")
            try:
                source_snapshot_sha256()
            except ReleaseError:
                pass
            else:
                fail("source snapshot accepted out-of-scope untracked input")
            outside.unlink()
            source.unlink()
            source.symlink_to(private)
            try:
                source_snapshot_sha256()
            except ReleaseError:
                pass
            else:
                fail("source snapshot followed a source symlink")
            sentinels += 7
        finally:
            ROOT = original_root
    return {"mode": "self-test", "ok": True, "sentinels": sentinels}


def require_native_host() -> None:
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        fail("release archives require a native arm64 macOS host")
    if run(["arch"], label="host architecture probe").strip() != b"arm64":
        fail("release archives require untranslated arm64 execution")


def require_clean_tree(allow_dirty: bool) -> bool:
    status = run(
        ["git", "status", "--porcelain=v1", "--untracked-files=normal"],
        label="Git worktree inspection",
    )
    dirty = bool(status)
    if dirty and not allow_dirty:
        entries = status.decode("utf-8", errors="replace").splitlines()
        shown = entries[:10]
        if len(entries) > len(shown):
            shown.append(f"... and {len(entries) - len(shown)} more")
        fail(
            "release archive construction requires a clean Git worktree; "
            "commit or stash tracked changes and remove or ignore untracked files: "
            + "; ".join(shown)
        )
    return dirty


def scratch_parent() -> Path:
    return release_evidence.scratch_parent()


def source_snapshot_sha256() -> str:
    tracked = set(run(["git", "ls-files", "--cached", "-z"], label="tracked source snapshot inventory").split(b"\0")) - {b""}
    untracked = set(run(["git", "ls-files", "--others", "--exclude-standard", "-z"], label="untracked source snapshot inventory").split(b"\0")) - {b""}
    prefixes = ("assets/", "contracts/", "crates/", "docs/", "fuzz/", "quality/", "release/", "tests/", "tools/")
    for name in untracked:
        relative = os.fsdecode(name)
        if not relative.startswith(prefixes):
            fail(f"nonignored untracked file is outside bounded source snapshot scope: {relative}")
    names = tracked | untracked
    if len(names) > 100_000:
        fail("source snapshot exceeds 100000 paths")
    entries = []
    total = 0
    for encoded in sorted(names):
        relative = os.fsdecode(encoded)
        path = ROOT / relative
        if Path(relative).is_absolute() or ".." in Path(relative).parts:
            fail("source snapshot contains an unsafe path")
        parent = ROOT
        for component in Path(relative).parts[:-1]:
            parent /= component
            if parent.is_symlink():
                fail(f"source snapshot contains a symlink parent: {relative}")
        try:
            descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
        except FileNotFoundError:
            entries.append({"path": relative, "deleted": True})
            continue
        except OSError as error:
            fail(f"cannot snapshot source path {relative}: {error}")
        try:
            before = os.fstat(descriptor)
            if not stat.S_ISREG(before.st_mode):
                fail(f"source snapshot requires regular files: {relative}")
            total += before.st_size
            if before.st_size > 64 * 1024 * 1024 or total > 512 * 1024 * 1024:
                fail("source snapshot exceeds its byte budget")
            digest = hashlib.sha256()
            with os.fdopen(descriptor, "rb", closefd=False) as opened:
                for block in iter(lambda: opened.read(1024 * 1024), b""):
                    digest.update(block)
            after = os.fstat(descriptor)
            if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
                fail(f"source changed while being snapshotted: {relative}")
            entries.append({"path": relative, "mode": stat.S_IMODE(before.st_mode), "sha256": digest.hexdigest()})
        finally:
            os.close(descriptor)
    return hashlib.sha256(release_evidence.canonical_bytes({"files": entries})).hexdigest()


def require_source_binding(provenance: dict[str, Any], gate: str) -> None:
    development = gate == "development"
    if provenance.get("artifact_class") != ("development-qualification" if development else "distribution"):
        fail("artifact class does not match requested gate")
    dirty = require_clean_tree(development)
    if provenance.get("source_dirty") is not dirty:
        fail("source dirty state changed since packaging")
    if provenance.get("source_snapshot_sha256") != source_snapshot_sha256():
        fail("source candidate bytes or modes changed since packaging")
    if provenance.get("source_commit") != run(["git", "rev-parse", "HEAD"], label="source commit probe").decode().strip():
        fail("source commit changed since packaging")
    if provenance.get("source_tree") != run(["git", "rev-parse", "HEAD^{tree}"], label="source tree probe").decode().strip():
        fail("source Git tree changed since packaging")
    if provenance.get("cargo_lock_sha256") != sha256_file(ROOT / "Cargo.lock"):
        fail("source lockfile changed since packaging")


def preflight() -> dict[str, Any]:
    require_native_host()
    require_clean_tree(False)
    return {
        "host": TARGET,
        "mode": "preflight",
        "ok": True,
    }


def source_files(directory: Path, label: str) -> list[Path]:
    if directory.is_symlink() or not directory.is_dir():
        fail(f"{label} source must be a regular directory")
    files: list[Path] = []
    for candidate in directory.rglob("*"):
        if candidate.is_symlink():
            fail(f"{label} source contains a symlink: {candidate.relative_to(ROOT)}")
        if candidate.is_file():
            files.append(candidate)
        elif not candidate.is_dir():
            fail(f"{label} source contains an unsupported node: {candidate.relative_to(ROOT)}")
    if not files:
        fail(f"{label} source is empty")
    return sorted(files, key=lambda item: item.relative_to(directory).as_posix())


def write_completions(podway: Path, destination: Path) -> None:
    destination.mkdir(parents=True)
    for shell, filename in sorted(COMPLETION_NAMES.items()):
        content = run([str(podway), "completions", shell], label=f"{shell} completion generation")
        if not content or not content.endswith(b"\n"):
            fail(f"{shell} completion output must be non-empty newline-terminated text")
        (destination / filename).write_bytes(content)


def copy_release_inputs(staging: Path, podway: Path) -> None:
    binary_directory = staging / "bin"
    binary_directory.mkdir(parents=True)
    shutil.copyfile(podway, binary_directory / "podway")
    os.chmod(binary_directory / "podway", 0o755)

    write_completions(podway, staging / "share/completions")
    for logical_name in ("presets", "schemas"):
        source = ROOT / repository_assets.ASSET_DIRECTORIES[logical_name]
        destination = staging / "share/podway" / logical_name
        for source_file in source_files(source, logical_name):
            relative = source_file.relative_to(source)
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source_file, target)

    for relative in contract_manifest_asset_paths():
        source = require_regular_file(
            ROOT / repository_assets.logical_source(relative),
            f"contract asset {relative.as_posix()}",
        )
        target = staging / "share/podway" / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)

    manifest = require_regular_file(
        ROOT / "contracts/contract-manifest-v2.json", "contract manifest"
    )
    manifest_target = staging / "share/podway/contracts/contract-manifest-v2.json"
    manifest_target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(manifest, manifest_target)

    for name in ("CHANGELOG.md", "LICENSE", "README.md"):
        source = require_regular_file(ROOT / name, name)
        shutil.copyfile(source, staging / name)


def staged_files(staging: Path) -> list[Path]:
    return sorted(
        (candidate for candidate in staging.rglob("*") if candidate.is_file()),
        key=lambda item: item.relative_to(staging).as_posix(),
    )


def expected_archive_files() -> set[str]:
    expected = {
        f"{ARCHIVE_ROOT}/bin/podway",
        f"{ARCHIVE_ROOT}/LICENSE",
        f"{ARCHIVE_ROOT}/README.md",
        f"{ARCHIVE_ROOT}/CHANGELOG.md",
    }
    expected.update(
        f"{ARCHIVE_ROOT}/share/completions/{filename}"
        for filename in COMPLETION_NAMES.values()
    )
    for logical_name in ("presets", "schemas"):
        source = ROOT / repository_assets.ASSET_DIRECTORIES[logical_name]
        expected.update(
            f"{ARCHIVE_ROOT}/share/podway/{logical_name}/{item.relative_to(source).as_posix()}"
            for item in source_files(source, logical_name)
        )
    expected.update(
        f"{ARCHIVE_ROOT}/share/podway/{relative.as_posix()}"
        for relative in contract_manifest_asset_paths()
    )
    expected.add(
        f"{ARCHIVE_ROOT}/share/podway/contracts/contract-manifest-v2.json"
    )
    return expected


def contract_manifest_document() -> dict[str, Any]:
    path = require_regular_file(
        ROOT / "contracts/contract-manifest-v2.json", "contract manifest"
    )
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        fail(f"contract manifest is not valid JSON: {error}")
    if not isinstance(manifest, dict):
        fail("contract manifest must be a JSON object")
    schema = manifest.get("schema_version")
    digest = manifest.get("digest")
    if schema != "podway.contract-manifest/v2":
        fail("contract manifest has an unsupported schema identity")
    if not isinstance(digest, str) or re.fullmatch(r"sha256:[0-9a-f]{64}", digest) is None:
        fail("contract manifest has an invalid digest identity")
    assets = manifest.get("assets")
    if not isinstance(assets, list) or not assets:
        fail("contract manifest must contain a non-empty asset list")
    for asset in assets:
        if not isinstance(asset, dict) or not isinstance(asset.get("path"), str):
            fail("contract manifest assets must contain string paths")
        relative = PurePosixPath(asset["path"])
        if relative.is_absolute() or ".." in relative.parts or not relative.parts:
            fail(f"contract manifest contains an unsafe asset path: {asset['path']}")
    return manifest


def contract_manifest_asset_paths() -> list[Path]:
    manifest = contract_manifest_document()
    return [Path(asset["path"]) for asset in manifest["assets"]]


def packaged_preset_manifest_paths() -> list[Path]:
    manifest = contract_manifest_document()
    paths = [Path(asset["path"]) for asset in manifest["assets"] if asset.get("kind") == "preset"]
    expected = [
        Path("presets/analysis-v2.yaml"),
        Path("presets/bug-fix-v2.yaml"),
        Path("presets/small-change-v2.yaml"),
        Path("presets/sw-dev-v2.yaml"),
    ]
    if paths != expected:
        fail(f"contract manifest preset identities drift: {paths}")
    return paths


def archive_member_bytes(archive: tarfile.TarFile, name: str, label: str) -> bytes:
    try:
        member = archive.getmember(name)
    except KeyError:
        fail(f"archive omits {label}: {name}")
    if not member.isfile():
        fail(f"archive {label} is not a regular file: {name}")
    extracted = archive.extractfile(member)
    if extracted is None:
        fail(f"archive {label} cannot be read: {name}")
    content = extracted.read(member.size + 1)
    if len(content) != member.size:
        fail(f"archive {label} length differs from its member metadata: {name}")
    return content


def verify_packaged_preset_identity(archive: tarfile.TarFile) -> list[str]:
    manifest_name = f"{ARCHIVE_ROOT}/share/podway/contracts/contract-manifest-v2.json"
    packaged_manifest = archive_member_bytes(archive, manifest_name, "contract manifest")
    source_manifest = (ROOT / "contracts/contract-manifest-v2.json").read_bytes()
    if packaged_manifest != source_manifest:
        fail("archive contract manifest bytes differ from the canonical source")
    try:
        manifest = json.loads(packaged_manifest)
    except json.JSONDecodeError as error:
        fail(f"archive contract manifest is invalid JSON: {error}")

    verified = []
    for relative in packaged_preset_manifest_paths():
        logical = relative.as_posix()
        asset = next(
            (
                candidate
                for candidate in manifest["assets"]
                if candidate.get("kind") == "preset" and candidate.get("path") == logical
            ),
            None,
        )
        if asset is None:
            fail(f"archive contract manifest omits preset identity: {logical}")
        member_name = f"{ARCHIVE_ROOT}/share/podway/{logical}"
        packaged = archive_member_bytes(archive, member_name, f"preset {logical}")
        source = (ROOT / repository_assets.logical_source(relative)).read_bytes()
        if packaged != source:
            fail(f"archive preset bytes differ from the canonical source: {logical}")
        observed_digest = f"sha256:{hashlib.sha256(packaged).hexdigest()}"
        if observed_digest != asset.get("sha256"):
            fail(f"archive preset digest differs from the contract manifest: {logical}")
        verified.append(logical)
    return verified


def add_directory(archive: tarfile.TarFile, name: str) -> None:
    info = tarfile.TarInfo(name=name.rstrip("/") + "/")
    info.type = tarfile.DIRTYPE
    info.mode = 0o755
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    info.mtime = 0
    archive.addfile(info)


def add_file(archive: tarfile.TarFile, source: Path, name: str) -> None:
    content = source.read_bytes()
    info = tarfile.TarInfo(name=name)
    info.mode = 0o755 if name.endswith("/bin/podway") else 0o644
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    info.mtime = 0
    info.size = len(content)
    with tempfile.SpooledTemporaryFile() as payload:
        payload.write(content)
        payload.seek(0)
        archive.addfile(info, payload)


def write_archive(staging: Path, destination: Path) -> None:
    files = staged_files(staging)
    directories = {ARCHIVE_ROOT}
    for source in files:
        archive_name = PurePosixPath(ARCHIVE_ROOT) / PurePosixPath(source.relative_to(staging).as_posix())
        directories.update(parent.as_posix() for parent in archive_name.parents if parent.as_posix() != ".")
    if destination.parent.is_symlink() or not destination.parent.is_dir():
        fail("archive output parent must be a regular directory")
    descriptor, name = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as archive:
                    for directory in sorted(directories):
                        add_directory(archive, directory)
                    for source in files:
                        archive_name = f"{ARCHIVE_ROOT}/{source.relative_to(staging).as_posix()}"
                        add_file(archive, source, archive_name)
            raw.flush()
            os.fsync(raw.fileno())
        temporary.chmod(0o644)
        os.replace(temporary, destination)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def inspect_archive(path: Path) -> list[str]:
    require_regular_file(path, "release archive")
    expected = expected_archive_files()
    observed: set[str] = set()
    members: set[str] = set()
    total_bytes = 0
    expected_directories = {parent.as_posix() for name in expected for parent in PurePosixPath(name).parents if parent.as_posix() != "."}
    with tarfile.open(path, mode="r:gz") as archive:
        for member in archive:
            pure = PurePosixPath(member.name)
            normalized = pure.as_posix()
            if normalized in members or member.name.rstrip("/") != normalized:
                fail(f"archive contains a duplicate or noncanonical path: {member.name}")
            members.add(normalized)
            total_bytes += member.size
            if len(members) > 10000 or member.size > 128 * 1024 * 1024 or total_bytes > 512 * 1024 * 1024:
                fail("archive exceeds its member budget")
            if pure.is_absolute() or ".." in pure.parts or not pure.parts or pure.parts[0] != ARCHIVE_ROOT:
                fail(f"archive contains an unsafe path: {member.name}")
            if member.isdir():
                if normalized not in expected_directories:
                    fail(f"archive contains an unexpected directory: {member.name}")
                if member.mode != 0o755:
                    fail(f"archive directory has an invalid mode: {member.name}")
                continue
            if not member.isfile():
                fail(f"archive contains a non-file entry: {member.name}")
            expected_mode = 0o755 if member.name in {f"{ARCHIVE_ROOT}/bin/podway"} else 0o644
            if member.mode != expected_mode:
                fail(f"archive file has an invalid mode: {member.name}")
            observed.add(member.name)
        verify_packaged_preset_identity(archive)
    if observed != expected:
        fail(f"archive layout drift: missing={sorted(expected - observed)}, extra={sorted(observed - expected)}")
    return sorted(observed)


def verify_source_payloads(extracted: Path, cli: Path) -> None:
    for name in ("CHANGELOG.md", "LICENSE", "README.md"):
        if require_regular_file(extracted / name, f"packaged {name}").read_bytes() != require_regular_file(ROOT / name, name).read_bytes():
            fail(f"packaged {name} differs from the source candidate")
    for shell, filename in sorted(COMPLETION_NAMES.items()):
        expected = run([str(cli), "completions", shell], label=f"independent {shell} completion check")
        if require_regular_file(extracted / "share/completions" / filename, "packaged completion").read_bytes() != expected:
            fail(f"packaged {shell} completion differs from the qualified CLI")
    for logical_name in ("presets", "schemas"):
        source_root = ROOT / repository_assets.ASSET_DIRECTORIES[logical_name]
        for source in source_files(source_root, logical_name):
            packaged = require_regular_file(extracted / "share/podway" / logical_name / source.relative_to(source_root), "packaged source payload")
            if source.read_bytes() != packaged.read_bytes():
                fail(f"packaged {logical_name} payload differs from source: {source.relative_to(source_root)}")
    for relative in contract_manifest_asset_paths():
        source = require_regular_file(ROOT / repository_assets.logical_source(relative), "source contract asset")
        packaged = require_regular_file(extracted / "share/podway" / relative, "packaged contract asset")
        if source.read_bytes() != packaged.read_bytes():
            fail(f"packaged contract asset differs from source: {relative}")
    manifest = Path("contracts/contract-manifest-v2.json")
    if (ROOT / manifest).read_bytes() != (extracted / "share/podway" / manifest).read_bytes():
        fail("packaged manifest differs from the exact source candidate")


def rust_toolchain() -> str:
    output = run(["rustc", "--version"], label="Rust toolchain probe").decode("utf-8").strip()
    if not output.startswith("rustc 1.97.1 "):
        fail(f"release archive requires rustc 1.97.1, observed {output}")
    return output


def validate_changelog_release_status(
    changelog: str, artifact_class: str
) -> dict[str, str]:
    headings = re.findall(
        r"^## (v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)) - "
        r"(Unreleased|[0-9]{4}-[0-9]{2}-[0-9]{2})$",
        changelog,
        flags=re.MULTILINE,
    )
    expected_version = f"v{PRODUCT_VERSION}"
    if not changelog.startswith("# Changelog\n"):
        fail("changelog must begin with the canonical title")
    if not headings:
        fail("changelog must contain a canonical version heading")
    unreleased_count = sum(status == "Unreleased" for _version, status in headings)
    if artifact_class == "distribution":
        if headings[0][0] != expected_version:
            fail(f"distribution changelog must open with the current version ## {expected_version}")
        if headings[0][1] == "Unreleased":
            fail("distribution changelog must use a published release date")
        if unreleased_count != 0:
            fail("distribution changelog must not contain an Unreleased section")
    else:
        if unreleased_count > 1:
            fail("development changelog must contain at most one Unreleased section")
        if headings[0][0] != expected_version:
            planned_version = tuple(int(part) for part in headings[0][0][1:].split("."))
            current_version = tuple(int(part) for part in PRODUCT_VERSION.split("."))
            if (headings[0][1] != "Unreleased" or planned_version <= current_version
                    or len(headings) < 2 or headings[1][0] != expected_version
                    or headings[1][1] == "Unreleased"):
                fail("development changelog must open with the current version or a newer Unreleased version followed by the published current version")
    return {"signing": "unsigned", "notarization": "not-attempted"}


def release_status(artifact_class: str) -> dict[str, str]:
    changelog = require_regular_file(ROOT / "CHANGELOG.md", "changelog").read_text(
        encoding="utf-8"
    )
    return validate_changelog_release_status(changelog, artifact_class)


def package(arguments: argparse.Namespace) -> dict[str, Any]:
    validate_package_mode(arguments.artifact_class, arguments.allow_dirty)
    gate = arguments.gate
    development = arguments.artifact_class == "development-qualification"
    if development != (gate == "development"):
        fail("development-qualification requires exactly the development gate")
    if gate != "patch" and (arguments.patch_base_commit is not None or arguments.prior_make_test_passed is not None):
        fail("non-patch evidence must not carry patch-gate inputs")
    if not development and (arguments.development_source_snapshot_sha256 is not None or arguments.development_checks is not None):
        fail("distribution must not carry development source snapshot admission")
    require_native_host()
    dirty = require_clean_tree(arguments.allow_dirty)
    source_commit = run(["git", "rev-parse", "HEAD"], label="source commit probe").decode().strip()
    source_tree = run(["git", "rev-parse", "HEAD^{tree}"], label="source tree probe").decode().strip()
    snapshot = source_snapshot_sha256()
    if development and arguments.development_source_snapshot_sha256 != snapshot:
        fail("development source snapshot must match the exact source checked and built before packaging")
    if development:
        if arguments.development_checks is None:
            fail("development packaging requires the frozen gate/build checks receipt")
        from qualify_development import validate_checks
        try:
            checks = json.loads(arguments.development_checks.read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            fail(f"cannot read development checks receipt: {error}")
        validate_checks(checks, snapshot, arguments.podway)
    patch_gate = None
    if gate == "patch":
        if not isinstance(arguments.patch_base_commit, str) or not isinstance(arguments.prior_make_test_passed, str):
            fail("patch packaging requires baseline and prior make-test inputs")
        try:
            eligibility = verify_patch_release.check(arguments.patch_base_commit, arguments.prior_make_test_passed)
        except verify_patch_release.PatchReleaseError as error:
            fail(f"patch packaging eligibility failed: {error}")
        patch_gate = {
            "prior_make_test_passed": eligibility["prior_make_test_passed"],
            "tested_baseline_commit": eligibility["base_commit"],
        }
    with tempfile.TemporaryDirectory(prefix="podway-release-", dir=scratch_parent()) as name:
        temporary = Path(name)
        podway = require_native_binary(
            snapshot_executable(arguments.podway, temporary / "inputs/podway", "podway"), "podway"
        )
        verify_artifact_class({"podway": podway}, arguments.artifact_class)
        contract_receipt = verify_release_contract(ROOT, podway, source_commit)
        output_directory = arguments.output_dir
        if output_directory.exists() and (output_directory.is_symlink() or not output_directory.is_dir()):
            fail("release output must be a regular directory")
        output_directory.mkdir(parents=True, exist_ok=True)
        archive_path = output_directory / f"{ARCHIVE_ROOT}.tar.gz"
        checksum_path = output_directory / f"{archive_path.name}.sha256"
        provenance_path = output_directory / f"{ARCHIVE_ROOT}.provenance.json"
        staging = temporary / ARCHIVE_ROOT
        staging.mkdir()
        copy_release_inputs(staging, podway)
        write_archive(staging, archive_path)
        entries = inspect_archive(archive_path)
        archive_digest = sha256_file(archive_path)
        release_evidence.atomic_write_bytes(checksum_path, f"{archive_digest}  {archive_path.name}\n".encode())
        provenance = {
            "archive": {"name": archive_path.name, "sha256": archive_digest},
            "binaries": {"podway": sha256_file(podway)},
            "build_identity": contract_receipt["build_identity"],
            "artifact_class": arguments.artifact_class,
            "cargo_lock_sha256": sha256_file(ROOT / "Cargo.lock"),
            "contract_manifest_digest": contract_receipt["contract_manifest_digest"],
            "contract_manifest_schema": contract_receipt["contract_manifest_schema"],
            "product": release_evidence.PRODUCT,
            "release_gate": release_evidence.DEVELOPMENT_GATE if development else (
                release_evidence.PATCH_RELEASE_GATE if gate == "patch" else release_evidence.RELEASE_GATE
            ),
            "release_gate_result": release_evidence.PASSED,
            "release_status": release_status(arguments.artifact_class),
            "schema": release_evidence.PATCH_PROVENANCE_SCHEMA if gate == "patch" else release_evidence.PROVENANCE_SCHEMA,
            "source_commit": source_commit,
            "source_dirty": dirty,
            "source_tree": source_tree,
            "source_snapshot_sha256": snapshot,
            "target": TARGET,
            "toolchain": rust_toolchain(),
            "version": PRODUCT_VERSION,
        }
        if gate == "patch":
            provenance["patch_gate"] = patch_gate
        else:
            provenance["packaged_conformance"] = {
                "result": release_evidence.PENDING,
                "scenarios": release_evidence.PACKAGED_CONFORMANCE_SCENARIOS,
            }
        require_source_binding(provenance, gate)
        release_evidence.validate_provenance(
            provenance, version=PRODUCT_VERSION, target=TARGET, commit=source_commit,
            tree=source_tree, conformance_result=None if gate == "patch" else release_evidence.PENDING,
        )
        release_evidence.atomic_write_json(provenance_path, provenance)
    return {
        "archive": str(archive_path.resolve()), "archive_sha256": archive_digest,
        "artifact_class": arguments.artifact_class, "publishable": not development,
        "checksum": str(checksum_path.resolve()), "entries": entries, "mode": "package",
        "ok": True, "provenance": str(provenance_path.resolve()),
    }


def verify_version(podway: Path) -> dict[str, Any]:
    require_native_host()
    source_commit = run(["git", "rev-parse", "HEAD"], label="source commit probe").decode().strip()
    with tempfile.TemporaryDirectory(prefix="podway-version-", dir=scratch_parent()) as name:
        cli = require_native_binary(snapshot_executable(podway, Path(name) / "podway", "podway"), "podway")
        receipt = verify_release_contract(ROOT, cli, source_commit)
    if receipt.get("version") != PRODUCT_VERSION:
        fail("release contract receipt does not report the product version")
    return {"build_identity": receipt["build_identity"], "mode": "verify-version", "ok": True, "version": PRODUCT_VERSION}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    package_parser = subparsers.add_parser("package", help="build and inspect a release archive")
    package_parser.add_argument("--podway", type=Path, required=True)
    package_parser.add_argument(
        "--artifact-class", choices=("development-qualification", "distribution"), required=True
    )
    package_parser.add_argument("--gate", choices=("full", "patch", "development"), default="full")
    package_parser.add_argument("--development-source-snapshot-sha256")
    package_parser.add_argument("--development-checks", type=Path)
    package_parser.add_argument("--patch-base-commit")
    package_parser.add_argument("--prior-make-test-passed")
    package_parser.add_argument("--output-dir", type=Path, required=True)
    package_parser.add_argument("--allow-dirty", action="store_true", help="test-only: package an uncommitted tree")
    subparsers.add_parser("preflight", help="verify release host and Git worktree state")
    subparsers.add_parser("self-test", help="run isolated artifact-class sentinels")
    subparsers.add_parser("source-snapshot", help="freeze the source identity before development gates and build")
    version_parser = subparsers.add_parser("verify-version", help="verify release binary version identities")
    version_parser.add_argument("--podway", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "self-test":
            result = self_test()
        elif arguments.command == "source-snapshot":
            result = {"mode": "source-snapshot", "ok": True, "source_snapshot_sha256": source_snapshot_sha256()}
        elif arguments.command == "preflight":
            result = preflight()
        elif arguments.command == "verify-version":
            result = verify_version(arguments.podway)
        else:
            result = package(arguments)
    except (
        OSError,
        ReleaseError,
        release_evidence.EvidenceError,
        tarfile.TarError,
        UnicodeError,
        json.JSONDecodeError,
    ) as error:
        print(json.dumps({"error": str(error), "mode": arguments.command, "ok": False}, sort_keys=True, separators=(",", ":")))
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
