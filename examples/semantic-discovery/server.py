#!/usr/bin/env python3
"""Deterministic manual fixture for semantic inspection and repair."""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


LOGIN = b"""<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Sign in</title></head>
<body>
  <main>
    <h1>Sign in</h1>
    <form id="login">
      <label for="email">Email</label>
      <input id="email" name="email" type="email" autocomplete="username">
      <label for="password">Password</label>
      <input id="password" name="password" type="password" autocomplete="current-password">
      <button type="submit" data-testid="login-submit">Sign in</button>
      <p id="error" role="alert" hidden>Invalid email or password</p>
    </form>
  </main>
  <script>
    document.getElementById('login').addEventListener('submit', event => {
      event.preventDefault();
      const email = document.getElementById('email').value;
      const password = document.getElementById('password').value;
      if (email === 'alice@example.com' && password === 'secret') {
        location.href = '/dashboard';
      } else {
        document.getElementById('error').hidden = false;
      }
    });
  </script>
</body>
</html>"""

DASHBOARD = b"""<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Dashboard</title></head>
<body><main><h1>Welcome, Alice</h1><a href="/login">Sign out</a></main></body>
</html>"""


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.split("?", 1)[0] in ("/", "/login"):
            self.respond(200, LOGIN)
        elif self.path.split("?", 1)[0] == "/dashboard":
            self.respond(200, DASHBOARD)
        else:
            self.respond(404, b"not found", "text/plain; charset=utf-8")

    def respond(self, status, body, content_type="text/html; charset=utf-8"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=3010)
    args = parser.parse_args()
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()

