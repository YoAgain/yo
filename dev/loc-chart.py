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
LANGUAGES = ("Rust", "Swift", "Kotlin", "Other")
# Test code of any language, counted on its own and shown last.
KINDS = LANGUAGES + ("Tests",)
COLORS = ("#bc7540", "#ad62be", "#468bc2", "#589663")
EXTENSIONS = {".rs": "Rust", ".swift": "Swift", ".kt": "Kotlin", ".kts": "Kotlin"}
OTHER = {".py", ".sh", ".bash", ".c", ".h", ".cpp", ".hpp", ".m", ".mm",
         ".js", ".jsx", ".ts", ".tsx", ".css", ".html", ".sql", ".proto"}


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
            language = EXTENSIONS.get(ext, "Other" if ext in OTHER else None)
            if language and is_test(path):
                language = "Tests"
            if language:
                totals[language] += int(added) - int(removed)
    if day is None:
        raise SystemExit("main has no commits")
    daily[day] = totals.copy()
    return sorted(daily.items())


def chart(points):
    """One line per day: the total (whole thousands) as a bar, the languages beside it, tests last."""
    peak = max(sum(counts.values()) for _, counts in points)
    lines = []
    first = points[0][0]
    for day, counts in points:
        total = sum(counts.values())
        eighths = round(total / peak * 24 * 8)
        bar = "█" * (eighths // 8) + " ▏▎▍▌▋▊▉"[eighths % 8].strip()
        parts = " · ".join(f"{kind} {round(counts[kind] / 1000)}k" for kind in KINDS if counts[kind] >= 500)
        label = f"Day {(day - first).days + 1}"
        lines.append(f"{label:<7} {round(total / 1000):>3}k  {bar:<25} {parts}")
    return "\n".join(lines)


def main():
    readme = ROOT / "README.md"
    text = readme.read_text()
    start, end = "<!-- loc -->", "<!-- /loc -->"
    if start not in text or end not in text:
        raise SystemExit("README.md has no <!-- loc --> … <!-- /loc --> block")
    block = f"{start}\n```text\n{chart(history())}\n```\n{end}"
    before, rest = text.split(start, 1)
    after = rest.split(end, 1)[1]
    readme.write_text(before + block + after)


if __name__ == "__main__":
    main()
