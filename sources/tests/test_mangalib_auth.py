#!/usr/bin/env python3
"""End-to-end test for the MangaLib token support (refresh / retry / persistence).

Everything here runs against a *mock* MangaLib API (a tiny HTTP server in this
file) - no real account and no network access are needed. The real rakuyomi
server binary is started with the real `ru.mangalib.tokens.aix` source, whose
`apiUrl` setting is pointed at the mock.

Usage (from the repository root):


    fork/build-sources.sh    # -> build/sources/*.aix
    (cd backend && cargo build -p server)
    python3 sources/tests/test_mangalib_auth.py

Environment overrides: RAKUYOMI_SERVER (server binary), MANGALIB_AIX (source).
"""

import base64
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SERVER_BIN = os.environ.get("RAKUYOMI_SERVER", os.path.join(ROOT, "backend/target/debug/server"))
AIX = os.environ.get("MANGALIB_AIX", os.path.join(ROOT, "build/sources/ru.mangalib.tokens.aix"))
SOURCE_ID = "ru.mangalib.tokens"
FIXTURE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures/search.json")
KNOWN_CLIENTS = {"1", "2", "3"}


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def make_jwt(aud: str, exp: int, sub: str = "4242", jti: str = "") -> str:
    header = b64url(json.dumps({"typ": "JWT", "alg": "RS256"}).encode())
    payload = b64url(json.dumps({"aud": aud, "jti": jti or os.urandom(8).hex(), "iat": int(time.time()),
                                 "nbf": int(time.time()), "exp": exp, "sub": sub, "scopes": []}).encode())
    return f"{header}.{payload}.{b64url(b'not-a-real-signature')}"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class MockApi:
    """Minimal Laravel-Passport-like MangaLib API."""

    def __init__(self):
        self.lock = threading.Lock()
        self.valid_access = set()
        self.refresh_tokens = {}  # refresh token -> client id
        self.revoked = set()  # rotated (revoked) refresh tokens
        self.requests = []  # (method, path, auth header, body)
        with open(FIXTURE, "rb") as f:
            self.search_body = f.read()
        api = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def _send(self, status, obj):
                body = obj if isinstance(obj, bytes) else json.dumps(obj).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                auth = self.headers.get("Authorization")
                with api.lock:
                    api.requests.append(("GET", self.path, auth, None))
                    authorized = auth is None or auth.removeprefix("Bearer ") in api.valid_access
                if not authorized:
                    return self._send(401, {"message": "Unauthenticated."})
                if self.path.startswith("/api/manga?"):
                    return self._send(200, api.search_body)
                if self.path.startswith("/api/auth/me"):
                    return self._send(200, {"data": {"id": 4242, "username": "mock"}})
                return self._send(404, {"message": "Not found"})

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                try:
                    body = json.loads(raw or b"{}")
                except ValueError:
                    body = {}
                with api.lock:
                    api.requests.append(("POST", self.path, self.headers.get("Authorization"), body))
                    if self.path != "/api/auth/oauth/token":
                        return self._send(404, {"message": "Not found"})
                    if body.get("grant_type") != "refresh_token":
                        return self._send(400, {"error": "unsupported_grant_type"})
                    client = str(body.get("client_id"))
                    if client not in KNOWN_CLIENTS:
                        return self._send(401, {"error": "invalid_client"})
                    rt = body.get("refresh_token")
                    if rt in api.revoked:
                        # What the real auth server answers for a rotated token.
                        return self._send(401, {"error": "invalid_request", "hint": "Token has been revoked",
                                                "message": "The refresh token is invalid."})
                    if api.refresh_tokens.get(rt) != client:
                        return self._send(400, {"error": "invalid_grant", "message": "The refresh token is invalid."})
                    # Rotation: the old refresh token is revoked.
                    del api.refresh_tokens[rt]
                    api.revoked.add(rt)
                    access, refresh = api.issue(client)
                return self._send(200, {"token_type": "Bearer", "expires_in": 3600,
                                        "access_token": access, "refresh_token": refresh})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def issue(self, client: str, exp_in: int = 3600, valid: bool = True):
        access = make_jwt(client, int(time.time()) + exp_in)
        refresh = "def50200" + os.urandom(24).hex()
        if valid:
            self.valid_access.add(access)
        self.refresh_tokens[refresh] = client
        return access, refresh

    def token_requests(self):
        return [r for r in self.requests if r[0] == "POST"]

    def search_auths(self):
        return [r[2] for r in self.requests if r[0] == "GET" and r[1].startswith("/api/manga?")]

    def close(self):
        self.server.shutdown()


