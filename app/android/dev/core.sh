#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/env.sh"
root=$(cd "$here/../../.." && pwd)
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"
export CARGO_TARGET_DIR="$root/target/android"
# Keep build concurrency bounded: libsignal and its post-quantum dependencies are large.
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
cd "$root"
out="$root/app/android/app/build"
mkdir -p "$out/generated/yo" "$out/rustJniLibs"
cargo ndk -t arm64-v8a -t x86_64 --platform 28 -o "$out/rustJniLibs" build --locked --release -p yo-bindings --lib
cargo build --locked -p yo-bindings --lib
cargo run --locked -p yo-bindings --bin uniffi-bindgen -- generate \
  --library "$CARGO_TARGET_DIR/aarch64-linux-android/release/libyo.so" \
  --config "$here/uniffi.toml" --language kotlin --out-dir "$out/generated/yo"
python3 "$here/fix-bindings.py" "$out/generated/yo/uniffi/yo/yo.kt"
