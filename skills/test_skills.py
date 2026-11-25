"""The skills' rules without a hub: `python3 -m unittest discover skills` (cargo test -p yo-cli runs it too)."""

import collections
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "status"))
import datetime  # noqa: E402
import json  # noqa: E402
import status  # noqa: E402
import yocli  # noqa: E402


class Limits(unittest.TestCase):
    def test_a_group_is_answered_once_per_window(self):
        cooldown, now = yocli.Cooldown(30), 1000.0
        self.assertTrue(cooldown.allow("group:a", now))
        self.assertFalse(cooldown.allow("group:a", now + 29))
        self.assertTrue(cooldown.allow("group:b", now + 1), "another group is not held back")
        self.assertTrue(cooldown.allow("group:a", now + 30))
        self.assertFalse(cooldown.allow("group:a", now + 31))
        self.assertEqual(len(cooldown.at), 1, "only groups within the window are kept")

    def test_sender_buckets_refill_expire_and_stay_bounded(self):
        limits, now = yocli.Buckets(), 1000.0
        for _ in range(3):
            self.assertTrue(limits.allow("sender", now))
        self.assertFalse(limits.allow("sender", now))
        self.assertFalse(limits.allow("sender", now + 0.999))
        self.assertTrue(limits.allow("sender", now + 1))
        self.assertFalse(limits.allow("sender", now + 1))
        for n in range(limits.limit - 1):
            self.assertTrue(limits.allow(str(n), now + 1))
        self.assertFalse(limits.allow("overflow", now + 1))
        self.assertFalse(limits.allow("sender", now + 1))
        self.assertTrue(limits.allow("fresh", now + 5))
        self.assertEqual(len(limits.buckets), 1)

    def test_quiet_for_names_split_and_trim(self):
        self.assertEqual(yocli.names(["leo, @mia", "noah"]), ["leo", " @mia", "noah"])
        self.assertEqual(yocli.Quiet(yocli.names(["leo, @mia"])).unresolved, ["leo", "mia"])


class Yo(unittest.TestCase):
    def test_a_stuck_or_missing_yo_fails_without_its_arguments(self):
        saved = yocli.YO
        try:
            yocli.YO = sys.executable
            with self.assertRaises(yocli.Failed) as stuck:
                yocli.yo("-c", "import time; time.sleep(5)", "SECRETREF", timeout=0.2)
            yocli.YO = os.path.join(HERE, "no-such-yo")
            with self.assertRaises(yocli.Failed) as missing:
                yocli.yo("send", "SECRETREF")
        finally:
            yocli.YO = saved
        self.assertTrue(stuck.exception.retryable)
        for failed in (stuck.exception, missing.exception):
            self.assertNotIn("SECRETREF", f"{failed} {failed.args} {failed.__cause__} {failed.__context__}")

    def test_nothing_registered_stops_without_registering(self):
        calls, saved = [], (yocli.yo, yocli.stop)

        def yo(*args, **_):
            calls.append(args)
            raise yocli.Failed(yocli.NOT_REGISTERED, "not registered: yo register NAME", False)

        def stop(code):
            raise SystemExit(code)

        yocli.yo, yocli.stop = yo, stop
        try:
            with self.assertRaises(SystemExit) as stopped:
                yocli.registered()
        finally:
            yocli.yo, yocli.stop = saved
        self.assertEqual((stopped.exception.code, calls), (yocli.NOT_REGISTERED, [("whoami",)]))

    def test_only_the_class_of_an_error_is_logged(self):
        failed = lambda text: yocli.reason(yocli.Failed(1, text, True))  # noqa: E731
        self.assertEqual(failed("network: error sending request for url (http://hub/v1/blobs/SECRET)"), "network")
        self.assertEqual(failed("refused: 429"), "refused")
        self.assertEqual(failed("/var/lib/yo/outbox/NAME.x.jpg: No such file"), "failed")
        self.assertEqual(yocli.reason(OSError("/var/lib/yo/outbox/NAME.x.jpg")), "OSError")


