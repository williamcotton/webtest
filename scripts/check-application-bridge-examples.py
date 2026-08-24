#!/usr/bin/env python3
"""Verify and optionally smoke-test the language-neutral application example matrix."""

import argparse
import json
from pathlib import Path
import queue
import shutil
import socket
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
MATRIX = {
    "node": ("node", "3101"), "ruby": ("ruby", "3102"), "go": ("go", "3103"),
    "python": ("python3", "3104"), "elixir": ("elixir", "3105"), "java": ("java", "3106"),
    "dotnet": ("dotnet", "3107"), "rust": ("cargo", "3108"), "php": ("php", "3109"),
}
BASE = ROOT / "examples" / "application-bridge"


def selected(name):
    return [name] if name else list(MATRIX)


def verify(names):
    expected_source = (BASE / "created-user.webtest").read_bytes()
    expected_functions = None
    expected_hash = None
    hash_command = ["cargo", "run", "-q", "-p", "webtest-app-bridge", "--example", "schema-hash", "--"]
    for name in names:
        directory = BASE / name
        assert (directory / "created-user.webtest").read_bytes() == expected_source, f"{name}: scenario differs"
        manifest_path = directory / ".webtest" / "app-schema.json"
        manifest = json.loads(manifest_path.read_text())
        expected_functions = expected_functions or manifest["functions"]
        expected_hash = expected_hash or manifest["schema_hash"]
        assert manifest["functions"] == expected_functions, f"{name}: operation schema differs"
        assert manifest["schema_hash"] == expected_hash, f"{name}: schema hash differs"
        computed = subprocess.check_output(hash_command + [str(manifest_path)], cwd=ROOT, text=True).strip()
        assert computed == manifest["schema_hash"], f"{name}: schema regeneration is dirty"
        with tempfile.TemporaryDirectory(prefix=f".schema-{name}-", dir=BASE) as temporary:
            regenerated = Path(temporary) / "app-schema.json"
            shutil.copy2(manifest_path, regenerated)
            subprocess.run(hash_command + ["--write", str(regenerated)], cwd=ROOT, check=True,
                           stdout=subprocess.DEVNULL)
            assert regenerated.read_bytes() == manifest_path.read_bytes(), (
                f"{name}: deterministic schema export is not current"
            )
        subprocess.run([str(ROOT / "target/debug/webtest"), "check", str(directory)], cwd=ROOT, check=True)
    print(f"application bridge matrix verified: {', '.join(names)}")


def free_port():
    with socket.socket() as candidate:
        candidate.bind(("127.0.0.1", 0))
        return candidate.getsockname()[1]


def assert_port_released(port):
    deadline = time.monotonic() + 2
    while True:
        try:
            with socket.socket() as probe:
                probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                probe.bind(("127.0.0.1", port))
            return
        except OSError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.05)


def prepare_project(name, configured_port, port, temporary):
    project = Path(temporary)
    shutil.copytree(
        BASE / name,
        project,
        dirs_exist_ok=True,
        ignore=shutil.ignore_patterns("node_modules", "target", "bin", "obj", "__pycache__"),
    )
    config = (project / "webtest.toml").read_text().replace(configured_port, str(port))
    (project / "webtest.toml").write_text(config)
    return project


def run_test(project, expected):
    result = subprocess.run(
        [str(ROOT / "target/debug/webtest"), "test", str(project), "--reporter", "human"],
        cwd=ROOT,
        check=False,
    )
    if result.returncode != expected:
        for artifact in sorted((project / ".webtest" / "artifacts").glob("*.dom.html")):
            print(f"unexpected DOM artifact {artifact}:\n{artifact.read_text(errors='replace')[:4096]}")
    assert result.returncode == expected, f"test exited {result.returncode}, expected {expected}"


def send_dap(process, sequence, command, arguments=None):
    body = json.dumps({
        "seq": sequence, "type": "request", "command": command, "arguments": arguments or {},
    }, separators=(",", ":")).encode()
    process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    process.stdin.flush()


