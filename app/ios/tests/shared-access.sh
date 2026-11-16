#!/usr/bin/env bash
# Exercises the app and extensions' actual lock helper on Linux, without an iOS device or the Rust FFI.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/YoCore.swift" <<'SWIFT'
public enum YoError: Error { case Network(message: String) }
SWIFT
swiftc -module-cache-path "$tmp/modules" -swift-version 6 -emit-library -emit-module -module-name YoCore "$tmp/YoCore.swift" -o "$tmp/libYoCore.so"
cp "$here/SharedAccess.swift" "$tmp/main.swift"
swiftc -module-cache-path "$tmp/modules" -swift-version 6 -I "$tmp" -L "$tmp" -lYoCore \
  "$here/../YoApp/Sources/YoApp/SharedAccess.swift" "$tmp/main.swift" -o "$tmp/check"
LD_LIBRARY_PATH="$tmp${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$tmp/check"
