"""Execute production recovery SQL under a VM-work budget, not a timing guess."""

import json
import re
import sqlite3
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "crates/bosn-registry/src"


def rust_string(path: Path, name: str, raw: bool = False) -> str:
    text = path.read_text(encoding="utf-8")
    pattern = rf'const {name}: &str = r#"(.*?)"#;' if raw else rf'const {name}: &str = "(.*?)";'
    match = re.search(pattern, text, flags=re.DOTALL)
    assert match, (path, name)
    return match[1] if raw else json.loads('"' + match[1] + '"')


@pytest.mark.parametrize("history", [10, 100, 1000])
def test_active_recovery_page_work_is_independent_of_snapshot_history(history: int) -> None:
    with sqlite3.connect(":memory:") as connection:
        connection.executescript(rust_string(REGISTRY / "lib.rs", "SCHEMA", raw=True))
        connection.executescript(rust_string(REGISTRY / "ci_state.rs", "SCHEMA", raw=True))
        detail = json.dumps({"state": "cleanup_required"})
        for run in range(100):
            connection.executemany(
                "INSERT INTO events(at,kind,detail) VALUES(0,?,?)",
                [(f"act.engine.v1:{run:036d}", detail)] * history,
            )
        connection.executemany(
            "INSERT INTO ci_current(family,id,detail,pending,tools_pending,"
            "created_at,updated_at,collect_after,collectible) "
            "VALUES(1,?,?,1,0,0,0,0,0)",
            [(f"{run:036d}", detail) for run in range(100)],
        )
        steps = 0

        def budget() -> int:
            nonlocal steps
            steps += 1000
            return int(steps > 100_000)

        connection.set_progress_handler(budget, 1000)
        query = rust_string(REGISTRY / "act/recovery_queue.rs", "ACTIVE")
        try:
            rows = connection.execute(query, ("", 65)).fetchall()
        except sqlite3.OperationalError as error:
            pytest.fail(
                f"bounded recovery page exceeded 100,000 VM steps: {error}; history={history}"
            )
        assert len(rows) == 65
