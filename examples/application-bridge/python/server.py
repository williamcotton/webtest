#!/usr/bin/env python3
import http.server, json, os, socket, threading
from pathlib import Path
from urllib.parse import parse_qs, urlparse

if os.environ.get("WEBTEST") != "1": raise RuntimeError("this example bridge is test-only")
USERS = {}
MANIFEST = json.loads((Path(__file__).parent / ".webtest/app-schema.json").read_text())

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def respond(self, body, kind="text/html"):
        self.send_response(200); self.send_header("content-type", kind); self.end_headers(); self.wfile.write(body.encode())
    def do_GET(self):
        if self.path == "/health": self.respond("ok", "text/plain")
        elif self.path == "/login": self.respond('<form method="post"><label>Email <input name="email"></label><button>Sign in</button></form>')
        else: self.send_error(404)
    def do_POST(self):
        if self.path != "/login": return self.send_error(404)
        body = self.rfile.read(int(self.headers.get("content-length", 0))).decode()
        email = parse_qs(body).get("email", [""])[0]
        self.respond(f"<p>Welcome, {email}</p>" if email in USERS else "<p>Invalid sign in</p>")

def send(stream, value):
    stream.write((json.dumps(value, separators=(",", ":")) + "\n").encode()); stream.flush()

def bridge():
    endpoint = os.environ["WEBTEST_BRIDGE"]; token = os.environ["WEBTEST_TOKEN"]
    if endpoint.startswith("tcp://"):
        parsed = urlparse(endpoint); sock = socket.create_connection((parsed.hostname, parsed.port))
    elif endpoint.startswith("unix:"): sock = socket.socket(socket.AF_UNIX); sock.connect(endpoint[5:])
    else: raise RuntimeError("unsupported endpoint")
    stream = sock.makefile("rwb")
    send(stream, {"type":"hello","protocol_versions":[1],"sdk":"webtest-python-example","sdk_version":"0.1.0","token":token,"capabilities":{"cancel":False,"events":False}})
    if json.loads(stream.readline()).get("type") != "hello_ok": return
    for line in stream:
        message = json.loads(line); kind = message.get("type"); identifier = message.get("id")
        if kind == "describe": send(stream, {"type":"schema","id":identifier,"protocol":1,"schema_hash":MANIFEST["schema_hash"],"functions":MANIFEST["functions"]})
        elif kind == "call":
            args = message["arguments"]; email = args["email"]
            if email in USERS: send(stream, {"type":"error","id":identifier,"code":"user.email_taken","message":"email already exists","retryable":False,"data":{}})
            else:
                user={"id":len(USERS)+1,"email":email,"admin":args.get("admin",False)}; USERS[email]=user
                send(stream, {"type":"result","id":identifier,"value":user})
        elif kind == "ping": send(stream, {"type":"pong","id":identifier})
        elif kind == "shutdown": send(stream, {"type":"shutdown_ok","id":identifier}); break

server = http.server.ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PORT","3104"))), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
try: bridge()
finally: server.shutdown(); server.server_close()
