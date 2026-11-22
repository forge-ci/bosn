#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Bump Bosn's release version, the way forge-ci/zccache and forge-ci/soldr do.

The version is written in one place: `[workspace.package].version` in the root
Cargo.toml. Everything else derives from it:

  - `bosn` (crates.io) and `bosn-python` inherit it (`version.workspace = true`);
  - the PyPI wheel reads it through maturin (`dynamic = ["version"]`);
  - `bosn.__version__` is the loaded extension's own version;
  - uv.lock records the project as dynamic, so it never changes on a bump.

This rewrites that one line and refreshes every lockfile that records the
workspace version -- the root Cargo.lock and each separate workspace's own
(`crates/bosn-widget/Cargo.lock`, which pins `bosn-service` by path) -- so
CI's `--locked` checks pass. Commit both files and open a PR. Merging a
version change to main releases nothing by itself: a release is an explicit
request for the exact merged SHA through the pretag gate (AGENTS.md,
.github/workflows/auto-release.yml, ci/release_gate.py).

Usage:
    ./bump patch    # 0.1.4 -> 0.1.5
    ./bump minor    # 0.1.4 -> 0.2.0
    ./bump major    # 0.1.4 -> 1.0.0
    ./bump 0.3.0    # an exact version, which must be greater
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERSION = re.compile(r'(\[workspace\.package\]\s*\nversion\s*=\s*")([^"]+)(")')
SEMVER = re.compile(r"(\d+)\.(\d+)\.(\d+)")


class BumpError(Exception):
    pass


def read_version(manifest: str) -> str:
    match = VERSION.search(manifest)
    if match is None:
        raise BumpError("Cargo.toml has no [workspace.package] version")
    return match.group(2)


def _parse(version: str) -> tuple[int, int, int]:
    match = SEMVER.fullmatch(version)
    if match is None:
        raise BumpError(f"{version!r} is not MAJOR.MINOR.PATCH")
    major, minor, patch = (int(part) for part in match.groups())
    return major, minor, patch


def next_version(current: str, part: str) -> str:
    major, minor, patch = _parse(current)
    if part == "patch":
        return f"{major}.{minor}.{patch + 1}"
    if part == "minor":
        return f"{major}.{minor + 1}.0"
    if part == "major":
        return f"{major + 1}.0.0"
    if _parse(part) <= (major, minor, patch):
        # PyPI and crates.io never accept a version twice, so this could not release.
        raise BumpError(f"{part} must be greater than {current}")
    return part


def write_version(manifest: str, version: str) -> str:
    rewritten, count = VERSION.subn(rf"\g<1>{version}\g<3>", manifest)
    if count != 1:
        raise BumpError("could not rewrite [workspace.package] version")
    return rewritten


def lockfile_dirs(root: Path) -> list[Path]:
    """Directories whose Cargo.lock must be refreshed after a bump: the root
    workspace, then every separate workspace under `crates/` (one with its own
    Cargo.lock). A path dependency on a bumped member records the new version
    in that lockfile too, and `--locked` refuses a stale one."""
    return [root] + sorted(lock.parent for lock in root.glob("crates/*/Cargo.lock"))


def main(argv: list[str] | None = None, *, root: Path = ROOT, refresh_lock: bool = True) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 1:
        print("usage: ./bump <patch|minor|major|X.Y.Z>", file=sys.stderr)
        return 2
    manifest_path = root / "Cargo.toml"
    manifest = manifest_path.read_text(encoding="utf-8")
    try:
        current = read_version(manifest)
        new = next_version(current, args[0])
    except BumpError as error:
        print(f"bump: {error}", file=sys.stderr)
        return 1

    manifest_path.write_text(write_version(manifest, new), encoding="utf-8")
    if refresh_lock:
        # Rewrites only workspace members' and path dependencies' entries; no
        # registry dependency moves.
        for directory in lockfile_dirs(root):
            subprocess.run(
                ["soldr", "cargo", "update", "--workspace", "--offline"],
                cwd=directory,
                check=True,
            )
    print(f"{current} -> {new}")
    if refresh_lock:
        print(
            "next: commit Cargo.toml and every Cargo.lock, open a PR; once merged, release v"
            + new
            + " through the pretag gate (AGENTS.md)"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
