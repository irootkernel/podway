#!/usr/bin/env python3
"""Exercise toolchain propagation and refusal without installing or upgrading Rust."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from unittest import mock

import qualify_development
import release_archive
import rust_toolchain


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="podway-toolchain-", dir="/tmp") as name:
        root = Path(name)
        manifest = root / "rust-toolchain.toml"
        for contents in (None, "[toolchain", '[toolchain]\nchannel = "stable"', '[toolchain]\nchannel = "1.97"', '[toolchain]\nchannel = 123', '[toolchain]\npath = "/other"'):
            if contents is not None:
                manifest.write_text(contents)
            try:
                rust_toolchain.pinned_version(root)
            except rust_toolchain.ToolchainError:
                pass
            else:
                raise RuntimeError("an absent or invalid exact pin was accepted")

        (root / "tools").mkdir()
        shutil.copy(rust_toolchain.ROOT / "Makefile", root / "Makefile")
        shutil.copy(rust_toolchain.ROOT / "tools/rust_toolchain.py", root / "tools")
        shutil.copy(rust_toolchain.ROOT / "tools/python", root / "tools")
        shutil.copy(rust_toolchain.ROOT / ".python-version", root)
        bin_dir = root / "bin"
        bin_dir.mkdir()
        rustup = bin_dir / "rustup"
        rustup.write_text('#!/bin/sh\n[ "$1 $2 $3 $4" = "which --toolchain $EXPECTED_RUST cargo" ] || exit 1\n[ "$MISSING_RUST" != yes ] || exit 1\nprintf "%s/cargo\\n" "$FIXTURE_BIN"\n')
        rustup.chmod(0o755)
        for tool in ("cargo", "rustc"):
            path = bin_dir / tool
            path.write_text(f'#!/bin/sh\nprintf "{tool} %s (fixture)\\n" "$EXPECTED_RUST"\n')
            path.chmod(0o755)
        for version in ("1.97.1", "1.99.1"):
            manifest.write_text(f'[toolchain]\nchannel = "{version}"\n')
            environment = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", "EXPECTED_RUST": version, "FIXTURE_BIN": str(bin_dir), "MISSING_RUST": "no"}
            result = subprocess.run(["make", "-s", "toolchain"], cwd=root, env=environment, capture_output=True, text=True, check=True)
            if result.stdout.splitlines() != ["Python " + (root / ".python-version").read_text().strip(), f"cargo {version} (fixture)", f"rustc {version} (fixture)"]:
                raise RuntimeError("Make did not propagate the fixture pin")
            environment["MISSING_RUST"] = "yes"
            result = subprocess.run(["make", "-s", "toolchain"], cwd=root, env=environment, capture_output=True, text=True, check=False)
            if result.returncode == 0 or f"Rust {version} is required" not in result.stderr:
                raise RuntimeError("Make did not refuse an unavailable pinned toolchain")
            with mock.patch.object(release_archive.repository_toolchain, "pinned_version", return_value=version):
                with mock.patch.object(release_archive, "run", return_value=f"rustc {version} (fixture)".encode()):
                    release_archive.rust_toolchain()
                with mock.patch.object(release_archive, "run", return_value=b"rustc 0.0.0 (wrong)"):
                    try:
                        release_archive.rust_toolchain()
                    except release_archive.ReleaseError:
                        pass
                    else:
                        raise RuntimeError("release packaging accepted a different compiler")
        if qualify_development.STEPS[-1][:4] != ("rustup", "run", rust_toolchain.pinned_version(), "cargo"):
            raise RuntimeError("development qualification does not use the repository pin")
    print("Rust toolchain propagation and refusal checks passed")


if __name__ == "__main__":
    main()
