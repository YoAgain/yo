"""What the skills share: running `yo`, reading `yo listen`, and the volatile limits.

Anyone can write to a bot, so what comes is hostile: a skill acts on a Yo, a photo, a video or a probe and nothing
else, never runs or logs what a message says, and logs counts only, never who.
"""

import json
import os
import subprocess
import sys
import threading
import time

YO = os.environ.get("YO_BIN", "yo")
# `yo`'s exit codes (app/cli/src/main.rs).
GONE, NOT_REGISTERED = 3, 4


class Failed(Exception):
    def __init__(self, code, error, retryable):
        super().__init__(error)
        self.code, self.error, self.retryable = code, error, retryable


def log(what):
    print(f"{os.path.basename(sys.argv[0])}: {what}", file=sys.stderr, flush=True)


def stop(code):
    """Ends the whole skill (any thread): systemd restarts it, unless the device is gone (3)."""
    if code == GONE:
        log("stopped: the hub no longer knows this device (replaced by another, the name released, or the hub reset)")
    sys.stderr.flush()
    os._exit(code)


def yo(*args, timeout=180):
    """Runs one `yo` command: its JSON output, or `Failed`. A device the hub no longer knows ends the skill."""
    done = subprocess.run([YO, *args], capture_output=True, text=True, timeout=timeout)
    if done.returncode == 0:
        return json.loads(done.stdout)
    if done.returncode == GONE:
        stop(GONE)
    try:
        error = json.loads(done.stderr.strip().splitlines()[-1])
    except (ValueError, IndexError):
        error = {"error": "no answer", "retryable": False}
    raise Failed(done.returncode, error.get("error"), error.get("retryable", False))


def registered(username):
    """The account (`whoami`): registered first as `username` (YO_USERNAME) when the directory has none yet."""
    wait = 1
    while True:
        try:
            return yo("whoami")
        except Failed as failed:
            if failed.code == NOT_REGISTERED and username:
                try:
                    yo("register", username)
                    log(f"registered {username}")
                    continue
                except Failed as again:
                    failed = again
            if not failed.retryable:
                log(f"stopped: {failed.error}")
                stop(1)
            log(f"{failed.error}, again in {wait} s")
            time.sleep(wait)
            wait = min(wait * 2, 60)


def listen(*flags):
    """`yo listen`'s events, one dict each, until it ends; then the skill ends with its exit code."""
    process = subprocess.Popen([YO, "listen", *flags], stdout=subprocess.PIPE, text=True, bufsize=1)
    for line in process.stdout:
        try:
            yield json.loads(line)
        except ValueError:
            continue
    stop(process.wait() or 1)


class Tally:
    """A count logged at most once a minute."""

    def __init__(self, what):
        self.what, self.n, self.logged, self.lock = what, 0, 0.0, threading.Lock()

    def add(self):
        with self.lock:
            self.n += 1
            if time.monotonic() - self.logged >= 60:
                self.logged = time.monotonic()
                log(f"{self.n} {self.what} so far")


class Cooldown:
    """One answer per key (a group) per `window` seconds; only keys within the window are kept."""

    def __init__(self, window):
        self.window, self.at = window, {}

    def allow(self, key, now):
        self.at = {k: t for k, t in self.at.items() if now - t < self.window}
        if key in self.at:
            return False
        self.at[key] = now
        return True


class Buckets:
    """Volatile token buckets per sender: 1/s, burst 3; idle ones expire after 3 s. At most `limit` at once and no
    eviction of active ones (a flood of fresh senders cannot reset another sender's limit)."""

    def __init__(self, rate=1.0, burst=3.0, idle=3.0, limit=1000):
        self.rate, self.burst, self.idle, self.limit, self.buckets = rate, burst, idle, limit, {}

    def allow(self, user, now):
        self.buckets = {u: b for u, b in self.buckets.items() if now - b[0] < self.idle}
        if user not in self.buckets and len(self.buckets) >= self.limit:
            return False
        last, tokens = self.buckets.get(user, (now, self.burst))
        tokens = min(tokens + (now - last) * self.rate, self.burst)
        if tokens < 1:
            self.buckets[user] = (now, tokens)
            return False
        self.buckets[user] = (now, tokens - 1)
        return True


class Quiet:
    """`--quiet-for`: users (other bots) never answered. Names are looked up once; one not found yet (or the hub not
    reachable) is tried again a minute later; its own name is left out."""

    def __init__(self, names, retry=60):
        self.unresolved = [n.strip().lstrip("@") for n in names if n.strip()]
        self.users, self.retry, self.tried = set(), retry, None

    def __contains__(self, user):
        return user in self.users

    def resolve(self):
        if not self.unresolved or (self.tried is not None and time.monotonic() - self.tried < self.retry):
            return
        self.tried = time.monotonic()
        left = []
        for name in self.unresolved:
            try:
                self.users.add(yo("whois", name)["user"])
            except Failed as failed:
                if failed.error == "protocol: oneself":
                    continue
                log(f"--quiet-for {name}: {failed.error}" + (", again later" if failed.retryable or failed.error == "not on Yo" else ""))
                if failed.retryable or failed.error == "not on Yo":
                    left.append(name)
        self.unresolved = left


def names(values):
    """`--quiet-for A,B` given once or more: the names."""
    return [n for value in values for n in value.split(",") if n.strip()]
