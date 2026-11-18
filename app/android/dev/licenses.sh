#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/env.sh"
root=$(cd "$here/../../.." && pwd)
cd "$root"
mkdir -p app/android/app/build/license-assets
cargo metadata --format-version 1 --locked --filter-platform aarch64-linux-android > app/android/app/build/cargo-metadata.json
python3 "$here/rust-licenses.py" app/android/app/build/cargo-metadata.json "$(git rev-parse HEAD)" \
  ' (development build; Android sources may include local changes)' "$root" > app/android/app/build/license-assets/Rust-Licenses.txt
