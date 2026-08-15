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


ROOT = Path(__file__).resolve().parents[1]
PRODUCT_VERSION = "0.2.3"
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
DEVELOPMENT_V2_PROBE_ENV = "PODWAY_DEVELOPMENT_V2_ADMISSION_PROBE"
DEVELOPMENT_V2_PROBE_TOKEN = "podway-development-v2-admission-v1"
DEVELOPMENT_V2_PROBE_ARGUMENT = "--podway-development-v2-admission-probe"


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
    completed = subprocess.run(
        arguments,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        fail(f"{label} failed with exit {completed.returncode}: {detail}")
    return completed.stdout


def require_regular_file(path: Path, label: str) -> Path:
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file: {path}")
    return path.resolve()


def snapshot_executable(source: Path, destination: Path, label: str) -> Path:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(source, flags)
    except OSError as error:
        fail(f"cannot open {label} as a regular non-symlink file: {source}: {error}")
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            fail(f"{label} must be a regular non-symlink file: {source}")
        if metadata.st_mode & stat.S_IXUSR == 0:
            fail(f"{label} is not executable: {source}")
        destination.parent.mkdir(parents=True, exist_ok=True)
        with os.fdopen(descriptor, "rb", closefd=False) as opened_source:
            with destination.open("xb") as opened_destination:
                shutil.copyfileobj(opened_source, opened_destination)
                opened_destination.flush()
                os.fsync(opened_destination.fileno())
        os.chmod(destination, 0o755)
    finally:
        os.close(descriptor)
    return destination.resolve()


def require_native_binary(path: Path, expected_name: str) -> Path:
    path = require_regular_file(path, expected_name)
    mode = path.stat().st_mode
    if mode & stat.S_IXUSR == 0:
        fail(f"{expected_name} is not executable: {path}")
    header = path.read_bytes()[:8]
    if len(header) != 8 or header[:4] != MACHO_64_LITTLE_ENDIAN:
        fail(f"{expected_name} is not a thin 64-bit little-endian Mach-O binary")
    if int.from_bytes(header[4:8], "little") != CPU_TYPE_ARM64:
        fail(f"{expected_name} is not an arm64 Mach-O binary")
    version_arguments = ["version"]
    version = run([str(path), *version_arguments], label=f"{expected_name} version probe")
    expected_version = f"{expected_name} {PRODUCT_VERSION}\n".encode()
    if version != expected_version:
        fail(f"{expected_name} version output does not equal {expected_version!r}")
    return path


def verify_release_contract(
    contract_root: Path,
    podway: Path,
    podwayd: Path,
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
            "--podwayd",
            str(podwayd),
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
    if not isinstance(receipt, dict) or receipt.get("schema") != "podway.contract-verification/v1":
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


def development_v2_admission_capability(path: Path) -> TestIsolationCapability:
    def run_probe(token: str) -> tuple[int, bytes, bytes] | None:
        environment = {"PATH": "/usr/bin:/bin", DEVELOPMENT_V2_PROBE_ENV: token}
        try:
            completed = subprocess.run(
                [str(path), DEVELOPMENT_V2_PROBE_ARGUMENT],
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

    enabled_probe = run_probe(DEVELOPMENT_V2_PROBE_TOKEN)
    disabled_probe = run_probe(f"{DEVELOPMENT_V2_PROBE_TOKEN}-disabled")
    if enabled_probe == (0, f"{DEVELOPMENT_V2_PROBE_TOKEN}\n".encode(), b""):
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
    expected = (
        TestIsolationCapability.ENABLED
        if artifact_class == "test-fixture"
        else TestIsolationCapability.DISABLED
    )
    mismatches = {
        name: {"expected_test_isolation": expected.value, "actual": actual.value}
        for name, actual in capabilities.items()
        if actual != expected
    }
    if mismatches:
        fail(f"binary isolation does not match --artifact-class {artifact_class}: {mismatches}")


def self_test() -> dict[str, Any]:
    same_name_cli = Path("/fixture/cli/product")
    same_name_daemon = Path("/fixture/daemon/product")

    def probe(
        enabled: set[Path], indeterminate: set[Path] | None = None
    ) -> Callable[[Path], TestIsolationCapability]:
        indeterminate = set() if indeterminate is None else indeterminate

        def capability(path: Path) -> TestIsolationCapability:
            if path in indeterminate:
                return TestIsolationCapability.INDETERMINATE
            if path in enabled:
                return TestIsolationCapability.ENABLED
            return TestIsolationCapability.DISABLED

        return capability

    def expect_artifact_class_rejection(
        artifact_class: str,
        enabled: set[Path],
        expected_role: str,
    ) -> None:
        try:
            verify_artifact_class(
                {"podway": same_name_cli, "podwayd": same_name_daemon},
                artifact_class,
                probe(enabled),
            )
        except ReleaseError as error:
            if expected_role not in str(error):
                fail(f"artifact-class self-test did not preserve binary role {expected_role}")
        else:
            fail(
                "artifact-class self-test accepted mismatched isolation for "
                f"{artifact_class} role {expected_role}"
            )

    verify_artifact_class(
        {"podway": same_name_cli, "podwayd": same_name_daemon},
        "test-fixture",
        probe({same_name_cli, same_name_daemon}),
    )
    verify_artifact_class(
        {"podway": same_name_cli, "podwayd": same_name_daemon},
        "distribution",
        probe(set()),
    )
    expect_artifact_class_rejection("test-fixture", {same_name_daemon}, "podway")
    expect_artifact_class_rejection("test-fixture", {same_name_cli}, "podwayd")
    expect_artifact_class_rejection("distribution", {same_name_cli}, "podway")
    expect_artifact_class_rejection("distribution", {same_name_daemon}, "podwayd")
    for artifact_class in ("test-fixture", "distribution"):
        try:
            verify_artifact_class(
                {"podway": same_name_cli, "podwayd": same_name_daemon},
                artifact_class,
                probe(set(), {same_name_cli}),
            )
        except ReleaseError as error:
            if "indeterminate" not in str(error) or "podway" not in str(error):
                fail("artifact-class self-test did not fail closed on an indeterminate probe")
        else:
            fail("artifact-class self-test accepted an indeterminate probe")

    try:
        validate_package_mode("distribution", True)
    except ReleaseError:
        pass
    else:
        fail("package-mode self-test accepted a dirty distribution")

    validate_package_mode("distribution", False)
    validate_package_mode("test-fixture", True)
    with tempfile.TemporaryDirectory(prefix="podway-release-self-test-") as temporary_name:
        temporary = Path(temporary_name)
        enabled_v2 = temporary / "v2-enabled"
        enabled_v2.write_text(
            "#!/bin/sh\n"
            f'if [ "$1" = "{DEVELOPMENT_V2_PROBE_ARGUMENT}" ] && '
            f'[ "${DEVELOPMENT_V2_PROBE_ENV}" = "{DEVELOPMENT_V2_PROBE_TOKEN}" ]; then\n'
            f'  printf "%s\\n" "{DEVELOPMENT_V2_PROBE_TOKEN}"\n'
            "  exit 0\n"
            "fi\n"
            "exit 1\n",
            encoding="utf-8",
        )
        enabled_v2.chmod(0o700)
        disabled_v2 = temporary / "v2-disabled"
        disabled_v2.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
        disabled_v2.chmod(0o700)
        if (
            development_v2_admission_capability(enabled_v2)
            is not TestIsolationCapability.ENABLED
            or development_v2_admission_capability(disabled_v2)
            is not TestIsolationCapability.DISABLED
        ):
            fail("development-v2 capability self-test misclassified a binary")
        source = temporary / "source"
        snapshot = temporary / "snapshot"
        source.write_bytes(b"verified bytes")
        source.chmod(0o700)
        snapshot_executable(source, snapshot, "self-test executable")
        source.write_bytes(b"replacement bytes")
        if snapshot.read_bytes() != b"verified bytes":
            fail("executable snapshot changed after its source was replaced")
    with tempfile.TemporaryDirectory(prefix="podway-preset-archive-self-test-") as temporary_name:
        temporary = Path(temporary_name)
        staging = temporary / "staging"
        manifest_target = staging / "share/podway/contracts/contract-manifest-v1.json"
        manifest_target.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / "contracts/contract-manifest-v1.json", manifest_target)
        for relative in packaged_preset_manifest_paths():
            source = ROOT / repository_assets.logical_source(relative)
            target = staging / "share/podway" / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
        archive_path = temporary / "presets.tar.gz"
        write_archive(staging, archive_path)
        with tarfile.open(archive_path, mode="r:gz") as archive:
            verify_packaged_preset_identity(archive)

        tampered = staging / "share/podway/presets/bug-fix-v2.yaml"
        tampered.write_bytes(tampered.read_bytes() + b"# tampered\n")
        tampered_archive = temporary / "tampered-presets.tar.gz"
        write_archive(staging, tampered_archive)
        try:
            with tarfile.open(tampered_archive, mode="r:gz") as archive:
                verify_packaged_preset_identity(archive)
        except ReleaseError as error:
            if "preset bytes differ" not in str(error):
                fail("packaged preset tamper sentinel returned the wrong error")
        else:
            fail("packaged preset tamper sentinel was accepted")
    if release_status() != {"signing": "unsigned", "notarization": "not-attempted"}:
        fail("release-note status sentinel returned an unexpected value")
    return {"mode": "self-test", "ok": True, "sentinels": 25}


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


def copy_release_inputs(staging: Path, podway: Path, podwayd: Path) -> None:
    binary_directory = staging / "bin"
    binary_directory.mkdir(parents=True)
    shutil.copyfile(podway, binary_directory / "podway")
    shutil.copyfile(podwayd, binary_directory / "podwayd")
    os.chmod(binary_directory / "podway", 0o755)
    os.chmod(binary_directory / "podwayd", 0o755)

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
        ROOT / "contracts/contract-manifest-v1.json", "contract manifest"
    )
    manifest_target = staging / "share/podway/contracts/contract-manifest-v1.json"
    manifest_target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(manifest, manifest_target)

    for name in ("LICENSE", "README.md", "RELEASE_NOTES.md"):
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
        f"{ARCHIVE_ROOT}/bin/podwayd",
        f"{ARCHIVE_ROOT}/LICENSE",
        f"{ARCHIVE_ROOT}/README.md",
        f"{ARCHIVE_ROOT}/RELEASE_NOTES.md",
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
        f"{ARCHIVE_ROOT}/share/podway/contracts/contract-manifest-v1.json"
    )
    return expected


