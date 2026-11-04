"""The excluded widget workspace must resolve its complete locked graph."""

from __future__ import annotations

import runpy
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COMMAND = [
    "soldr",
    "cargo",
    "metadata",
    "--locked",
    "--format-version",
    "1",
    "--manifest-path",
    "crates/bosn-widget/Cargo.toml",
]


def test_both_lint_drivers_require_full_widget_resolution() -> None:
    lint = runpy.run_path(str(ROOT / "ci/lint.py"))
    gate = runpy.run_path(str(ROOT / "ci/local_gate.py"))
    assert COMMAND in lint["CHECKS"]
    assert COMMAND in gate["LANES"]["rust"]
    # --no-deps returns success on stale locks; it must never weaken this guard.
    assert "--no-deps" not in COMMAND


def resolve(root: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(COMMAND + list(extra), cwd=root, capture_output=True, text=True)


def test_current_widget_lock_resolves() -> None:
    result = resolve(ROOT)
    assert result.returncode == 0, result.stderr


def test_stale_widget_lock_requires_full_resolution(tmp_path: Path) -> None:
    # Model the widget's path dependency changing without refreshing its lock.
    widget = tmp_path / "crates/bosn-widget"
    core = tmp_path / "crates/bosn-core"
    for crate in (widget, core):
        (crate / "src").mkdir(parents=True)
        (crate / "src/lib.rs").write_text("", encoding="utf-8")
    manifest = core / "Cargo.toml"
    manifest.write_text('[package]\nname="bosn-core"\nversion="0.1.0"\n', encoding="utf-8")
    (widget / "Cargo.toml").write_text(
        '[package]\nname="bosn-widget"\nversion="0.1.0"\n'
        '[dependencies]\nbosn-core={path="../bosn-core"}\n',
        encoding="utf-8",
    )
    generated = subprocess.run(
        ["soldr", "cargo", "generate-lockfile", "--manifest-path", str(widget / "Cargo.toml")],
        capture_output=True,
        text=True,
    )
    assert generated.returncode == 0, generated.stderr
    manifest.write_text('[package]\nname="bosn-core"\nversion="0.2.0"\n', encoding="utf-8")
    lock = widget / "Cargo.lock"
    stale = lock.read_bytes()
    shallow = resolve(tmp_path, "--no-deps")
    assert shallow.returncode == 0, shallow.stderr
    result = resolve(tmp_path)
    assert result.returncode != 0
    assert "--locked" in result.stderr, result.stderr
    assert lock.read_bytes() == stale
