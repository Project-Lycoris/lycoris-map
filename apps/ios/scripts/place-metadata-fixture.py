#!/usr/bin/env python3
"""Loopback-only controlled HTTP fixture for PlaceMetadataUITests.

Deliberately not the Rust backend. It serves the new marker contract
(`venueType`, read-only `hoursTimezone`) for the real list -> detail UI, plus a
minimal synthetic account flow so the native venue Picker can be exercised
without touching production. Binds only to 127.0.0.1 and identifies itself via
`/__ui_fixture`; the tests refuse to run without that identity.

Run with the iOS **Test** build configuration, then stop this process.
"""
import json
import re
import threading
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

FIXTURE = "lycoris-place-metadata"

# The fixed synthetic account is created by the fixture itself so the tests do
# not persist a random account that would be lost on a fixture restart.
USERNAME = "ios_metadata_fixture"
PASSWORD = "Metadata-Fixture-1"

# Accessible toilets with recognized venues and one non-toilet without a
# tag. The opening hours let the test pin an instant inside the closing-soon
# window (Asia/Shanghai 09:00-22:00, so 21:30-22:00 local).
MARKERS = [
    {"id": 21, "version": 1, "lat": 31.2304, "lng": 121.4737,
     "category": "accessible_toilet", "title": "Metro Accessible Toilet",
     "description": "Metadata fixture", "openTimeStart": "09:00", "openTimeEnd": "22:00",
     "hoursTimezone": "Asia/Shanghai", "venueType": "metro",
     "contentLanguage": "en", "isPublic": True, "reviewStatus": "APPROVED"},
    {"id": 22, "version": 1, "lat": 31.2310, "lng": 121.4740,
     "category": "accessible_toilet", "title": "School Accessible Toilet",
     "description": "Metadata fixture", "openTimeStart": "09:00", "openTimeEnd": "22:00",
     "hoursTimezone": "Asia/Shanghai", "venueType": "school",
     "contentLanguage": "en", "isPublic": True, "reviewStatus": "APPROVED"},
    {"id": 23, "version": 1, "lat": 31.2316, "lng": 121.4743,
     "category": "baby_room", "title": "Nursing Room No Tag",
     "description": "Metadata fixture", "openTimeStart": "09:00", "openTimeEnd": "22:00",
     "hoursTimezone": "Asia/Shanghai",
     "contentLanguage": "en", "isPublic": True, "reviewStatus": "APPROVED"},
    {"id": 24, "version": 1, "lat": 31.2320, "lng": 121.4746,
     "category": "accessible_toilet", "title": "Airport Accessible Toilet",
     "description": "Metadata fixture", "openTimeStart": "09:00", "openTimeEnd": "22:00",
     "hoursTimezone": "Asia/Shanghai", "venueType": "airport",
     "contentLanguage": "en", "isPublic": True, "reviewStatus": "APPROVED"},
    {"id": 25, "version": 1, "lat": 31.2324, "lng": 121.4749,
     "category": "accessible_toilet", "title": "Public Accessible Toilet",
     "description": "Metadata fixture", "openTimeStart": "09:00", "openTimeEnd": "22:00",
     "hoursTimezone": "Asia/Shanghai", "venueType": "public_toilet",
     "contentLanguage": "en", "isPublic": True, "reviewStatus": "APPROVED"},
]

# One deterministic saved place exercises the authenticated sidebar list and
# detail flow. These read fixtures do not imply support for favorite writes.
FAVORITE_IDS = (22,)


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.accounts = {USERNAME: PASSWORD}
        self.sessions = {}
        self.writes = []
        self.next_id = 100
        self.public_id = "ios_metadata_fixture_0"

    def reset(self):
        with self.lock:
            self.accounts = {USERNAME: PASSWORD}
            self.sessions = {}
            self.writes = []
            self.next_id = 100
            # A fresh publicId per reset isolates any draft persisted on the
            # simulator: the app's own account-change cleanup discards drafts
            # that belong to a previous owner.
            self.public_id = f"ios_metadata_fixture_{uuid.uuid4().hex[:8]}"

    def public_id_for(self, _username):
        return self.public_id


