#!/usr/bin/env python3
"""Draw the lines-of-code chart for README.md: python3 -I dev/loc-chart.py (stdlib and Chromium).

Writes the numbers into dev/loc-chart.html and renders it with headless Chromium to dev/loc-chart.png, which the README
shows: a line of lines per day without tests, and one bar of today's code by part, each label under its part.

Each day counts the tree of main's last commit that day (first parents, UTC, clock regressions clamped): lines of
source per part (Hub, Admin, Core, Apps), blanks and comments included, binary files skipped. Test code counts apart
as Tests: files in test directories or named as tests, and in Rust files everything from `#[cfg(test)]` on. Only
committed main history counts, never the working tree.
"""
from datetime import datetime, timedelta, timezone
from pathlib import Path
import math
import subprocess

ROOT = Path(__file__).resolve().parents[1]
# By part of the project: the server (hub, admin page), the shared core, the apps (iOS, Android, YoGPT), tests (tests/,
# fuzz/ and test files anywhere). Development scripts (dev/ and the like) are not counted.
KINDS = ("Hub", "Admin", "Core", "Apps", "Tests")
COLORS = ("#bc7540", "#ad62be", "#468bc2", "#589663")
EXTENSIONS = {".rs": "Rust", ".swift": "Swift", ".kt": "Kotlin", ".kts": "Kotlin"}
OTHER = {".py", ".sh", ".bash", ".c", ".h", ".cpp", ".hpp", ".m", ".mm",
         ".js", ".jsx", ".ts", ".tsx", ".css", ".html", ".sql", ".proto"}


def part(path):
    """Which part of the project a source file belongs to, or None."""
    if is_test(path):
        return "Tests"
    top = path.split("/", 1)[0]
    return {"hub": "Hub", "admin": "Admin", "core": "Core", "app": "Apps"}.get(top)


def is_test(path):
    """Test code: anything under a tests/test/androidTest directory, or a file named as a test."""
    parts = path.lower().split("/")
    stem = Path(path).stem.lower()
    return (any(p in ("tests", "test", "androidtest", "testdata", "fuzz") for p in parts[:-1])
            or stem.endswith(("_test", "_tests", "test", "tests")) or stem.startswith("test_"))


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args])


def counts_at(commit):
    """Lines per part in the tree of one commit. A Rust file's lines from its `#[cfg(test)]` on are tests, as
    are files in test directories."""
    totals = dict.fromkeys(KINDS, 0)
    names = [n.decode("utf-8", "surrogateescape") for n in git("ls-tree", "-r", "-z", "--name-only", commit).split(b"\0") if n]
    wanted = [n for n in names if Path(n).suffix.lower() in EXTENSIONS or Path(n).suffix.lower() in OTHER]
    wanted = [n for n in wanted if part(n)]
    if not wanted:
        return totals
    blobs = subprocess.run(["git", "-C", str(ROOT), "cat-file", "--batch"], check=True, capture_output=True,
                           input="".join(f"{commit}:{n}\n" for n in wanted).encode()).stdout
    at = 0
    for name in wanted:
        header_end = blobs.index(b"\n", at)
        size = int(blobs[at:header_end].split()[2])
        body = blobs[header_end + 1:header_end + 1 + size]
        at = header_end + 1 + size + 1
        if b"\0" in body[:8000]:
            continue
        lines = body.split(b"\n")
        kind = part(name)
        if kind != "Tests" and name.endswith(".rs"):
            cut = next((i for i, line in enumerate(lines) if line.strip() == b"#[cfg(test)]"), len(lines))
            totals[kind] += cut
            totals["Tests"] += len(lines) - cut
        else:
            totals[kind] += len(lines)
    return totals


def history():
    """The parts' lines at the last commit of each day on main (UTC days, clock regressions clamped)."""
    raw = git("log", "main", "--first-parent", "--reverse", "--format=%H %ct").decode().split()
    last = {}
    day = None
    for commit, stamp in zip(raw[::2], raw[1::2]):
        current = datetime.fromtimestamp(int(stamp), timezone.utc).date()
        day = max(day, current) if day else current
        last[day] = commit
    if not last:
        raise SystemExit("main has no commits")
    return [(d, counts_at(c)) for d, c in sorted(last.items())]


# Today's client by app, tests not counted.
APPS = (("Core", "core/"), ("iOS", "app/ios/"), ("Android", "app/android/"), ("YoGPT", "app/yogpt/"))


def app_counts():
    """Lines of code on main's tip per app (tests and binary files left out)."""
    names = git("ls-tree", "-r", "-z", "--name-only", "main").split(b"\0")
    counts = {name: 0 for name, _ in APPS}
    for raw in names:
        path = raw.decode("utf-8", "surrogateescape")
        ext = Path(path).suffix.lower()
        if not path or is_test(path) or (ext not in EXTENSIONS and ext not in OTHER):
            continue
        app = next((name for name, prefix in APPS if path.startswith(prefix)), None)
        if app:
            blob = git("show", f"main:{path}")
            if b"\0" not in blob[:8000]:
                lines = blob.split(b"\n")
                if path.endswith(".rs"):  # a Rust file's tests start at #[cfg(test)]
                    lines = lines[:next((i for i, l in enumerate(lines) if l.strip() == b"#[cfg(test)]"), len(lines))]
                counts[app] += len(lines)
    return counts


def main():
    import json
    import os
    import shutil
    import tempfile
    points = history()
    code = lambda counts: sum(n for kind, n in counts.items() if kind != "Tests")
    today = points[-1][1]
    apps = app_counts()
    data = {"days": [code(c) for _, c in points],
            "parts": [["Core", today["Core"]], ["iOS", apps["iOS"]], ["Android", apps["Android"]],
                      ["Server", today["Hub"] + today["Admin"]], ["CLI", apps["YoGPT"]], ["Tests", today["Tests"]]]}
    page = (ROOT / "dev" / "loc-chart.html").read_text().replace("DATA", json.dumps(data))
    browser = shutil.which("chromium") or shutil.which("google-chrome-stable") or shutil.which("google-chrome")
    if not browser:
        raise SystemExit("needs Chromium to render the chart")
    with tempfile.TemporaryDirectory() as tmp:
        html = Path(tmp) / "chart.html"
        html.write_text(page)
        out = ROOT / "dev" / "loc-chart.png"
        subprocess.run([browser, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                        "--force-device-scale-factor=2", "--window-size=900,370", f"--screenshot={out}", str(html)],
                       check=True, capture_output=True, timeout=60)
    readme = ROOT / "README.md"
    text = readme.read_text()
    start, end = "<!-- loc -->", "<!-- /loc -->"
    if start not in text or end not in text:
        raise SystemExit("README.md has no <!-- loc --> … <!-- /loc --> block")
    block = f"{start}\n![Lines of code: per day, by part and by app](dev/loc-chart.png)\n{end}"
    before, rest = text.split(start, 1)
    after = rest.split(end, 1)[1]
    readme.write_text(before + block + after)


if __name__ == "__main__":
    main()
