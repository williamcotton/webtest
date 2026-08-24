#!/usr/bin/env python3
"""Transport-independent black-box protocol-1 conformance harness."""

import argparse
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import sys
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]
MAX = 1_048_576
STDERR_MAX = 65_536
EXPECTED_MANIFEST = json.loads(Path(__file__).with_name("app-schema.json").read_text())


class Peer:
    def __init__(self, command, token="conformance-token", protocol="1", transport="stdio"):
        self.transport = transport
        self.temporary = None
        self.listener = None
        self.connection = None
        environment = dict(os.environ, WEBTEST_TOKEN=token, WEBTEST_PROTOCOL=protocol)
        if transport == "stdio":
            environment["WEBTEST_BRIDGE"] = "stdio"
            stdin = subprocess.PIPE
        elif transport == "unix":
            self.temporary = tempfile.TemporaryDirectory(prefix="webtest-conformance-")
            endpoint = str(Path(self.temporary.name) / "bridge.sock")
            self.listener = socket.socket(socket.AF_UNIX)
            self.listener.bind(endpoint)
            self.listener.listen(1)
            self.listener.settimeout(3)
            environment["WEBTEST_BRIDGE"] = f"unix:{endpoint}"
            stdin = subprocess.DEVNULL
        elif transport == "tcp":
            self.listener = socket.socket()
            self.listener.bind(("127.0.0.1", 0))
            self.listener.listen(1)
            self.listener.settimeout(3)
            host, port = self.listener.getsockname()
            environment["WEBTEST_BRIDGE"] = f"tcp://{host}:{port}"
            stdin = subprocess.DEVNULL
        else:
            raise AssertionError(f"unknown transport {transport}")

        self.process = subprocess.Popen(
            command, stdin=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment
        )
        self.stderr = bytearray()
        self.extra_stdout = bytearray()
        self.stderr_thread = threading.Thread(
            target=self._drain_bounded, args=(self.process.stderr, self.stderr, STDERR_MAX), daemon=True
        )
        self.stderr_thread.start()
        if transport == "stdio":
            self.reader = self.process.stdout
            self.writer = self.process.stdin
            self.stdout_thread = None
        else:
            self.stdout_thread = threading.Thread(
                target=self._drain_bounded,
                args=(self.process.stdout, self.extra_stdout, STDERR_MAX),
                daemon=True,
            )
            self.stdout_thread.start()
            self.connection, _ = self.listener.accept()
            self.reader = self.connection.makefile("rb")
            self.writer = self.connection.makefile("wb")
        self.frames = queue.Queue()
        self.reader_thread = threading.Thread(target=self._read_frames, daemon=True)
        self.reader_thread.start()

    @staticmethod
    def _drain_bounded(stream, target, maximum):
        while True:
            chunk = stream.read(8_192)
            if not chunk:
                return
            remaining = maximum + 1 - len(target)
            if remaining > 0:
                target.extend(chunk[:remaining])

    def _read_frames(self):
        while True:
            line = self.reader.readline(MAX + 2)
            self.frames.put(line or None)
            if not line:
                return

    def receive(self):
        try:
            line = self.frames.get(timeout=3)
        except queue.Empty as error:
            raise AssertionError("bridge did not produce a frame before the deadline") from error
        assert line and len(line) <= MAX + 1, "missing or oversized bridge frame"
        value = json.loads(line)
        assert isinstance(value, dict), "bridge frame is not an object"
        return value

    def send(self, value):
        encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()
        assert len(encoded) <= MAX
        self.send_raw(encoded + b"\n")

    def send_raw(self, value):
        self.writer.write(value)
        self.writer.flush()

    def close_input(self):
        try:
            self.writer.close()
        except (BrokenPipeError, OSError):
            pass
        if self.connection:
            try:
                self.connection.shutdown(socket.SHUT_WR)
            except OSError:
                pass

    def wait(self, expected=None):
        try:
            return_code = self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=2)
            raise AssertionError("bridge did not exit before the deadline")
        self.stderr_thread.join(1)
        if self.stdout_thread:
            self.stdout_thread.join(1)
        assert len(self.stderr) <= STDERR_MAX, "stderr was not bounded"
        assert b"conformance-token" not in self.stderr, "authentication token leaked to stderr"
        if self.transport != "stdio":
            assert not self.extra_stdout, "socket bridge wrote logs or protocol bytes to stdout"
        if expected is not None:
            assert return_code == expected, f"bridge exited {return_code}, expected {expected}"
        if self.listener:
            self.listener.close()
        if self.connection:
            self.connection.close()
        if self.temporary:
            self.temporary.cleanup()
        return return_code

    def close(self, expected=0):
        self.close_input()
        return self.wait(expected)


