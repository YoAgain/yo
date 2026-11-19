"""App channels and states from App Store Connect fixtures (no network), and the ES256 signature's shape."""
import os
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import users

STORE = {
    "builds": [
        {"build": "202611191257", "version": "1.0.0", "uploaded": "2026-11-19T12:57:00Z", "expired": False},
        {"build": "202611181859", "version": "1.0.0", "uploaded": "2026-11-18T18:59:00Z", "expired": False},
        {"build": "202611171622", "version": "1.0.0", "uploaded": "2026-11-17T16:22:00Z", "expired": False},
        {"build": "202608011200", "version": "0.9.0", "uploaded": "2026-08-01T12:00:00Z", "expired": True},
    ],
    "full": False,
    "public": ["202611181859", "202611171622", "202608011200"],
    "team": ["202611191257", "202611181859", "202611171622", "202608011200"],
    "appstore": [],
}


class ClassifyTests(unittest.TestCase):
    def c(self, app, store=STORE, bot=None):
        return users.classify(app, store, bot)

    def test_testflight_public(self):
        self.assertEqual((self.c("ios/202611181859")["channel"], self.c("ios/202611181859")["state"]), ("testflight", "newest"))
        self.assertEqual(self.c("ios/202611181859")["label"], "TestFlight 1859")
        self.assertEqual(self.c("ios/202611171622")["state"], "older")
        self.assertEqual(self.c("ios/202608011200")["state"], "expired")

    def test_testflight_internal(self):
        r = self.c("ios/202611191257")
        self.assertEqual((r["channel"], r["state"], r["label"]), ("internal", "newest", "TestFlight (internal) 1257"))

    def test_dev_builds(self):
        self.assertEqual(self.c("ios/1")["channel"], "dev")
        self.assertIsNone(self.c("ios/1")["state"])
        self.assertEqual(self.c("ios/202611191300")["channel"], "dev")  # not in App Store Connect

    def test_old_builds_beyond_the_list_are_expired(self):
        self.assertEqual(self.c("ios/202601010000", {**STORE, "full": True})["state"], "expired")

    def test_app_store(self):
        store = {**STORE, "appstore": [{"version": "1.0.0", "build": "202611171622", "live": False},
                                       {"version": "1.0.1", "build": "202611181859", "live": True}]}
        r = self.c("ios/202611181859", store)
        self.assertEqual((r["channel"], r["state"], r["label"]), ("appstore", "newest", "App Store 1.0.1 (1859)"))
        self.assertEqual(self.c("ios/202611171622", store)["state"], "older")

    def test_without_app_store_connect_never_update(self):
        for store in (None, {}, {"error": "HTTPError"}):
            r = self.c("ios/202611171622", store)
            self.assertEqual((r["channel"], r["state"]), ("ios", None))

    def test_other_platforms(self):
        self.assertEqual(self.c("yogpt/202611182101", bot="yogpt/202611182059")["state"], "newest")
        self.assertEqual(self.c("yogpt/202611181000", bot="yogpt/202611182059")["state"], "older")
        self.assertEqual(self.c("yogpt/202611181000")["state"], None)
        self.assertEqual(self.c("android/202611160001")["state"], None)
        self.assertEqual(self.c(None)["channel"], "unknown")

    def test_app_builds_from_both_hubs(self):
        out = {"asc": STORE, "live": {"apps": [["ios/202611171622", 2]], "users": [{"app": "ios/1", "device_list": []}],
                                      "yogpt_min": "yogpt/202611182059"},
               "dev": {"apps": [["yogpt/202611182101", 1], ["unknown", 3]], "users": []}}
        b = users.app_builds(out)
        self.assertEqual(set(b), {"ios/202611171622", "ios/1", "yogpt/202611182101"})
        self.assertEqual(b["yogpt/202611182101"]["state"], "newest")


class AscTests(unittest.TestCase):
    def test_not_configured(self):
        with patch.dict(os.environ, {"YO_ASC_KEY_PATH": "", "YO_ASC_KEY_ID": "", "YO_ASC_ISSUER_ID": ""}):
            self.assertIsNone(users.asc({}))

    def test_cache_and_failure_keep_data(self):
        env = {"YO_ASC_KEY_PATH": "/k", "YO_ASC_KEY_ID": "K", "YO_ASC_ISSUER_ID": "I"}
        with patch.dict(os.environ, env), patch.object(users, "asc_fetch", side_effect=OSError("down")) as fetch:
            fresh = {**STORE, "checked": 1000}
            self.assertIs(users.asc(fresh, now=1000 + 60), fresh)
            fetch.assert_not_called()
            failed = users.asc(fresh, now=1000 + users.ASC_EVERY_S)
            self.assertEqual(failed["builds"], STORE["builds"])
            self.assertIn("down", failed["error"])
        with patch.dict(os.environ, env), patch.object(users, "asc_fetch", return_value=STORE):
            self.assertNotIn("error", users.asc(failed, now=5000))

    def test_der_to_raw(self):
        der = bytes([0x30, 0x45, 0x02, 0x21, 0x00]) + b"\x81" * 32 + bytes([0x02, 0x20]) + b"\x02" * 32
        self.assertEqual(users.der_to_raw(der), b"\x81" * 32 + b"\x02" * 32)
        short = bytes([0x30, 0x06, 0x02, 0x01, 0x05, 0x02, 0x01, 0x07])
        self.assertEqual(users.der_to_raw(short), (5).to_bytes(32, "big") + (7).to_bytes(32, "big"))

    def test_token_signs_es256(self):
        with tempfile.TemporaryDirectory() as d:
            key = os.path.join(d, "k.p8")
            try:
                subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", key],
                               check=True, capture_output=True)
            except (OSError, subprocess.CalledProcessError):
                self.skipTest("no openssl")
            token = users.asc_token({"KEY_PATH": key, "KEY_ID": "K", "ISSUER_ID": "I"}, now=1)
            head, body, sig = token.split(".")
            import base64
            self.assertEqual(len(base64.urlsafe_b64decode(sig + "==")), 64)


if __name__ == "__main__":
    unittest.main()
