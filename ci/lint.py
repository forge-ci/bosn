"""Lint driver for bosn. Invoked by ./lint via `uv run python ci/lint.py`."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# rustfmt: the workspace, and every crate it excludes (tests/test_rustfmt_coverage.py).
# `--no-rust` drops these: the host lint lane runs them through `soldr lint`.
# `--no-soldr` drops every soldr command (these and the widget resolution, which
# the `lint-widget` lane's `--locked` Clippy proves): it is a `soldr lint`
# extra, and an extra may not re-enter soldr.
RUST_FMT_CHECKS: list[list[str]] = [
    ["soldr", "cargo", "fmt", "--all", "--check"],
    ["soldr", "cargo", "fmt", "--check", "--manifest-path", "crates/bosn-widget/Cargo.toml"],
]

CHECKS: list[list[str]] = [
    ["ruff", "format", "--check", "."],
    ["ruff", "check", "."],
    ["pyright"],
    *RUST_FMT_CHECKS,
    # Full resolution is essential: --no-deps accepts a stale widget lockfile.
    [
        "soldr",
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--manifest-path",
        "crates/bosn-widget/Cargo.toml",
    ],
    # Ctrl-C correctness: ruff's BLE001 flags blind excepts but cannot see the *missing*
    # KeyboardInterrupt sibling handler, which is the actual defect.
    [sys.executable, str(ROOT / "ci" / "lint_kbi.py")],
    [sys.executable, str(ROOT / "ci" / "lint_no_macos_runners.py")],
    [sys.executable, str(ROOT / "ci" / "lint_no_vcpkg_bootstrap.py")],
    # The CLI/daemon/wheels never link a webview toolkit (the widget builds alone).
    [sys.executable, str(ROOT / "ci" / "lint_no_webview_in_core.py")],
    # Hard gate: every Rust/Python/shell source file stays under 1,000 lines.
    [sys.executable, str(ROOT / "ci" / "lint_file_length.py")],
    # DRY: an embedded file's directory is written once per file (one base macro).
    [sys.executable, str(ROOT / "ci" / "lint_include_base.py")],
]


def run(cmd: list[str]) -> int:
    print(f"\n>>> {' '.join(cmd)}", flush=True)
    return subprocess.run(cmd, cwd=ROOT).returncode


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else list(argv)
    checks = CHECKS
    if "--no-rust" in argv:
        argv.remove("--no-rust")
        checks = [check for check in CHECKS if check not in RUST_FMT_CHECKS]
    if "--no-soldr" in argv:
        argv.remove("--no-soldr")
        checks = [check for check in checks if check[0] != "soldr"]
    failed: list[str] = []
    for check in checks:
        cmd = check + argv if argv else check
        if run(cmd) != 0:
            failed.append(" ".join(Path(part).name for part in cmd[:2]))
    if failed:
        print(f"\nLINT FAILED: {', '.join(failed)}", file=sys.stderr)
        return 1
    print("\nLINT OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
