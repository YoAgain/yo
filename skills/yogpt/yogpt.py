#!/usr/bin/env python3
"""YoGPT: answers every Yo, after a moment (1 to 3 s), with a Yo or a few (one mostly, five at most), keeping the
rhythm the Yos came in. A photo or video is answered with one reply picture from `--replies` (NNN.jpg, at most
512 KB, never one of the last 50 that sender got while others are left); a plain Yo carries one only with
`--picture-every N` (one answer in N). What it is sent is never opened: `yo listen --drain-media` fetches the blobs
into nothing. In a group it answers into the group, at most once per 30 s; never `--quiet-for` users (other bots).
It leaves groups it is alone in. Bounded: 200 answers owed to one sender, 32 senders at once; more are dropped.

The operator's mailbox: `NAME.anything.jpg` put into `$YO_STATE/outbox` goes to NAME as a photo, then the file is
removed (renamed `.failed` when it could not go). `$YO_STATE/stats.json` says how many people it knows, once a minute.

  YO_STATE=… YO_HUB=… yogpt.py [--replies DIR] [--picture-every N] [--quiet-for A,B]     (once: yo register YOGPT)
"""

import argparse
import collections
import json
import os
import random
import re
import sys
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import yocli  # noqa: E402

# How many Yos an answer has, in percent: one mostly, more ever rarer, five at most.
YOS = [70, 17, 8, 4, 1]
THINK_MS = tuple(int(v) for v in os.environ.get("YOGPT_THINK_MS", "1000,3000").split(","))
MAX_GAP_MS = 5000
GROUP_COOLDOWN = int(os.environ.get("YOGPT_GROUP_COOLDOWN_MS", "30000")) / 1000
MAX_ANSWERING, MAX_OWED = 32, 200
MAX_PICTURE, MAX_PICTURES_LISTED = 512 * 1024, 1000
RECENT_PICTURES, MAX_REMEMBERED = 50, 1000
GROUP = "group:"
draw = random.SystemRandom()


def yos(p):
    """How many Yos for a percentile `p` in 0..100, by `YOS`."""
    total = 0
    for i, weight in enumerate(YOS):
        total += weight
        if p < total:
            return i + 1
    return len(YOS)


def picture_due(roll, every):
    """Whether a roll (uniform in 0..every) gives a picture: one in `every`, none for 0."""
    return every > 0 and roll == 0


def picture_name(name):
    return re.fullmatch(r"[0-9]+\.jpg", name) is not None


class Recent:
    """The reply pictures each sender got last, in memory only: `RECENT_PICTURES` per sender, `MAX_REMEMBERED`
    senders, the one remembered longest forgotten first."""

    def __init__(self):
        self.by_sender = collections.OrderedDict()

    def of(self, user):
        return self.by_sender.get(user, collections.deque())

    def sent(self, user, name):
        if user not in self.by_sender:
            if len(self.by_sender) >= MAX_REMEMBERED:
                self.by_sender.popitem(last=False)
            self.by_sender[user] = collections.deque(maxlen=RECENT_PICTURES)
        self.by_sender[user].append(name)


def choose(names, recent):
    """Which of `names`, uniform among those not in `recent`; when every one is, among all but the last one sent."""
    pool = [i for i, n in enumerate(names) if n not in recent]
    if not pool:
        pool = [i for i, n in enumerate(names) if not recent or n != recent[-1]]
    if not pool:
        pool = list(range(len(names)))
    return draw.choice(pool) if pool else None


def pick_picture(directory, recent):
    """A reply picture's path from the directory as it is now, not one of `recent` while others are left."""
    try:
        listed = [e.name for _, e in zip(range(MAX_PICTURES_LISTED), os.scandir(directory))]
    except OSError:
        return None
    names = [n for n in listed if picture_name(n)]
    i = choose(names, recent)
    if i is None:
        return None
    path = os.path.join(directory, names[i])
    try:
        size = os.path.getsize(path)
    except OSError:
        return None
    return (names[i], path) if 0 < size <= MAX_PICTURE else None