def contract_manifest_document() -> dict[str, Any]:
    path = require_regular_file(
        ROOT / "contracts/contract-manifest-v1.json", "contract manifest"
    )
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        fail(f"contract manifest is not valid JSON: {error}")
    if not isinstance(manifest, dict):
        fail("contract manifest must be a JSON object")
    schema = manifest.get("schema_version")
    digest = manifest.get("digest")
    if schema != "podway.contract-manifest/v1":
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
    manifest_name = f"{ARCHIVE_ROOT}/share/podway/contracts/contract-manifest-v1.json"
    packaged_manifest = archive_member_bytes(archive, manifest_name, "contract manifest")
    source_manifest = (ROOT / "contracts/contract-manifest-v1.json").read_bytes()
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
    info.mode = 0o755 if name.endswith("/bin/podway") or name.endswith("/bin/podwayd") else 0o644
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
    with destination.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as archive:
                for directory in sorted(directories):
                    add_directory(archive, directory)
                for source in files:
                    archive_name = f"{ARCHIVE_ROOT}/{source.relative_to(staging).as_posix()}"
                    add_file(archive, source, archive_name)


def inspect_archive(path: Path) -> list[str]:
    expected = expected_archive_files()
    observed: set[str] = set()
    with tarfile.open(path, mode="r:gz") as archive:
        for member in archive.getmembers():
            pure = PurePosixPath(member.name)
            if pure.is_absolute() or ".." in pure.parts or not pure.parts or pure.parts[0] != ARCHIVE_ROOT:
                fail(f"archive contains an unsafe path: {member.name}")
            if member.isdir():
                if member.mode != 0o755:
                    fail(f"archive directory has an invalid mode: {member.name}")
                continue
            if not member.isfile():
                fail(f"archive contains a non-file entry: {member.name}")
            expected_mode = 0o755 if member.name in {f"{ARCHIVE_ROOT}/bin/podway", f"{ARCHIVE_ROOT}/bin/podwayd"} else 0o644
            if member.mode != expected_mode:
                fail(f"archive file has an invalid mode: {member.name}")
            observed.add(member.name)
        verify_packaged_preset_identity(archive)
    if observed != expected:
        fail(f"archive layout drift: missing={sorted(expected - observed)}, extra={sorted(observed - expected)}")
    return sorted(observed)


