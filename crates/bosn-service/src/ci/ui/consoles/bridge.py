"""Pinned-TLS RFB transport, executed inside the disposable console container.

stdin/stdout are RFB bytes. Only finite error codes go to stderr, never VNC
credentials or framebuffer data. EOF, inactivity, or the lifetime cap exits the
container. No listening socket and no dependency beyond Python's standard library.
"""

import hashlib
import hmac
import os
import select
import signal
import socket
import ssl
import sys
import time


def relay(host: str, port: int, pin: str, idle: float = 600, lifetime: float = 43200):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    # The operator-provisioned certificate pin is the trust anchor. Compare it
    # before reading or forwarding even the RFB greeting, let alone credentials.
    context.verify_mode = ssl.CERT_NONE
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.set_ciphers("AES256-GCM-SHA384:ECDHE+AESGCM")
    started = last = time.monotonic()
    with socket.create_connection((host, port), timeout=15) as tcp:
        with context.wrap_socket(tcp, server_hostname=host) as remote:
            actual = hashlib.sha256(remote.getpeercert(binary_form=True)).hexdigest()
            if not hmac.compare_digest(actual, pin.lower()):
                raise ValueError("certificate_changed")
            remote.settimeout(15)
            while time.monotonic() - started < lifetime:
                budget = min(idle - (time.monotonic() - last), 1)
                if budget <= 0:
                    raise TimeoutError("console_idle")
                readable, _, _ = select.select(
                    [remote, sys.stdin.buffer], [], [], 0 if remote.pending() else budget
                )
                if remote in readable or remote.pending():
                    data = remote.recv(65536)
                    if not data:
                        return
                    sys.stdout.buffer.write(data)
                    sys.stdout.buffer.flush()
                    last = time.monotonic()
                if sys.stdin.buffer in readable:
                    data = os.read(sys.stdin.fileno(), 65536)
                    if not data:
                        return
                    remote.sendall(data)
                    last = time.monotonic()


def main():
    signal.signal(signal.SIGALRM, lambda *_: sys.exit(0))
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    signal.alarm(43200)
    try:
        relay(sys.argv[1], int(sys.argv[2]), sys.argv[3])
    except ValueError:
        print("certificate_changed", file=sys.stderr, flush=True)
        return 2
    except TimeoutError:
        print("console_timeout", file=sys.stderr, flush=True)
        return 3
    except OSError:
        print("console_unreachable", file=sys.stderr, flush=True)
        return 4
    return 0


if __name__ == "__main__":
    sys.exit(main())
