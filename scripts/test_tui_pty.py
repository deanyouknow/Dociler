#!/usr/bin/env python3
"""Drive Dociler's Unix TUI through a PTY against a loopback mock provider."""

from __future__ import annotations

import argparse
import errno
import fcntl
import http.server
import json
import os
import pathlib
import pty
import re
import select
import signal
import socket
import struct
import tempfile
import termios
import threading
import time


ANSI_CSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
ANSI_OSC = re.compile(r"\x1b\][^\x07]*(?:\x07|\x1b\\)")


def visible_output(buffer: bytearray) -> str:
    output = buffer.decode("utf-8", "replace")
    return ANSI_CSI.sub("", ANSI_OSC.sub("", output))


class MockProvider(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), MockHandler)
        self.lock = threading.Lock()
        self.requests: list[dict[str, object]] = []
        self.stream_count = 0
        self.verification_count = 0
        self.verified = threading.Event()
        self.first_sent = threading.Event()
        self.partial_sent = threading.Event()
        self.cancelled = threading.Event()


class MockHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    @property
    def provider(self) -> MockProvider:
        return self.server  # type: ignore[return-value]

    def log_message(self, _format: str, *_args: object) -> None:
        pass

    def send_bytes(self, content_type: str, body: bytes) -> None:
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if self.path != "/v1/models":
            self.send_error(404)
            return
        self.send_bytes(
            "application/json", json.dumps({"data": [{"id": "pty-model"}]}).encode()
        )

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", "0"))
        request = json.loads(self.rfile.read(length))
        with self.provider.lock:
            self.provider.requests.append(request)
        if not request.get("stream"):
            with self.provider.lock:
                self.provider.verification_count += 1
            self.send_bytes(
                "application/json",
                json.dumps(
                    {"choices": [{"message": {"content": "verification-ok"}}]}
                ).encode(),
            )
            self.provider.verified.set()
            return

        with self.provider.lock:
            self.provider.stream_count += 1
            stream_number = self.provider.stream_count
        if stream_number == 1:
            body = (
                b'data: {"choices":[{"delta":{"content":"PTY-FIRST-OK"}}]}\n\n'
                b"data: [DONE]\n\n"
            )
            self.send_bytes("text/event-stream", body)
            self.provider.first_sent.set()
            return

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(
            b'data: {"choices":[{"delta":{"content":"PTY-CANCEL-PARTIAL"}}]}\n\n'
        )
        self.wfile.flush()
        self.provider.partial_sent.set()
        self.connection.settimeout(0.1)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                if self.connection.recv(1, socket.MSG_PEEK) == b"":
                    self.provider.cancelled.set()
                    return
            except TimeoutError:
                continue
            except OSError:
                self.provider.cancelled.set()
                return


