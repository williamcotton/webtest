#!/usr/bin/env python3
"""Deterministic Milestone C.5 client using only WebTest's public interface."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from http.server import ThreadingHTTPServer

sys.dont_write_bytecode = True

from server import Handler


REQUIRED_REFERENCES = (
    "browser.open",
    "browser.fill",
    "browser.click",
    "assertion.locator_state",
    "locator.label",
    "locator.role",
)


def write(path: Path, source: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source, encoding="utf-8")


def wrap_example(example: dict) -> str:
    source = example["source"]
    kind = example["source_kind"]
    context = example["enclosing_context"]
    if kind in ("source_file", "declaration_fragment"):
        return source
    if kind == "block_fragment":
        return f'test "example" {{ {source} }}'
    if kind == "locator_fragment":
        return f'test "example" {{ browser {{ click {source} }} }}'
    if kind == "statement_fragment" and context == "scope.browser":
        return f'test "example" {{ browser {{ {source} }} }}'
    if kind == "statement_fragment" and context == "scope.server":
        return f'test "example" {{ server {{ {source} }} }}'
    if kind == "statement_fragment":
        return f'test "example" {{ {source} }}'
    raise AssertionError(f"unsupported example context: {kind}/{context}")


class Client:
    def __init__(self, executable: Path, project: Path, chrome_path: str | None):
        self.executable = executable
        self.project = project
        self.chrome_path = chrome_path

    def run(self, *arguments: str, expected: int = 0) -> dict:
        command = [str(self.executable), *arguments]
        if self.chrome_path and arguments[0] in ("inspect", "test"):
            command.extend(("--chrome-path", self.chrome_path))
        environment = os.environ.copy()
        environment["WEBTEST_CACHE_DIR"] = str(self.project / ".cache")
        completed = subprocess.run(
            command,
            cwd=self.project,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=90,
            check=False,
        )
        assert completed.returncode == expected, (
            f"{' '.join(command)} returned {completed.returncode}, expected {expected}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
        return json.loads(completed.stdout)


def verify(executable: Path, chrome_path: str | None) -> None:
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    port = server.server_address[1]
    try:
        with tempfile.TemporaryDirectory(prefix="webtest-semantic-discovery-") as directory:
            project = Path(directory)
            write(
                project / "webtest.toml",
                f"""[project]
name = "semantic-discovery-acceptance"
test_roots = ["tests"]

[browser]
base_url = "http://127.0.0.1:{port}"
test_id_attribute = "data-testid"

[inspection]
max_elements = 100
max_candidates_per_element = 4
max_text_bytes = 256
include_hidden = false

[redaction]
headers = ["authorization", "cookie", "set-cookie"]
json_fields = ["password", "token", "secret"]
query_params = ["token", "code", "key"]
""",
            )
            client = Client(executable, project, chrome_path)

            index = client.run("describe", "--reporter", "json")
            assert index["description_schema_version"] == 1
            indexed = {
                item
                for children in index["categories"].values()
                for item in children
            }
            assert set(REQUIRED_REFERENCES).issubset(indexed)
            assert "provider.http" in indexed

            for number, reference in enumerate(REQUIRED_REFERENCES):
                description = client.run(
                    "describe", reference, "--reporter", "json"
                )
                assert description["id"] == reference
                assert description["syntax_forms"]
                assert len(description["examples"]) >= 2
                assert description["allowed_contexts"]
                assert description["provenance"]["content_trust"] == "installed"
                for example_number, example in enumerate(description["examples"]):
                    example_path = (
                        project / "examples" / f"{number}-{example_number}.webtest"
                    )
                    write(example_path, wrap_example(example))
                    checked = client.run(
                        "check", str(example_path), "--reporter", "json"
                    )
                    assert checked["exit_class"] == "success"

            search = client.run(
                "describe",
                "--search",
                "activate button pointer",
                "--reporter",
                "json",
            )
            assert search["results"][0]["id"] == "browser.click"

            illegal = project / "tests" / "illegal.webtest"
            write(
                illegal,
                'test "illegal" { server { click role("button", name: "Sign in") } }',
            )
            illegal_report = client.run(
                "check", str(illegal), "--reporter", "json", expected=1
            )
            illegal_diagnostics = illegal_report["files"][0]["diagnostics"]
            assert any(
                "browser.click" in diagnostic.get("reference_queries", [])
                for diagnostic in illegal_diagnostics
            )

            inspection = client.run(
                "inspect", "/login?token=must-not-leak", "--reporter", "json"
            )
            assert inspection["inspection_schema_version"] == 1
            assert "must-not-leak" not in json.dumps(inspection)
            elements = inspection["elements"]
            preferred = [item["preferred_locator"]["source"] for item in elements]
            assert 'label("Email")' in preferred
            assert 'label("Password")' in preferred
            assert 'role("button", name: "Sign in")' in preferred
            assert all(not source.startswith(("css(", "xpath(")) for source in preferred)

            emitted = project / "tests" / "emitted.webtest"
            assertions = "\n".join(
                f"        expect {source}.visible" for source in preferred
            )
            write(
                emitted,
                f'''test "emitted locators resolve" {{
    browser {{
        open "/login"
{assertions}
    }}
}}
''',
            )
            client.run("check", str(emitted), "--reporter", "json")
            client.run("test", str(emitted), "--reporter", "json")

            wrong = project / "tests" / "repair.webtest"
            wrong_source = '''test "repair" {
    browser {
        open "/login"
        click role("button", name: "Log in")
    }
}
'''
            write(wrong, wrong_source)
            failure = client.run(
                "test", str(wrong), "--reporter", "json", expected=1
            )
            structured = failure["files"][0]["tests"][0]["failure"]
            assert structured["code"] == "runtime.locator_not_found"
            candidates = [
                hint["replacement"]["source"]
                for hint in structured["repair_hints"]
                if hint["kind"] == "locator_candidate"
            ]
            replacement = 'role("button", name: "Sign in")'
            assert replacement in candidates

            corrected = wrong_source.replace(
                'role("button", name: "Log in")', replacement
            )
            write(wrong, corrected)
            passed = client.run("test", str(wrong), "--reporter", "json")
            assert passed["exit_class"] == "success"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("webtest", type=Path)
    parser.add_argument("--chrome-path")
    arguments = parser.parse_args()
    verify(arguments.webtest.resolve(), arguments.chrome_path)
    print("semantic-discovery acceptance passed")


if __name__ == "__main__":
    main()
