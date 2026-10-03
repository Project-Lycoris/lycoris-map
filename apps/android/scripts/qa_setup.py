#!/usr/bin/env python3
"""Create only local synthetic Android QA fixtures; never connect to another origin."""

import argparse
import http.cookiejar
import json
import os
import re
from pathlib import Path
import secrets
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

HERE = Path(__file__).resolve().parent
DEFAULT_STATE = HERE.parent / "app/build/qa"
ORIGIN = "http://127.0.0.1:18186"
ENVIRONMENT = "lycoris-android-synthetic-v1"
SENTINEL_TITLE = "Android QA Environment Sentinel v1"
SENTINEL_DESCRIPTION = "Synthetic Android QA only. Never production data."
DATABASE = "lycoris_android_qa"
DB_CONTAINER = "lycoris-android-qa-postgres"
BACKEND_CONTAINER = "lycoris-android-qa-backend"
# Matches users::REGISTER_ADVISORY_LOCK_KEY so fixture creation cannot race signup.
REGISTER_LOCK = 0x4C59_434F_5249_5301


def private_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")
    temporary.replace(path)


def local_docker_host():
    # --context takes precedence over DOCKER_HOST, but reject any nonlocal
    # override rather than letting a caller accidentally select a remote engine.
    override = os.environ.get("DOCKER_HOST", "")
    if override and not override.startswith("unix:///"):
        raise RuntimeError("Synthetic setup requires a local Unix-socket Docker engine.")
    context = os.environ.get("DOCKER_CONTEXT", "")
    if override and not context:
        return override
    result = subprocess.run(["docker", "context", "inspect", *([context] if context else [])],
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError("Cannot inspect the local QA Docker context.")
    try:
        endpoint = json.loads(result.stdout)[0]["Endpoints"]["docker"]["Host"]
    except (ValueError, KeyError, IndexError, TypeError):
        raise RuntimeError("Invalid QA Docker context.") from None
    if not isinstance(endpoint, str) or not endpoint.startswith("unix:///"):
        raise RuntimeError("Synthetic setup requires a local Unix-socket Docker context.")
    return endpoint


def docker(*arguments, input_text=None):
    endpoint = local_docker_host()
    # Pin the inspected endpoint for the command; changes to the selected context
    # or inherited overrides must not redirect writes between verification and use.
    environment = {key: value for key, value in os.environ.items()
                   if key not in ("DOCKER_HOST", "DOCKER_CONTEXT")}
    result = subprocess.run(["docker", "--host", endpoint, *arguments], input=input_text, text=True,
                            env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        # Docker inspect/environment output must never appear in error reports.
        raise RuntimeError(f"Local QA Docker command failed ({arguments[0]}). Check the QA log.")
    return result.stdout


def compose(state, *arguments):
    docker("compose", "--env-file", str(state / "backend.env"),
           "-f", str(HERE / "qa.compose.yml"), *arguments)


def inspect_environment(name):
    item = json.loads(docker("inspect", name))[0]
    return item, dict(value.split("=", 1) for value in item["Config"]["Env"] if "=" in value)


def verify_database():
    item, env = inspect_environment(DB_CONTAINER)
    if (env.get("POSTGRES_DB") != DATABASE or env.get("POSTGRES_USER") != DATABASE
            or item.get("State", {}).get("Running") is not True
            or item.get("Name") != "/" + DB_CONTAINER
            or (item["Config"].get("Labels") or {}).get("com.docker.compose.service") != "postgres"
            or (item["Config"].get("Labels") or {}).get("com.docker.compose.project") != "lycoris-android-qa"):
        raise RuntimeError("Refusing non-Android-QA database container.")
    result = docker("exec", DB_CONTAINER, "psql", "-U", DATABASE, "-d", DATABASE,
                    "-Atc", "SELECT current_database();").strip()
    if result != DATABASE:
        raise RuntimeError("Refusing database identity mismatch.")


def verify_backend():
    verify_database()
    item, env = inspect_environment(BACKEND_CONTAINER)
    url = urllib.parse.urlsplit(env.get("DATABASE_URL", ""))
    binding = item["HostConfig"].get("PortBindings", {}).get("18186/tcp", [])
    if not (url.hostname == "postgres" and url.port == 5432 and url.path == "/" + DATABASE
            and url.username == DATABASE and env.get("SERVER_PORT") == "18186"
            and env.get("SESSION_COOKIE_NAME") == "LYCORIS_ANDROID_QA"
            and binding == [{"HostIp": "127.0.0.1", "HostPort": "18186"}]
            and item.get("State", {}).get("Running") is True
            and item.get("Name") == "/" + BACKEND_CONTAINER
            and (item["Config"].get("Labels") or {}).get("com.docker.compose.service") == "backend"
            and (item["Config"].get("Labels") or {}).get("com.docker.compose.project") == "lycoris-android-qa"):
        raise RuntimeError("Refusing backend that is not the isolated Android QA service.")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        raise RuntimeError("QA service redirect refused.")


def client():
    return urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(),
                                      urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))


