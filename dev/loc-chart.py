#!/usr/bin/env python3
"""Draw the lines-of-code chart for README.md: python3 -I dev/loc-chart.py (stdlib and Chromium).

Writes the numbers into dev/loc-chart.html and renders it with headless Chromium to dev/loc-chart.png, which the README
shows: a line of lines per day without tests, and today's code by part and the client by app as shares.

Follow main's first parents, diffing merges against their first parent so merged
code counts exactly once. Disable rename detection: moves between languages must
subtract the old extension and add the new one. Binary files are skipped.
Counts include blank lines and comments, not just executable statements. Test code
(tests/ directories and test-named files) is counted apart, as Tests.
Committer dates are UTC; clock regressions are clamped to the preceding day.
Only committed main history counts, never the working tree.
"""
from datetime import datetime, timedelta, timezone
from pathlib import Path
import math
import subprocess

ROOT = Path(__file__).resolve().parents[1]
# By part of the project: the server (hub, admin page), the shared core, the apps (iOS, Android, YoGPT), tests (tests/,
# fuzz/ and test files anywhere). Development scripts (dev/ and the like) are not counted.
KINDS = ("Hub", "Core", "Apps", "Tests")
COLORS = ("#bc7540", "#ad62be", "#468bc2", "#589663")
EXTENSIONS = {".rs": "Rust", ".swift": "Swift", ".kt": "Kotlin", ".kts": "Kotlin"}
OTHER = {".py", ".sh", ".bash", ".c", ".h", ".cpp", ".hpp", ".m", ".mm",
         ".js", ".jsx", ".ts", ".tsx", ".css", ".html", ".sql", ".proto"}


def part(path):
    """Which part of the project a source file belongs to, or None."""
    if is_test(path):
        return "Tests"
    top = path.split("/", 1)[0]
    return {"hub": "Hub", "admin": "Hub", "core": "Core", "app": "Apps"}.get(top)


def is_test(path):
    """Test code: anything under a tests/test/androidTest directory, or a file named as a test."""
    parts = path.lower().split("/")
    stem = Path(path).stem.lower()
    return (any(p in ("tests", "test", "androidtest", "testdata", "fuzz") for p in parts[:-1])
            or stem.endswith(("_test", "_tests", "test", "tests")) or stem.startswith("test_"))


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args])


def history():
    # NUL delimiters preserve spaces, tabs and newlines in tracked filenames.
    raw = git("log", "main", "--first-parent", "--reverse", "--diff-merges=first-parent",
              "--root", "--no-renames", "--numstat", "-z", "--format=%x00COMMIT %ct%x00")
    totals = dict.fromkeys(KINDS, 0)
    daily = {}
    day = None
    for token in raw.split(b"\0"):
        token = token.lstrip(b"\n")
        if token.startswith(b"COMMIT "):
            if day is not None:
                daily[day] = totals.copy()
            stamp = int(token.split()[1])
            current = datetime.fromtimestamp(stamp, timezone.utc).date()
            day = max(day, current) if day else current
        elif token:
            added, removed, name = token.split(b"\t", 2)
            if added == b"-" or removed == b"-":
                continue
            path = name.decode("utf-8", "surrogateescape")
            ext = Path(path).suffix.lower()
            kind = part(path) if (ext in EXTENSIONS or ext in OTHER) else None
            if kind:
                totals[kind] += int(added) - int(removed)
    if day is None:
        raise SystemExit("main has no commits")
    daily[day] = totals.copy()
    return sorted(daily.items())


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
                counts[app] += blob.count(b"\n")
    return counts


def table(head, cells):
    """A one-row Markdown table: the heads, then the numbers in whole thousands, right-aligned."""
    return "\n".join([
        "| " + " | ".join(head) + " |",
        "|" + "|".join("---:" for _ in head) + "|",
        "| " + " | ".join(cells) + " |",
    ])


def tables(points):
    """Three small tables: lines per day without tests, today's code by part, today's client by app."""
    k = lambda n: f"{round(n / 1000)}k"
    first = points[0][0]
    code = lambda counts: sum(n for kind, n in counts.items() if kind != "Tests")
    days = table([f"Day {(day - first).days + 1}" for day, _ in points], [k(code(c)) for _, c in points])
    today = points[-1][1]
    parts = table([*KINDS, "Total"], [k(today[kind]) for kind in KINDS] + [k(sum(today.values()))])
    apps = app_counts()
    client = table([*apps, "Total"], [k(n) for n in apps.values()] + [k(sum(apps.values()))])
    return (f"Lines of code on `main`, without tests, day by day:\n\n{days}\n\n"
            f"Today, by part:\n\n{parts}\n\nThe client today, by app (without tests):\n\n{client}")


def main():
    import json
    import os
    import shutil
    import tempfile
    points = history()
    code = lambda counts: sum(n for kind, n in counts.items() if kind != "Tests")
    today = points[-1][1]
    data = {"days": [code(c) for _, c in points], "parts": [[k, today[k]] for k in KINDS],
            "apps": [[k, v] for k, v in app_counts().items()]}
    page = (ROOT / "dev" / "loc-chart.html").read_text().replace("DATA", json.dumps(data))
    browser = shutil.which("chromium") or shutil.which("google-chrome-stable") or shutil.which("google-chrome")
    if not browser:
        raise SystemExit("needs Chromium to render the chart")
    with tempfile.TemporaryDirectory() as tmp:
        html = Path(tmp) / "chart.html"
        html.write_text(page)
        out = ROOT / "dev" / "loc-chart.png"
        subprocess.run([browser, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                        "--force-device-scale-factor=2", "--window-size=900,450", f"--screenshot={out}", str(html)],
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
