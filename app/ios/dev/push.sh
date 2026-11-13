#!/usr/bin/env bash
# Build YoApp, sign it for development and install it on an iPhone without a cable (Tailscale or Wi-Fi).
#   app/ios/dev/push.sh PHONE        PHONE: the phone's IP address or tailnet name (or set YO_PHONE)
# Once before, with omarchy-apple-dev (OAD, default ../omarchy-apple-dev next to this repo or set OAD):
# pair the phone over USB (tools/tailnet-tunnel.py pair) and make the development identity
# (tools/provision-dev.py --bundle-id app.justyo.ios.dev). Both stay outside the repository. No sudo: dev/wireless.py
# tunnels in userspace. PY (or local.env) names pymobiledevice3's Python, default ~/pymobile3-venv/bin/python.
set -euo pipefail
# Settings of this machine (YO_PHONE, OAD) may sit in dev/local.env, which git ignores.
here=$(cd "$(dirname "$0")" && pwd)
[ -f "$here/local.env" ] && . "$here/local.env"
PHONE=${1:-${YO_PHONE:?usage: push.sh PHONE (IP address or tailnet name), or set YO_PHONE}}
app=$(cd "$here/../YoApp" && pwd)
OAD=${OAD:-$(cd "$app/../../../.." && pwd)/omarchy-apple-dev}
[ -x "$OAD/device-run.sh" ] || { echo "omarchy-apple-dev not found at $OAD (set OAD)" >&2; exit 2; }
[[ $PHONE =~ ^[0-9.]+$ ]] || PHONE=$(tailscale ip -4 "$PHONE" 2>/dev/null)
PATH="$(dirname "$(readlink -f "$(command -v swift)")"):$HOME/.local/bin:$PATH"
"$here/core.sh"   # the Rust core and its Swift bindings, into YoApp/build/core
cd "$app"
xtool dev build --configuration release   # optimised: SwiftUI in debug is noticeably slower
# What this script installs is "Yo Dev": its own app id (beside "Yo" from TestFlight or the App Store), named Yo like the other,
# the icon inverted, and the dev hub (Hub.isDevApp). TestFlight and App Store builds are left as they are.
python3 -I - xtool/YoApp.app/Info.plist <<'PY'
import plistlib, sys
info = plistlib.load(open(sys.argv[1], "rb"))
info["CFBundleIdentifier"] = info["CFBundleIdentifier"].removesuffix(".dev") + ".dev"
info["CFBundleDisplayName"] = "Yo"
plistlib.dump(info, open(sys.argv[1], "wb"))
PY
cp AppIcon-dev.png xtool/YoApp.app/AppIcon.png
# Without the extensions (share sheet, notification service): they share the state through the App Group of Yo from the
# App Store (group.app.justyo.ios), which Yo Dev has not; Yo Dev finds no group and keeps its state to itself.
rm -rf xtool/YoApp.app/PlugIns
mkdir -p build
"$here/sign.sh" xtool/YoApp.app build/YoApp.ipa   # the app's own entitlements on top of the profile's
bundle=$(python3 -I -c 'import plistlib,sys; print(plistlib.load(open(sys.argv[1],"rb"))["CFBundleIdentifier"])' xtool/YoApp.app/Info.plist)
"${PY:-$HOME/pymobile3-venv/bin/python}" -I "$here/wireless.py" "$PHONE" build/YoApp.ipa "$bundle"
