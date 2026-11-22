"""The local gate uses the shared CI tool and source workflow checks. Isolation guards
refuse a bare host and the isolated image sets the marker, and every CI job
waits for the verify job."""

from __future__ import annotations

from pathlib import Path

import tomllib
import yaml

ROOT = Path(__file__).resolve().parent.parent

GATE = tomllib.loads((ROOT / "local-gate.toml").read_text(encoding="utf-8"))["gate"]
WORKFLOW = yaml.safe_load((ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8"))


LINT_WORDS = (
    "clippy",
    "cargo fmt",
    "dylint",
    "ruff",
    "pyright",
    "soldr lint",
    "./lint",
    "ci/lint.py",
)


def test_lint_never_runs_under_bosn() -> None:
    """Lint never runs under bosn; soldr lint only.

    The host soldr lanes `lint` and `lint-widget` are the only lint lanes and
    never touch bosn. No bosn task lints, and no workflow job a bosn lane
    replays holds a lint step.
    """
    lint_lanes = {name: lane for name, lane in GATE["lanes"].items() if "lint" in name}
    assert set(lint_lanes) == {"lint", "lint-widget"}
    for name, lane in lint_lanes.items():
        assert lane["run"][:4] == ["soldr", "lint", "rust", "--check"], name
        assert "bosn" not in lane["tools"] and "bosn" not in lane["run"], name
    bosn = tomllib.loads((ROOT / "bosn.toml").read_text(encoding="utf-8"))
    for name, task in bosn["task"].items():
        assert "lint" not in name, name
        assert not any(word in task["cmd"] for word in LINT_WORDS), name
    replayed = {item["job"] for item in GATE["replay"]["selections"]}
    for job in replayed:
        for step in WORKFLOW["jobs"][job]["steps"]:
            run = step.get("run", "")
            assert not any(word in run for word in LINT_WORDS), (job, step.get("name"))
    for name, lane in GATE["lanes"].items():
        if lane["run"][0] == "bosn":
            assert lane["run"][lane["run"].index("--job") + 1] in replayed, name
    assert "lint" not in replayed and "lint" not in {
        job["source-job"].split(":", 1)[1] for job in GATE["replay"]["jobs"]
    }


def test_every_declared_lane_runs_a_source_bound_isolated_replay() -> None:
    lanes = {name: lane for name, lane in GATE["lanes"].items() if lane["run"][0] != "soldr"}
    assert set(lanes) == {"rust", "tests"}
    assert GATE["run"][:3] == ["bosn", "ci", "run"]
    replay = GATE["replay"]
    assert replay["qualified"] and replay["report-source"] == "stdout"
    assert replay["provider-query"] == ["bosn", "ci", "runners", "list", "--json"]
    for name, lane in lanes.items():
        command = lane["run"]
        assert command[:3] == ["bosn", "ci", "run"]
        selection = next(item for item in replay["selections"] if item["lane"] == name)
        assert command[command.index("--job") + 1] == selection["job"]
        assert command[command.index("--event") + 1] == selection["event"]
        assert command[command.index("--input") + 1] == f"tier={selection['inputs']['tier']}"
        assert "--ci-output" in command and "verify:skip_rust" in command
    assert all("steps" not in job and "key" not in job for job in replay["jobs"])
    requested = {
        command[index + 1]
        for lane in lanes.values()
        for command in [lane["run"]]
        for index, token in enumerate(command)
        if token == "--ci-output"
    }
    full = GATE["run"]
    assert requested <= {
        full[index + 1] for index, token in enumerate(full) if token == "--ci-output"
    }


def test_the_remote_lint_job_runs_the_host_lint_lanes() -> None:
    """GATE-001: the remote `lint` job runs each host lint lane's command, and
    the root `soldr lint` extras carry the Python guards and policy lint."""
    lint = WORKFLOW["jobs"]["lint"]
    commands = " ".join(" ".join(step.get("run", "").split()) for step in lint["steps"])
    for name in ("lint", "lint-widget"):
        assert " ".join(GATE["lanes"][name]["run"]) in commands, name
    root = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    extra = [" ".join(cmd) for cmd in root["workspace"]["metadata"]["soldr"]["lint"]["extra"]]
    assert any("ci/lint.py --no-soldr" in cmd for cmd in extra)
    assert any("ci-lint local-gate lint" in cmd for cmd in extra)
    assert not any(cmd.startswith("soldr") for cmd in extra), "an extra may not re-enter soldr"
    linux = [step.get("run", "") for step in WORKFLOW["jobs"]["linux"]["steps"]]
    assert "./test" in linux
    definition = yaml.safe_load((ROOT / "ci-attestations.yml").read_text(encoding="utf-8"))
    assert set(definition["jobs"]["ci.yml:lint"]) == {
        path
        for path, gate in definition["gates"].items()
        if gate["lane"] in ("lint", "lint-widget")
    }


def test_the_isolation_guards_refuse_a_bare_host_and_the_image_lifts_them() -> None:
    marker = GATE["isolation"]["marker"]
    for guard in (ROOT / GATE["isolation"]["guard"], ROOT / "ci" / "test_guard.sh"):
        text = guard.read_text(encoding="utf-8")
        assert marker in text and ('"CI"' in text or "CI:-" in text), guard
    assert f"ENV {marker}=1" in (ROOT / "docker" / "test.Dockerfile").read_text(encoding="utf-8")
    assert 'runner = "ci/test_guard.sh"' in (ROOT / ".cargo" / "config.toml").read_text(
        encoding="utf-8"
    )
    bosn = tomllib.loads((ROOT / "bosn.toml").read_text(encoding="utf-8"))
    for task in ("test", "rust-test"):
        assert bosn["task"][task]["stack"] == "test", task


def test_every_ci_job_waits_for_the_verify_job() -> None:
    jobs = WORKFLOW["jobs"]
    assert "if" not in jobs["verify"], "the verify job always runs (GATE-002)"

    def needs(job: str) -> list[str]:
        value = jobs[job].get("needs", [])
        return [value] if isinstance(value, str) else list(value)

    def reaches_verify(job: str) -> bool:
        return job == "verify" or any(reaches_verify(parent) for parent in needs(job))

    exempt = set(GATE.get("verify-exempt", []))
    assert [job for job in jobs if job not in exempt and not reaches_verify(job)] == []
    assert all("needs" not in jobs[job] for job in exempt), "exempt jobs wait on nothing"


def test_every_trusted_skip_maps_to_attested_gates_and_is_wired() -> None:
    definition = yaml.safe_load((ROOT / "ci-attestations.yml").read_text(encoding="utf-8"))
    lanes = set(GATE["lanes"])
    assert {gate["lane"] for gate in definition["gates"].values()} <= lanes
    skip = GATE["trust"]["skip"]
    assert sorted(definition["jobs"]) == sorted(skip), "skip exactly the mapped jobs"
    for ref in skip:
        gates = definition["jobs"][ref]
        assert gates and set(gates) <= set(definition["gates"]), ref
        covered = set(GATE["trust"]["covered-by"][ref])
        assert {definition["gates"][gate]["lane"] for gate in gates} <= covered, ref
        job = WORKFLOW["jobs"][ref.split(":", 1)[1]]
        assert f"needs.verify.outputs.skip_{ref.split(':', 1)[1]} != 'true'" in job["if"], ref


def test_ci_verifies_with_the_ci_lint_the_gate_runs() -> None:
    pin = next(
        step["with"]["ref"]
        for step in WORKFLOW["jobs"]["verify"]["steps"]
        if step.get("with", {}).get("repository") == "forge-ci/ci.yml"
    )
    checkout = next(
        step
        for step in WORKFLOW["jobs"]["verify"]["steps"]
        if step.get("with", {}).get("repository") == "forge-ci/ci.yml"
    )
    assert checkout["with"]["ref"] == pin
    assert pin in (ROOT / "local-gate.toml").read_text(encoding="utf-8")
    root = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    assert any(
        any(pin in token for token in cmd) and "local-gate" in cmd
        for cmd in root["workspace"]["metadata"]["soldr"]["lint"]["extra"]
    )
