"""Tests for the Azure Artifact Signing step of the Windows release wheel.

Signing itself needs Azure credentials and only runs in the release workflow;
these tests cover everything around it: the pinned jsign command line, the
structural signature check that fails the step on an unsigned PE, the RECORD
rewrite that keeps the signed wheel installable, and the workflow wiring that
signs before the wheel is smoke-tested or uploaded. PE images are assembled
with ``struct.pack`` so no binary fixture ships.
"""

from __future__ import annotations

import importlib.util
import struct
import subprocess
import sys
import zipfile
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]

SPEC = importlib.util.spec_from_file_location(
    "sign_windows_release", ROOT / "ci" / "sign_windows_release.py"
)
assert SPEC is not None and SPEC.loader is not None
sign = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = sign
SPEC.loader.exec_module(sign)

PUBLISHER = "Zachary Vorhies"
EXE = "bosn-0.1.4.data/scripts/bosn.exe"
PYD = "bosn/_native.pyd"
RECORD = "bosn-0.1.4.dist-info/RECORD"


def make_pe(cert: bytes | None = None, *, pe32_plus: bool = True) -> bytes:
    """A minimal PE whose security directory points at ``cert`` (if any)."""
    pe_offset = 0x40
    image = bytearray(0x200)
    image[0:2] = b"MZ"
    struct.pack_into("<I", image, 0x3C, pe_offset)
    image[pe_offset : pe_offset + 4] = b"PE\0\0"
    optional = pe_offset + 24
    struct.pack_into("<H", image, optional, 0x20B if pe32_plus else 0x10B)
    if cert is not None:
        directories = optional + (112 if pe32_plus else 96)
        struct.pack_into("<II", image, directories + 4 * 8, len(image), len(cert))
        image += cert
    return bytes(image)


def win_certificate(payload: bytes) -> bytes:
    return struct.pack("<IHH", 8 + len(payload), 0x0200, 0x0002) + payload


SIGNED_PAYLOAD = b"\x30\x82" + PUBLISHER.encode() + sign.RFC3161_TIMESTAMP_OID
SIGNED_PE = make_pe(win_certificate(SIGNED_PAYLOAD))


def make_wheel(path: Path, members: dict[str, bytes]) -> Path:
    record = "".join(
        f"{name},{sign.record_hash(data)},{len(data)}\n" for name, data in members.items()
    )
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in members.items():
            archive.writestr(name, data)
        archive.writestr(RECORD, record + f"{RECORD},,\n")
    return path


def test_unsigned_pe_is_rejected() -> None:
    assert "no Authenticode signature" in (sign.signature_problem(make_pe(), PUBLISHER) or "")


@pytest.mark.parametrize("pe32_plus", [True, False])
def test_timestamped_signature_by_publisher_passes(pe32_plus: bool) -> None:
    image = make_pe(win_certificate(SIGNED_PAYLOAD), pe32_plus=pe32_plus)
    assert sign.signature_problem(image, PUBLISHER) is None


def test_wrong_publisher_and_missing_timestamp_are_rejected() -> None:
    other = make_pe(win_certificate(b"CN=Someone Else" + sign.RFC3161_TIMESTAMP_OID))
    assert "publisher" in (sign.signature_problem(other, PUBLISHER) or "")
    untimed = make_pe(win_certificate(PUBLISHER.encode()))
    assert "timestamp" in (sign.signature_problem(untimed, PUBLISHER) or "")


def test_verify_wheel_fails_on_an_unsigned_pyd(tmp_path: Path) -> None:
    wheel = make_wheel(tmp_path / "w.whl", {EXE: SIGNED_PE, PYD: make_pe()})
    with pytest.raises(sign.SigningError, match="_native.pyd"):
        sign.verify_wheel(wheel, PUBLISHER)


def test_verify_wheel_fails_on_a_stale_record(tmp_path: Path) -> None:
    wheel = make_wheel(tmp_path / "w.whl", {EXE: make_pe(), PYD: make_pe()})
    _infos, members = sign.read_wheel(wheel)
    members[EXE] = SIGNED_PE
    members[PYD] = SIGNED_PE
    sign.write_wheel(wheel, _infos, members)
    with pytest.raises(sign.SigningError, match="RECORD"):
        sign.verify_wheel(wheel, PUBLISHER)


