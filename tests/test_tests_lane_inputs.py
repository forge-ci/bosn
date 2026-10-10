"""The tests lane may skip Rust test-only sources (bosn#503, GATE-007).

`local-gate.toml` excludes Rust test sources from the `tests` lane's input set, so
an edit that touches only them reuses the lane's recorded pass instead of
rebuilding the dev wheel. That is safe only while both hold:

1. no excluded file can reach the wheel: each is declared only behind
   `#[cfg(test)]` (or is an integration-test target), never `include!`d; and
2. every check the tests lane runs over those files also runs in the rust lane,
   which keeps the whole tree as its input.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import tomllib
import yaml

ROOT = Path(__file__).resolve().parent.parent
GATE = tomllib.loads((ROOT / "local-gate.toml").read_text(encoding="utf-8"))["gate"]
WORKFLOW = yaml.safe_load((ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8"))

# The checks in the tests lane that read Rust test sources. Each must also run in
# the rust job, so excluding the sources from the tests lane drops nothing.
SHARED_CHECKS = (
    "python3 ci/lint_file_length.py",
    "python3 ci/lint_include_base.py",
    "tests/test_publish_amalgamate.py",
    "tests/test_tests_lane_inputs.py",
)

MOD_DECL = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.MULTILINE)
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
CFG_TEST = re.compile(r"#\[cfg\((?:all\()?test\b")
INCLUDE = re.compile(r'include(?:_str|_bytes)?!\(\s*"([^"]+)"')


def glob_to_regex(pattern: str) -> re.Pattern[str]:
    """ci-lint's GATE-007 glob: `**/` spans directories, `*` stays in one."""
    out = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("^" + "".join(out) + "$")


EXCLUDE = [glob_to_regex(p) for p in GATE["lanes"]["tests"].get("exclude", [])]


def excluded(path: str) -> bool:
    return any(rx.match(path) for rx in EXCLUDE)


def rust_sources() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "crates/**.rs"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout
    return out.split()


def module_candidates(source: Path, name: str, path_attr: str | None) -> list[Path]:
    if path_attr is not None:
        return [(source.parent / path_attr).resolve()]
    owns_dir = source.name in {"lib.rs", "main.rs", "mod.rs"} or source.parent.name == "bin"
    base = source.parent if owns_dir else source.parent / source.stem
    return [(base / f"{name}.rs").resolve(), (base / name / "mod.rs").resolve()]


def test_only_rust_sources_are_excluded() -> None:
    exclude = GATE["lanes"]["tests"]["exclude"]
    assert exclude, "the tests lane declares its Rust test-source exclusion"
    assert all(pattern.startswith("crates/") and pattern.endswith(".rs") for pattern in exclude)


def test_excluded_sources_never_reach_the_wheel() -> None:
    sources = rust_sources()
    hidden = {(ROOT / p).resolve() for p in sources if excluded(p)}
    assert hidden, "the exclusion matches the repository's test sources"
    ungated = []
    for rel in sources:
        if excluded(rel):
            continue
        source = ROOT / rel
        text = source.read_text(encoding="utf-8")
        lines = text.splitlines()
        for match in MOD_DECL.finditer(text):
            line = text.count("\n", 0, match.start())
            attrs = []
            above = line - 1
            while above >= 0 and lines[above].strip().startswith(("#[", "//")):
                attrs.append(lines[above])
                above -= 1
            path_attr = next((m.group(1) for a in attrs for m in [PATH_ATTR.search(a)] if m), None)
            targets = set(module_candidates(source, match.group(1), path_attr)) & hidden
            if targets and not any(CFG_TEST.search(a) for a in attrs):
                ungated.append(f"{rel}:{line + 1} mod {match.group(1)}")
        for target in INCLUDE.findall(text):
            if (source.parent / target).resolve() in hidden:
                ungated.append(f"{rel} includes {target}")
    assert ungated == [], f"test-only sources reachable outside cfg(test): {ungated}"


def test_the_rust_job_runs_every_check_that_reads_excluded_sources() -> None:
    commands = "\n".join(step.get("run", "") for step in WORKFLOW["jobs"]["rust"]["steps"])
    missing = [check for check in SHARED_CHECKS if check not in commands]
    assert missing == [], f"rust job must also run: {missing}"
    rust_lane = GATE["lanes"]["rust"]
    assert "exclude" not in rust_lane, "the rust lane keeps the whole tree as input"
