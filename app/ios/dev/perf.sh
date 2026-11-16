#!/usr/bin/env bash
# No device/network work until explicitly invoked. Machine settings remain ignored.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
[ ! -f "$here/local.env" ] || source "$here/local.env"
cd "$here/../../.."
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p target/perf target/perf-state
exec 9>target/perf/runner.lock
flock -n 9 || { echo 'A perf harness is already running' >&2; exit 2; }
if [[ ${1:-} == --summary ]]; then
  exec python3 -I "$here/perf_parse.py" target/perf/runs.jsonl
fi
: "${YO_PHONE:?Set YO_PHONE in ignored dev/local.env}"
: "${PERF_TO:?Set PERF_TO to the dev app TEST account name}"
: "${PERF_GROUP:?Set PERF_GROUP to a test group name (6–20 members)}"
: "${PERF_PREFIX:?Set PERF_PREFIX to a unique TEST account prefix}"
: "${PERF_PHOTO:?Set PERF_PHOTO to a synthetic JPEG path}"
# This is an explicit install only when the operator runs this script.
"$here/push.sh" "$YO_PHONE"
"${CARGO:-$HOME/.cargo/bin/cargo}" build --release -p yo-tests --bin yo-send
exec "${PY:-$HOME/pymobile3-venv/bin/python}" -I "$here/perf_device.py" "$YO_PHONE" \
  --to "$PERF_TO" --group "$PERF_GROUP" --prefix "$PERF_PREFIX" --photo "$PERF_PHOTO" --runs "${N:-300}"
