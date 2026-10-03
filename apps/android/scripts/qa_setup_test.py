"""Synthetic bootstrap must never bypass verification on a real API or database."""

import contextlib
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import qa_setup as qa


def fixture(role="fixture"):
    return {"username": f"android_qa_{role}_1234abcd", "nickname": f"Android QA {role.title()}",
            "email": f"android-qa-{role}-1234abcd@example.invalid", "password": "synthetic-password"}


def container(name, service, environment):
    return {"Name": "/" + name, "State": {"Running": True}, "Config": {"Labels": {
        "com.docker.compose.project": "lycoris-android-qa", "com.docker.compose.service": service}},
        "HostConfig": {"PortBindings": {"18186/tcp": [{"HostIp": "127.0.0.1", "HostPort": "18186"}]}}}, environment


class SetupTest(unittest.TestCase):
    def test_init_creates_only_ordinary_invalid_domain_accounts_and_private_files(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            state = Path(directory)
            qa.init(state)
            before = (state / "credentials.json").read_bytes()
            qa.init(state)
            self.assertEqual(before, (state / "credentials.json").read_bytes())
            users = json.loads(before)["users"]
            self.assertEqual(["fixture", "alice", "bob"], [qa.validate_fixture_user(user) for user in users])
            for name in ["credentials.json", "backend.env"]:
                self.assertEqual(0o600, (state / name).stat().st_mode & 0o777)

    def test_fixture_validation_rejects_external_identity_admin_and_invalid_password(self):
        changes = [{"username": "real-user"}, {"username": "android_qa_admin_1234abcd"},
                   {"username": "android_qa_fixture_1234abcd'; DELETE FROM users;--"},
                   {"email": "real@example.com"}, {"nickname": "Unexpected identity"},
                   {"role": "ADMIN"}, {"password": "short"}, {"password": "a" * 73},
                   {"password": "a" * 8 + "\x00"}, {"password": "密" * 8}]
        for change in changes:
            with self.subTest(change=list(change)), patch.object(qa, "verify_backend") as guard, patch.object(qa, "docker") as docker:
                with self.assertRaises(RuntimeError):
                    qa.seed_user(fixture() | change)
                guard.assert_not_called()
                docker.assert_not_called()

    def test_seed_refuses_mismatched_backend_before_writing(self):
        with patch.object(qa, "verify_backend", side_effect=RuntimeError("wrong identity")), patch.object(qa, "docker") as docker:
            with self.assertRaises(RuntimeError):
                qa.seed_user(fixture())
            docker.assert_not_called()

    def test_seed_hashes_password_via_stdin_without_api_registration_or_password_reset(self):
        user = fixture() | {"password": "quotes'\\and$qa_seed$"}
        with patch.object(qa, "verify_backend") as guard, patch.object(qa, "docker") as docker, patch.object(qa, "request") as request:
            qa.seed_user(user)
            guard.assert_called_once()
            request.assert_not_called()
            arguments = docker.call_args.args
            sql = docker.call_args.kwargs["input_text"]
            self.assertNotIn(user["password"], " ".join(arguments))
            self.assertIn("crypt(fixture->>'password', gen_salt('bf', 8))", sql)
            self.assertIn("'USER'", sql)
            self.assertIn("IF NOT EXISTS", sql)
            self.assertNotIn("UPDATE users", sql)
            self.assertIn("current_database() <> 'lycoris_android_qa'", sql)
            self.assertIn("ON_ERROR_STOP=1", arguments)
            self.assertIn("-X", arguments)
            tag = next(line[3:] for line in sql.splitlines() if line.startswith("DO "))
            self.assertNotIn(tag, user["password"])
            self.assertIn("quotes''", sql)

    def test_seed_logs_in_after_sql_fixture_and_keeps_stable_marker_keys(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            users = [fixture(role) for role in ["fixture", "alice", "bob"]]
            qa.private_json(state / "credentials.json", {"testEnvironment": qa.ENVIRONMENT, "users": users})
            owner = "8f5b510c-c67a-4d4f-9874-0d29119462c9"
            calls = []
            def request(opener, method, path, value=None):
                calls.append((method, path, value))
                if path == "/api/login":
                    return {"data": {"publicId": owner}}
                if method == "POST":
                    return {"id": 41}
                return {"title": qa.SENTINEL_TITLE, "userPublicId": owner}
            with patch.object(qa, "verify_backend"), patch.object(qa, "seed_user") as seed, \
                    patch.object(qa, "request", side_effect=request), patch.object(qa, "docker"), \
                    contextlib.redirect_stdout(io.StringIO()):
                qa.seed(state)
                self.assertEqual(3, seed.call_count)
            self.assertEqual(3, sum(path == "/api/login" for _, path, _ in calls))
            self.assertFalse(any("register" in path or "verification" in path for _, path, _ in calls))
            self.assertEqual(5, len({value["clientRequestId"] for method, path, value in calls if path.startswith("/api/markers?")}))
            self.assertEqual(owner, json.loads((state / "fixtures.json").read_text())["sentinel"]["ownerPublicId"])

    def test_wrong_environment_or_extra_accounts_block_entire_seed_before_first_write(self):
        for environment, users in [("production", [fixture()]),
                                   (qa.ENVIRONMENT, [fixture(), fixture("alice"), fixture("bob"), fixture("bob")])]:
            with tempfile.TemporaryDirectory() as directory:
                state = Path(directory)
                qa.private_json(state / "credentials.json", {"testEnvironment": environment, "users": users})
                with patch.object(qa, "verify_backend"), patch.object(qa, "seed_user") as seed:
                    with self.assertRaises(RuntimeError):
                        qa.seed(state)
                    seed.assert_not_called()

    def test_remote_override_rejected_before_any_docker_command(self):
        for host in ["tcp://127.0.0.1:2375", "ssh://production", "unix://remote/path"]:
            with patch.dict(os.environ, {"DOCKER_HOST": host}, clear=True), patch.object(qa.subprocess, "run") as run:
                with self.assertRaises(RuntimeError):
                    qa.docker("exec", qa.DB_CONTAINER, "psql")
                run.assert_not_called()

    def test_remote_selected_context_never_executes_mutation(self):
        result = subprocess.CompletedProcess([], 0, json.dumps([{"Endpoints": {"docker": {"Host": "ssh://production"}}}]), "")
        with patch.dict(os.environ, {}, clear=True), patch.object(qa.subprocess, "run", return_value=result) as run:
            with self.assertRaises(RuntimeError):
                qa.docker("exec", qa.DB_CONTAINER, "psql")
            self.assertEqual(["docker", "context", "inspect"], run.call_args.args[0])
            self.assertEqual(1, run.call_count)

    def test_context_override_is_checked_even_when_docker_host_is_local(self):
        result = subprocess.CompletedProcess([], 0, json.dumps([{"Endpoints": {"docker": {"Host": "tcp://remote:2375"}}}]), "")
        with patch.dict(os.environ, {"DOCKER_HOST": "unix:///tmp/docker.sock", "DOCKER_CONTEXT": "remote"}, clear=True), \
                patch.object(qa.subprocess, "run", return_value=result) as run:
            with self.assertRaises(RuntimeError):
                qa.docker("exec", qa.DB_CONTAINER, "psql")
            self.assertEqual(["docker", "context", "inspect", "remote"], run.call_args.args[0])

    def test_local_endpoint_is_pinned_and_docker_error_hides_stdin_and_output(self):
        result = subprocess.CompletedProcess([], 1, "secret response", "secret password")
        with patch.dict(os.environ, {"DOCKER_HOST": "unix:///tmp/docker.sock"}, clear=True), \
                patch.object(qa.subprocess, "run", return_value=result) as run:
            with self.assertRaisesRegex(RuntimeError, "Local QA Docker command failed") as error:
                qa.docker("exec", qa.DB_CONTAINER, "psql", input_text="secret input")
            self.assertNotIn("secret", str(error.exception))
            self.assertEqual(["docker", "--host", "unix:///tmp/docker.sock", "exec"], run.call_args.args[0][:4])
            self.assertNotIn("DOCKER_HOST", run.call_args.kwargs["env"])

    def test_database_identity_requires_exact_container_service_project_and_database(self):
        good = container(qa.DB_CONTAINER, "postgres", {"POSTGRES_DB": qa.DATABASE, "POSTGRES_USER": qa.DATABASE})
        for field in ["name", "project", "service", "database", "user", "stopped"]:
            item, env = copy.deepcopy(good)
            if field == "name": item["Name"] = "/production"
            if field == "stopped": item["State"]["Running"] = False
            if field == "project": item["Config"]["Labels"]["com.docker.compose.project"] = "production"
            if field == "service": item["Config"]["Labels"]["com.docker.compose.service"] = "production"
            if field == "database": env["POSTGRES_DB"] = "production"
            if field == "user": env["POSTGRES_USER"] = "production"
            with self.subTest(field=field), patch.object(qa, "inspect_environment", return_value=(item, env)), patch.object(qa, "docker") as docker:
                with self.assertRaises(RuntimeError): qa.verify_database()
                docker.assert_not_called()
        with patch.object(qa, "inspect_environment", return_value=good), patch.object(qa, "docker", return_value="production"):
            with self.assertRaises(RuntimeError): qa.verify_database()

    def test_backend_identity_rejects_public_binding_wrong_db_or_service(self):
        good = container(qa.BACKEND_CONTAINER, "backend", {
            "DATABASE_URL": "postgres://lycoris_android_qa:synthetic@postgres:5432/lycoris_android_qa",
            "SERVER_PORT": "18186", "SESSION_COOKIE_NAME": "LYCORIS_ANDROID_QA"})
        for field in ["name", "service", "binding", "database", "cookie", "stopped"]:
            item, env = copy.deepcopy(good)
            if field == "name": item["Name"] = "/production"
            if field == "stopped": item["State"]["Running"] = False
            if field == "service": item["Config"]["Labels"]["com.docker.compose.service"] = "app"
            if field == "binding": item["HostConfig"]["PortBindings"]["18186/tcp"][0]["HostIp"] = "0.0.0.0"
            if field == "database": env["DATABASE_URL"] = "postgres://user:secret@production:5432/lycoris_android_qa"
            if field == "cookie": env["SESSION_COOKIE_NAME"] = "PRODUCTION"
            with self.subTest(field=field), patch.object(qa, "verify_database"), patch.object(qa, "inspect_environment", return_value=(item, env)):
                with self.assertRaises(RuntimeError): qa.verify_backend()


if __name__ == "__main__":
    unittest.main()
