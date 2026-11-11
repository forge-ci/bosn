"""The console's shipped control scripts must handle idle CI and late closes."""

import subprocess


def test_console_controls() -> None:
    result = subprocess.run(
        ["node", "ci/test_console_controls.js"],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert "stale events ignored; idle server refresh passed" in result.stdout