def test_verify_requires_at_least_one_pe(tmp_path: Path) -> None:
    wheel = make_wheel(tmp_path / "w.whl", {"bosn/__init__.py": b""})
    with pytest.raises(sign.SigningError, match="no Windows PE"):
        sign.verify_wheel(wheel, PUBLISHER)


def test_sign_wheel_replaces_pe_bytes_and_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    wheel = make_wheel(
        tmp_path / "w.whl", {"bosn/__init__.py": b"x", EXE: make_pe(), PYD: make_pe()}
    )
    seen: list[list[str]] = []

    def fake_jsign(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        env = kwargs["env"]
        assert isinstance(env, dict) and env[sign.TOKEN_ENV] == "token"
        seen.append(command)
        for arg in command[command.index("--replace") + 1 :]:
            Path(arg).write_bytes(SIGNED_PE)
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(sign, "access_token", lambda: "token")
    monkeypatch.setattr(sign.subprocess, "run", fake_jsign)
    config = sign.SigningConfig("eus.codesigning.azure.net", "acct", "prof", PUBLISHER)
    sign.sign_wheel(Path("jsign.jar"), config, wheel)
    assert len(seen) == 1 and "token" not in seen[0]
    sign.verify_wheel(wheel, PUBLISHER)
    _infos, members = sign.read_wheel(wheel)
    assert list(members) == ["bosn/__init__.py", EXE, PYD, RECORD]


def test_resolve_wheel_requires_one_windows_wheel(tmp_path: Path) -> None:
    with pytest.raises(sign.SigningError, match="exactly one"):
        sign.resolve_wheel(str(tmp_path / "*.whl"))
    linux = tmp_path / "bosn-0.1.4-cp310-abi3-manylinux_2_39_x86_64.whl"
    linux.write_bytes(b"")
    with pytest.raises(sign.SigningError, match="not a Windows wheel"):
        sign.resolve_wheel(str(linux))


def test_endpoint_and_publisher_normalisation() -> None:
    assert sign.endpoint_host("https://eus.codesigning.azure.net/") == "eus.codesigning.azure.net"
    with pytest.raises(sign.SigningError):
        sign.endpoint_host("https://example.com/")
    dn = "CN=Zachary Vorhies, O=Zachary Vorhies, L=Berkeley, C=US"
    assert sign.publisher_common_name(dn) == PUBLISHER
    assert sign.publisher_common_name(PUBLISHER) == PUBLISHER


def test_jsign_command_keeps_the_token_off_argv() -> None:
    config = sign.SigningConfig("eus.codesigning.azure.net", "acct", "prof", PUBLISHER)
    command = sign.jsign_command(Path("jsign.jar"), config, [Path("a.exe")])
    assert command[command.index("--storetype") + 1] == "TRUSTEDSIGNING"
    assert command[command.index("--keystore") + 1] == "eus.codesigning.azure.net"
    assert command[command.index("--alias") + 1] == "acct/prof"
    assert command[command.index("--storepass") + 1] == f"env:{sign.TOKEN_ENV}"
    assert command[command.index("--tsaurl") + 1] == "http://timestamp.acs.microsoft.com"
    assert command[command.index("--tsmode") + 1] == "RFC3161"
    assert command[command.index("--alg") + 1] == "SHA-256"
    assert command[-1] == "a.exe"


def test_release_signs_the_windows_wheel_before_smoke_and_upload() -> None:
    workflow = yaml.safe_load(
        (ROOT / ".github/workflows/auto-release.yml").read_text(encoding="utf-8")
    )
    job = workflow["jobs"]["native-wheel"]
    assert job["permissions"] == {"contents": "read", "id-token": "write"}
    names = [step.get("name", "") for step in job["steps"]]
    login = names.index("Azure login for Artifact Signing")
    signing = names.index("Authenticode-sign the Windows wheel")
    check = names.index("Windows reports a Valid Authenticode signature")
    assert names.index("Build platform wheel") < login < signing < check
    assert check < names.index("Install and smoke-test platform wheel outside the checkout")
    assert check < names.index("Upload wheel")
    for index in (login, signing, check):
        assert job["steps"][index]["if"] == "runner.os == 'Windows'"
    assert job["steps"][login]["with"]["allow-no-subscriptions"] is True
    assert "check-wheel" in job["steps"][check]["run"]
