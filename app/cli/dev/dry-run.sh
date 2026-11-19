#!/bin/sh
# Dry run of `yo` on a copy of a live state directory (app/cli/README.md, "Server switch"): the copy is opened offline,
# never the directory itself, and nothing reaches a hub, so nothing registers and no session moves on.
#
#   app/cli/dev/dry-run.sh DIR [YO_BINARY]     (as root on the server: DIR under /var/lib/private/)
#
# Prints the account (user, username, device) and its counts as `yo status` sees them, and whether the state file of
# the build before (`state`) was taken in (`state.v2`). Compare user and device with the live values: they must be
# the same. The copy is removed afterwards (DRY_KEEP=1 keeps it). Exit code: yo's (0 ok, 4 nothing registered, …).
set -eu

source=${1:?usage: dry-run.sh DIR [YO_BINARY]}
yo=${2:-${YO_BIN:-/opt/yo/yo}}
[ -d "$source" ] || { echo "dry-run: $source is not a directory" >&2; exit 2; }

copy=$(mktemp -d "${TMPDIR:-/tmp}/yo-dry.XXXXXX")
chmod 700 "$copy"
cleanup() { [ "${DRY_KEEP:-0}" = 1 ] && echo "dry-run: copy kept in $copy" >&2 || rm -rf "$copy"; }
trap cleanup EXIT

# Reads the source only. A listener running there may be mid-write: stop its unit first, or run again on a mismatch.
cp -a "$source/." "$copy/"
rm -f "$copy/control.sock"

# Offline: no network namespace where one can be made, and a hub address nobody listens on (port 9, discard) in any
# case. The live environment (YO_HUB, pins) is not passed on.
offline=""
if unshare -n true 2>/dev/null; then offline="unshare -n"; fi
set +e
env -i PATH=/usr/bin:/bin YO_STATE="$copy" $offline "$yo" --hub http://127.0.0.1:9 status
code=$?
set -e

if [ -e "$source/state" ]; then
    if [ -e "$copy/state.v2" ] && [ ! -e "$copy/state" ]; then
        echo "dry-run: the state file of before was taken in (state.v2)" >&2
    else
        echo "dry-run: the state file of before was NOT taken in" >&2
        [ "$code" = 0 ] && code=1
    fi
fi
exit "$code"
