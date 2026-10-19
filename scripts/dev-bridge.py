#!/usr/bin/env python3
"""Dev-only HTTP bridge from the desktop UI (running in a plain browser via
`npm run dev`) to a tpd Unix socket, so the UI can be exercised and
screenshotted without the Tauri shell.

    python3 scripts/dev-bridge.py --factory /path/to/scratch/factory

Listens on 127.0.0.1 only. Never point it at your real factory: anything that
can reach the port can drive tpd. Vite proxies /__tp to it (vite.config.ts).
"""

import argparse
import json
import os
import socket
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def connect(sock_path, timeout=30):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(sock_path)
    return s


def call(sock_path, method, params):
    s = connect(sock_path)
    try:
        f = s.makefile("rwb")
        f.write(json.dumps({"id": 1, "method": method, "params": params}).encode() + b"\n")
        f.flush()
        resp = json.loads(f.readline())
    finally:
        s.close()
    if resp.get("error"):
        e = resp["error"]
        raise RuntimeError(f"{e.get('code')}: {e.get('message')}")
    return resp.get("result")


class Handler(BaseHTTPRequestHandler):
    sock_path = ""

    def log_message(self, *_):
        pass

    def reply(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        cmd = self.path.rsplit("/", 1)[-1]
        try:
            if cmd == "rpc":
                self.reply(200, {"ok": call(self.sock_path, body["method"], body.get("params"))})
            elif cmd == "daemon_status":
                try:
                    self.reply(200, {"ok": call(self.sock_path, "status", None)})
                except OSError:
                    self.reply(200, {"ok": None})
            elif cmd == "app_ready":
                self.reply(200, {"ok": None})
            else:
                self.reply(200, {"err": f"{cmd} is not available in the browser preview"})
        except (OSError, RuntimeError, ValueError) as e:
            self.reply(200, {"err": str(e)})

    def do_GET(self):
        if not self.path.endswith("/events"):
            self.reply(404, {"err": "not found"})
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        try:
            s = connect(self.sock_path, timeout=None)
        except OSError as e:
            self.wfile.write(f"event: down\ndata: {json.dumps(str(e))}\n\n".encode())
            return
        try:
            f = s.makefile("rwb")
            f.write(b'{"id":1,"method":"subscribe","params":null}\n')
            f.flush()
            f.readline()
            self.wfile.write(b"event: up\ndata: null\n\n")
            self.wfile.flush()
            for line in f:
                self.wfile.write(b"data: " + line.strip() + b"\n\n")
                self.wfile.flush()
        except (OSError, BrokenPipeError):
            pass
        finally:
            s.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--factory", required=True)
    ap.add_argument("--port", type=int, default=1421)
    a = ap.parse_args()
    home = os.path.expanduser("~/factory")
    if os.path.realpath(a.factory) == os.path.realpath(home):
        raise SystemExit("refusing to bridge the real ~/factory; use a scratch factory")
    Handler.sock_path = os.path.join(a.factory, "tpd.sock")
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    print(f"dev-bridge on http://127.0.0.1:{a.port} -> {Handler.sock_path}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