class RakuyomiServer:
    def __init__(self, home):
        self.home = home
        self.port = free_port()
        self.log_path = os.path.join(home, "server.log")
        self.proc = None

    def start(self):
        env = dict(os.environ, RAKUYOMI_TCP_PORT=str(self.port), RUST_LOG="info")
        self.log = open(self.log_path, "ab")
        self.proc = subprocess.Popen([SERVER_BIN, self.home], stdout=self.log, stderr=subprocess.STDOUT, env=env)
        deadline = time.time() + 60
        while time.time() < deadline:
            try:
                self.get("/installed-sources")
                return
            except Exception:
                time.sleep(0.3)
        raise RuntimeError("server did not start, see " + self.log_path)

    def get(self, path):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.port}{path}", timeout=60) as r:
            return json.loads(r.read())

    def search(self, q="naruto"):
        result = self.get(f"/mangas?q={q}")
        return [m["id"] for m in result[0]]

    def stop(self):
        if self.proc:
            self.proc.terminate()
            self.proc.wait(10)
            self.proc = None
            self.log.close()

    def log_text(self):
        with open(self.log_path, "rb") as f:
            return f.read().decode(errors="replace")


class MangaLibAuthTest(unittest.TestCase):
    def setUp(self):
        self.api = MockApi()
        self.home = tempfile.mkdtemp(prefix="rakuyomi-auth-test-")
        os.makedirs(os.path.join(self.home, "sources"))
        shutil.copy(AIX, os.path.join(self.home, "sources", SOURCE_ID + ".aix"))
        self.server = RakuyomiServer(self.home)
        self.secrets = []

    def tearDown(self):
        self.server.stop()
        self.api.close()
        log = self.server.log_text()
        for secret in self.secrets:
            self.assertNotIn(secret, log, "a token leaked into the server log")
        if os.environ.get("KEEP_TEST_HOME"):
            print("test home:", self.home, file=sys.stderr)
        else:
            shutil.rmtree(self.home, ignore_errors=True)

    # helpers -------------------------------------------------------------
    def import_settings(self, values):
        values = dict(values, apiUrl=self.api.url)
        os.makedirs(os.path.join(self.home, "import"), exist_ok=True)
        with open(os.path.join(self.home, "import", SOURCE_ID + ".json"), "w") as f:
            json.dump(values, f)
        for v in values.values():
            if isinstance(v, str) and len(v) > 20 and not v.startswith("http"):
                self.secrets.append(v)

    def stored(self):
        with open(os.path.join(self.home, "settings.json")) as f:
            return json.load(f).get("source_settings", {}).get(SOURCE_ID, {})

    def wait_stored(self, key, predicate, timeout=10):
        deadline = time.time() + timeout
        while time.time() < deadline:
            value = self.stored().get(key)
            if predicate(value):
                return value
            time.sleep(0.2)
        self.fail(f"setting {key} not persisted as expected, got {self.stored().get(key)!r}")

    def assert_search_ok(self):
        ids = self.server.search()
        self.assertIn("195--naruto", ids)

    # tests -----------------------------------------------------------------
    def test_valid_token_is_sent(self):
        access, refresh = self.api.issue("1")
        self.import_settings({"accessToken": access, "refreshToken": refresh})
        self.server.start()
        self.assertFalse(os.path.exists(os.path.join(self.home, "import", SOURCE_ID + ".json")),
                         "import file must be deleted after import")
        self.assert_search_ok()
        self.assertEqual(self.api.search_auths(), [f"Bearer {access}"])
        self.assertEqual(self.api.token_requests(), [])

    def test_expired_jwt_is_refreshed_proactively_and_persisted(self):
        access, refresh = self.api.issue("1", exp_in=-60)
        self.import_settings({"accessToken": access, "refreshToken": refresh})
        self.server.start()
        self.assert_search_ok()
        posts = self.api.token_requests()
        self.assertEqual(len(posts), 1)
        self.assertEqual(posts[0][3]["client_id"], 1 if isinstance(posts[0][3]["client_id"], int) else "1")
        auths = self.api.search_auths()
        self.assertEqual(len(auths), 1, "expired token must not be sent at all")
        new_access = auths[0].removeprefix("Bearer ")
        self.assertNotEqual(new_access, access)
        new_refresh = self.wait_stored("refreshToken", lambda v: v and v != refresh)
        self.wait_stored("accessToken", lambda v: v == new_access)
        self.secrets += [new_access, new_refresh]

        # The rotated pair must survive a restart (old refresh token is revoked).
        self.server.stop()
        self.server.start()
        self.assert_search_ok()
        self.assertEqual(len(self.api.token_requests()), 1, "no refresh needed after restart")
        self.assertEqual(self.api.search_auths()[-1], f"Bearer {new_access}")

    def test_rejected_token_is_refreshed_and_request_retried(self):
        # Not expired according to the JWT, but the server revoked it.
        access, refresh = self.api.issue("1", valid=False)
        self.import_settings({"accessToken": access, "refreshToken": refresh})
        self.server.start()
        self.assert_search_ok()
        auths = self.api.search_auths()
        self.assertEqual(len(auths), 2)
        self.assertEqual(auths[0], f"Bearer {access}")
        self.assertNotEqual(auths[1], auths[0])
        self.assertEqual(len(self.api.token_requests()), 1)
        self.wait_stored("accessToken", lambda v: v == auths[1].removeprefix("Bearer "))
        self.secrets.append(auths[1].removeprefix("Bearer "))

    def test_opaque_access_token_and_unknown_client_fallback(self):
        # Access token is not a JWT -> client id unknown, refresh token belongs to client 3.
        self.api.valid_access.add("opaque-revoked-token-0123456789")
        self.api.valid_access.discard("opaque-revoked-token-0123456789")
        access, refresh = "opaque-revoked-token-0123456789", self.api.issue("3")[1]
        self.import_settings({"accessToken": access, "refreshToken": refresh})
        self.server.start()
        self.assert_search_ok()
        clients = [str(p[3]["client_id"]) for p in self.api.token_requests()]
        self.assertEqual(clients[-1], "3")
        self.assertIn("1", clients)
        self.wait_stored("authClientId", lambda v: v == "3")

    def test_console_token_json_import_file(self):
        # Exactly what copy(JSON.stringify(JSON.parse(localStorage.auth).token)) puts
        # into the clipboard, saved as the import file.
        access, refresh = self.api.issue("1", exp_in=-10)
        token = {"token_type": "Bearer", "expires_in": 2678400, "access_token": access,
                 "refresh_token": refresh, "timestamp": int(time.time() * 1000)}
        self.import_settings(token)
        self.server.start()
        self.assert_search_ok()
        self.assertEqual(len(self.api.token_requests()), 1)
        stored = self.stored()
        for raw_key in ("access_token", "refresh_token", "token_type", "expires_in", "timestamp"):
            self.assertNotIn(raw_key, stored)
        new_refresh = self.wait_stored("refreshToken", lambda v: v and v != refresh)
        self.secrets += [stored.get("accessToken") or "", new_refresh]

    def test_localstorage_auth_json_is_accepted(self):
        access, refresh = self.api.issue("1", exp_in=-10)
        auth = {"token": {"token_type": "Bearer", "access_token": access, "refresh_token": refresh,
                          "expires_in": 3600, "timestamp": int(time.time() * 1000)}, "auth": {"id": 4242}}
        self.import_settings({"authJson": json.dumps(auth)})
        self.secrets += [access, refresh]
        self.server.start()
        self.assert_search_ok()
        self.assertEqual(len(self.api.token_requests()), 1)
        self.wait_stored("refreshToken", lambda v: v and v != refresh)
        self.wait_stored("authJson", lambda v: not v)

    def test_whole_auth_json_pasted_into_access_token_field(self):
        access, refresh = self.api.issue("1")
        auth = {"token": {"access_token": access, "refresh_token": refresh}}
        self.import_settings({"accessToken": json.dumps(auth)})
        self.secrets += [access, refresh]
        self.server.start()
        self.assert_search_ok()
        self.assertEqual(self.api.search_auths(), [f"Bearer {access}"])

    def test_invalid_refresh_token_does_not_loop(self):
        access = make_jwt("1", int(time.time()) - 60)
        self.import_settings({"accessToken": access, "refreshToken": "def50200-revoked-refresh-token-xyz"})
        self.server.start()
        try:
            self.server.search()
        except Exception:
            pass
        self.assertLessEqual(len(self.api.token_requests()), 6)
        self.assertLessEqual(len(self.api.search_auths()), 2)
        self.assertEqual(self.stored().get("refreshToken"), "def50200-revoked-refresh-token-xyz")

    def test_revoked_refresh_token_is_tried_once(self):
        access = make_jwt("1", int(time.time()) - 60)
        refresh = self.api.issue("1")[1]
        del self.api.refresh_tokens[refresh]
        self.api.revoked.add(refresh)
        self.import_settings({"accessToken": access, "refreshToken": refresh})
        self.server.start()
        try:
            self.server.search()
        except Exception:
            pass
        # Revoked for every client: no point in trying the other client ids.
        self.assertEqual(len(self.api.token_requests()), 1)

    def test_anonymous_without_tokens(self):
        self.import_settings({})
        self.server.start()
        self.assert_search_ok()
        self.assertEqual(self.api.search_auths(), [None])


if __name__ == "__main__":
    for path in (SERVER_BIN, AIX):
        if not os.path.exists(path):
            sys.exit(f"missing {path}; see the module docstring for how to build it")
    unittest.main(verbosity=2)