class PtySession:
    def __init__(self, executable: pathlib.Path, config_dir: pathlib.Path) -> None:
        self.pid, self.fd = pty.fork()
        self.buffer = bytearray()
        self.search_from = 0
        self.reaped = False
        if self.pid == 0:
            environment = os.environ.copy()
            environment["DOCILER_CONFIG_DIR"] = str(config_dir)
            os.execve(str(executable), [str(executable)], environment)
        self.resize(24, 100)

    def resize(self, rows: int, columns: int) -> None:
        fcntl.ioctl(
            self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0)
        )
        if self.pid:
            os.kill(self.pid, signal.SIGWINCH)

    def send_line(self, value: str) -> None:
        os.write(self.fd, value.encode() + b"\r")

    def send_escape(self) -> None:
        os.write(self.fd, b"\x1b")

    def read_once(self, timeout: float) -> None:
        ready, _, _ = select.select([self.fd], [], [], timeout)
        if not ready:
            return
        try:
            chunk = os.read(self.fd, 65_536)
        except OSError as error:
            if error.errno == errno.EIO:
                return
            raise
        self.buffer.extend(chunk)
        if len(self.buffer) > 4 * 1024 * 1024:
            raise AssertionError("PTY output exceeded 4 MiB")

    def wait_for(self, text: str, timeout: float = 8.0) -> None:
        deadline = time.monotonic() + timeout
        needle = "".join(text.split())
        while time.monotonic() < deadline:
            output = "".join(visible_output(self.buffer).split())
            position = output.find(needle, self.search_from)
            if position >= 0:
                self.search_from = position + len(needle)
                return
            self.read_once(min(0.1, deadline - time.monotonic()))
        tail = visible_output(self.buffer)[-2_000:]
        raise AssertionError(f"timed out waiting for {text!r}; PTY tail: {tail!r}")

    def drain_for(self, duration: float) -> None:
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            self.read_once(min(0.05, deadline - time.monotonic()))

    def wait_for_exit(self, timeout: float = 8.0) -> int:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.read_once(0.05)
            waited, status = os.waitpid(self.pid, os.WNOHANG)
            if waited == self.pid:
                self.reaped = True
                return os.waitstatus_to_exitcode(status)
        raise AssertionError("Dociler did not exit and restore the terminal")

    def close(self) -> None:
        if not self.reaped:
            try:
                os.kill(self.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                os.waitpid(self.pid, 0)
            except ChildProcessError:
                pass
        os.close(self.fd)


def assert_conversation(provider: MockProvider) -> None:
    with provider.lock:
        streams = [request for request in provider.requests if request.get("stream")]
    if len(streams) != 2:
        raise AssertionError(f"expected two streamed turns, got {len(streams)}")
    second_messages = streams[1].get("messages")
    serialized = json.dumps(second_messages, ensure_ascii=False)
    for expected in ("first PTY prompt", "PTY-FIRST-OK", "second PTY prompt"):
        if expected not in serialized:
            raise AssertionError(f"second turn omitted prior context: {expected}")


def wait_for_file(path: pathlib.Path, timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.is_file():
            return
        time.sleep(0.02)
    raise AssertionError(f"timed out waiting for {path}")


def wait_for_verifications(
    provider: MockProvider, expected: int, timeout: float = 5.0
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        with provider.lock:
            if provider.verification_count >= expected:
                return
        time.sleep(0.02)
    raise AssertionError(f"timed out waiting for {expected} profile verifications")


def run(executable: pathlib.Path) -> None:
    if os.name != "posix":
        raise SystemExit("PTY integration requires a POSIX host")
    executable = executable.resolve()
    if not executable.is_file():
        raise SystemExit(f"Dociler executable not found: {executable}")

    provider = MockProvider()
    server_thread = threading.Thread(target=provider.serve_forever, daemon=True)
    server_thread.start()
    session: PtySession | None = None
    try:
        with tempfile.TemporaryDirectory(prefix="dociler-pty-") as temporary:
            root = pathlib.Path(temporary)
            session = PtySession(executable, root / "config")
            session.wait_for("Profilename")
            session.send_line("pty-profile")
            session.drain_for(0.25)
            session.send_line(f"http://127.0.0.1:{provider.server_port}/v1")
            session.drain_for(0.25)
            session.send_line("pty-model")
            session.drain_for(0.25)
            session.send_line("")
            if not provider.verified.wait(5):
                raise AssertionError("onboarding verification did not reach the mock provider")
            config_path = root / "config" / "config.json"
            wait_for_file(config_path)
            session.drain_for(1.0)

            session.resize(30, 120)
            session.drain_for(0.2)
            session.send_line("first PTY prompt")
            if not provider.first_sent.wait(5):
                raise AssertionError("first chat turn did not reach the mock provider")
            session.drain_for(0.5)
            session.send_line("/connect check pty-profile")
            wait_for_verifications(provider, 2)
            session.drain_for(0.5)
            session.send_line("second PTY prompt")
            if not provider.partial_sent.wait(5):
                raise AssertionError("second chat turn did not reach the mock provider")
            session.drain_for(0.2)
            session.send_escape()
            if not provider.cancelled.wait(2):
                raise AssertionError("mock provider did not observe cancellation disconnect")
            session.drain_for(0.5)

            session.send_line("/exit")
            if session.wait_for_exit() != 0:
                raise AssertionError("Dociler exited unsuccessfully")
            if b"\x1b[?1049h" not in session.buffer or b"\x1b[?1049l" not in session.buffer:
                raise AssertionError("alternate-screen enter/leave sequence was incomplete")

            output = visible_output(session.buffer)
            for expected in ("PTY-FIRST-OK", "PTY-CANCEL-PARTIAL"):
                if expected not in output:
                    raise AssertionError(f"terminal output omitted {expected}")

            config = config_path.read_text()
            if "pty-profile" not in config:
                raise AssertionError("verified profile was not persisted")
            for private_text in ("first PTY prompt", "second PTY prompt", "PTY-FIRST-OK"):
                if private_text in config:
                    raise AssertionError("conversation content leaked into configuration")
            assert_conversation(provider)
    finally:
        if session is not None:
            session.close()
        provider.shutdown()
        provider.server_close()
        server_thread.join(timeout=2)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("executable", type=pathlib.Path)
    arguments = parser.parse_args()
    run(arguments.executable)
    print(
        "Passed: TUI PTY onboarding, health check, multi-turn chat, cancellation, resize, and cleanup."
    )


if __name__ == "__main__":
    main()
