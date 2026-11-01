#!/usr/bin/env python3
"""The one required check: fail unless every job of this run attempt passed.

It reads the run attempt's whole job list from the GitHub API instead of
`needs.*.result`, because a reusable-workflow job can end `cancelled` while its
caller reports success. A job blocks the gate when its conclusion is in
BLOCKING, or when it has not completed. Skipped jobs pass, exactly as a skipped
required check passed branch protection before this gate existed.

EXEMPT is matched by exact name and lists only jobs that validate nothing:
this job itself, queue bookkeeping, timing reports, cache janitors and
build-only cache writers. A job with real validation is never exempt.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from typing import Any

SELF = "CI summary"
EXEMPT = frozenset(
    {
        SELF,
        "Cancel superseded queue runs",
        "CI queue timing",
        "End-of-queue cache janitor",
        "ci-pre / Cache janitor",
        "Cache writer (base zccache family)",
        "Cache writer (linux-gate suffix)",
    }
)
BLOCKING = frozenset({"failure", "cancelled", "timed_out", "startup_failure", "action_required"})
ATTEMPTS = 5
RETRYABLE_HTTP = frozenset({403, 429, 500, 502, 503, 504})


@dataclass(frozen=True)
class Job:
    name: str
    status: str
    conclusion: str | None

    @classmethod
    def parse(cls, raw: dict[str, Any]) -> Job:
        conclusion = raw["conclusion"]
        return cls(name=str(raw["name"]), status=str(raw["status"]), conclusion=conclusion)


def blockers(jobs: Iterable[Job]) -> list[str]:
    """Return `name: reason` for every non-exempt job that blocks the gate."""
    found = []
    for job in jobs:
        if job.name in EXEMPT:
            continue
        if job.status != "completed":
            found.append(f"{job.name}: {job.status} (not completed)")
        elif job.conclusion in BLOCKING:
            found.append(f"{job.name}: {job.conclusion}")
    return found


Fetch = Callable[[str], Any]


def _http_fetch(token: str | None) -> Fetch:
    def fetch(path: str) -> Any:
        request = urllib.request.Request(
            f"https://api.github.com/{path}",
            headers={
                "Accept": "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28",
                **({"Authorization": f"Bearer {token}"} if token else {}),
            },
        )
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 - fixed host
            return json.load(response)

    return fetch


def with_retry(fetch: Fetch, sleep: Callable[[float], None] = time.sleep) -> Fetch:
    """Retry rate limits, server errors and network faults with backoff."""

    def retried(path: str) -> Any:
        for attempt in range(1, ATTEMPTS + 1):
            try:
                return fetch(path)
            except urllib.error.HTTPError as error:
                if error.code not in RETRYABLE_HTTP or attempt == ATTEMPTS:
                    raise
                print(f"CI summary: GET {path} -> HTTP {error.code}; retry {attempt}")
            except (urllib.error.URLError, TimeoutError) as error:
                if attempt == ATTEMPTS:
                    raise
                print(f"CI summary: GET {path} -> {error}; retry {attempt}")
            sleep(min(60.0, 5.0 * 2 ** (attempt - 1)))
        raise AssertionError("unreachable")

    return retried


def fetch_jobs(fetch: Fetch, repo: str, run_id: int, attempt: int) -> list[Job]:
    jobs: list[Job] = []
    page = 1
    while True:
        document = fetch(
            f"repos/{repo}/actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100&page={page}"
        )
        batch = [Job.parse(raw) for raw in document["jobs"]]
        jobs.extend(batch)
        if len(batch) < 100:
            return jobs
        page += 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Fail unless every non-exempt job of this run attempt passed."
    )
    parser.add_argument("--repo", required=True)
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--run-attempt", type=int, required=True)
    args = parser.parse_args(argv)
    fetch = with_retry(_http_fetch(os.environ.get("GITHUB_TOKEN")))
    jobs = fetch_jobs(fetch, args.repo, args.run_id, args.run_attempt)
    for job in jobs:
        mark = "exempt" if job.name in EXEMPT else (job.conclusion or job.status)
        print(f"  {mark:>15}  {job.name}")
    bad = blockers(jobs)
    if not any(job.name == SELF for job in jobs):
        bad.append(f"{SELF}: missing from its own job list (wrong run or attempt)")
    if bad:
        for item in bad:
            print(f"::error::CI summary blocked by {item}")
        return 1
    print(f"CI summary: {len(jobs)} jobs, none blocking")
    return 0


if __name__ == "__main__":
    sys.exit(main())