def dap_cleanup(project):
    process = subprocess.Popen(
        [str(ROOT / "target/debug/webtest"), "dap", "--headless"],
        cwd=project,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    messages = queue.Queue()

    def read_messages():
        try:
            while True:
                header = process.stdout.readline()
                if not header:
                    return
                assert header.lower().startswith(b"content-length:")
                length = int(header.split(b":", 1)[1])
                assert process.stdout.readline() == b"\r\n"
                messages.put(json.loads(process.stdout.read(length)))
        except BaseException as error:
            messages.put(error)

    threading.Thread(target=read_messages, daemon=True).start()
    program = str((project / "created-user.webtest").resolve())
    send_dap(process, 1, "initialize", {"adapterID": "webtest"})
    send_dap(process, 2, "launch", {"program": program})
    send_dap(process, 3, "setBreakpoints", {
        "source": {"path": program}, "breakpoints": [{"line": 6}],
    })
    send_dap(process, 4, "configurationDone")
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            message = messages.get(timeout=0.25)
        except queue.Empty:
            if process.poll() is not None:
                break
            continue
        if isinstance(message, BaseException):
            raise message
        if message.get("type") == "event" and message.get("event") == "stopped":
            send_dap(process, 5, "disconnect")
            process.wait(timeout=5)
            assert process.returncode == 0, process.stderr.read().decode(errors="replace")
            return
    process.kill()
    process.wait(timeout=2)
    raise AssertionError(
        "DAP did not pause after the app call: " + process.stderr.read().decode(errors="replace")
    )


def cleanup_cases(name, configured_port):
    # Test failure cleanup.
    port = free_port()
    with tempfile.TemporaryDirectory(prefix=f".failure-{name}-", dir=BASE) as temporary:
        project = prepare_project(name, configured_port, port, temporary)
        source = (project / "created-user.webtest").read_text().replace(
            "Welcome, alice@example.com", "This text must never exist"
        )
        (project / "created-user.webtest").write_text(source)
        run_test(project, 1)
    assert_port_released(port)

    # Whole-test timeout cleanup after application startup.
    port = free_port()
    with tempfile.TemporaryDirectory(prefix=f".timeout-{name}-", dir=BASE) as temporary:
        project = prepare_project(name, configured_port, port, temporary)
        server = project / "server.ts"
        original = server.read_text()
        source = original.replace(
            '  ({ email, admin = false }: { email: string; admin?: boolean }): User => {',
            '  async ({ email, admin = false }: { email: string; admin?: boolean }): Promise<User> => {\n'
            '    await new Promise((resolve) => setTimeout(resolve, 1000));',
        )
        assert source != original, "Node timeout fixture did not inject the delayed bridge call"
        server.write_text(source)
        with (project / "webtest.toml").open("a") as config:
            config.write(
                '\n[timeouts]\nbrowser_command = "250ms"\naction = "250ms"\n'
                'assertion = "250ms"\nnavigation = "250ms"\ntest = "250ms"\n'
            )
        run_test(project, 3)
    assert_port_released(port)

    # DAP disconnect cleanup after create_user has run and before the first browser step.
    port = free_port()
    with tempfile.TemporaryDirectory(prefix=f".dap-{name}-", dir=BASE) as temporary:
        project = prepare_project(name, configured_port, port, temporary)
        dap_cleanup(project)
    assert_port_released(port)
    print(f"{name}: failure, timeout, and DAP cleanup cases passed")


def smoke(name, require_toolchain, run_cleanup_cases):
    tool, configured_port = MATRIX[name]
    if shutil.which(tool) is None:
        message = f"SKIP {name}: `{tool}` is not installed"
        if require_toolchain:
            raise SystemExit(message)
        print(message)
        return
    port = free_port()
    with tempfile.TemporaryDirectory(prefix=f".smoke-{name}-", dir=BASE) as temporary:
        project = prepare_project(name, configured_port, port, temporary)
        run_test(project, 0)
    assert_port_released(port)
    print(f"{name}: smoke passed and port {port} was released")
    if run_cleanup_cases:
        cleanup_cases(name, configured_port)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--example", choices=MATRIX)
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--require-toolchain", action="store_true")
    parser.add_argument(
        "--cleanup-cases", action="store_true",
        help="also prove owned-process cleanup after failure, timeout, and DAP disconnect",
    )
    args = parser.parse_args()
    if args.cleanup_cases and args.example != "node":
        parser.error("--cleanup-cases currently uses the Node lifecycle fixture")
    names = selected(args.example)
    verify(names)
    if args.smoke:
        for name in names:
            smoke(name, args.require_toolchain, args.cleanup_cases)


if __name__ == "__main__":
    main()