class YoGPT:
    def __init__(self, replies, every):
        self.replies, self.every = replies, every
        self.pending, self.recent, self.lock = {}, Recent(), threading.Lock()
        self.dropped, self.answered = yocli.Tally("dropped (too much at once)"), yocli.Tally("answers")

    def offer(self, to, picture, sent_at):
        """One answer owed to `to` (a user or `group:ID`): in turn after those owed already, each sender on its own."""
        with self.lock:
            owed = self.pending.get(to)
            if owed is not None:
                if len(owed) < MAX_OWED:
                    owed.append((picture, sent_at))
                else:
                    self.dropped.add()
                return
            if len(self.pending) >= MAX_ANSWERING:
                return self.dropped.add()
            self.pending[to] = collections.deque([(picture, sent_at)])
        threading.Thread(target=self.answer_pending, args=(to,), daemon=True).start()

    def answer_pending(self, to):
        time.sleep(draw.uniform(*THINK_MS) / 1000)
        while True:
            with self.lock:
                picture, last = self.pending[to][0]
            began = time.monotonic()
            self.answer(to, picture)
            with self.lock:
                owed = self.pending[to]
                owed.popleft()
                if not owed:
                    del self.pending[to]
                    return
                gap = min(max(owed[0][1] - last, 0), MAX_GAP_MS) / 1000
            time.sleep(max(gap - (time.monotonic() - began), 0))

    def answer(self, to, to_picture):
        n = 1 if to_picture else yos(draw.randrange(100))
        with_picture = 0 if to_picture else None
        if not to_picture and picture_due(draw.randrange(max(self.every, 1)), self.every):
            with_picture = draw.randrange(n)
        for i in range(n):
            try:
                if i == with_picture:
                    self.send_picture(to)
                else:
                    yocli.yo("send", to)
            except yocli.Failed as failed:
                return yocli.log(f"Yo {i + 1} of {n}: {yocli.reason(failed)}")
        self.answered.add()

    def send_picture(self, to):
        """A Yo with a reply picture; a plain one when there is none or the hub takes no picture now."""
        with self.lock:
            recent = list(self.recent.of(to))
        picked = pick_picture(self.replies, recent) if self.replies else None
        if picked is None:
            return yocli.yo("send", to)
        name, path = picked
        try:
            yocli.yo("send", to, "--photo", path)
        except yocli.Failed as failed:
            yocli.log(f"picture: {yocli.reason(failed)}")
            return yocli.yo("send", to)
        # Only a picture that went counts against a repeat.
        with self.lock:
            self.recent.sent(to, name)


def leave_empty(me):
    """Leaves every group it is alone in (the others left or removed one another): nobody to answer there."""
    try:
        for group in yocli.yo("groups"):
            if not group["left"] and group["members"] and all(m == me for m in group["members"]):
                yocli.yo("groups", "leave", group["id"])
                yocli.log("left a group it was alone in")
    except yocli.Failed as failed:
        yocli.log(f"leaving empty groups: {yocli.reason(failed)}")


def mailbox(directory, stats):
    """`NAME.anything.jpg` in `directory` goes to NAME as a photo; `stats` is written once a minute."""
    os.makedirs(directory, exist_ok=True)
    ticks = 0
    while True:
        time.sleep(5)
        if ticks % 12 == 0:
            try:
                contacts = yocli.yo("status")["contacts"]
                with open(stats + ".new", "w") as f:
                    json.dump({"contacts": contacts}, f)
                os.replace(stats + ".new", stats)
            except (yocli.Failed, OSError):
                pass
        ticks += 1
        try:
            entries = [e for _, e in zip(range(20), os.scandir(directory))]
        except OSError:
            continue
        for entry in entries:
            if not entry.name.endswith(".jpg"):
                continue
            to = entry.name.split(".")[0]
            try:
                if not to or not 0 < entry.stat().st_size <= MAX_PICTURE:
                    raise yocli.Failed(1, "not a picture to send", False)
                yocli.yo("send", to, "--photo", entry.path)
                os.remove(entry.path)
                yocli.log("mail sent")
            except (yocli.Failed, OSError) as failed:
                yocli.log(f"mail: {yocli.reason(failed)}")
                try:
                    os.replace(entry.path, entry.path[: -len(".jpg")] + ".failed")
                except OSError:
                    pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--replies")
    parser.add_argument("--picture-every", type=int, default=0)
    parser.add_argument("--quiet-for", action="append", default=[])
    args = parser.parse_args()
    state = os.environ["YO_STATE"]
    me = yocli.registered()["user"]
    quiet = yocli.Quiet(yocli.names(args.quiet_for))
    bot, cooldown = YoGPT(args.replies, args.picture_every), yocli.Cooldown(GROUP_COOLDOWN)
    quieted, cooled = yocli.Tally("from --quiet-for users not answered"), yocli.Tally("group Yos dropped (cooldown)")
    outbox = os.path.join(state, "outbox")
    threading.Thread(target=mailbox, args=(outbox, os.path.join(state, "stats.json")), daemon=True).start()
    for event in yocli.listen("--drain-media"):
        kind = event.get("type")
        if kind == "ready":
            leave_empty(me)
        quiet.resolve()
        if kind == "group_changed":
            leave_empty(me)
        if kind != "message" or event["content"] not in ("yo", "photo", "video"):
            continue
        if event["from"] in quiet:
            quieted.add()
            continue
        to = event["reply_to"]
        if to.startswith(GROUP) and not cooldown.allow(to, time.monotonic()):
            cooled.add()
            continue
        bot.offer(to, event["content"] != "yo", event["sent_at_ms"])


if __name__ == "__main__":
    main()