def ready(command, *, token="conformance-token", protocol="1", transport="stdio"):
    peer = Peer(command, token=token, protocol=protocol, transport=transport)
    hello = peer.receive()
    assert hello["type"] == "hello" and int(protocol) in hello["protocol_versions"]
    assert hello["token"] == token
    assert hello["sdk"] and hello["sdk_version"]
    assert hello["capabilities"]["cancel"] and hello["capabilities"]["events"]
    peer.send({
        "type": "hello_ok", "protocol": int(protocol), "run_id": "conformance",
        "max_message_bytes": MAX, "unknown_optional": True,
    })
    peer.send({"type": "describe", "id": 1, "unknown_optional": True})
    schema = peer.receive()
    assert schema["type"] == "schema" and schema["id"] == 1
    assert schema["schema_hash"] == EXPECTED_MANIFEST["schema_hash"]
    assert schema["functions"] == EXPECTED_MANIFEST["functions"]
    return peer, schema


def call(peer, identifier, function, value, deadline=1_000):
    peer.send({
        "type": "call", "id": identifier, "function": function,
        "arguments": value, "deadline_ms": deadline, "unknown_optional": True,
    })
    return receive_terminal(peer, identifier)


def receive_terminal(peer, identifier):
    while True:
        message = peer.receive()
        if message["type"] == "event":
            assert message == {
                "type": "event", "call_id": identifier, "kind": "progress",
                "value": {"phase": "waiting"},
            }
            continue
        assert message["id"] == identifier
        return message


def expect_protocol_failure(command, payload, name):
    peer, _ = ready(command)
    peer.send_raw(payload)
    peer.close_input()
    return_code = peer.wait()
    assert return_code != 0, f"{name} did not terminate the bridge"
    assert b"conformance-token" not in peer.stderr


def expect_message_failure(command, messages, name):
    peer, _ = ready(command)
    for message in messages:
        peer.send(message)
    peer.close_input()
    return_code = peer.wait()
    assert return_code != 0, f"{name} did not terminate the bridge"


def transport_smoke(command, transport):
    if transport == "unix" and not hasattr(socket, "AF_UNIX"):
        return
    peer, _ = ready(command, transport=transport)
    peer.send({"type": "ping", "id": 90})
    assert peer.receive() == {"type": "pong", "id": 90}
    peer.send({"type": "shutdown", "id": 91})
    assert peer.receive() == {"type": "shutdown_ok", "id": 91}
    peer.close()


