#!/usr/bin/env python3
"""Generate docs/loc.svg with python3 -I dev/loc-chart.py (stdlib only).

Follow main's first parents, diffing merges against their first parent so merged
code counts exactly once. Disable rename detection: moves between languages must
subtract the old extension and add the new one. Binary files are skipped.
Counts include blank lines and comments, not just executable statements.
Committer dates are UTC; clock regressions are clamped to the preceding day.
Only committed main history counts, never the working tree.
"""
from datetime import datetime, timedelta, timezone
from pathlib import Path
import math
import subprocess

ROOT = Path(__file__).resolve().parents[1]
LANGUAGES = ("Rust", "Swift", "Kotlin", "Other")
COLORS = ("#bc7540", "#ad62be", "#468bc2", "#589663")
EXTENSIONS = {".rs": "Rust", ".swift": "Swift", ".kt": "Kotlin", ".kts": "Kotlin"}
OTHER = {".py", ".sh", ".bash", ".c", ".h", ".cpp", ".hpp", ".m", ".mm",
         ".js", ".jsx", ".ts", ".tsx", ".css", ".html", ".sql", ".proto"}


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args])


def history():
    # NUL delimiters preserve spaces, tabs and newlines in tracked filenames.
    raw = git("log", "main", "--first-parent", "--reverse", "--diff-merges=first-parent",
              "--root", "--no-renames", "--numstat", "-z", "--format=%x00COMMIT %ct%x00")
    totals = dict.fromkeys(LANGUAGES, 0)
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
            ext = Path(name.decode("utf-8", "surrogateescape")).suffix.lower()
            language = EXTENSIONS.get(ext, "Other" if ext in OTHER else None)
            if language:
                totals[language] += int(added) - int(removed)
    if day is None:
        raise SystemExit("main has no commits")
    daily[day] = totals.copy()
    return sorted(daily.items())


def chart(points):
    width, height = 800, 360
    left, right, top, bottom = 76, 770, 65, 285
    start, end = points[0][0], points[-1][0]
    span = max(1, (end - start).days)
    peak = max(1, *(max(counts.values()) for _, counts in points))
    step = 10 ** math.floor(math.log10(peak))
    step *= next(n for n in (0.2, 0.5, 1, 2) if peak / (step * n) <= 6)
    ceiling = math.ceil(peak / step) * step

    def x(day):
        return left + (day - start).days / span * (right - left)

    def y(count):
        return bottom - count / ceiling * (bottom - top)

    svg = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img" aria-labelledby="title desc">',
           '<title id="title">Lines of code by language on main</title>',
           '<desc id="desc">Cumulative daily source lines from Git numstat, including comments and blanks.</desc>',
           '<style>text{font:13px system-ui,sans-serif;fill:#808080}.grid{stroke:#808080;stroke-opacity:.25}</style>',
           '<text x="76" y="22">Lines of code</text>']
    for i, (language, color) in enumerate(zip(LANGUAGES, COLORS)):
        pos = left + i * 166
        svg += [f'<path d="M{pos},43 h22" stroke="{color}" stroke-width="3"/>',
                f'<text x="{pos + 28}" y="47">{language}</text>']
    for i in range(round(ceiling / step) + 1):
        count = i * step
        label = f"{count / 1000:g}k" if count >= 1000 else f"{count:g}"
        svg += [f'<line x1="{left}" x2="{right}" y1="{y(count):.1f}" y2="{y(count):.1f}" class="grid"/>',
                f'<text x="{left - 10}" y="{y(count) + 4:.1f}" text-anchor="end">{label}</text>']
    for language, color in zip(LANGUAGES, COLORS):
        path = f"M{x(start):.1f},{y(points[0][1][language]):.1f}"
        for day, counts in points[1:]:
            path += f" H{x(day):.1f} V{y(counts[language]):.1f}"
        svg.append(f'<path d="{path}" fill="none" stroke="{color}" stroke-width="2.5"/>')
    for i in range(4 if span >= 3 else 2):
        fraction = i / (3 if span >= 3 else 1)
        day = start + timedelta(days=round(span * fraction))
        if day > end:
            continue
        svg.append(f'<text x="{x(day):.1f}" y="309" text-anchor="middle">{day.isoformat()}</text>')
    svg += ['<text x="423" y="340" text-anchor="middle">Commit date (UTC)</text>', '</svg>']
    return "\n".join(svg) + "\n"


if __name__ == "__main__":
    points = history()
    output = ROOT / "docs" / "loc.svg"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(chart(points), encoding="utf-8")
    print(f"Wrote {output.relative_to(ROOT)} through {points[-1][0]}")
