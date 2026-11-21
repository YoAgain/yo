"""The skills' rules without a hub: `python3 -m unittest discover skills` (cargo test -p yo-cli runs it too)."""

import collections
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "yogpt"))
sys.path.insert(0, os.path.join(HERE, "status"))
import datetime  # noqa: E402
import json  # noqa: E402
import status  # noqa: E402
import yocli  # noqa: E402
import yogpt  # noqa: E402


class YoGPTRules(unittest.TestCase):
    def test_every_answer_is_one_picture_and_they_vary(self):
        with tempfile.TemporaryDirectory() as d:
            for i in range(1, 6):
                with open(os.path.join(d, f"{i:03}.jpg"), "wb") as f:
                    f.write(b"x")
            bot, sent, old = yogpt.YoGPT(d), [], yogpt.yocli.yo
            yogpt.yocli.yo = lambda *args: sent.append(args)
            try:
                for _ in range(10):
                    bot.answer("u")
            finally:
                yogpt.yocli.yo = old
        self.assertEqual(len(sent), 10, "one message per answer")
        self.assertTrue(all(a[:3] == ("send", "u", "--photo") for a in sent), "never a plain Yo")
        names = [os.path.basename(a[3]) for a in sent]
        self.assertEqual(sorted(names[:5]), [f"{i:03}.jpg" for i in range(1, 6)], "each once before any again")
        self.assertTrue(all(x != y for x, y in zip(names, names[1:])), "never the same twice in a row")

    def test_a_plain_yo_only_when_no_picture_can_go(self):
        sent, old = [], yogpt.yocli.yo

        def refuse_pictures(*args):
            if "--photo" in args:
                raise yogpt.yocli.Failed(1, "no pictures now", False)
            sent.append(args)

        try:
            yogpt.yocli.yo = lambda *args: sent.append(args)
            yogpt.YoGPT(None).answer("u")
            with tempfile.TemporaryDirectory() as d:
                with open(os.path.join(d, "001.jpg"), "wb") as f:
                    f.write(b"x")
                yogpt.yocli.yo = refuse_pictures
                yogpt.YoGPT(d).answer("u")
        finally:
            yogpt.yocli.yo = old
        self.assertEqual(sent, [("send", "u"), ("send", "u")])

    def test_reply_pictures_are_named_by_number_and_read_bounded(self):
        self.assertTrue(yogpt.picture_name("007.jpg") and yogpt.picture_name("200.jpg"))
        for name in ["", ".jpg", "a.jpg", "007.jpeg", "007.jpg.sh", "../1.jpg", "1.JPG"]:
            self.assertFalse(yogpt.picture_name(name), name)
        with tempfile.TemporaryDirectory() as d:
            self.assertIsNone(yogpt.pick_picture(d, []), "nothing there")
            with open(os.path.join(d, "001.jpg"), "wb") as f:
                f.write(b"x" * (yogpt.MAX_PICTURE + 1))
            self.assertIsNone(yogpt.pick_picture(d, []), "too large")
            with open(os.path.join(d, "001.jpg"), "wb") as f:
                f.write(b"x")
            with open(os.path.join(d, "notes.txt"), "wb") as f:
                f.write(b"x")
            self.assertEqual(yogpt.pick_picture(d, []), ("001.jpg", os.path.join(d, "001.jpg")))

    def test_every_picture_comes_up_about_equally_often(self):
        names = [f"{i:03}.jpg" for i in range(1, 21)]
        counts = [0] * 20
        for _ in range(20_000):
            counts[yogpt.choose(names, [])] += 1
        # 1000 expected each, a standard deviation of about 31.
        self.assertTrue(all(850 <= c <= 1150 for c in counts), counts)
        self.assertIsNone(yogpt.choose([], []))

    def test_no_picture_again_to_the_same_sender_within_the_last_fifty(self):
        names = [f"{i:03}.jpg" for i in range(1, 201)]
        recent, sent, counts = yogpt.Recent(), [], [0] * 200
        for _ in range(20_000):
            i = yogpt.choose(names, recent.of("a"))
            self.assertNotIn(i, sent[-yogpt.RECENT_PICTURES:])
            recent.sent("a", names[i])
            sent.append(i)
            counts[i] += 1
        self.assertTrue(all(60 <= c <= 140 for c in counts), counts)
        self.assertEqual(len(recent.of("b")), 0, "another sender is not held back")
        # Fewer pictures than the window: never the same twice in a row, and every one comes up.
        few, recent, last, seen = names[:5], yogpt.Recent(), None, set()
        for _ in range(1000):
            i = yogpt.choose(few, recent.of("a"))
            self.assertNotEqual(i, last)
            recent.sent("a", few[i])
            last = i
            seen.add(i)
        self.assertEqual(seen, set(range(5)))
        self.assertEqual(yogpt.choose(names[:1], recent.of("a")), 0, "a single picture is still sent")

    def test_recent_pictures_stay_bounded(self):
        recent = yogpt.Recent()
        for i in range(yogpt.MAX_REMEMBERED + 10):
            for j in range(yogpt.RECENT_PICTURES + 5):
                recent.sent(f"user{i}", f"{j:03}.jpg")
        self.assertEqual(len(recent.by_sender), yogpt.MAX_REMEMBERED)
        self.assertEqual(len(recent.of("user0")), 0, "the oldest sender forgotten")
        last = recent.of(f"user{yogpt.MAX_REMEMBERED + 9}")
        self.assertEqual((len(last), last[0]), (yogpt.RECENT_PICTURES, "005.jpg"))


class Limits(unittest.TestCase):
    def test_a_group_is_answered_once_per_window(self):
        cooldown, now = yocli.Cooldown(30), 1000.0
        self.assertTrue(cooldown.allow("group:a", now))
        self.assertFalse(cooldown.allow("group:a", now + 29))
        self.assertTrue(cooldown.allow("group:b", now + 1), "another group is not held back")
        self.assertTrue(cooldown.allow("group:a", now + 30))
        self.assertFalse(cooldown.allow("group:a", now + 31))
        self.assertEqual(len(cooldown.at), 1, "only groups within the window are kept")

    def test_a_failed_answer_never_blocks_its_sender(self):
        bot = yogpt.YoGPT(None)
        bot.pending["u"] = collections.deque([0])
        bot.answer = lambda to: 1 / 0
        old, yogpt.THINK_MS = yogpt.THINK_MS, (0, 0)
        try:
            bot.answer_pending("u")
        finally:
            yogpt.THINK_MS = old
        self.assertNotIn("u", bot.pending, "the next Yo of this sender is answered again")

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
