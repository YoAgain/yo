#!/usr/bin/env python3
"""PING: a visible echo contact. Everything it gets goes back once, as it came, at once: a Yo as a Yo, a photo as the
same photo, a video as the same video (fetched and opened in memory by `yo send --media`, sealed anew, never stored).
In a group into the group, a group once per 30 s; never to `--quiet-for` users (the other bots). Per sender 1/s,
burst 3; at most 64 photos, videos and group Yos wait for the one echo worker, more are dropped. Logs counts only.

  YO_STATE=… YO_HUB=… [YO_USERNAME=PING] ping.py [--quiet-for YOGPT,PINGTEST]
"""

import argparse
import os
import queue
import sys
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import yocli  # noqa: E402

GROUP_COOLDOWN = int(os.environ.get("PING_GROUP_COOLDOWN_MS", "30000")) / 1000
WAITING = 64


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--quiet-for", action="append", default=[])
    args = parser.parse_args()
    yocli.registered(os.environ.get("YO_USERNAME"))
    quiet = yocli.Quiet(yocli.names(args.quiet_for))
    limits, cooldown = yocli.Buckets(), yocli.Cooldown(GROUP_COOLDOWN)
    dropped, quieted, cooled = yocli.Tally("dropped"), yocli.Tally("from --quiet-for users"), yocli.Tally("cooled")
    answered = [0]
    lock = threading.Lock()

    def echo(*send):
        try:
            yocli.yo("send", *send)
        except yocli.Failed:
            return dropped.add()
        with lock:
            answered[0] += 1
            yocli.log(f"echo answered={answered[0]}")

    jobs = queue.Queue(WAITING)

    def worker():
        while True:
            echo(*jobs.get())

    threading.Thread(target=worker, daemon=True).start()
    for event in yocli.listen():
        quiet.resolve()
        if event.get("type") != "message" or event["content"] not in ("yo", "photo", "video"):
            continue
        sender, to = event["from"], event["reply_to"]
        if sender in quiet:
            quieted.add()
            continue
        if event["group"] and not cooldown.allow(to, time.monotonic()):
            cooled.add()
            continue
        if not limits.allow(sender, time.monotonic()):
            dropped.add()
            continue
        if event["content"] == "yo" and not event["group"]:
            echo(to)
            continue
        send = (to,) if event["content"] == "yo" else (to, "--media", (event["media"] or {}).get("ref", ""))
        try:
            jobs.put_nowait(send)
        except queue.Full:
            dropped.add()


if __name__ == "__main__":
    main()
