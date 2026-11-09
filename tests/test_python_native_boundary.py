"""The installed Python package is a native client boundary, not a second daemon."""

from __future__ import annotations

from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parent.parent
PACKAGE = ROOT / "src" / "bosn"


def test_python_api_reexports_the_native_extension() -> None:
    import bosn
    from bosn import _native

    for name in bosn.__all__:
        if name != "__version__":
            assert getattr(bosn, name) is getattr(_native, name)


def test_python_version_comes_from_the_native_extension() -> None:
    text = (PACKAGE / "__init__.py").read_text(encoding="utf-8")
    assert "__version__: str = native_version()" in text


def test_distribution_has_no_python_runtime_lifecycle_dependencies() -> None:
    project = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    assert project["project"]["dependencies"] == []
    # The `bosn` command is the native binary staged into the wheel's scripts
    # tree by bosn_build_backend, not a Python console-script entry point.
    assert "scripts" not in project["project"]
    assert project["project"]["requires-python"] == ">=3.10"
