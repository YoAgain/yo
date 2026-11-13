#!/usr/bin/env bash
# Builds the Rust core (core/bindings) for the iPhone and its Swift bindings, into YoApp/build/core (ignored):
#   yoFFI.xcframework   libyo.a for arm64 iOS with yoFFI.h and its module map (Package.swift: binary target yoFFI)
#   swift/yo.swift      UniFFI's Swift side (Package.swift: target YoCore)
# Runs with the host's rustup (the toolchain of rust-toolchain.toml, plus `rustup target add aarch64-apple-ios`):
# on Linux against the darwin Swift SDK of omarchy-apple-dev, on macOS (CI) against Xcode's iPhoneOS SDK. No C
# compiler is needed: the core has no C dependencies. dev/push.sh calls it before the app's build.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
[ -f "$here/local.env" ] && . "$here/local.env"
root=$(cd "$here/../../.." && pwd)
app="$root/app/ios/YoApp"
out="$app/build/core"
target=aarch64-apple-ios
PATH="$HOME/.cargo/bin:$PATH"

if [ "$(uname)" = Darwin ]; then
  sdk=$(xcrun --sdk iphoneos --show-sdk-path)
else
  sdk=$(swift sdk configure --show-configuration darwin arm64-apple-ios | sed -n 's/^sdkRootPath: //p')
fi
[ -d "$sdk" ] || { echo "no iPhoneOS SDK (xcrun or swift sdk configure)" >&2; exit 2; }

# A target directory of its own: dev/cargo's container builds in ./target under another path, so sharing it would
# rebuild everything each time one of them runs.
export CARGO_TARGET_DIR="$root/target/ios"
cd "$root"

# Only the staticlib: the cdylib in Cargo.toml would need Apple's ld64. Small rather than fast: the core is
# mostly waiting for the network.
SDKROOT="$sdk" IPHONEOS_DEPLOYMENT_TARGET=26.0 \
  CARGO_PROFILE_RELEASE_OPT_LEVEL=s CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
  CARGO_PROFILE_RELEASE_DEBUG=false \
  cargo rustc -q -p yo-bindings --lib --release --target "$target" --crate-type staticlib
lib="$CARGO_TARGET_DIR/$target/release/libyo.a"

# UniFFI reads the interface from the library itself (library mode).
rm -rf "$out"
mkdir -p "$out/swift" "$out/gen"
cargo run -q -p yo-bindings --bin uniffi-bindgen -- \
  generate --library "$lib" --language swift --out-dir "$out/gen" >/dev/null
mv "$out/gen/yo.swift" "$out/swift/"

# The xcframework by hand (no xcodebuild here): one static library for arm64 iOS, its header and module map.
fw="$out/yoFFI.xcframework"
lib_dir="$fw/ios-arm64"
mkdir -p "$lib_dir/Headers"
cp "$lib" "$lib_dir/libyo.a"
# Rust's compiler_builtins brings compiler-rt's C members (atomics, __muldc3 ...), which the app's link also takes
# from libclang_rt.ios.a; Apple's ld64 lets that pass, ld64.lld calls them duplicates. Drop each member whose
# global symbols are all in libclang_rt.ios.a (omarchy-apple-dev FINDINGS.md, item 69). Not on macOS.
dropped=0
if [ "$(uname)" != Darwin ]; then
  rt="$(swift sdk configure --show-configuration darwin arm64-apple-ios | sed -n 's/^swiftResourcesPath: //p')/clang/lib/darwin/libclang_rt.ios.a"
  [ -f "$rt" ] || { echo "no libclang_rt.ios.a at $rt" >&2; exit 2; }
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  symbols() { llvm-nm --defined-only --extern-only "$1" 2>/dev/null | awk 'NF>=3 {print $NF}' | sort -u; }
  symbols "$rt" > "$tmp/rt"
  (cd "$tmp" && llvm-ar x "$lib_dir/libyo.a")
  while read -r member; do
    symbols "$tmp/$member" > "$tmp/symbols"
    if [ -s "$tmp/symbols" ] && ! comm -23 "$tmp/symbols" "$tmp/rt" | grep -q .; then
      llvm-ar d "$lib_dir/libyo.a" "$member"
      dropped=$((dropped + 1))
    fi
  done < <(llvm-ar t "$lib_dir/libyo.a" | grep -v '\.rcgu\.o$')   # Rust's own members are never C
  llvm-ranlib "$lib_dir/libyo.a"
fi
cp "$out/gen/yoFFI.h" "$lib_dir/Headers/"
# SwiftPM looks for module.modulemap; UniFFI names it after the module.
cp "$out/gen/yoFFI.modulemap" "$lib_dir/Headers/module.modulemap"
rm -r "$out/gen"
cat > "$fw/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AvailableLibraries</key>
	<array>
		<dict>
			<key>BinaryPath</key>
			<string>libyo.a</string>
			<key>HeadersPath</key>
			<string>Headers</string>
			<key>LibraryIdentifier</key>
			<string>ios-arm64</string>
			<key>LibraryPath</key>
			<string>libyo.a</string>
			<key>SupportedArchitectures</key>
			<array>
				<string>arm64</string>
			</array>
			<key>SupportedPlatform</key>
			<string>ios</string>
		</dict>
	</array>
	<key>CFBundlePackageType</key>
	<string>XFWK</string>
	<key>XCFrameworkFormatVersion</key>
	<string>1.0</string>
</dict>
</plist>
EOF
echo "core: $(du -h "$lib_dir/libyo.a" | cut -f1) $lib_dir/libyo.a ($dropped compiler-rt members dropped)"
