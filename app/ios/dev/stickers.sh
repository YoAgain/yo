#!/usr/bin/env bash
# The iMessage sticker pack, YoStickers.appex, into APP (the TestFlight build; dev/actool.sh runs it when ship.sh
# compiles the app's icon, before the bundles are stamped and signed). It is what Xcode's sticker pack target
# (product type com.apple.product-type.app-extension.messages-sticker-pack) builds, and has no code, so xtool.yml
# cannot build it: Apple's MessagesApplicationExtensionStub (arm64) as its executable, YoApp/YoStickers-Info.plist with
# the bundle's id (the app's + .stickers), versions and minimum iOS, and Resources/Stickers.xcassets compiled by
# actool as Xcode does (the stickers, the Messages icon; the partial Info.plist merged in). Needs Xcode.
#   usage: dev/stickers.sh APP        REAL_ACTOOL names actool (default: xcrun's)
#          dev/stickers.sh --support IPA
# --support: the stub beside Payload/ in the signed .ipa, at MessagesApplicationExtensionSupport/, as Xcode's export
# puts it; without it App Store Connect rejects the build (90620, "Your iMessage app contains an invalid sticker pack").
set -euo pipefail
stub="$(xcrun --sdk iphoneos --show-sdk-platform-path)/Library/Application Support/MessagesApplicationExtensionStub/MessagesApplicationExtensionStub"
if [ "${1:-}" = --support ]; then
  ipa=$(cd "$(dirname "${2:?usage: stickers.sh --support IPA}")" && pwd)/$(basename "$2")
  stage=$(mktemp -d)
  trap 'rm -rf "$stage"' EXIT
  mkdir "$stage/MessagesApplicationExtensionSupport"
  cp "$stub" "$stage/MessagesApplicationExtensionSupport/"
  (cd "$stage" && zip -qry "$ipa" MessagesApplicationExtensionSupport)
  echo "stickers.sh: MessagesApplicationExtensionSupport in $(basename "$ipa")"
  exit 0
fi
app=${1:?usage: stickers.sh APP}
yo=$(cd "$(dirname "$0")/../YoApp" && pwd)
actool=${REAL_ACTOOL:-$(xcrun -f actool)}
appex=$app/PlugIns/YoStickers.appex
rm -rf "$appex"
mkdir -p "$appex"
# As Xcode leaves it: Apple's stub as it is (iOS 10, so it is sealed SHA-1 first, with SHA-256 beside), arm64 only.
lipo "$stub" -extract arm64 -output "$appex/YoStickers"
partial=$(mktemp)
trap 'rm -f "$partial"' EXIT
python3 -I - "$app/Info.plist" "$yo/YoStickers-Info.plist" "$appex/Info.plist" <<'PY'
import plistlib, sys
app, own, out = sys.argv[1:]
app, info = (plistlib.load(open(p, "rb")) for p in (app, own))
info.update({
    "CFBundleExecutable": "YoStickers",
    "CFBundleIdentifier": app["CFBundleIdentifier"] + ".stickers",
    "CFBundleName": "YoStickers",
    "CFBundleShortVersionString": app["CFBundleShortVersionString"],
    "CFBundleVersion": app.get("CFBundleVersion", "1"),  # ship.sh stamps the build number after
    "MinimumOSVersion": app.get("MinimumOSVersion", "26.0"),
})
plistlib.dump(info, open(out, "wb"), fmt=plistlib.FMT_BINARY)
PY
bid=$(python3 -I -c 'import plistlib,sys; print(plistlib.load(open(sys.argv[1],"rb"))["CFBundleIdentifier"])' "$appex/Info.plist")
min=$(python3 -I -c 'import plistlib,sys; print(plistlib.load(open(sys.argv[1],"rb"))["MinimumOSVersion"])' "$appex/Info.plist")
"$actool" "$yo/Resources/Stickers.xcassets" --compile "$appex" --output-format human-readable-text --errors --warnings \
  --output-partial-info-plist "$partial" --app-icon "iMessage App Icon" --compress-pngs \
  --enable-on-demand-resources NO --include-sticker-content --stickers-icon-role extension \
  --sticker-pack-identifier-prefix "$bid.sticker-pack." --development-region en \
  --target-device iphone --target-device ipad --minimum-deployment-target "$min" --platform iphoneos
python3 -I - "$appex/Info.plist" "$partial" <<'PY'
import plistlib, sys
info, partial = (plistlib.load(open(p, "rb")) for p in sys.argv[1:])
info.update(partial)
for key in ("IMStickerPackPath", "MSMessagesExtensionStoreIconName", "CFBundleIcons"):
    if key not in info:
        sys.exit(f"stickers.sh: actool wrote no {key}")
plistlib.dump(info, open(sys.argv[1], "wb"), fmt=plistlib.FMT_BINARY)
PY
[ -f "$appex/Assets.car" ] && [ -f "$appex/Sticker Pack.stickerpack/Info.plist" ] \
  || { echo "stickers.sh: no Assets.car or sticker pack in $appex" >&2; exit 1; }
echo "stickers.sh: $bid, $(ls "$appex/Sticker Pack.stickerpack" | grep -c '\.png$') stickers"
