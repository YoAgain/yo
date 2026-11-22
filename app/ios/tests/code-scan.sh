#!/usr/bin/env bash
# The camera's Yo codes on Linux, without a device or the Rust FFI: which QR texts are Yo codes (Invite.name) and
# that each asks once while in view (CodeSightings). The core's username rules are stubbed; the core tests them.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
app="$here/../YoApp/Sources/YoApp"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/YoCore.swift" <<'SWIFT'
public func usernameValid(name: String) -> Bool { !name.isEmpty && name.count <= 16 && name.contains(where: \.isLetter) }
SWIFT
swiftc -module-cache-path "$tmp/modules" -swift-version 6 -emit-library -emit-module -module-name YoCore "$tmp/YoCore.swift" -o "$tmp/libYoCore.so"
cat > "$tmp/Username.swift" <<'SWIFT'
enum Username {
  static func clean(_ typed: String) -> String { typed.filter { $0.isLetter || $0.isNumber } }
  static func skeleton(_ name: String) -> String { name.lowercased() }
}
SWIFT
cp "$here/CodeScan.swift" "$tmp/main.swift"
swiftc -module-cache-path "$tmp/modules" -swift-version 6 -I "$tmp" -L "$tmp" -lYoCore \
  "$app/App/Invite.swift" "$app/List/CodeSightings.swift" "$tmp/Username.swift" "$tmp/main.swift" -o "$tmp/check"
LD_LIBRARY_PATH="$tmp${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$tmp/check"
