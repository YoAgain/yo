"""Summarize completed runs; percentiles here are worst per-run percentiles, not pooled frames."""
import json
import sys
from collections import defaultdict
from pathlib import Path


def records(path):
    if not Path(path).exists():
        return []
    return [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]


def summarize(path):
    groups = defaultdict(list)
    for row in records(path):
        groups[row['scenario']].append(row)
    lines = ['scenario | runs | worst run p50/p95/p99/max ms | hitches | hitch ms | worst stall ms / spans | launch-to-list ms',
             '--- | ---: | --- | ---: | ---: | ---: | ---:']
    for name, rows in sorted(groups.items()):
        worst = max(rows, key=lambda r: r['worst_stall_ms'])
        frames = '/'.join(f'{max(r[k] for r in rows):.2f}' for k in ('p50_ms', 'p95_ms', 'p99_ms', 'max_ms'))
        lines.append(f"{name} | {len(rows)} | {frames} | {sum(r['hitches'] for r in rows)} | "
                     f"{sum(r['hitch_ms'] for r in rows):.2f} | {worst['worst_stall_ms']:.2f} / {', '.join(worst['spans'])} | {max(r.get('launch_list_ms', -1) for r in rows):.2f}")
    result = '\n'.join(lines) + '\n'
    Path(path).with_name('summary.md').write_text(result)
    print(result)


if __name__ == '__main__':
    summarize(sys.argv[1])
