"""Regression coverage for the #252 hosted-macOS-runner policy."""

from __future__ import annotations

import sys
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ci"))
import lint_no_macos_runners as lint


def test_current_workflows_allow_only_release_macos_runner() -> None:
    root = Path(__file__).resolve().parents[1]
    assert lint.check(root / ".github/workflows") == 0


def test_ci_schedules_no_hosted_mac() -> None:
    # A starved macOS queue cancelled full CI on main (run 41495193520); CI
    # proves Darwin wheels on Linux only, so no ci.yml job may name a Mac.
    root = Path(__file__).resolve().parents[1]
    ci = yaml.safe_load((root / ".github/workflows/ci.yml").read_text())
    assert (
        ci["jobs"]["lint-no-macos-runners"]["name"]
        == "CI policy (hosted macOS: release smoke only)"
    )
    assert "darwin-hosted-smoke" not in ci["jobs"]
    assert list(lint.offenders(ci)) == []


def test_lint_rejects_former_full_ci_mac_job(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    release = yaml.safe_load((root / ".github/workflows/auto-release.yml").read_text())
    job = dict(release["jobs"]["darwin-hosted-smoke"])
    job["if"] = "needs.select-tier.outputs.full == 'true'"
    job["needs"] = ["select-tier", "darwin-cross-wheel"]
    workflow = tmp_path / "ci.yml"
    workflow.write_text(yaml.safe_dump({"jobs": {"darwin-hosted-smoke": job}}))
    assert lint.check(workflow) == 1


def test_release_hosted_job_gates_wheel_collection() -> None:
    root = Path(__file__).resolve().parents[1]
    release = yaml.safe_load((root / ".github/workflows/auto-release.yml").read_text())
    assert "darwin-hosted-smoke" in release["jobs"]["collect"]["needs"]


def test_lint_rejects_runs_on_and_matrix_macos_labels() -> None:
    document = yaml.safe_load(
        """jobs:
  direct:
    runs-on: macos-latest
  indirect:
    runs-on: ${{ matrix.runner }}
    strategy:
      matrix:
        runner: [ubuntu-latest, macos-15-intel]
"""
    )
    assert list(lint.offenders(document)) == [
        ("jobs.direct.runs-on", "macos-latest"),
        ("jobs.indirect.strategy.matrix", "macos-15-intel"),
    ]


def test_lint_ignores_macos_prose_and_conditions() -> None:
    document = yaml.safe_load(
        """jobs:
  cross:
    name: Darwin wheel (not macos-latest)
    runs-on: ubuntu-latest
    if: runner.os != 'macOS'
"""
    )
    assert list(lint.offenders(document)) == []


def test_lint_rejects_hosted_runner_outside_full_or_release(tmp_path: Path) -> None:
    workflow = tmp_path / "ci.yml"
    workflow.write_text("jobs:\n  ordinary:\n    runs-on: macos-15\n")
    assert lint.check(workflow) == 1
    workflow.write_text(
        "jobs:\n  darwin-hosted-smoke:\n    if: true\n"
        "    needs: darwin-cross-wheel\n    runs-on: macos-15\n"
    )
    assert lint.check(workflow) == 1


def test_release_exemption_requires_exact_workflow_path(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    workflow = root / ".github/workflows/auto-release.yml"
    release = yaml.safe_load(workflow.read_text())
    path = "jobs.darwin-hosted-smoke.strategy.matrix"
    assert lint._allowed_job(workflow, release, path)
    for name in ("ci.yml", "auto-release.yml"):
        copied = tmp_path / name
        copied.write_text(yaml.safe_dump(release))
        assert lint.check(copied) == 1
    assert not lint._allowed_job(root / ".github/workflows/ci.yml", release, path)
    release["jobs"]["darwin-hosted-smoke"]["if"] = "true"
    assert not lint._allowed_job(workflow, release, path)