def run(command):
    peer, first_schema = ready(command)
    peer.send({"type": "describe", "id": 2})
    second_schema = peer.receive()
    assert first_schema["functions"] == second_schema["functions"]
    assert first_schema["schema_hash"] == second_schema["schema_hash"]
    operation = first_schema["functions"]["create_user"]
    assert operation["documentation"] and operation["retry_safe"] is False
    assert operation["params"]["fields"]["admin"]["optional"] is True
    assert operation["params"]["fields"]["admin"]["default"] is False
    assert operation["params"]["fields"]["email"]["secret"] is False

    values = [
        ("echo_null", None),
        ("echo_boolean", True),
        ("echo_integer", 42),
        ("echo_float", 3.5),
        ("echo_string", "héllo\n世界"),
        ("echo_array", ["a", "β"]),
        ("echo_optional", None),
        ("echo_optional", "present"),
        ("echo_object", {
            "name": "nested", "tags": ["one", "two"],
            "metadata": {"active": True, "score": 9.5},
        }),
    ]
    for identifier, (function, value) in enumerate(values, start=10):
        result = call(peer, identifier, function, {"value": value})
        assert result == {"type": "result", "id": identifier, "value": value}

    defaulted = call(peer, 30, "create_user", {"email": "álîçé@example.com"})
    assert defaulted == {
        "type": "result", "id": 30,
        "value": {"id": 1, "email": "álîçé@example.com", "admin": False},
    }
    duplicate_user = call(peer, 31, "create_user", {"email": "álîçé@example.com"})
    assert duplicate_user["type"] == "error"
    assert duplicate_user["code"] == "user.email_taken" and not duplicate_user["retryable"]
    invalid = call(peer, 32, "echo_integer", {"value": "not an integer"})
    assert invalid["type"] == "error" and invalid["id"] == 32
    deep = "leaf"
    for _ in range(40):
        deep = [deep]
    invalid_deep = call(peer, 33, "echo_array", {"value": deep})
    assert invalid_deep["type"] == "error" and invalid_deep["id"] == 33

    peer.send({"type": "call", "id": 40, "function": "wait",
               "arguments": {"delay_ms": 80}, "deadline_ms": 500})
    peer.send({"type": "call", "id": 41, "function": "wait",
               "arguments": {"delay_ms": 5}, "deadline_ms": 500})
    terminals = []
    while len(terminals) < 2:
        message = peer.receive()
        if message["type"] == "event":
            assert message["call_id"] in {40, 41}
            assert message["value"] == {"phase": "waiting"}
        else:
            terminals.append(message)
    first, second = terminals
    assert [first["id"], second["id"]] == [41, 40], "results were not correlated out of order"

    peer.send({"type": "call", "id": 42, "function": "wait",
               "arguments": {"delay_ms": 1_000}, "deadline_ms": 2_000})
    peer.send({"type": "cancel", "id": 42, "reason": "test_timeout"})
    cancelled = receive_terminal(peer, 42)
    assert cancelled["type"] == "error" and cancelled["id"] == 42

    deadline = call(peer, 43, "wait", {"delay_ms": 200}, deadline=10)
    assert deadline["type"] == "error" and deadline["id"] == 43
    peer.send({"type": "ping", "id": 50})
    assert peer.receive() == {"type": "pong", "id": 50}
    peer.send({"type": "shutdown", "id": 51})
    assert peer.receive() == {"type": "shutdown_ok", "id": 51}
    peer.close()

    bad_auth = Peer(command, token="wrong-token")
    assert bad_auth.receive()["token"] == "wrong-token"
    bad_auth.send({"type": "hello_error", "code": "authentication_failed", "message": "rejected"})
    bad_auth.close_input()
    assert bad_auth.wait() != 0

    bad_version = Peer(command, protocol="2")
    assert bad_version.receive()["type"] == "hello"
    bad_version.send({"type": "hello_error", "code": "unsupported_protocol", "message": "no overlap"})
    bad_version.close_input()
    assert bad_version.wait() != 0

    expect_protocol_failure(command, b"{not json}\n", "malformed JSON")
    expect_protocol_failure(command, b"\xff\n", "invalid UTF-8")
    expect_protocol_failure(command, b"{" + b" " * (MAX + 1) + b"\n", "oversized frame")
    expect_protocol_failure(command, b'{"type":"ping","id":1}', "truncated frame")
    expect_message_failure(command, [{"type": "future_message", "optional": True}], "unknown message")
    expect_message_failure(command, [{"type": "result", "id": 999, "value": None}], "unknown response ID")
    expect_message_failure(command, [
        {"type": "call", "id": 70, "function": "wait", "arguments": {"delay_ms": 1_000}, "deadline_ms": 2_000},
        {"type": "call", "id": 70, "function": "wait", "arguments": {"delay_ms": 1_000}, "deadline_ms": 2_000},
    ], "duplicate request ID")

    abrupt, _ = ready(command)
    abrupt.send({"type": "call", "id": 80, "function": "wait",
                 "arguments": {"delay_ms": 1_000}, "deadline_ms": 2_000})
    abrupt.close_input()
    assert abrupt.wait() != 0, "abrupt EOF with a pending call was accepted"

    transport_smoke(command, "unix")
    transport_smoke(command, "tcp")
    print("protocol conformance passed:", " ".join(command))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command or [sys.executable, str(Path(__file__).with_name("reference_bridge.py"))]
    run(command)


if __name__ == "__main__":
    main()
