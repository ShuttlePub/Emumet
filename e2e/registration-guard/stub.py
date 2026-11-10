"""Isolated fault-injection webhook + local SMTP sink. NOT a protection adapter."""
import json
from pathlib import Path
import socketserver
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STATE = {"mode": "deny", "calls": [], "mail": []}
LOCK = threading.Lock()


class SMTP(socketserver.StreamRequestHandler):
    def handle(self):
        self.wfile.write(b"220 fixture SMTP\r\n")
        data = None
        for line in self.rfile:
            if data is not None:
                if line == b".\r\n":
                    with LOCK:
                        STATE["mail"].append(b"".join(data).decode(errors="replace"))
                    data = None
                    self.wfile.write(b"250 queued locally\r\n")
                else:
                    data.append(line)
            elif line.upper().startswith(b"DATA"):
                data = []
                self.wfile.write(b"354 end with dot\r\n")
            elif line.upper().startswith(b"EHLO"):
                self.wfile.write(b"250-fixture\r\n250 OK\r\n")
            elif line.upper().startswith(b"QUIT"):
                self.wfile.write(b"221 bye\r\n")
                break
            else:
                self.wfile.write(b"250 OK\r\n")


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body):
        raw = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        try:
            self.wfile.write(raw)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        if self.path == "/.well-known/openid-configuration":
            return self.reply(200, {"issuer": "http://guard:8080",
                                    "jwks_uri": "http://guard:8080/jwks"})
        if self.path == "/jwks":
            path = Path("/fixture/artifacts/runtime/jwks.json")
            return self.reply(200, json.loads(path.read_text()))
        if self.path == "/app-health":
            try:
                urllib.request.urlopen("http://app:8080/", timeout=2)
            except urllib.error.HTTPError:
                return self.reply(200, {"listening": True})
            except (OSError, urllib.error.URLError):
                return self.reply(503, {"listening": False})
            return self.reply(200, {"listening": True})
        with LOCK:
            self.reply(200, STATE)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        if self.path == "/control":
            with LOCK:
                STATE["mode"] = body["mode"]
            return self.reply(200, {"mode": body["mode"]})
        if self.path == "/relation-tuples/check":
            return self.reply(200, {"allowed": True})
        if self.path == "/account-probe":
            req = urllib.request.Request(
                "http://app:8080/api/v1/accounts",
                json.dumps({"name": body["name"], "is_bot": False}).encode(),
                {"Content-Type": "application/json", "Authorization": "Bearer " + body["jwt"]},
            )
            try:
                response = urllib.request.urlopen(req, timeout=45)
            except urllib.error.HTTPError as error:
                response = error
            text = response.read().decode()
            try:
                result = json.loads(text)
            except ValueError:
                result = text
            return self.reply(200, {"app_status": response.status, "body": result})
        if self.path != "/guard":
            return self.reply(404, {})
        identity = body.get("identity", {})
        flow = body.get("flow", {})
        # No password credentials, cookies, or session tokens retained.
        call = {
            "context_keys": list(body),
            "identity_id": identity.get("id"),
            "email": identity.get("traits", {}).get("email"),
            "verifiable_addresses": identity.get("verifiable_addresses", []),
            "flow_id": flow.get("id"),
            "flow_type": flow.get("type"),
            "transient_payload": body.get("transient_payload", flow.get("transient_payload")),
        }
        with LOCK:
            mode = STATE["mode"]
            call["mode"] = mode
            STATE["calls"].append(call)
        if mode == "timeout":
            time.sleep(40)
        if mode == "disconnect":
            self.connection.close()
            return
        if mode in ("allow", "override"):
            return self.reply(200, {})
        if mode == "malformed":
            return self.reply(200, b"not-json")
        if mode == "empty":
            return self.reply(204, b"")
        if mode == "false_json":
            return self.reply(200, {"allowed": False})
        if mode == "created_malformed":
            return self.reply(201, b"not-json")
        status = {"deny": 403, "server_error": 503, "rate_limit": 429}.get(mode, 403)
        self.reply(status, {"messages": [{"instance_ptr": "#/", "messages": [
            {"id": 4000001, "text": "fixture registration denied", "type": "error"}
        ]}]})

    def do_PUT(self):
        # Account creation's permission side effect, not a registration guard call.
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.path == "/admin/relation-tuples":
            return self.reply(204, b"")
        self.reply(404, {})


if __name__ == "__main__":
    socketserver.ThreadingTCPServer.allow_reuse_address = True
    smtp = socketserver.ThreadingTCPServer(("0.0.0.0", 1025), SMTP)
    threading.Thread(target=smtp.serve_forever, daemon=True).start()
    ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
