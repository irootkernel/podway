#!/usr/bin/env python3
"""Verify exact Python selection without changing installed interpreters."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="podway-python-", dir="/tmp") as name:
        root = Path(name)
        (root / "tools").mkdir()
        shutil.copy(ROOT / "tools/python", root / "tools")
        shutil.copy(ROOT / "Makefile", root)
        (root / "tools/requirements-dev.txt").write_text("")
        pin = root / ".python-version"
        bin_dir = root / "bin"
        bin_dir.mkdir()
        marker = root / "system-python-used"
        system = bin_dir / "python3"
        system.write_text(f'#!/bin/sh\ntouch "{marker}"\nexit 99\n')
        system.chmod(0o755)
        selected = bin_dir / "selected-python"
        selected.write_text('#!/bin/sh\nif [ "$1" = -I ]; then printf "%s\\n" "$TEST_VERSION"; elif [ "$1" = tools/rust_toolchain.py ]; then printf "1.97.1\\n"; else printf "selected %s\\n" "$TEST_VERSION"; fi\n')
        selected.chmod(0o755)
        uv = bin_dir / "uv"
        uv.write_text('#!/bin/sh\n[ "$*" = "python find --system --no-project --no-config --no-python-downloads --resolve-links $REQUESTED_VERSION" ] || exit 2\n[ "$MISSING_PYTHON" != yes ] || exit 1\nprintf "%s\\n" "$SELECTED_PYTHON"\n')
        uv.chmod(0o755)
        environment = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", "SELECTED_PYTHON": str(selected), "MISSING_PYTHON": "no", "VIRTUAL_ENV": str(root / "unrelated-environment")}
        rustup = bin_dir / "rustup"
        rustup.write_text('#!/bin/sh\nprintf "%s/cargo\\n" "$FIXTURE_BIN"\n')
        rustup.chmod(0o755)
        environment["FIXTURE_BIN"] = str(bin_dir)
        launcher = str(root / "tools/python")
        for version in ("3.14.6", "3.15.2"):
            pin.write_text(version + "\n")
            environment.update(TEST_VERSION=version, REQUESTED_VERSION=version)
            result = subprocess.run([launcher, "--version"], env=environment, capture_output=True, text=True, check=True)
            if result.stdout.strip() != f"selected {version}":
                raise RuntimeError("Python selection did not follow the changed pin")
            recipe = subprocess.check_output(["make", "-s", "-n", "python-prepare"], cwd=root, env=environment, text=True)
            if f'-m venv "target/dev-python/{version}"' not in recipe:
                raise RuntimeError("a Python pin change did not select a fresh environment")
        for version, observed, missing in (("3.14.6", "3.15.2", "no"), ("3.14.6", "3.14.6", "yes"), ("3.14", "3.14", "no"), ("3.14.6\n3.15.2", "3.14.6", "no")):
            pin.write_text(version + "\n")
            environment.update(TEST_VERSION=observed, REQUESTED_VERSION=version, MISSING_PYTHON=missing)
            result = subprocess.run([launcher, "--version"], env=environment, capture_output=True, text=True, check=False)
            if result.returncode == 0 or result.stdout:
                raise RuntimeError("an invalid, unavailable or mismatched Python was accepted")
        pin.unlink()
        result = subprocess.run([launcher, "--version"], env=environment, capture_output=True, text=True, check=False)
        if result.returncode == 0 or marker.exists():
            raise RuntimeError("Python selection fell back to system python3")

        # A stale ready marker must not admit an interpreter with a different version.
        expected = (ROOT / ".python-version").read_text().strip()
        different = "3.15.2" if expected != "3.15.2" else "3.14.6"
        pin.write_text(different + "\n")
        environment.update(TEST_VERSION=different, REQUESTED_VERSION=different, MISSING_PYTHON="no")
        venv = root / "target/dev-python" / different
        (venv / "bin").mkdir(parents=True)
        (venv / "bin/python3").symlink_to(sys.executable)
        (root / "tools/requirements-dev.txt").write_text("")
        (venv / ".requirements-ready").touch()
        refused = subprocess.run(["make", "-s", "python-prepare"], cwd=root, env=environment, capture_output=True, text=True)
        if refused.returncode == 0 or "development Python mismatch" not in refused.stderr:
            raise RuntimeError("a stale development environment was accepted")

        # Real execution from outside the repository still uses this repo's pin.
        expected = (ROOT / ".python-version").read_text().strip()
        result = subprocess.run([str(ROOT / "tools/python"), "-c", "import platform; print(platform.python_version())"], cwd=root, env={**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}"}, capture_output=True, text=True, check=False)
        # The fixture uv intentionally refuses this request. It must not trigger a fallback.
        if result.returncode == 0 or marker.exists():
            raise RuntimeError("failed discovery used system Python")
        actual = subprocess.check_output([str(ROOT / "tools/python"), "-c", "import platform; print(platform.python_version())"], cwd=root, text=True).strip()
        if actual != expected or sys.version_info[:3] != tuple(map(int, expected.split("."))):
            raise RuntimeError("the verifier or selected interpreter differs from the pin")
    print("Python toolchain selection and refusal checks passed")


if __name__ == "__main__":
    main()
