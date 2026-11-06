#!/usr/bin/env python3
"""Select the CI tier from the current event payload, including label edits."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path

# The widget is its own Cargo workspace (excluded from the root one), so no
# routine lane builds or tests it. A PR or main push touching it selects the
# WebKitGTK `widget` job by path (AGENTS.md, "CI and release gate").
WIDGET_PATHS = ("crates/bosn-widget/",)


def select(
    event_name: str,
    payload: dict,
    manual_tier: str = "minimal",
    manual_sha: str = "",
) -> str:
    if event_name == "workflow_dispatch":
        if manual_tier not in {"minimal", "test", "full"}:
            raise ValueError(f"invalid manual CI tier: {manual_tier}")
        if manual_tier == "full" and not re.fullmatch(r"[0-9a-fA-F]{40}", manual_sha):
            raise ValueError("full manual CI requires a 40-character commit_sha")
        return manual_tier
    # Main pushes and the nightly schedule run the expensive tier, so it is
    # reached without a hand dispatch; PRs and the merge queue stay cheap.
    if event_name in {"push", "schedule"}:
        return "full"
    if event_name == "pull_request":
        labels = {label["name"] for label in payload["pull_request"].get("labels", [])}
        if "ci-full" in labels:
            return "full"
        if "ci-test" in labels:
            return "test"
    return "minimal"


def diff_base(event_name: str, payload: dict) -> str:
    """The commit a PR or main push is compared against; empty when none."""
    if event_name == "pull_request":
        return payload["pull_request"]["base"]["sha"]
    if event_name == "merge_group":
        return payload["merge_group"]["base_sha"]
    if event_name == "push":
        before = payload.get("before", "")
        return "" if not before or set(before) == {"0"} else before
    return ""


def touches_widget(changed: list[str]) -> bool:
    return any(path.startswith(WIDGET_PATHS) for path in changed)


def changed_files(base: str) -> list[str] | None:
    """Paths changed since `base`; None when the diff cannot be computed."""
    diff = subprocess.run(
        ["git", "diff", "--name-only", f"{base}...HEAD"],
        check=False,
        capture_output=True,
        text=True,
    )
    if diff.returncode != 0:
        return None
    return diff.stdout.splitlines()


def selects_widget(base: str) -> bool:
    """An unknown diff runs the widget lane: skipping could pass an untested change."""
    changed = changed_files(base)
    if changed is None:
        print(f"::notice::cannot diff against {base}; selecting the widget lane")
        return True
    return touches_widget(changed)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--event", required=True)
    parser.add_argument("--payload", type=Path, required=True)
    parser.add_argument("--manual-tier", default="minimal")
    parser.add_argument("--manual-sha", default="")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    payload = json.loads(args.payload.read_text())
    tier = select(args.event, payload, args.manual_tier, args.manual_sha)
    base = diff_base(args.event, payload)
    widget = tier == "full" or (bool(base) and selects_widget(base))
    with args.output.open("a") as output:
        output.write(f"tier={tier}\n")
        output.write(f"test={'true' if tier in {'test', 'full'} else 'false'}\n")
        output.write(f"full={'true' if tier == 'full' else 'false'}\n")
        output.write(f"widget={'true' if widget else 'false'}\n")
    print(f"Bosn CI tier: {tier}; widget lane: {widget}")


if __name__ == "__main__":
    main()
