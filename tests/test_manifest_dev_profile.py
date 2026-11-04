"""Manifest edit loops reuse the wrappers' dev-wheel cache key."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

if sys.version_info >= (3, 11):
    import tomllib
else:
    import tomli as tomllib

ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.skipif(os.name != "posix", reason="the manifest test stack runs POSIX sh")
@pytest.mark.parametrize("task", ["test", "lint"])
@pytest.mark.parametrize("profile", [None, "release"])
def test_manifest_uv_commands_share_dev_profile_default(
    tmp_path: Path, task: str, profile: str | None
) -> None:
    command = tomllib.loads((ROOT / "bosn.toml").read_text())["task"][task]["cmd"]
    (tmp_path / "ci").mkdir()
    shutil.copyfile(ROOT / "ci/dev_profile.sh", tmp_path / "ci/dev_profile.sh")
    binaries = tmp_path / "bin"
    binaries.mkdir()
    for name, body in {
        "soldr": "exit 0\n",
        "uv": 'printf "%s\\n" "${BOSN_WHEEL_PROFILE-unset}" >> "$PROFILE_LOG"\n',
    }.items():
        executable = binaries / name
        executable.write_text(f"#!/bin/sh\n{body}")
        executable.chmod(0o755)
    log = tmp_path / "profiles"
    environment = os.environ.copy()
    environment.pop("BOSN_WHEEL_PROFILE", None)
    if profile is not None:
        environment["BOSN_WHEEL_PROFILE"] = profile
    environment["PATH"] = str(binaries) + os.pathsep + environment["PATH"]
    environment["PROFILE_LOG"] = str(log)
    subprocess.run(
        ["sh", "-eu", "-c", command],
        cwd=tmp_path,
        env=environment,
        check=True,
        capture_output=True,
        text=True,
        timeout=5,
    )
    # Both wheel installation and the following uv run need the same cache key.
    assert log.read_text().splitlines() == [profile or "dev"] * 2