def rust_toolchain() -> str:
    output = run(["rustc", "--version"], label="Rust toolchain probe").decode("utf-8").strip()
    if not output.startswith("rustc 1.97.1 "):
        fail(f"release archive requires rustc 1.97.1, observed {output}")
    return output


def release_status() -> dict[str, str]:
    notes = (ROOT / "RELEASE_NOTES.md").read_text(encoding="utf-8")
    required = (
        "Podway 0.2.3 is a release candidate and has not been published",
        "During LaunchAgent replacement",
        "`podway daemon install`",
        "three built-in Procedure v2 presets",
        "fails closed with `LEGACY_PROCEDURE_STATE_UNSUPPORTED`",
        "podway-0.2.3-aarch64-apple-darwin.tar.gz.sha256",
        "podway-0.2.3-aarch64-apple-darwin.dolgorae-handoff.json",
        "native Apple Silicon macOS",
        "same-user local tool",
        "qualified release candidate admits Procedure v2 sessions normally",
        "does not contain the development-only admission unlock",
        "publish only the unchanged qualified artifacts after explicit release authorization",
        "No MCP server or MCP transport is included",
        "unsigned and not notarized",
    )
    missing = [text for text in required if text not in notes]
    if missing:
        fail(f"release notes omit required release facts: {missing}")
    return {"signing": "unsigned", "notarization": "not-attempted"}


