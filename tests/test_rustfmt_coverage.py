"""Every Cargo manifest in the repository is rustfmt-checked (bosn#418).

`cargo fmt --all` only reaches the root workspace's members, so a crate the
workspace excludes (the desktop widget) needs its own `--manifest-path` check
in both the local gate's `rust` lane and `./lint`.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ci"))
from tracked_files import tracked_files

ROOT = Path(__file__).resolve().parent.parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


local_gate = _load("local_gate", ROOT / "ci" / "local_gate.py")
lint = _load("bosn_lint", ROOT / "ci" / "lint.py")


def manifests() -> list[Path]:
    return sorted(path for path in tracked_files(ROOT) if path.name == "Cargo.toml")


def workspace_members() -> set[str]:
    root = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    return {"Cargo.toml", *(f"{m}/Cargo.toml" for m in root["workspace"]["members"])}


def fmt_checks(commands: list[list[str]]) -> list[list[str]]:
    return [c for c in commands if c[:3] == ["soldr", "cargo", "fmt"] and "--check" in c]


def covered(manifest: str, checks: list[list[str]]) -> bool:
    for check in checks:
        if "--manifest-path" in check:
            if check[check.index("--manifest-path") + 1] == manifest:
                return True
        elif "--all" in check and manifest in workspace_members():
            return True
    return False


def test_manifests_are_found() -> None:
    rels = {m.relative_to(ROOT).as_posix() for m in manifests()}
    assert "Cargo.toml" in rels
    assert "crates/bosn-widget/Cargo.toml" in rels


def _assert_all_covered(commands: list[list[str]], where: str) -> None:
    checks = fmt_checks(commands)
    missing = [
        rel
        for rel in (m.relative_to(ROOT).as_posix() for m in manifests())
        if not covered(rel, checks)
    ]
    assert not missing, f"{where} has no rustfmt check for: {missing}"


def test_local_gate_rust_lane_formats_every_manifest() -> None:
    _assert_all_covered(local_gate.LANES["rust"], "ci/local_gate.py rust lane")


def test_lint_formats_every_manifest() -> None:
    _assert_all_covered(lint.CHECKS, "ci/lint.py (./lint)")


def test_an_uncovered_owned_manifest_is_still_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
    for relative in ("crates/forgotten/Cargo.toml", "_vender/dependency/Cargo.toml"):
        path = tmp_path / relative
        path.parent.mkdir(parents=True)
        path.write_text("")
    monkeypatch.setattr(sys.modules[__name__], "ROOT", tmp_path)
    with pytest.raises(AssertionError, match="crates/forgotten/Cargo.toml") as error:
        _assert_all_covered([["soldr", "cargo", "fmt", "--all", "--check"]], "fixture")
    assert "_vender" not in str(error.value)