STATE = State()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, value, status=200, headers=None):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def body(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length <= 0:
            return {}
        try:
            return json.loads(self.rfile.read(length))
        except json.JSONDecodeError:
            return {}

    def session_user(self):
        cookie = self.headers.get("Cookie") or ""
        for part in cookie.split(";"):
            name, _, value = part.strip().partition("=")
            if name == "sid":
                return STATE.sessions.get(value)
        return None

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path
        query = parse_qs(parsed.query)
        if path == "/__ui_fixture":
            self.reply({"fixture": FIXTURE, "markers": len(MARKERS)})
        elif path == "/__ui_fixture/writes":
            with STATE.lock:
                self.reply(STATE.writes)
        elif path == "/api/me":
            user = self.session_user()
            self.reply(
                {"code": 0, "data": {"publicId": STATE.public_id_for(user)}}
                if user else {"code": 401},
                200 if user else 401)
        elif path in ("/api/markers/me/favorites", "/api/markers/me/favorites/details"):
            if not self.session_user():
                self.reply({"code": 401}, 401)
            elif path.endswith("/details"):
                self.reply([m for m in MARKERS if m["id"] in FAVORITE_IDS])
            else:
                self.reply(list(FAVORITE_IDS))
        elif path == "/api/markers/me/created":
            if not self.session_user():
                self.reply({"code": 401}, 401)
            else:
                self.reply([])
        elif re.fullmatch(r"/api/markers/[0-9]+", path):
            marker_id = int(path.rsplit("/", 1)[-1])
            match = next((m for m in MARKERS if m["id"] == marker_id), None)
            self.reply(match if match else {"code": 404}, 200 if match else 404)
        elif path == "/api/markers/search":
            term = (query.get("q", [""])[0] or "").lower()
            self.reply([m for m in MARKERS if term in m["title"].lower()
                        or term in (m.get("description") or "").lower()])
        elif path == "/api/markers/nearby":
            category = query.get("category", [None])[0]
            self.reply([m for m in MARKERS if not category or m["category"] == category])
        elif path == "/api/markers/viewport":
            self.reply(MARKERS)
        else:
            self.reply({"code": 404}, 404)

    def do_POST(self):
        parsed = urlparse(self.path)
        path = parsed.path
        if path == "/__ui_fixture/reset":
            STATE.reset()
            self.reply({"fixture": FIXTURE, "reset": True})
        elif path in ("/api/login", "/api/register"):
            fields = self.body()
            username = str(fields.get("username", "")).strip()
            password = str(fields.get("password", ""))
            if path.endswith("register"):
                STATE.accounts[username] = password
            elif STATE.accounts.get(username) != password:
                self.reply({"code": 401}, 401)
                return
            sid = f"synthetic-{uuid.uuid4().hex[:8]}"
            STATE.sessions[sid] = username
            self.reply(
                {"code": 0, "data": {"publicId": STATE.public_id_for(username)}},
                headers={"Set-Cookie": f"sid={sid}; Path=/; HttpOnly"})
        elif path == "/api/logout":
            cookie = self.headers.get("Cookie") or ""
            for part in cookie.split(";"):
                name, _, value = part.strip().partition("=")
                if name == "sid":
                    STATE.sessions.pop(value, None)
            self.reply({}, headers={
                "Set-Cookie": "sid=; Path=/; Max-Age=0; HttpOnly"})
        elif path == "/api/markers":
            user = self.session_user()
            if not user:
                self.reply({"code": 401}, 401)
                return
            fields = self.body()
            with STATE.lock:
                STATE.next_id += 1
                marker = dict(MARKERS[0])
                marker.update({
                    "id": STATE.next_id, "title": fields.get("title", ""),
                    "lat": fields.get("lat"), "lng": fields.get("lng"),
                    "category": fields.get("category", "accessible_toilet"),
                    "reviewStatus": "PENDING",
                })
                marker.pop("markImage", None)
                marker["venueType"] = (
                    fields.get("venueType", "other")
                    if marker["category"] == "accessible_toilet" else None)
            with STATE.lock:
                STATE.writes.append({"method": "POST", "body": fields})
            self.reply(marker)
        else:
            self.reply({"code": 404}, 404)

    def do_PATCH(self):
        parsed = urlparse(self.path)
        path = parsed.path
        if not re.fullmatch(r"/api/markers/[0-9]+", path):
            self.reply({"code": 404}, 404)
            return
        if not self.session_user():
            self.reply({"code": 401}, 401)
            return
        marker_id = int(path.rsplit("/", 1)[-1])
        match = next((m for m in MARKERS if m["id"] == marker_id), None)
        if not match:
            self.reply({"code": 404}, 404)
            return
        fields = self.body()
        with STATE.lock:
            STATE.writes.append({"method": "PATCH", "id": marker_id, "body": fields})
        # A real backend keeps the published point unchanged while the edit is
        # pending review; this fixture never fakes an immediate live change.
        self.reply(dict(match))


if __name__ == "__main__":
    print(f"Place metadata iOS UI fixture ({FIXTURE}) on 127.0.0.1:8080", flush=True)
    ThreadingHTTPServer(("127.0.0.1", 8080), Handler).serve_forever()
