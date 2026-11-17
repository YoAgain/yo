#!/usr/bin/env bash
# Measure the dev "!" / "Yo" physics (YoApp/Sources/YoApp/BangSim.swift) on this machine, optimised as in the app.
#   app/ios/dev/bang-bench.sh [extra.swift ...]    (extra files with -D OLD: a baseline to compare against)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${TMPDIR:-/tmp}/bang-bench
flags=(-D BANG_BENCH)
[ $# -gt 0 ] && flags+=(-D OLD)
swiftc -O "${flags[@]}" "$here/../YoApp/Sources/YoApp/BangSim.swift" "$here/bang-bench/main.swift" "$@" -o "$out"
"$out"
