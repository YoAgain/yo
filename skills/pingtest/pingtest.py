#!/usr/bin/env python3
"""PINGTEST: the round-trip probe the apps measure (Account title, Status: Server roundtrip). A probe carries a random
encrypted token; it goes back at once, only to the token's sender (`yo echo-probe`). Probes are user-triggered and
silent: no contact, request, list event or notification at the app. Nothing per user is stored or logged, no history,
no timing; the per-sender limits (1/s, burst 3) live in memory only and expire after 3 s. Logs counts only. Anything
else sent to it is taken in and left unanswered; photos' and videos' blobs are fetched into nothing, never opened.

  YO_STATE=… YO_HUB=… pingtest.py     (once before: yo register PINGTEST)
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import yocli  # noqa: E402


def main():
    yocli.registered()
    limits, dropped, answered = yocli.Buckets(), yocli.Tally("dropped"), 0
    for event in yocli.listen("--drain-media"):
        if event.get("type") != "probe":
            continue
        if not limits.allow(event["from"], time.monotonic()):
            dropped.add()
            continue
        try:
            yocli.yo("echo-probe", event["from"], event["token"])
        except yocli.Failed:
            dropped.add()
            continue
        answered += 1
        yocli.log(f"echo answered={answered}")


if __name__ == "__main__":
    main()
