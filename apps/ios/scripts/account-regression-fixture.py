#!/usr/bin/env python3
"""Loopback-only controlled HTTP fixture for AccountRegressionUITests.

This is deliberately not the Rust backend. It injects delayed responses to
verify native UI timing; backend auth behavior is tested in auth_integration.
Run with the iOS Test configuration, then stop this process when finished.
"""
import json
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

LOCK = threading.Lock()
STATE = {"owner": str(uuid.uuid4()), "saved": False, "identityDelay": 0, "detailDelay": 0,
         "anonymous": False}
MARKER = {"id": 1, "version": 1, "lat": 31.2304, "lng": 121.4737,
          "category": "accessible_toilet", "title": "Cold Edit Fixture",
          "description": "Controlled native account regression fixture",
          "contentLanguage": "en", "isPublic": True, "reviewStatus": "APPROVED"}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, value, status=200):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        path = urlsplit(self.path).path
        with LOCK:
            state = STATE.copy()
        if path == "/__ui_fixture":
            self.reply({"fixture": "lycoris-account-regression", **state})
        elif path == "/api/me":
            time.sleep(state["identityDelay"])
            if state["anonymous"]:
                self.reply({"code": 401}, 401)
            else:
                self.reply({"code": 0, "data": {"publicId": state["owner"], "username": "UI Fixture"}})
        elif path == "/api/markers/me/favorites/details":
            self.reply([MARKER] if state["saved"] else [])
        elif path == "/api/markers/me/created":
            self.reply([])
        elif path == "/api/markers/1":
            time.sleep(state["detailDelay"])
            self.reply(MARKER)
        elif path in ("/api/markers/viewport", "/api/markers/search", "/api/markers/nearby"):
            self.reply([MARKER])
        else:
            self.reply({"code": 404}, 404)

    def do_POST(self):
        path = urlsplit(self.path).path
        if path == "/__ui_fixture":
            fields = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            with LOCK:
                if fields.pop("reset", False):
                    STATE.update(owner=str(uuid.uuid4()), saved=False, identityDelay=0, detailDelay=0,
                                 anonymous=False)
                if "anonymous" in fields:
                    STATE["anonymous"] = bool(fields["anonymous"])
                for key in ("identityDelay", "detailDelay"):
                    if key in fields:
                        STATE[key] = min(5, max(0, float(fields[key])))
            self.reply({"fixture": "lycoris-account-regression"})
        elif path == "/api/markers/1/favorite":
            with LOCK:
                STATE["saved"] = True
            self.reply({})
        else:
            self.reply({"code": 404}, 404)

    def do_DELETE(self):
        if urlsplit(self.path).path == "/api/markers/1/favorite":
            with LOCK:
                STATE["saved"] = False
            self.reply({})
        else:
            self.reply({"code": 404}, 404)


if __name__ == "__main__":
    print("Controlled iOS UI fixture listening only on 127.0.0.1:8080", flush=True)
    ThreadingHTTPServer(("127.0.0.1", 8080), Handler).serve_forever()
