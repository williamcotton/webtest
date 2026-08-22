#!/usr/bin/env python3

import argparse
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit


LOGIN_PAGE = b"""<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Simple Server Sign In</title>
    <style>
        :root {
            color-scheme: light;
            font-family: system-ui, sans-serif;
            background: #f4f5f7;
            color: #17202a;
        }
        body {
            min-height: 100vh;
            margin: 0;
            display: grid;
            place-items: center;
        }
        main {
            width: min(28rem, calc(100% - 2rem));
        }
        h1 {
            margin: 0 0 1.5rem;
            font-size: 2rem;
            letter-spacing: 0;
        }
        form {
            display: grid;
            gap: 1rem;
        }
        label {
            display: grid;
            gap: 0.4rem;
            font-weight: 600;
        }
        input, button {
            box-sizing: border-box;
            min-height: 2.75rem;
            border: 1px solid #aeb6bf;
            border-radius: 6px;
            font: inherit;
        }
        input {
            width: 100%;
            padding: 0.65rem 0.75rem;
            background: white;
        }
        button {
            padding: 0.65rem 1rem;
            border-color: #17202a;
            background: #17202a;
            color: white;
            cursor: pointer;
            font-weight: 700;
        }
        button:disabled {
            cursor: wait;
            opacity: 0.65;
        }
        [role="status"] {
            min-height: 1.5rem;
            margin: 1rem 0 0;
        }
    </style>
</head>
<body>
    <main>
        <h1>Sign in</h1>
        <form id="sign-in">
            <label>Email <input name="email" type="email" autocomplete="username" required></label>
            <button type="submit">Sign in</button>
        </form>
        <p id="status" role="status"></p>
    </main>
    <script>
        const form = document.getElementById("sign-in");
        const status = document.getElementById("status");
        const button = form.querySelector("button");

        form.addEventListener("submit", async (event) => {
            event.preventDefault();
            button.disabled = true;
            status.textContent = "";

            try {
                const response = await fetch("/api/login", {
                    method: "POST",
                    headers: { "Content-Type": "application/json" },
                    body: JSON.stringify({ email: form.elements.email.value }),
                });
                const result = await response.json();

                if (!response.ok) {
                    status.textContent = result.error;
                    return;
                }

                history.pushState({}, "", "/dashboard");
                status.textContent = result.message;
            } catch (error) {
                status.textContent = "Unable to sign in";
            } finally {
                button.disabled = false;
            }
        });
    </script>
</body>
</html>
"""


class ClientError(Exception):
    def __init__(self, status, message):
        super().__init__(message)
        self.status = status
        self.message = message


class UserStore:
    def __init__(self):
        self._lock = threading.Lock()
        self._next_id = 1
        self._users = {}

    def create(self, email):
        with self._lock:
            user = {"id": self._next_id, "email": email}
            self._next_id += 1
            self._users[email] = user
            return user.copy()

    def find(self, email):
        with self._lock:
            user = self._users.get(email)
            return user.copy() if user else None


class AppServer(ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, address):
        super().__init__(address, AppHandler)
        self.users = UserStore()


class AppHandler(BaseHTTPRequestHandler):
    server_version = "WebTestExample/1.0"

    def do_GET(self):
        path = urlsplit(self.path).path
        if path in ("/", "/login", "/dashboard"):
            self._send(200, "text/html; charset=utf-8", LOGIN_PAGE)
            return
        self._send_json(404, {"error": "not found"})

    def do_POST(self):
        path = urlsplit(self.path).path
        try:
            payload = self._read_json_object()
            email = self._read_email(payload)

            if path == "/api/test/users":
                self._send_json(201, self.server.users.create(email))
                return

            if path == "/api/login":
                user = self.server.users.find(email)
                if user is None:
                    self._send_json(401, {"error": "Unknown user"})
                    return
                self._send_json(200, {"message": "Welcome, " + user["email"]})
                return

            self._send_json(404, {"error": "not found"})
        except ClientError as error:
            self._send_json(error.status, {"error": error.message})

    def _read_json_object(self):
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError as error:
            raise ClientError(400, "invalid content length") from error

        if length <= 0:
            raise ClientError(400, "request body is required")
        if length > 65536:
            raise ClientError(413, "request body is too large")

        try:
            payload = json.loads(self.rfile.read(length))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ClientError(400, "request body must be valid JSON") from error
        if not isinstance(payload, dict):
            raise ClientError(400, "request body must be a JSON object")
        return payload

    @staticmethod
    def _read_email(payload):
        email = payload.get("email")
        if not isinstance(email, str) or "@" not in email:
            raise ClientError(422, "email must be a valid string")
        return email

    def _send_json(self, status, payload):
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self._send(status, "application/json; charset=utf-8", body)

    def _send(self, status, content_type, body):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)


def main():
    parser = argparse.ArgumentParser(description="Run the WebTest simple server example")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", default=3001, type=int)
    args = parser.parse_args()

    server = AppServer((args.host, args.port))
    print("Simple server listening on http://{}:{}".format(args.host, args.port))
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
