"""Keep the widget-launched dashboard's live log viewer stable."""

import subprocess
import unittest
from pathlib import Path


class CiUiLiveLogTests(unittest.TestCase):
    def test_live_log_dom_and_cursor(self) -> None:
        subprocess.run(
            ["node", "ci/test_ci_ui_live_logs.js"],
            cwd=Path(__file__).resolve().parents[1],
            check=True,
        )
