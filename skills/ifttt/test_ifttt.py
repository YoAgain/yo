"""The IFTTT service without a hub: `python3 -m unittest discover skills/ifttt`."""

import json
import os
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, ".."))
import ifttt  # noqa: E402
import qr  # noqa: E402
import yocli  # noqa: E402

KEY, CID, SECRET = "service-key", "client", "client-secret"
REDIRECT = "https://ifttt.com/channels/yo/authorize"
ALICE = "a" * 32
SIG = "s" * 86


class FakeYo:
    def __init__(self):
        self.sent, self.added, self.good = [], [], set()
        self.fail = None

    def verify(self, user, origin, ticket, signature):
        return (user, origin, ticket, signature) in self.good, "alice"

    def add(self, user):
        self.added.append(user)

    def send(self, user, photo=None):
        if self.fail:
            raise self.fail
        self.sent.append((user, photo))


class FakeRealtime:
    def __init__(self):
        self.users = []

    def notify(self, user):
        self.users.append(user)


class Service(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        env = {"IFTTT_SERVICE_KEY": KEY, "IFTTT_CLIENT_ID": CID, "IFTTT_CLIENT_SECRET": SECRET,
               "IFTTT_TEST_USER": "b" * 32}
        self.config = ifttt.Config(env)
        self.store = ifttt.Store(os.path.join(self.dir.name, "db.sqlite"))
        self.yo, self.realtime = FakeYo(), FakeRealtime()
        self.server = ifttt.make_server(("127.0.0.1", 0), self.config, self.store, self.yo, self.realtime)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}/connect"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.store.db.close()
        self.dir.cleanup()

    def call(self, method, path, body=None, headers=None, form=False):
        data = None
        headers = dict(headers or {})
        if body is not None:
            if form:
                data = urllib.parse.urlencode(body).encode()
                headers["Content-Type"] = "application/x-www-form-urlencoded"
            else:
                data = json.dumps(body).encode()
                headers["Content-Type"] = "application/json"
        request = urllib.request.Request(self.base + path, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request) as r:
                raw, status = r.read(), r.status
        except urllib.error.HTTPError as e:
            raw, status = e.read(), e.code
            e.close()
        try:
            return status, json.loads(raw) if raw else None
        except ValueError:
            return status, raw.decode()

    def link(self, allow=True):
        """The whole linking flow; the token, or the redirect when refused."""
        status, page = self.call("GET", "/authorize?" + urllib.parse.urlencode(
            {"client_id": CID, "redirect_uri": REDIRECT, "response_type": "code", "state": "s1", "scope": "ifttt"}))
        self.assertEqual(status, 200)
        ticket = urllib.parse.parse_qs(page.split("yo://connect?")[1].split('"')[0].replace("&amp;", "&"))["ticket"][0]
        poll = page.split("/connect/wait?p=")[1].split('"')[0]
        self.assertEqual(self.call("GET", f"/wait?p={poll}")[1]["state"], "waiting")
        self.yo.good.add((ALICE, "https://justyo.co", ticket, SIG))
        if allow:
            status, out = self.call("POST", "/approve", {"ticket": ticket, "user": ALICE, "signature": SIG})
            self.assertEqual((status, out["ok"]), (200, True))
            self.assertEqual(self.yo.added, [ALICE])
        else:
            self.assertEqual(self.call("POST", "/approve", {"ticket": ticket, "allow": False})[0], 200)
        state = self.call("GET", f"/wait?p={poll}")[1]
        self.assertEqual(self.call("GET", f"/wait?p={poll}")[1]["state"], "expired", "a redirect is given once")
        redirect = urllib.parse.urlsplit(state["redirect"])
        self.assertEqual(f"{redirect.scheme}://{redirect.netloc}{redirect.path}", REDIRECT)
        query = dict(urllib.parse.parse_qsl(redirect.query))
        self.assertEqual(query["state"], "s1")
        if not allow:
            return query
        status, out = self.call("POST", "/token", {"grant_type": "authorization_code", "code": query["code"],
                                                   "client_id": CID, "client_secret": SECRET,
                                                   "redirect_uri": REDIRECT}, form=True)
        self.assertEqual(status, 200)
        self.assertEqual(out["token_type"], "Bearer")
        again = self.call("POST", "/token", {"grant_type": "authorization_code", "code": query["code"],
                                             "client_id": CID, "client_secret": SECRET}, form=True)
        self.assertEqual(again, (400, {"error": "invalid_grant", "error_description": "Code unknown, used or expired"}))
        return out["access_token"]

    def auth(self, token):
        return {"Authorization": f"Bearer {token}"}

    def test_link_poll_and_act(self):
        token = self.link()
        self.assertEqual(self.store.run("SELECT hash FROM tokens")[0][0], ifttt.digest(token), "kept hashed only")
        status, info = self.call("GET", "/ifttt/v1/user/info", headers=self.auth(token))
        self.assertEqual((status, info["data"]["id"], info["data"]["name"]), (200, ALICE, "alice"))
        self.assertEqual(info["data"]["url"], "https://justyo.co/alice")
        # Yos arrive: newest first, limit honoured, ids stable, the identity remembered for realtime.
        for kind, at in (("yo", 1_000_000), ("photo", 2_000_000), ("yo", 3_000_000)):
            self.store.record(ALICE, kind, at)
        body = {"trigger_identity": "ti1", "triggerFields": {}, "user": {"timezone": "UTC"}, "extra": {"x": 1}}
        status, out = self.call("POST", "/ifttt/v1/triggers/you_yo_ifttt", body, self.auth(token))
        self.assertEqual(status, 200)
        self.assertEqual([i["kind"] for i in out["data"]], ["yo", "photo", "yo"])
        self.assertEqual([i["meta"]["timestamp"] for i in out["data"]], [3000, 2000, 1000])
        self.assertEqual(out["data"][0]["received_at"], "1970-01-01T00:50:00Z")
        again = self.call("POST", "/ifttt/v1/triggers/you_yo_ifttt", {**body, "limit": 1}, self.auth(token))[1]
        self.assertEqual([i["meta"]["id"] for i in again["data"]], [out["data"][0]["meta"]["id"]])
        self.assertEqual(self.call("POST", "/ifttt/v1/triggers/you_yo_ifttt", {**body, "limit": 0},
                                   self.auth(token))[1], {"data": []})
        self.assertEqual(self.store.identities(ALICE), ["ti1"])
        self.assertEqual(self.call("DELETE", "/ifttt/v1/triggers/you_yo_ifttt/trigger_identity/ti1",
                                   headers={"IFTTT-Service-Key": KEY})[0], 200)
        self.assertEqual(self.store.identities(ALICE), [])
        # The action Yos the user.
        status, out = self.call("POST", "/ifttt/v1/actions/yo_me", {"actionFields": {"photo_url": ""}},
                                self.auth(token))
        self.assertEqual(status, 200)
        self.assertTrue(out["data"][0]["id"])
        self.assertEqual(self.yo.sent, [(ALICE, None)])

    def test_refused_goes_back_denied(self):
        query = self.link(allow=False)
        self.assertEqual(query["error"], "access_denied")
        self.assertNotIn("code", query)

    def test_bad_signature_and_reuse_are_refused(self):
        status, page = self.call("GET", "/authorize?" + urllib.parse.urlencode(
            {"client_id": CID, "redirect_uri": REDIRECT, "response_type": "code", "state": "s"}))
        ticket = urllib.parse.parse_qs(page.split("yo://connect?")[1].split('"')[0].replace("&amp;", "&"))["ticket"][0]
        for user, sig in (("--hub=x", SIG), (ALICE, "--state"), ("g" * 32, SIG)):
            self.assertEqual(self.call("POST", "/approve", {"ticket": ticket, "user": user, "signature": sig})[0], 400)
        self.assertEqual(self.call("POST", "/approve", {"ticket": ticket, "user": ALICE, "signature": "x" * 86})[0], 403)
        self.yo.good.add((ALICE, "https://justyo.co", ticket, SIG))
        self.assertEqual(self.call("POST", "/approve", {"ticket": ticket, "user": ALICE, "signature": SIG})[0], 410,
                         "one try per ticket")
        self.assertEqual(self.yo.added, [])
        self.assertEqual(self.call("POST", "/approve", {"ticket": "nope", "user": ALICE, "signature": "x"})[0], 410)

    def test_authorize_checks_client_and_redirect(self):
        for query in ({"client_id": "other", "redirect_uri": REDIRECT},
                      {"client_id": CID, "redirect_uri": "https://evil.example/cb"}):
            status, page = self.call("GET", "/authorize?" + urllib.parse.urlencode(query))
            self.assertEqual(status, 400)
            self.assertNotIn("yo://connect", page)

    def test_token_needs_the_client_secret(self):
        status, out = self.call("POST", "/token", {"grant_type": "authorization_code", "code": "c", "client_id": CID,
                                                   "client_secret": "wrong"}, form=True)
        self.assertEqual((status, out["error"]), (401, "invalid_client"))

    def test_api_needs_its_keys(self):
        self.assertEqual(self.call("GET", "/ifttt/v1/status")[0], 401)
        self.assertEqual(self.call("GET", "/ifttt/v1/status", headers={"IFTTT-Service-Key": KEY}), (200, None))
        status, out = self.call("GET", "/ifttt/v1/user/info", headers=self.auth("nope"))
        self.assertEqual(status, 401)
        self.assertTrue(out["errors"][0]["message"])
        self.assertEqual(self.call("POST", "/ifttt/v1/actions/yo_me", {}, self.auth("nope"))[0], 401)
        self.assertEqual(self.call("GET", "/elsewhere")[0], 404)

    def test_setup_gives_a_token_and_three_events(self):
        self.assertEqual(self.call("POST", "/ifttt/v1/test/setup")[0], 401)
        status, out = self.call("POST", "/ifttt/v1/test/setup", headers={"IFTTT-Service-Key": KEY})
        self.assertEqual(status, 200)
        token = out["data"]["accessToken"]
        self.assertIn("you_yo_ifttt", out["data"]["samples"]["triggers"])
        self.assertIn("yo_me", out["data"]["samples"]["actions"])
        events = self.call("POST", "/ifttt/v1/triggers/you_yo_ifttt", {"limit": 50}, self.auth(token))[1]["data"]
        self.assertEqual(len(events), 3)
        self.assertEqual(len({e["meta"]["id"] for e in events}), 3)

    def test_action_skips_a_bad_photo_and_a_lost_friend(self):
        token = self.link()
        status, out = self.call("POST", "/ifttt/v1/actions/yo_me", {"actionFields": {"photo_url": "http://x/a.jpg"}},
                                self.auth(token))
        self.assertEqual((status, out["errors"][0]["status"]), (400, "SKIP"))
        self.yo.fail = yocli.Failed(1, "not on Yo", False)
        status, out = self.call("POST", "/ifttt/v1/actions/yo_me", {"actionFields": {}}, self.auth(token))
        self.assertEqual((status, out["errors"][0]["status"]), (400, "SKIP"))
        self.yo.fail = yocli.Failed(5, "unauthorized", True)
        self.assertEqual(self.call("POST", "/ifttt/v1/actions/yo_me", {"actionFields": {}}, self.auth(token))[0], 503)

    def test_events_are_kept_for_linked_users_only_and_capped(self):
        self.assertFalse(self.store.linked(ALICE))
        self.store.issue(ALICE, "alice")
        for i in range(ifttt.KEEP + 10):
            self.store.record(ALICE, "yo", i)
        rows = self.store.events(ALICE, 1000)
        self.assertEqual(len(rows), ifttt.KEEP)
        self.assertEqual(rows[0][1], ifttt.KEEP + 9)

    def test_bodies_are_capped(self):
        status, _ = self.call("POST", "/ifttt/v1/triggers/you_yo_ifttt", {"x": "y" * (ifttt.MAX_BODY + 1)},
                              self.auth(self.store.issue(ALICE, "alice")))
        self.assertEqual(status, 400)

    def test_page_carries_the_pairing_code_and_its_font(self):
        status, page = self.call("GET", "/authorize?" + urllib.parse.urlencode(
            {"client_id": CID, "redirect_uri": REDIRECT, "response_type": "code", "state": "s"}))
        ticket = urllib.parse.parse_qs(page.split("yo://connect?")[1].split('"')[0].replace("&amp;", "&"))["ticket"][0]
        code = ifttt.pairing_code(ticket)
        self.assertRegex(code, r"^[0-9]{4}$")
        self.assertIn("".join(f"<span>{d}</span>" for d in code), page)
        request = urllib.request.Request(self.base + "/font.ttf")
        with urllib.request.urlopen(request) as r:
            self.assertEqual((r.status, r.headers["Content-Type"]), (200, "font/ttf"))

    def test_too_many_waiting_requests_are_refused(self):
        ifttt.MAX_PENDING, saved = 1, ifttt.MAX_PENDING
        try:
            query = "/authorize?" + urllib.parse.urlencode({"client_id": CID, "redirect_uri": REDIRECT})
            self.assertEqual(self.call("GET", query)[0], 200)
            self.assertEqual(self.call("GET", query)[0], 503)
        finally:
            ifttt.MAX_PENDING = saved

    def test_an_action_needs_its_fields(self):
        token = self.store.issue(ALICE, "alice")
        status, out = self.call("POST", "/ifttt/v1/actions/yo_me", {}, self.auth(token))
        self.assertEqual(status, 400)
        self.assertEqual(self.yo.sent, [])


class Photos(unittest.TestCase):
    def test_only_public_https_hosts(self):
        for url in ["http://example.com/a.jpg", "https://127.0.0.1/a.jpg", "https://localhost/a.jpg",
                    "https://10.0.0.1/a.jpg", "https://[::1]/a.jpg", "https://169.254.169.254/a.jpg", "https:///a.jpg"]:
            photo, problem = ifttt.fetch_photo(url)
            self.assertIsNone(photo, url)
            self.assertTrue(problem, url)
        self.assertTrue(ifttt.public_host("8.8.8.8"))


class QR(unittest.TestCase):
    def test_sizes_and_limits(self):
        self.assertEqual(len(qr.matrix("yo")), 21)
        self.assertEqual(len(qr.matrix("https://justyo.co/connect/approve?t=" + "x" * 32)), 37)
        self.assertEqual(len(qr.matrix("x" * 200)), 57)
        with self.assertRaises(ValueError):
            qr.matrix("x" * 300)
        self.assertTrue(qr.svg("yo").startswith("<svg"))


if __name__ == "__main__":
    unittest.main()
