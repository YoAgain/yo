#!/usr/bin/env python3
"""Yo-to-Yo round trip between two CLI accounts: A sends a Yo to B, B's responder answers it at once over B's
listener's control socket (as a bot does), A's listener prints the answer.

  dev/latency.py YO_BINARY STATE_A STATE_B [N] [--socket]

Both states are signed up and friends already. Each round: t0, `yo send B` (a new process; --socket: A's control
socket directly), then A's listener line for B's answer. Prints per round: round trip, A->B one way (B's line minus
A's sent_at_ms), B->A one way (A's line minus B's sent_at_ms), and the medians. Lines older than the start are skipped.
"""
import json, os, socket, statistics, subprocess, sys, threading, time

yo, state_a, state_b = sys.argv[1:4]
rest = sys.argv[4:]
n = int(next((a for a in rest if a.isdigit()), "20"))
via_socket = "--socket" in rest
hub = os.environ.get("YO_HUB_URL", "https://dev.justyo.app")
extra = os.environ.get("YO_EXTRA", "").split()
env = dict(os.environ)


def cmd(state, *args):
    return [yo, "--hub", hub, *extra, "--state", state, "--json", *args]


def whoami(state):
    out = subprocess.run(cmd(state, "status"), capture_output=True, text=True, env=env).stdout
    return json.loads(out)


def control(state, words):
    s = socket.socket(socket.AF_UNIX)
    s.connect(os.path.join(state, "control.sock"))
    s.sendall((json.dumps(words) + "\n").encode())
    data = b""
    while not data.endswith(b"\n"):
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
    s.close()
    return data


start_ms = int(time.time() * 1000)
user_a, user_b = whoami(state_a)["user"], whoami(state_b)["user"]
lines = {"a": [], "b": []}
cond = threading.Condition()


def reader(name, proc, on_line):
    for raw in proc.stdout:
        at = time.time() * 1000
        try:
            value = json.loads(raw)
        except ValueError:
            continue
        with cond:
            lines[name].append((at, value))
            cond.notify_all()
        on_line(at, value)


def responder(at, value):
    if value.get("type") == "message" and value.get("from") == user_a and value.get("sent_at_ms", 0) >= start_ms:
        control(state_b, ["send", user_a])


listeners = {}
for name, state, hook in (("a", state_a, lambda at, v: None), ("b", state_b, responder)):
    log = open(os.path.join(state, "listen.log"), "w")
    proc = subprocess.Popen(cmd(state, "listen"), stdout=subprocess.PIPE, stderr=log, text=True, env=env)
    listeners[name] = proc
    threading.Thread(target=reader, args=(name, proc, hook), daemon=True).start()
with cond:
    cond.wait_for(lambda: all(any(v.get("type") == "ready" for _, v in lines[k]) for k in "ab"), timeout=30)
time.sleep(2)

rtt, a_to_b, b_to_a = [], [], []
lost = 0
gap = float(os.environ.get("GAP", "1.5"))
try:
    for i in range(n):
        with cond:
            seen_a, seen_b = len(lines["a"]), len(lines["b"])
        t0 = time.time() * 1000
        if via_socket:
            control(state_a, ["send", user_b])
        else:
            subprocess.run(cmd(state_a, "send", user_b), capture_output=True, env=env)
        sent_done = time.time() * 1000

        def answered():
            return next((x for x in lines["a"][seen_a:] if x[1].get("type") == "message"
                         and x[1].get("from") == user_b and x[1].get("sent_at_ms", 0) >= t0 - 5), None)

        with cond:
            got = cond.wait_for(answered, timeout=5)
            arrival = next((x for x in lines["b"][seen_b:] if x[1].get("type") == "message"
                            and x[1].get("from") == user_a), None)
        if not got:
            print(f"{i:2d} lost (no answer in 5 s)", flush=True)
            lost += 1
            continue
        at, value = got
        rtt.append(at - t0)
        if arrival:
            a_to_b.append(arrival[0] - arrival[1]["sent_at_ms"])
        b_to_a.append(at - value["sent_at_ms"])
        print(f"{i:2d} rtt {at - t0:6.1f}  send-cmd {sent_done - t0:6.1f}  a->b {a_to_b[-1] if arrival else -1:6.1f}"
              f"  b->a {b_to_a[-1]:6.1f}", flush=True)
        if i + 1 < n:
            if gap > 5:
                print(f"   idle {gap:.0f} s on purpose (GAP)", flush=True)
            time.sleep(gap)
finally:
    for proc in listeners.values():
        proc.terminate()
    for proc in listeners.values():
        proc.wait()


def summary(name, xs):
    if xs:
        xs = sorted(xs)
        print(f"{name:6s} median {statistics.median(xs):6.1f}  min {xs[0]:6.1f}  p90 {xs[int(len(xs) * 0.9) - 1]:6.1f}"
              f"  max {xs[-1]:6.1f}  n {len(xs)}")


print(f"lost {lost}")
summary("rtt", rtt)
summary("a->b", a_to_b)
summary("b->a", b_to_a)