def write_json(path: Path, value: Any) -> None:
    path.write_text(
        json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )


def package(arguments: argparse.Namespace) -> dict[str, Any]:
    validate_package_mode(arguments.artifact_class, arguments.allow_dirty)
    require_native_host()
    dirty = require_clean_tree(arguments.allow_dirty)
    source_commit = run(["git", "rev-parse", "HEAD"], label="source commit probe").decode().strip()
    source_tree = run(["git", "rev-parse", "HEAD^{tree}"], label="source tree probe").decode().strip()
    with tempfile.TemporaryDirectory(prefix="podway-release-") as temporary_name:
        temporary = Path(temporary_name)
        podway = require_native_binary(
            snapshot_executable(arguments.podway, temporary / "inputs/podway", "podway"),
            "podway",
        )
        podwayd = require_native_binary(
            snapshot_executable(arguments.podwayd, temporary / "inputs/podwayd", "podwayd"),
            "podwayd",
        )
        verify_artifact_class(
            {"podway": podway, "podwayd": podwayd}, arguments.artifact_class
        )
        if arguments.artifact_class == "distribution":
            capability = development_v2_admission_capability(podwayd)
            if capability is not TestIsolationCapability.DISABLED:
                fail(
                    "distribution daemon exposes or ambiguously reports the "
                    f"development-v2 admission unlock: {capability.value}"
                )
        contract_receipt = verify_release_contract(ROOT, podway, podwayd, source_commit)

        output_directory = arguments.output_dir
        if output_directory.exists() and (
            output_directory.is_symlink() or not output_directory.is_dir()
        ):
            fail("release output must be a regular directory")
        output_directory.mkdir(parents=True, exist_ok=True)
        archive_path = output_directory / f"{ARCHIVE_ROOT}.tar.gz"
        checksum_path = output_directory / f"{archive_path.name}.sha256"
        provenance_path = output_directory / f"{ARCHIVE_ROOT}.provenance.json"
        staging = temporary / ARCHIVE_ROOT
        staging.mkdir()
        copy_release_inputs(staging, podway, podwayd)
        write_archive(staging, archive_path)
        entries = inspect_archive(archive_path)
        archive_digest = sha256_file(archive_path)
        checksum_path.write_text(f"{archive_digest}  {archive_path.name}\n", encoding="utf-8")
        provenance = {
            "archive": {"name": archive_path.name, "sha256": archive_digest},
            "binaries": {"podway": sha256_file(podway), "podwayd": sha256_file(podwayd)},
            "build_identity": contract_receipt["build_identity"],
            "artifact_class": arguments.artifact_class,
            "cargo_lock_sha256": sha256_file(ROOT / "Cargo.lock"),
            "contract_manifest_digest": contract_receipt["contract_manifest_digest"],
            "contract_manifest_schema": contract_receipt["contract_manifest_schema"],
            "packaged_conformance": {
                "result": release_evidence.PENDING,
                "scenarios": release_evidence.PACKAGED_CONFORMANCE_SCENARIOS,
            },
            "product": release_evidence.PRODUCT,
            "release_gate": (
                "test-fixture"
                if arguments.artifact_class == "test-fixture"
                else "make test + fuzzing: passed"
            ),
            "release_gate_result": release_evidence.PASSED,
            "release_status": release_status(),
            "schema": "podway.release-provenance/v1",
            "source_commit": source_commit,
            "source_dirty": dirty,
            "source_tree": source_tree,
            "target": TARGET,
            "toolchain": rust_toolchain(),
            "version": PRODUCT_VERSION,
        }
    if arguments.artifact_class == "distribution":
        try:
            release_evidence.validate_provenance(
                provenance,
                version=PRODUCT_VERSION,
                target=TARGET,
                commit=source_commit,
                tree=source_tree,
                conformance_result=release_evidence.PENDING,
            )
        except release_evidence.EvidenceError as error:
            fail(str(error))
        release_evidence.atomic_write_json(provenance_path, provenance)
    else:
        write_json(provenance_path, provenance)
    return {
        "archive": str(archive_path.resolve()),
        "archive_sha256": archive_digest,
        "checksum": str(checksum_path.resolve()),
        "entries": entries,
        "mode": "package",
        "ok": True,
        "provenance": str(provenance_path.resolve()),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    package_parser = subparsers.add_parser("package", help="build and inspect a release archive")
    package_parser.add_argument("--podway", type=Path, required=True)
    package_parser.add_argument("--podwayd", type=Path, required=True)
    package_parser.add_argument(
        "--artifact-class", choices=("test-fixture", "distribution"), required=True
    )
    package_parser.add_argument("--output-dir", type=Path, required=True)
    package_parser.add_argument("--allow-dirty", action="store_true", help="test-only: package an uncommitted tree")
    subparsers.add_parser("preflight", help="verify release host and Git worktree state")
    subparsers.add_parser("self-test", help="run isolated artifact-class sentinels")
    arguments = parser.parse_args()
    try:
        if arguments.command == "self-test":
            result = self_test()
        elif arguments.command == "preflight":
            result = preflight()
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
