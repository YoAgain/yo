#!/usr/bin/env python3
"""Write the lines-of-code chart (plain text) into README.md: python3 -I dev/loc-chart.py (stdlib only).

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


def chart(points):
    """The total lines without tests per day as upright columns, the day under each and the total on top."""
    code = lambda counts: sum(n for kind, n in counts.items() if kind != "Tests")
    totals = [code(counts) for _, counts in points]
    first = points[0][0]
    days = [f"Day {(day - first).days + 1}" for day, _ in points]
    height, peak = 4, max(totals) or 1
    column = max(7, (LINE - 5) // len(points))
    eighths = [round(t / peak * height * 8) for t in totals]
    lines = []
    for row in range(height + 1, 0, -1):
        cells = []
        for k, (t, e) in enumerate(zip(totals, eighths)):
            full, rest = divmod(e, 8)
            tip = full + (1 if rest else 0)
            if row <= full:
                cell = "█████"
            elif row == full + 1 and rest:
                cell = " ▁▂▃▄▅▆▇"[rest] * 5
            elif row == tip + 1:
                cell = f"{round(t / 1000)}k"
            else:
                cell = ""
            cells.append(f"{cell:^{column}}")
        lines.append("     " + "".join(cells).rstrip())
    lines.append("     " + "".join(f"{d:^{column}}" for d in days).rstrip())
    return "\n".join(lines)


def shares(counts, names):
    """One bar of the whole, as wide as the daily chart, each part apart by a space; under each part, centred,
    its name and size."""
    total = sum(counts[name] for name in names) or 1
    width = LINE - 5 - (len(names) - 1)
    cells = [max(1, round(counts[name] / total * width)) for name in names]
    cells[cells.index(max(cells))] += width - sum(cells)
    bar = " ".join("█" * n for n in cells)
    # Each label centred under its part; where labels would touch or run past the bar's end, they move apart
    # just enough (right first, then back left from the end).
    labels = [f"{name} {round(counts[name] / 1000)}k" for name in names]
    starts, at = [], 0
    for label, n in zip(labels, cells):
        starts.append(max(at + (n - len(label)) // 2, starts[-1] + len(labels[len(starts) - 1]) + 1 if starts else 0))
        at += n + 1
    limit = len(bar)
    for k in range(len(labels) - 1, -1, -1):
        starts[k] = min(starts[k], limit - len(labels[k]))
        limit = starts[k] - 1
    legend = ""
    for label, start in zip(labels, starts):
        legend += " " * (start - len(legend)) + label
    return f"{round(total / 1000):>2}k  {bar}\n     {legend}"


# Width of every chart line: the width of a README code block on GitHub.
LINE = 86
# Width of a part's column in the first chart.
WIDTH = 7
# The second chart: today's code by app, tests not counted.
APPS = (("Core", "core/"), ("iOS", "app/ios/"), ("Android", "app/android/"), ("YoGPT", "app/yogpt/"))


def by_app():
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
    return shares(counts, [name for name, _ in APPS])


def main():
    readme = ROOT / "README.md"
    text = readme.read_text()
    start, end = "<!-- loc -->", "<!-- /loc -->"
    if start not in text or end not in text:
        raise SystemExit("README.md has no <!-- loc --> … <!-- /loc --> block")
    points = history()
    block = (f"{start}\n```text\nLines of code per day, without tests\n{chart(points)}\n\n"
             f"The code today, by part\n{shares(points[-1][1], KINDS)}\n\n"
             f"The client today, by app\n{by_app()}\n```\n{end}")
    before, rest = text.split(start, 1)
    after = rest.split(end, 1)[1]
    readme.write_text(before + block + after)


if __name__ == "__main__":
    main()
