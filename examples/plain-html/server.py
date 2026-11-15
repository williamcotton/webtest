#!/usr/bin/env python3
"""Serve the static fixture, including WebTest's isolated worker ports."""

from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path


if __name__ == "__main__":
    port = int(os.environ.get("WEBTEST_APP_PORT", "3000"))
    handler = partial(SimpleHTTPRequestHandler, directory=str(Path(__file__).parent))
    with ThreadingHTTPServer(("127.0.0.1", port), handler) as server:
        server.serve_forever()
