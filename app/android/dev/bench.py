#!/usr/bin/env python3
"""Repeatable device benchmarks for the Android app on the DEV hub (never the live one).

  dev/bench.py --cli DIR --peer NAME --me NAME [--runs N] [--trace] > result.json

What it measures, on the USB phone with the QA package (`-PyoDeviceSoak=true`, `app.justyo.android.dev.soak`) signed in
as `--me`, and a CLI account in `--cli DIR` (a `yo` state directory, `yo` on PATH or YO_BIN) that is `--me`'s friend:

  cold_start_ms     am start -W after force-stop (TotalTime), N runs
  receive_ms        the peer's Yo (its sent_at) to the phone's sync answer, the bar's arrival and the next frame
  send_ms           a tap on the top bar to the core's send start/end, the "Sent" frame, the peer's receipt
  frames            main-thread frames over the 8.3 ms budget (120 Hz) while Yos come and go (Perfetto, --trace)
  gfxinfo           the system's own janky-frame count over the same scenario

The app logs latency marks (logcat tag YoLat) only when launched with `--ez yo_perf true` (Lat.kt). The peer's bar
must be the top bar: each round starts with a Yo from the peer, which puts it there. Phone and host clocks are
matched by the round trip of `echo $EPOCHREALTIME`. Frames need the Python `perfetto` package (trace processor).
"""

import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import threading
import time

PACKAGE = "app.justyo.android.dev.soak"
ACTIVITY = f"{PACKAGE}/app.justyo.android.DefaultIcon"


def adb(*args, **kw):
    return subprocess.run(["adb", *args], capture_output=True, text=True, **kw).stdout


def summary(values):
    values = sorted(v for v in values if v is not None)
    if not values:
        return None
    pick = lambda q: values[min(len(values) - 1, int(round(q * (len(values) - 1))))]
    return {"n": len(values), "median": round(statistics.median(values), 1), "p90": round(pick(0.9), 1),
            "min": round(values[0], 1), "max": round(values[-1], 1)}


def clock_offset_us():
    """The phone's wall clock minus the host's, from the fastest of seven round trips."""
    best = None
    for _ in range(7):
        before = time.time()
        phone = float(adb("shell", "echo $EPOCHREALTIME").strip())
        after = time.time()
        if best is None or after - before < best[1]:
            best = ((phone - (before + after) / 2) * 1e6, after - before)
    return best[0]


class Cli:
    def __init__(self, state):
        self.env = dict(os.environ, YO_STATE=state)
        self.bin = os.environ.get("YO_BIN", "yo")
        self.received = []

    def run(self, *args):
        return subprocess.run([self.bin, *args], env=self.env, capture_output=True, text=True)

    def listen(self):
        self.process = subprocess.Popen([self.bin, "listen", "--drain-media"], env=self.env, text=True,
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)

        def read():
            for line in self.process.stdout:
                now = time.time() * 1e6
                try:
                    if json.loads(line).get("type") == "message":
                        self.received.append(now)
                except ValueError:
                    pass
        threading.Thread(target=read, daemon=True).start()
        time.sleep(4)

    def stop(self):
        self.process.terminate()


def marks(offset):
    log = adb("logcat", "-d", "-s", "YoLat")
    found = []
    for line in log.splitlines():
        match = re.search(r"YoLat\s*:\s*(\d+) (.*)", line)
        if match:
            found.append((int(match.group(1)) - offset, match.group(2)))
    return found


def first_after(found, start, prefix, within=8e6):
    return next((t for t, what in found if what.startswith(prefix) and start <= t <= start + within), None)


def cold_start(runs):
    times = []
    for _ in range(runs):
        adb("shell", "am", "force-stop", PACKAGE)
        time.sleep(1.5)
        out = adb("shell", "am", "start", "-W", "-n", ACTIVITY)
        match = re.search(r"TotalTime: (\d+)", out)
        if match:
            times.append(int(match.group(1)))
        time.sleep(2)
    return summary(times)


def launch_with_marks():
    adb("shell", "am", "force-stop", PACKAGE)
    time.sleep(1)
    adb("shell", "am", "start", "-n", ACTIVITY, "--ez", "yo_perf", "true")
    time.sleep(5)