def request(opener, method, path, value=None):
    if not path.startswith("/") or path.startswith("//"):
        raise ValueError("QA requests must use a relative local path.")
    data = json.dumps(value).encode() if value is not None else None
    req = urllib.request.Request(ORIGIN + path, data=data, method=method,
                                 headers={"Content-Type": "application/json", "User-Agent": "LycorisAndroidSyntheticQA/1"})
    try:
        with opener.open(req, timeout=20) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        # No response bodies: they may contain profile or cookie information.
        raise RuntimeError(f"Synthetic QA {method} {path.split('?')[0]} returned HTTP {error.code}.") from None


def init(state):
    state.mkdir(parents=True, exist_ok=True)
    env_path = state / "backend.env"
    if not env_path.exists():
        descriptor = os.open(env_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as stream:
            stream.write("ANDROID_QA_DB_PASSWORD=" + secrets.token_hex(24) + "\n")
            stream.write("ANDROID_QA_BUILD_PROXY=" + os.getenv("ANDROID_QA_BUILD_PROXY", "") + "\n")
    credentials_path = state / "credentials.json"
    if not credentials_path.exists():
        suffix = secrets.token_hex(4)
        users = [{"username": f"android_qa_{role}_{suffix}", "nickname": f"Android QA {role.title()}",
                  "email": f"android-qa-{role}-{suffix}@example.invalid", "password": secrets.token_urlsafe(24)}
                 for role in ("fixture", "alice", "bob")]
        private_json(credentials_path, {"testEnvironment": ENVIRONMENT, "users": users})
    print("Synthetic QA configuration is ready in " + str(state) + ". Credentials are not printed.")


def wait_ready():
    opener = client()
    for _ in range(45):
        try:
            request(opener, "GET", "/health/ready")
            return
        except (RuntimeError, urllib.error.URLError, TimeoutError):
            time.sleep(2)
    raise RuntimeError("Local synthetic backend did not become ready.")


def validate_fixture_user(user):
    if not isinstance(user, dict):
        raise RuntimeError("Invalid synthetic fixture account.")
    username = user.get("username", "")
    match = re.fullmatch(r"android_qa_(fixture|alice|bob)_([0-9a-f]{8})", username) if isinstance(username, str) else None
    if not match:
        raise RuntimeError("Invalid synthetic fixture account name.")
    role, suffix = match.groups()
    password = user.get("password", "")
    if (user.get("email") != f"android-qa-{role}-{suffix}@example.invalid"
            or user.get("nickname") != f"Android QA {role.title()}"
            or "role" in user or not isinstance(password, str)
            or not 8 <= len(password) <= 72 or not password.isascii()
            or any(ord(char) < 32 or ord(char) == 127 for char in password)):
        raise RuntimeError("Refusing a non-synthetic or invalid fixture account.")
    return role


def seed_user(user):
    validate_fixture_user(user)
    verify_backend()
    # No SMTP bypass in the API: seed only this verified synthetic database.
    # PostgreSQL's bundled pgcrypto produces a real bcrypt hash. Plaintext stays
    # on stdin, never in argv/logs, and existing account passwords are not reset.
    payload = json.dumps({key: user[key] for key in ("username", "nickname", "email", "password")})
    literal = "'" + payload.replace("'", "''") + "'"
    block_tag = "$qa_seed_" + secrets.token_hex(16) + "$"
    while block_tag in payload:
        block_tag = "$qa_seed_" + secrets.token_hex(16) + "$"
    statement = """BEGIN;
SET LOCAL standard_conforming_strings = on;
CREATE EXTENSION IF NOT EXISTS pgcrypto;
SELECT pg_advisory_xact_lock(""" + str(REGISTER_LOCK) + """);
DO """ + block_tag + """
DECLARE fixture jsonb := """ + literal + """::jsonb;
BEGIN
    IF current_database() <> 'lycoris_android_qa' OR current_user <> 'lycoris_android_qa' THEN
        RAISE EXCEPTION 'Not the synthetic QA database';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM users WHERE username = fixture->>'username') THEN
        IF EXISTS (SELECT 1 FROM users WHERE lower(email) = fixture->>'email') THEN
            RAISE EXCEPTION 'Synthetic fixture email already belongs to another account';
        END IF;
        INSERT INTO users(public_id, username, nickname, email, password, role)
        VALUES (gen_random_uuid(), fixture->>'username', fixture->>'nickname', fixture->>'email',
                crypt(fixture->>'password', gen_salt('bf', 8)), 'USER');
    END IF;
    IF (SELECT count(*) FROM users WHERE username = fixture->>'username') <> 1
       OR NOT EXISTS (SELECT 1 FROM users WHERE username = fixture->>'username'
                      AND email = fixture->>'email' AND role = 'USER' AND NOT deleted) THEN
        RAISE EXCEPTION 'Synthetic fixture account identity mismatch';
    END IF;
END """ + block_tag + """;
COMMIT;
"""
    docker("exec", "-i", DB_CONTAINER, "psql", "-X", "-U", DATABASE, "-d", DATABASE,
           "-v", "ON_ERROR_STOP=1", "-f", "-", input_text=statement)


def seed(state):
    verify_backend()
    credentials_path = state / "credentials.json"
    credentials = json.loads(credentials_path.read_text())
    if credentials.get("testEnvironment") != ENVIRONMENT:
        raise RuntimeError("Refusing credentials from a different environment.")
    users = credentials.get("users", [])
    roles = [validate_fixture_user(user) for user in users]
    if sorted(roles) != ["alice", "bob", "fixture"]:
        raise RuntimeError("Expected exactly the three ordinary synthetic fixture accounts.")
    fixture_client = None
    owner = None
    for user in users:
        seed_user(user)
        opener = client()
        username = user["username"]
        data = request(opener, "POST", "/api/login", {key: user[key] for key in ("username", "password")})
        public_id = str(uuid.UUID(data["data"]["publicId"]))
        user["publicId"] = public_id
        if username.startswith("android_qa_fixture_"):
            fixture_client, owner = opener, public_id
    if fixture_client is None or owner is None:
        raise RuntimeError("Missing synthetic fixture owner.")
    private_json(credentials_path, credentials)
    fixtures = [
        ("sentinel", SENTINEL_TITLE, SENTINEL_DESCRIPTION, "self_definition", -70.0, 0.0),
        ("toilet", "Android QA Accessible Toilet", "Step-free synthetic facility for Android testing.", "accessible_toilet", 31.2304, 121.4737),
        ("baby", "Android QA Nursing Room", "Synthetic nursing room. No production location.", "baby_room", 31.2308, 121.4741),
        ("clinic", "Android QA Medical Institution", "Synthetic medical institution for category and nearby tests.", "friendly_clinic", 31.2300, 121.4732),
        ("other", "Android QA Other Place", "Synthetic other category for stable marker-color tests.", "self_definition", 31.2310, 121.4730),
    ]
    receipts = {}
    for key, title, description, category, lat, lng in fixtures:
        marker = request(fixture_client, "POST", "/api/markers?lang=en", {
            "clientRequestId": f"android-qa-{key}-v1", "title": title, "description": description,
            "language": "en", "category": category, "lat": lat, "lng": lng,
        })
        marker_id = marker["id"]
        if not isinstance(marker_id, int) or marker_id <= 0:
            raise RuntimeError("Invalid synthetic marker receipt.")
        # Approval is confined to this exact fixture owner's idempotency key and ID.
        docker("exec", DB_CONTAINER, "psql", "-U", DATABASE, "-d", DATABASE, "-v", "ON_ERROR_STOP=1", "-Atc",
               f"UPDATE map_markers SET review_status='APPROVED' WHERE id={marker_id} "
               f"AND user_public_id='{owner}' AND client_request_id='android-qa-{key}-v1';")
        receipts[key] = {"markerId": marker_id, "title": title, "description": description,
                         "ownerPublicId": owner, "clientRequestId": f"android-qa-{key}-v1"}
    sentinel = receipts["sentinel"]
    private_json(state / "fixtures.json", {"testEnvironment": ENVIRONMENT, "protocolVersion": 1,
                 "sentinel": sentinel, "markers": receipts, "center": {"lat": 31.2304, "lng": 121.4737}})
    observed = request(client(), "GET", f"/api/markers/{sentinel['markerId']}?lang=en")
    if observed.get("title") != SENTINEL_TITLE or observed.get("userPublicId") != owner:
        raise RuntimeError("Synthetic sentinel cannot be verified publicly.")
    print("Synthetic users and 5 owned fixture markers ready; credentials saved privately.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("init", "start", "seed", "verify"))
    parser.add_argument("--state-dir", type=Path, default=DEFAULT_STATE)
    args = parser.parse_args()
    state = args.state_dir.resolve()
    if args.action == "init":
        init(state)
    elif args.action == "start":
        init(state)
        compose(state, "build", "backend")
        compose(state, "up", "-d", "postgres", "redis")
        # Empty, purpose-owned database only; no production connection overrides accepted.
        for _ in range(30):
            try:
                verify_database()
                break
            except RuntimeError:
                time.sleep(2)
        else:
            raise RuntimeError("Synthetic database is unavailable.")
        compose(state, "run", "--rm", "backend", "--migrate")
        compose(state, "up", "-d", "backend")
        wait_ready()
        seed(state)
    elif args.action == "seed":
        seed(state)
    else:
        verify_backend()
        wait_ready()
        print("Verified isolated Android QA backend on loopback port 18186.")


if __name__ == "__main__":
    main()
