#!/usr/bin/env python3
"""Build Podway's immutable Aquarium development-service bundle."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NEXT_VERSION = "v0.2.8"
PROJECT_ID = "podway"


class ProducerError(RuntimeError):
    pass


def _run(*arguments: str, environment: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        arguments,
        cwd=ROOT,
        env=environment,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="strict",
    )
    if result.returncode != 0:
        raise ProducerError(result.stderr.strip() or f"command failed: {arguments[0]}")
    return result.stdout.strip()


def description() -> dict[str, str]:
    return {
        "schema": "aquarium-dev-producer-description/v2",
        "project_id": PROJECT_ID,
        "next_version": NEXT_VERSION,
        "artifact_kind": "managed-service",
        "artifact_path": "bundle",
        "command_path": "bin/podway",
        "controller_path": "libexec/aquarium-dev-service",
    }


def file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return f"sha256:{digest.hexdigest()}"


def tree_digest(root: Path, *, exclude: frozenset[str] = frozenset()) -> str:
    digest = hashlib.sha256()
    files: list[Path] = []
    for path in root.rglob("*"):
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode):
            raise ProducerError("generation bundles cannot contain symbolic links")
        if path.is_file() and path.relative_to(root).as_posix() not in exclude:
            files.append(path)
    for path in sorted(files):
        relative = path.relative_to(root).as_posix().encode("utf-8")
        digest.update(relative)
        digest.update(b"\0")
        digest.update(bytes.fromhex(file_digest(path)[7:]))
        digest.update(b"\n")
    return f"sha256:{digest.hexdigest()}"


def _validate_source() -> str:
    if Path(_run("git", "rev-parse", "--show-toplevel")) != ROOT:
        raise ProducerError("the producer must run at the canonical Podway Git root")
    if _run("git", "symbolic-ref", "--short", "HEAD") != "main":
        raise ProducerError("Aquarium development builds require local main")
    head = _run("git", "rev-parse", "HEAD")
    if _run("git", "rev-parse", "refs/heads/main") != head:
        raise ProducerError("HEAD must equal local main")
    if _run("git", "status", "--porcelain=v1", "--untracked-files=all"):
        raise ProducerError("Aquarium development builds require a clean worktree")
    if len(head) != 40 or any(character not in "0123456789abcdef" for character in head):
        raise ProducerError("Git returned an unsupported object identity")
    return head


def _validate_output(value: str | None) -> Path:
    if not value:
        raise ProducerError("AQUARIUM_DEV_OUTPUT is required")
    output = Path(value)
    if not output.is_absolute() or output != Path(os.path.normpath(output)):
        raise ProducerError("AQUARIUM_DEV_OUTPUT must be an absolute normalized path")
    metadata = output.lstat()
    if stat.S_ISLNK(metadata.st_mode) or not output.is_dir():
        raise ProducerError("AQUARIUM_DEV_OUTPUT must be a regular directory")
    if any(output.iterdir()):
        raise ProducerError("AQUARIUM_DEV_OUTPUT must be empty")
    return output


def _copy_executable(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(0o755)


def launcher_source() -> str:
    return (
        "#!/bin/sh\n"
        "set -eu\n"
        "self=$0\n"
        "while [ -L \"$self\" ]; do\n"
        "  base=$(CDPATH= cd -- \"${self%/*}\" && pwd -P) || exit 126\n"
        "  link=$(/usr/bin/readlink \"$self\") || exit 126\n"
        "  case $link in\n"
        "    /*) self=$link ;;\n"
        "    *) self=$base/$link ;;\n"
        "  esac\n"
        "done\n"
        "here=$(CDPATH= cd -- \"${self%/*}\" && pwd -P) || exit 126\n"
        "bundle=$(CDPATH= cd -- \"$here/..\" && pwd -P) || exit 126\n"
        "generation=$(CDPATH= cd -- \"$bundle/..\" && pwd -P) || exit 126\n"
        "project=$(CDPATH= cd -- \"$generation/..\" && pwd -P) || exit 126\n"
        "artifacts=$(CDPATH= cd -- \"$project/..\" && pwd -P) || exit 126\n"
        "host=$(CDPATH= cd -- \"$artifacts/..\" && pwd -P) || exit 126\n"
        "generation_id=${generation##*/}\n"
        "case $generation_id in ''|*[!0-9a-f]*) exit 126 ;; esac\n"
        "[ \"${#generation_id}\" -eq 40 ] || exit 126\n"
        "[ \"${bundle##*/}\" = bundle ] || exit 126\n"
        "[ \"${project##*/}\" = podway ] || exit 126\n"
        "[ \"${artifacts##*/}\" = artifacts ] || exit 126\n"
        "PODWAY_DEV_HOME=$host/runtime/podway\n"
        "export PODWAY_DEV_HOME\n"
        "exec \"$bundle/libexec/podway\" --dev \"$@\"\n"
    )


def _remove_owned_output(path: Path) -> None:
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    if stat.S_ISDIR(metadata.st_mode) and not stat.S_ISLNK(metadata.st_mode):
        shutil.rmtree(path)
    else:
        path.unlink()


def build(output_value: str | None) -> dict[str, str]:
    output = _validate_output(output_value)
    head = _validate_source()
    build_root = output / ".build"
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = os.fspath(build_root)
    environment["CARGO_INCREMENTAL"] = "0"
    bundle = output / "bundle"
    external_manifest = output / ".aquarium-manifest.json"
    try:
        _run(
            "cargo",
            "build",
            "--release",
            "--locked",
            "-p",
            "podway-cli",
            "--bin",
            "podway",
            "-p",
            "podway-daemon",
            "--bin",
            "podwayd",
            environment=environment,
        )

        cli = bundle / "libexec/podway"
        daemon = bundle / "libexec/podwayd"
        controller = bundle / "libexec/aquarium-dev-service"
        launcher = bundle / "bin/podway"
        _copy_executable(build_root / "release/podway", cli)
        _copy_executable(build_root / "release/podwayd", daemon)
        _copy_executable(ROOT / "tools/aquarium_dev_service.py", controller)
        launcher.parent.mkdir(parents=True, exist_ok=True)
        launcher.write_text(launcher_source(), encoding="utf-8")
        launcher.chmod(0o755)
        shutil.rmtree(build_root)

        development_version = f"{NEXT_VERSION}-dev.{head[:12]}"
        internal_manifest = {
            "schema": "podway.aquarium-dev-bundle/v1",
            "project_id": PROJECT_ID,
            "git_sha": head,
            "development_version": development_version,
            "payload_sha256": tree_digest(bundle),
            "controller_protocol": "aquarium-dev-service/v1",
            "runtime_protocol": "podway.managed-runtime/v3",
            "executables": {
                "launcher": {"path": "bin/podway", "sha256": file_digest(launcher)},
                "cli": {"path": "libexec/podway", "sha256": file_digest(cli)},
                "daemon": {"path": "libexec/podwayd", "sha256": file_digest(daemon)},
                "controller": {
                    "path": "libexec/aquarium-dev-service",
                    "sha256": file_digest(controller),
                },
            },
        }
        (bundle / "manifest.json").write_text(
            json.dumps(internal_manifest, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        manifest = {
            "schema": "aquarium-dev-artifact-manifest/v2",
            "project_id": PROJECT_ID,
            "git_sha": head,
            "development_version": development_version,
            "artifact_kind": "managed-service",
            "artifact_path": "bundle",
            "command_path": "bin/podway",
            "controller_path": "libexec/aquarium-dev-service",
            "sha256": tree_digest(bundle),
        }
        external_manifest.write_text(
            json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        return manifest
    except Exception:
        for owned in (build_root, bundle, external_manifest):
            _remove_owned_output(owned)
        raise


def main(arguments: list[str]) -> int:
    try:
        if arguments == ["describe"]:
            result = description()
        elif arguments == ["build"]:
            result = build(os.environ.get("AQUARIUM_DEV_OUTPUT"))
        else:
            raise ProducerError("usage: aquarium_dev_producer.py describe|build")
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0
    except (OSError, ProducerError, UnicodeError) as error:
        print(f"aquarium development producer failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