def latency(cli, me, runs, top_bar_y):
    offset = clock_offset_us()
    adb("logcat", "-c")
    cli.listen()
    sends, taps = [], []
    for _ in range(runs):
        cli.run("send", me)
        sends.append(time.time() * 1e6)
        time.sleep(2.5)
        taps.append(time.time() * 1e6)
        adb("shell", "input", "tap", "540", str(top_bar_y))
        time.sleep(2.5)
    time.sleep(2)
    cli.stop()
    found = marks(offset)
    receive = {"sync": [], "arrival": [], "frame": []}
    for after in sends:
        start = after - 3e6
        arrival = first_after(found, start, "arrival")
        if arrival is None:
            continue
        sent_at = int(next(what for t, what in found if t == arrival).split()[1]) * 1000
        sync = max((t for t, what in found if what.startswith("sync.got") and t <= arrival), default=None)
        frame = first_after(found, arrival, "frame arrival")
        receive["sync"].append((sync - sent_at) / 1e3 if sync else None)
        receive["arrival"].append((arrival - sent_at) / 1e3)
        receive["frame"].append((frame - sent_at) / 1e3 if frame else None)
    send = {"begin": [], "end": [], "frame": [], "peer": []}
    for tap in (t for t, what in found if what == "tap"):
        for key, prefix in (("begin", "send.begin"), ("end", "send.end"), ("frame", "frame sent")):
            at = first_after(found, tap, prefix)
            send[key].append((at - tap) / 1e3 if at else None)
        peer = next((t for t in cli.received if tap <= t <= tap + 8e6), None)
        send["peer"].append((peer - tap) / 1e3 if peer else None)
    return ({k: summary(v) for k, v in receive.items()}, {k: summary(v) for k, v in send.items()})


TRACE_CONFIG = """
buffers { size_kb: 65536 fill_policy: RING_BUFFER }
data_sources { config { name: "linux.ftrace" ftrace_config {
  ftrace_events: "sched/sched_switch" atrace_categories: "gfx" atrace_categories: "view"
  atrace_apps: "%s" } } }
data_sources { config { name: "linux.process_stats" } }
duration_ms: %d
"""


def frames(cli, me, runs, top_bar_y, trace):
    adb("shell", "dumpsys", "gfxinfo", PACKAGE, "reset")
    seconds = runs * 5 + 4
    if trace:
        subprocess.run(["adb", "shell", "perfetto", "--txt", "-c", "-", "-o", "/data/misc/perfetto-traces/bench.pftrace",
                        "-d"], input=TRACE_CONFIG % (PACKAGE, seconds * 1000), capture_output=True, text=True)
        time.sleep(1.5)
    for _ in range(runs):
        cli.run("send", me)
        time.sleep(2.5)
        adb("shell", "input", "tap", "540", str(top_bar_y))
        time.sleep(2.5)
    info = adb("shell", "dumpsys", "gfxinfo", PACKAGE)
    gfx = {}
    for key, pattern in (("total", r"Total frames rendered: (\d+)"), ("janky", r"Janky frames: (\d+)"),
                         ("p50_ms", r"50th percentile: (\d+)ms"), ("p90_ms", r"90th percentile: (\d+)ms"),
                         ("p99_ms", r"99th percentile: (\d+)ms")):
        match = re.search(pattern, info)
        gfx[key] = int(match.group(1)) if match else None
    result = {"gfxinfo": gfx}
    if trace:
        while adb("shell", "pidof", "perfetto").strip():
            time.sleep(1)
        path = "/tmp/yo-bench.pftrace" if os.access("/tmp", os.W_OK) else "bench.pftrace"
        adb("pull", "/data/misc/perfetto-traces/bench.pftrace", path)
        adb("shell", "rm", "/data/misc/perfetto-traces/bench.pftrace")
        from perfetto.trace_processor import TraceProcessor
        tp = TraceProcessor(trace=path)
        rows = tp.query(f"""select s.dur from slice s join thread_track t on s.track_id = t.id join thread using(utid)
            join process using(upid) where process.name = '{PACKAGE}' and thread.tid = process.pid
            and s.name like 'Choreographer#doFrame%'""")
        durations = [r.dur / 1e6 for r in rows]
        tp.close()
        result["main_thread"] = {
            "frames": len(durations),
            "over_8_3ms": sum(d > 8.33 for d in durations),
            "over_16_6ms": sum(d > 16.6 for d in durations),
            "over_budget_ms_per_event": round(sum(max(0, d - 8.33) for d in durations) / (2 * runs), 1),
            "worst_ms": round(max(durations, default=0), 1),
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cli", required=True, help="the peer's yo state directory")
    parser.add_argument("--me", required=True, help="the phone's username")
    parser.add_argument("--runs", type=int, default=20)
    parser.add_argument("--top-bar-y", type=int, default=200, help="the top bar's centre in screen pixels")
    parser.add_argument("--trace", action="store_true", help="also record a Perfetto trace of the frames")
    parser.add_argument("--label", default="")
    args = parser.parse_args()
    cli = Cli(args.cli)
    result = {"label": args.label, "device": adb("shell", "getprop", "ro.product.model").strip(),
              "time": time.strftime("%Y-%m-%dT%H:%M:%S")}
    result["cold_start_ms"] = cold_start(max(5, args.runs // 2))
    launch_with_marks()
    cli.run("send", args.me)
    time.sleep(3)
    result["receive_ms"], result["send_ms"] = latency(cli, args.me, args.runs, args.top_bar_y)
    result["frames"] = frames(cli, args.me, max(5, args.runs // 4), args.top_bar_y, args.trace)
    json.dump(result, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
