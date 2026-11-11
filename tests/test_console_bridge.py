"""Exercise real TLS sockets and the actual RFB byte transport, not a mock."""

import hashlib
import select
import socket
import ssl
import subprocess
import sys
import threading
from pathlib import Path

import pytest

BRIDGE = Path(__file__).resolve().parents[1] / "crates/bosn-service/src/ci/ui/consoles/bridge.py"


@pytest.fixture
def certificate(tmp_path):
    key, cert = tmp_path / "key.pem", tmp_path / "cert.pem"
    subprocess.run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            str(key),
            "-out",
            str(cert),
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
        ],
        check=True,
        capture_output=True,
    )
    pin = hashlib.sha256(ssl.PEM_cert_to_DER_cert(cert.read_text())).hexdigest()
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    return context, pin


@pytest.mark.parametrize("correct_pin", [True, False])
def test_transport_checks_certificate_before_forwarding_rfb(certificate, correct_pin):
    context, pin = certificate
    greeting = b"RFB 003.008\n"
    received = []
    errors = []
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(10)

        def server():
            try:
                client, _ = listener.accept()
                with context.wrap_socket(client, server_side=True) as connection:
                    connection.settimeout(10)
                    connection.sendall(greeting)
                    data = connection.recv(65536)
                    received.append(data)
                    if data:
                        connection.sendall(data)
            except (ConnectionResetError, BrokenPipeError, ssl.SSLEOFError) as error:
                if correct_pin:
                    errors.append(error)  # Only a pin refusal may abort TLS here.
            except KeyboardInterrupt:
                raise
            except Exception as error:
                errors.append(error)

        worker = threading.Thread(target=server)
        worker.start()
        process = subprocess.Popen(
            [
                sys.executable,
                str(BRIDGE),
                "127.0.0.1",
                str(listener.getsockname()[1]),
                pin if correct_pin else "00" * 32,
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert process.stdin is not None
        assert process.stdout is not None
        try:
            if correct_pin:
                assert select.select([process.stdout], [], [], 15)[0]
                assert process.stdout.read(len(greeting)) == greeting
                payload = b"\x00\xffRFB-input\n"
                process.stdin.write(payload)
                process.stdin.flush()
                assert select.select([process.stdout], [], [], 15)[0]
                assert process.stdout.read(len(payload)) == payload
            stdout, stderr = process.communicate(
                input=None if correct_pin else b"must-not-cross-untrusted-TLS", timeout=15
            )
            assert stdout == b"", "A rejected certificate must not forward the RFB greeting"
            assert process.returncode == (0 if correct_pin else 2)
            assert stderr == (b"" if correct_pin else b"certificate_changed\n")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            worker.join(timeout=15)
        assert not worker.is_alive()
        assert not errors
        if not correct_pin:
            assert not any(received), "No RFB credentials may cross an untrusted TLS connection"
