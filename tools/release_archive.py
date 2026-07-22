#!/usr/bin/env python3
"""Build and inspect the deterministic Podway Apple Silicon release archive."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import stat
import subprocess
import tarfile
import tempfile
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
PRODUCT_VERSION = "0.1.0"
TARGET = "aarch64-apple-darwin"
ARCHIVE_ROOT = f"podway-{PRODUCT_VERSION}-{TARGET}"
MACHO_64_LITTLE_ENDIAN = b"\xcf\xfa\xed\xfe"
CPU_TYPE_ARM64 = 0x0100000C
COMPLETION_NAMES = {
    "bash": "podway.bash",
    "fish": "podway.fish",
    "zsh": "podway.zsh",
}


class ReleaseError(RuntimeError):
    pass


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
    version_arguments = ["version"] if expected_name == "podway" else ["--version"]
    version = run([str(path), *version_arguments], label=f"{expected_name} version probe")
    expected_version = f"{expected_name} {PRODUCT_VERSION}\n".encode()
    if version != expected_version:
        fail(f"{expected_name} version output does not equal {expected_version!r}")
    return path


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
        fail("release archive construction requires a clean Git worktree")
    return dirty


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
    for name in ("presets", "schemas"):
        source = ROOT / name
        destination = staging / "share/podway" / name
        for source_file in source_files(source, name):
            relative = source_file.relative_to(source)
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source_file, target)

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
    for name in ("presets", "schemas"):
        source = ROOT / name
        expected.update(
            f"{ARCHIVE_ROOT}/share/podway/{name}/{item.relative_to(source).as_posix()}"
            for item in source_files(source, name)
        )
    return expected


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
        "public v1 IPC, output, error, workspace, procedure, and SQLite contracts",
        "schema-0 state is upgraded transactionally to schema-v1",
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
    require_native_host()
    dirty = require_clean_tree(arguments.allow_dirty)
    podway = require_native_binary(arguments.podway, "podway")
    podwayd = require_native_binary(arguments.podwayd, "podwayd")
    output_directory = arguments.output_dir
    if output_directory.exists() and (output_directory.is_symlink() or not output_directory.is_dir()):
        fail("release output must be a regular directory")
    output_directory.mkdir(parents=True, exist_ok=True)

    archive_path = output_directory / f"{ARCHIVE_ROOT}.tar.gz"
    checksum_path = output_directory / f"{archive_path.name}.sha256"
    provenance_path = output_directory / f"{ARCHIVE_ROOT}.provenance.json"
    with tempfile.TemporaryDirectory(prefix="podway-release-") as temporary_name:
        staging = Path(temporary_name) / ARCHIVE_ROOT
        staging.mkdir()
        copy_release_inputs(staging, podway, podwayd)
        write_archive(staging, archive_path)

    entries = inspect_archive(archive_path)
    archive_digest = sha256_file(archive_path)
    checksum_path.write_text(f"{archive_digest}  {archive_path.name}\n", encoding="utf-8")
    provenance = {
        "archive": {"name": archive_path.name, "sha256": archive_digest},
        "binaries": {"podway": sha256_file(podway), "podwayd": sha256_file(podwayd)},
        "cargo_lock_sha256": sha256_file(ROOT / "Cargo.lock"),
        "release_gate": "test-fixture" if arguments.allow_dirty else "make test: passed",
        "release_status": release_status(),
        "schema": "podway.release-provenance/v1",
        "source_commit": run(["git", "rev-parse", "HEAD"], label="source commit probe").decode().strip(),
        "source_dirty": dirty,
        "target": TARGET,
        "toolchain": rust_toolchain(),
        "version": PRODUCT_VERSION,
    }
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
    package_parser.add_argument("--output-dir", type=Path, required=True)
    package_parser.add_argument("--allow-dirty", action="store_true", help="test-only: package an uncommitted tree")
    arguments = parser.parse_args()
    try:
        result = package(arguments)
    except (OSError, ReleaseError, tarfile.TarError, UnicodeError, json.JSONDecodeError) as error:
        print(json.dumps({"error": str(error), "mode": arguments.command, "ok": False}, sort_keys=True, separators=(",", ":")))
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
