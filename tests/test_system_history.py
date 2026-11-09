"""Browser contracts for persistent /system history (#821)."""

import subprocess
import unittest
from pathlib import Path


class SystemHistoryTests(unittest.TestCase):
    def test_history_charts_and_refresh(self) -> None:
        subprocess.run(
            ["node", "ci/test_system_history.js"],
            cwd=Path(__file__).resolve().parents[1],
            check=True,
        )
