#!/bin/sh
# Every third-party dependency to its newest release, pinned exactly, then everything built and tested; one commit
# "Dependencies: ..." on success, the tree as it was on failure. Run by hand: there is no waiting period.
#
#   ./update-deps.sh            # update, test, build the iOS app, commit
#   ./update-deps.sh --dry-run  # only list what would change, with links to the release notes
#
# crates.io dependencies in every Cargo.toml (workspace, members, fuzz) become `=newest`; a git dependency with a
# tag (libsignal) gets its newest release tag and that commit as `rev`, its [patch] entries too (dev/update-deps.py).
# The iOS app has no Swift packages but the core; it is rebuilt with it.
set -eu
root=$(cd "$(dirname "$0")" && pwd)
cd "$root"

if [ "${1:-}" = "--dry-run" ]; then
  exec python3 -I dev/update-deps.py --dry-run
fi

if [ -n "$(git status --porcelain)" ]; then
  echo "update-deps: the working tree is not clean; commit or set aside first" >&2
  exit 1
fi
head=$(git rev-parse HEAD)
log=$(mktemp)
step=""
restore() {
  git reset -q --hard "$head"
  echo "update-deps: FAILED at: $step (everything restored to $(git rev-parse --short "$head"))" >&2
  echo "--- last lines of its output ($log) ---" >&2
  tail -n 40 "$log" >&2
  exit 1
}
run() {
  step="$*"
  echo "== $step"
  "$@" >"$log" 2>&1 || restore
}

changes=$(python3 -I dev/update-deps.py) || { step="dev/update-deps.py"; echo "$changes" >"$log"; restore; }
echo "$changes"
if [ "$changes" = "nothing to update" ]; then
  exit 0
fi

run dev/cargo update
run env YO_CWD=fuzz dev/cargo update
run dev/cargo clippy --workspace --all-targets -- -D warnings
run dev/cargo test --workspace
run app/ios/dev/core.sh
run sh -c 'cd app/ios/YoApp && xtool dev build --configuration release'

names=$(echo "$changes" | sed -E 's/^[^:]*: ([^ ]+) [^ ]+ -> ([^ ]+).*/\1 \2/' | sort -u | paste -sd, - | sed 's/,/, /g')
git add -u
git -c commit.gpgsign=false commit -q -F - <<EOF
Dependencies: $names

$changes
EOF
echo "update-deps: committed $(git rev-parse --short HEAD)"
