#!/usr/bin/env python3
"""No-SDK protocol-1 stdio fixture used by conformance and integration tests."""

import json
import os
import socket
import sys
import threading
from urllib.parse import urlparse
from pathlib import Path

MAX = 1_048_576
PROJECT_MANIFEST = Path.cwd() / ".webtest" / "app-schema.json"
MANIFEST = json.loads(
    (PROJECT_MANIFEST if PROJECT_MANIFEST.is_file() else Path(__file__).with_name("app-schema.json"))
    .read_text()
)
USERS = []
USERS_LOCK = threading.Lock()
IN_FLIGHT = {}
IN_FLIGHT_LOCK = threading.Lock()
WRITE_LOCK = threading.Lock()
READER = sys.stdin.buffer
WRITER = sys.stdout.buffer


def send(message):
    encoded = json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode()
    if len(encoded) > MAX:
        raise RuntimeError("frame too large")
    with WRITE_LOCK:
        WRITER.write(encoded + b"\n")
        WRITER.flush()


def receive():
    line = READER.readline(MAX + 2)
    if not line:
        return None
    if len(line) > MAX + 1:
        raise RuntimeError("frame too large")
    if not line.endswith(b"\n"):
        raise RuntimeError("truncated bridge frame")
    value = json.loads(line)
    if not isinstance(value, dict):
        raise RuntimeError("message must be an object")
    return value


def terminal_error(identifier, code, message):
    send({"type": "error", "id": identifier, "code": code, "message": message,
          "retryable": False, "data": {}})


def call_worker(message, cancelled):
    identifier = message["id"]
    function = message.get("function")
    arguments = message.get("arguments", {})
    try:
        if function == "create_user":
            email = arguments.get("email")
            with USERS_LOCK:
                if any(user["email"] == email for user in USERS):
                    terminal_error(identifier, "user.email_taken",
                                   "a user with that email already exists")
                    return
                user = {"id": len(USERS) + 1, "email": email,
                        "admin": arguments.get("admin", False)}
                USERS.append(user)
            send({"type": "result", "id": identifier, "value": user})
        elif function and function.startswith("echo_"):
            value = arguments.get("value")
            valid = {
                "echo_null": value is None,
                "echo_boolean": isinstance(value, bool),
                "echo_integer": isinstance(value, int) and not isinstance(value, bool),
                "echo_float": isinstance(value, (int, float)) and not isinstance(value, bool),
                "echo_string": isinstance(value, str),
                "echo_array": isinstance(value, list) and all(isinstance(item, str) for item in value),
                "echo_optional": value is None or isinstance(value, str),
                "echo_object": isinstance(value, dict),
            }.get(function, False)
            if valid:
                send({"type": "result", "id": identifier, "value": value})
            else:
                terminal_error(identifier, "validation.invalid", "argument validation failed")
        elif function == "wait":
            delay_ms = arguments.get("delay_ms", 0)
            deadline_ms = message.get("deadline_ms", delay_ms)
            send({"type": "event", "call_id": identifier, "kind": "progress",
                  "value": {"phase": "waiting"}})
            timed_out = deadline_ms < delay_ms
            interrupted = cancelled.wait(min(delay_ms, deadline_ms) / 1000.0)
            if interrupted:
                terminal_error(identifier, "call.cancelled", "call was cancelled")
            elif timed_out:
                terminal_error(identifier, "call.deadline", "call deadline elapsed")
            else:
                send({"type": "result", "id": identifier, "value": "completed"})
        else:
            terminal_error(identifier, "function.unknown", "unknown function")
    finally:
        with IN_FLIGHT_LOCK:
            IN_FLIGHT.pop(identifier, None)


def dispatch_call(message):
    identifier = message["id"]
    cancelled = threading.Event()
    with IN_FLIGHT_LOCK:
        if identifier in IN_FLIGHT:
            raise RuntimeError(f"duplicate request ID {identifier}")
        thread = threading.Thread(target=call_worker, args=(message, cancelled), daemon=True)
        IN_FLIGHT[identifier] = (thread, cancelled)
        thread.start()


def connect_from_env():
    endpoint = os.environ.get("WEBTEST_BRIDGE", "stdio")
    if endpoint in ("stdio", "stdio:"):
        return None
    if endpoint.startswith("unix:"):
        connection = socket.socket(socket.AF_UNIX)
        connection.connect(endpoint.removeprefix("unix:"))
    elif endpoint.startswith("tcp://"):
        parsed = urlparse(endpoint)
        if parsed.hostname not in ("127.0.0.1", "localhost", "::1"):
            raise RuntimeError("refusing non-loopback bridge endpoint")
        connection = socket.create_connection((parsed.hostname, parsed.port))
    else:
        raise RuntimeError(f"unsupported bridge endpoint {endpoint}")
    return connection


def main():
    global READER, WRITER
    connection = connect_from_env()
    if connection:
        READER = connection.makefile("rb")
        WRITER = connection.makefile("wb")
    advertised = int(os.environ.get("WEBTEST_PROTOCOL", "1"))
    send({
        "type": "hello",
        "protocol_versions": [advertised],
        "sdk": "webtest-no-sdk-reference",
        "sdk_version": "0.1.0",
        "token": os.environ.get("WEBTEST_TOKEN", ""),
        "capabilities": {"cancel": True, "events": True},
    })
    hello = receive()
    if not hello or hello.get("type") != "hello_ok":
        return 2
    while True:
        message = receive()
        if message is None:
            with IN_FLIGHT_LOCK:
                return 4 if IN_FLIGHT else 0
        kind = message.get("type")
        identifier = message.get("id")
        if kind == "describe":
            send({"type": "schema", "id": identifier, "protocol": 1,
                  "schema_hash": MANIFEST["schema_hash"], "functions": MANIFEST["functions"]})
        elif kind == "call":
            dispatch_call(message)
        elif kind == "ping":
            send({"type": "pong", "id": identifier})
        elif kind == "shutdown":
            with IN_FLIGHT_LOCK:
                calls = list(IN_FLIGHT.values())
            for _, cancelled in calls:
                cancelled.set()
            for thread, _ in calls:
                thread.join(1)
            send({"type": "shutdown_ok", "id": identifier})
            return 0
        elif kind == "cancel":
            with IN_FLIGHT_LOCK:
                call = IN_FLIGHT.get(identifier)
            if call:
                call[1].set()
        else:
            return 3


if __name__ == "__main__":
    raise SystemExit(main())