class StatusRules(unittest.TestCase):
    def config(self, data):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "c.json")
            if data is not None:
                with open(path, "w") as f:
                    f.write(data if isinstance(data, str) else json.dumps(data))
            return status.config(path)

    def test_config_is_off_unless_whole_and_valid(self):
        off = {"every_minutes": 0, "baseline_users": 0}
        self.assertEqual(self.config({"every_minutes": 30, "to": "Lina", "baseline_users": 3}),
                         {"every_minutes": 30, "baseline_users": 3})
        self.assertEqual(self.config({"every_minutes": 0}), off)
        for bad in [None, "{", "[]", {"every_minutes": 7}, {"every_minutes": "30"}, {"every_minutes": True},
                    {"every_minutes": 30, "baseline_users": -1}]:
            with self.subTest(bad=bad):
                self.assertEqual(self.config(bad), off)

    def test_release_features_are_the_bullets_plain(self):
        notes = "Seventeenth release candidate.\n\n- iOS: **bold** `code`\n* [Link](https://x) text\n  - nested\n"
        self.assertEqual(status.features(notes), ["iOS: bold code", "Link text", "nested"])
        self.assertEqual(status.features("Intro.\n\nOne thing.\n\nTwo\nlines."), ["One thing.", "Two lines."])
        self.assertEqual(status.features(""), [])

    def test_sends_only_to_the_fixed_user_id(self):
        user = "ab" * 16
        self.assertEqual(status.recipient(" " + user.upper() + "\n"), user)
        for bad in ["", "Lina", "ab" * 15, "ab" * 17, "group:" + user, "zz" * 16]:
            self.assertIsNone(status.recipient(bad), bad)
        sent = []
        real, yocli.yo = yocli.yo, lambda *a, **k: sent.append(a)
        try:
            for to, allowed in [("cd" * 16, user), ("Lina", user), (user, None), ("group:" + user, user)]:
                with self.subTest(to=to), self.assertRaises(PermissionError):
                    status.send_photo(to, allowed, "/x.jpg")
            status.send_photo(user, user, "/x.jpg")
        finally:
            yocli.yo = real
        self.assertEqual(sent, [("send", user, "--photo", "/x.jpg")])

    def test_only_a_yo_from_the_recipient_asks_for_a_report(self):
        user, other = "ab" * 16, "cd" * 16
        yo = {"type": "message", "from": user, "group": None, "reply_to": user, "content": "yo", "media": None}
        cooldown = yocli.Cooldown(status.ON_DEMAND)
        for event in [{**yo, "from": other, "reply_to": other}, {**yo, "content": "photo"}, {**yo, "content": "other"},
                      {**yo, "group": "g1", "reply_to": "group:g1"}, {"type": "new_chat", "user": user},
                      {"type": "probe", "from": user}]:
            with self.subTest(event=event):
                self.assertFalse(status.asked(event, user, cooldown, 0.0))
        self.assertTrue(status.asked(yo, user, cooldown, 0.0))
        self.assertFalse(status.asked(yo, user, cooldown, 10.0), "once per 30 s")
        self.assertTrue(status.asked(yo, user, cooldown, 31.0))

    def test_slots_are_whole_multiples_of_berlin_time(self):
        at = lambda *a: datetime.datetime(2026, *a, tzinfo=status.BERLIN)  # noqa: E731
        self.assertEqual(status.next_slot(at(11, 19, 16, 29, 59), 30), at(11, 19, 16, 30))
        self.assertEqual(status.next_slot(at(11, 19, 16, 30), 30), at(11, 19, 17, 0))
        self.assertEqual(status.next_slot(at(11, 19, 23, 50), 120), at(11, 20, 0, 0))
        self.assertEqual(status.next_slot(at(11, 19, 8, 0), 1440), at(11, 19, 9, 0))
        self.assertEqual(status.next_slot(at(11, 19, 9, 0), 1440), at(11, 20, 9, 0))
        self.assertIsNone(status.next_slot(at(11, 19, 9, 0), 0))

    def test_rows_show_what_is_missing(self):
        n = {"users": 42, "crashes": None, "up": 6, "total": 6, "load": "0.12"}
        self.assertEqual(status.rows(n, 30), [("USERS", "42"), ("SINCE THE POST", "+12"), ("SERVICES", "6/6 up"),
                                              ("LOAD · CRASHES", "0.12 · ?")])
        self.assertEqual(status.rows({**n, "users": None}, 30)[1], ("SINCE THE POST", "?"))


if __name__ == "__main__":
    unittest.main()
