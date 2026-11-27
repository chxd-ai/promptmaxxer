#!/usr/bin/env python3
"""Headless smoke test for a built promptmaxxer binary (CI runs it on Windows; it works on macOS too).

Starts a mock OpenAI-compatible server (streaming SSE chat completions) on 127.0.0.1, writes a temp settings
file that points the `custom-api` engine at it (PMX_SETTINGS), pipes a draft into
`promptmaxxer --rewrite --engine custom-api --no-brief`, and checks that the mock's text comes out and the exit
code is 0. No real model call, no key.

Usage: python tests/smoke_headless.py <path to promptmaxxer or promptmaxxer.exe>
"""
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading

TEXT = "Fix the login page: find why sign-in fails for some users, repair it, and add a test that covers it."
PARTS = [TEXT[:30], TEXT[30:70], TEXT[70:]]
DRAFT = "the login page is broken for some people fix it"
MODEL = "mock-model"
SEEN = []


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        try:
            body = json.loads(self.rfile.read(n) or b"{}")
        except ValueError:
            body = {}
        SEEN.append((self.path, body))
        if not self.path.endswith("/chat/completions"):
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.end_headers()
        chunks = [{"choices": [{"delta": {"role": "assistant", "content": ""}}]}]
        chunks += [{"choices": [{"delta": {"content": p}, "finish_reason": None}]} for p in PARTS]
        chunks += [{"choices": [{"delta": {}, "finish_reason": "stop"}]}]
        for c in chunks:
            self.wfile.write(f"data: {json.dumps(c)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def log_message(self, *args):
        pass


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    exe = os.path.abspath(sys.argv[1])
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]
    with tempfile.TemporaryDirectory() as tmp:
        settings = os.path.join(tmp, "settings.json")
        with open(settings, "w", encoding="utf-8") as f:
            json.dump({"custom_api": {"base_url": f"http://127.0.0.1:{port}/v1", "fast_model": MODEL, "max_model": ""}, "onboarded": True}, f)
        env = dict(os.environ, PMX_SETTINGS=settings)
        r = subprocess.run(
            [exe, "--rewrite", "--engine", "custom-api", "--no-brief"],
            input=DRAFT.encode(), capture_output=True, env=env, timeout=120,
        )
    server.shutdown()
    out, err = r.stdout.decode("utf-8", "replace"), r.stderr.decode("utf-8", "replace")
    print(f"exit code: {r.returncode}\nstdout: {out!r}\nstderr: {err}")
    problems = []
    if r.returncode != 0:
        problems.append(f"exit code {r.returncode}, expected 0")
    if out.strip() != TEXT:
        problems.append("stdout is not the mock's text")
    if not SEEN:
        problems.append("the mock server got no request")
    else:
        path, body = SEEN[0]
        if body.get("model") != MODEL or body.get("stream") is not True:
            problems.append(f"unexpected request: {path} model={body.get('model')!r} stream={body.get('stream')!r}")
        if DRAFT not in json.dumps(body.get("messages", [])):
            problems.append("the draft is not in the request")
    for p in problems:
        print("FAIL:", p)
    if not problems:
        print("PASS: headless rewrite through the mock server")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
