"""The test wheel's dependency cache matches the artifacts ./install builds."""

import shlex
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[1]


def test_linux_wheel_dependency_cache_matches_backend():
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
    configurations = []
    for job in ("linux", "cache-writer-linux"):
        setup = next(
            step["with"]
            for step in workflow["jobs"][job]["steps"]
            if step.get("uses", "").startswith("forge-ci/setup-soldr@")
        )
        assert setup["prebuild-deps"] == "soldr-cook"
        assert setup["cargo-registry-cache"] is True
        configurations.append(shlex.split(setup["prebuild-deps-flags"]))
    assert (
        configurations
        == [["--profile", "dev", "--package", "bosn-python", "--", "--features", "wheel"]] * 2
    )
