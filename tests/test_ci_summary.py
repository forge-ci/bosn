"""The aggregate `CI summary` gate fails on every unsuccessful non-exempt job."""

import io
import sys
import urllib.error
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "ci"))
import ci_summary as gate  # noqa: E402

CI = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())


def job(name: str, conclusion: str | None = "success", status: str = "completed") -> gate.Job:
    return gate.Job(name=name, status=status, conclusion=conclusion)


@pytest.mark.parametrize("conclusion", sorted(gate.BLOCKING))
def test_every_blocking_conclusion_fails(conclusion: str) -> None:
    jobs = [job("Rust workspace (locked tests)", conclusion), job("Linux (lint + unit + docker)")]
    assert gate.blockers(jobs) == [f"Rust workspace (locked tests): {conclusion}"]


def test_success_skipped_and_neutral_pass() -> None:
    jobs = [
        job("Linux (lint + unit + docker)"),
        job("Hosted macOS wheel smoke (${{ matrix.target }})", "skipped"),
        job("Full CI coverage (exact candidate SHA)", "skipped"),
        job("ci-pre / Publish ci-attestations", "neutral"),
    ]
    assert gate.blockers(jobs) == []


def test_unfinished_job_blocks() -> None:
    assert gate.blockers([job("Verify local gate", None, "in_progress")])


def test_exempt_list_is_narrow_and_matches_real_jobs() -> None:
    for name in gate.EXEMPT:
        assert gate.blockers([job(name, "failure")]) == []
    names = {str(j.get("name", key)) for key, j in CI["jobs"].items()}
    names |= {"ci-pre / Cache janitor"}
    assert gate.EXEMPT <= names
    for validating in ("Verify local gate", "ci-pre / Publish ci-attestations"):
        assert validating not in gate.EXEMPT
    for key in (
        "rust",
        "linux",
        "native-wheel",
        "darwin-cross-wheel",
        "darwin-hosted-smoke",
        "widget",
        "full-coverage",
        "lint-no-macos-runners",
        "verify",
        # Every lane needs it: a failed selector skips them all, and skipped passes.
        "select-tier",
    ):
        assert CI["jobs"][key]["name"] not in gate.EXEMPT, key


def test_job_waits_for_every_job_but_the_janitor() -> None:
    summary = CI["jobs"]["ci-summary"]
    assert summary["name"] == gate.SELF
    assert set(summary["needs"]) == set(CI["jobs"]) - {"ci-summary", "queue-cache-janitor"}
    assert summary["permissions"] == {"contents": "read", "actions": "read"}
    run = summary["steps"][-1]["run"]
    assert "github.run_attempt" in run and run.startswith("python3 ci/ci_summary.py")


def test_rate_limited_call_is_retried() -> None:
    calls: list[str] = []

    def flaky(path: str) -> dict:
        calls.append(path)
        if len(calls) < 3:
            raise urllib.error.HTTPError(path, 403, "rate limited", {}, io.BytesIO())  # type: ignore[arg-type]
        return {"jobs": []}

    assert gate.with_retry(flaky, sleep=lambda _: None)("x") == {"jobs": []}
    assert len(calls) == 3


def test_not_found_is_not_retried() -> None:
    calls: list[str] = []

    def missing(path: str) -> dict:
        calls.append(path)
        raise urllib.error.HTTPError(path, 404, "nope", {}, io.BytesIO())  # type: ignore[arg-type]

    with pytest.raises(urllib.error.HTTPError):
        gate.with_retry(missing, sleep=lambda _: None)("x")
    assert len(calls) == 1


def test_main_reports_blockers(monkeypatch: pytest.MonkeyPatch, capsys) -> None:
    pages = {
        "jobs": [
            {"name": "CI summary", "status": "in_progress", "conclusion": None},
            {
                "name": "Rust workspace (locked tests)",
                "status": "completed",
                "conclusion": "failure",
            },
        ]
    }
    monkeypatch.setattr(gate, "_http_fetch", lambda token: lambda path: pages)
    assert gate.main(["--repo", "o/r", "--run-id", "1", "--run-attempt", "2"]) == 1
    assert "blocked by Rust workspace (locked tests): failure" in capsys.readouterr().out
    pages["jobs"][1]["conclusion"] = "success"
    assert gate.main(["--repo", "o/r", "--run-id", "1", "--run-attempt", "2"]) == 0
