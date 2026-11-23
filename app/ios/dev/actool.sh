#!/usr/bin/env bash
# actool for the TestFlight build (testflight.yml: ACTOOL=this, REAL_ACTOOL=Xcode's actool). omarchy-apple-dev's
# compile-app-icon.sh merges actool's partial Info.plist into the app's key by key, so its CFBundleIcons (the primary
# and the alternates compiled from Resources/*.icon, xtool.env's ALTERNATE_APP_ICONS) replaces ours and the other
# icons to choose from in Appearance (CFBundleAlternateIcons with loose PNGs at the bundle root: OG2014, Lilac2015)
# are lost. So every alternate of the built Info.plist that actool did not compile is carried over into the partial
# one here, before it is merged; actool's own entries (CFBundleIconName) win.
set -euo pipefail
"${REAL_ACTOOL:?Xcode actool}" "$@"
bundle= partial=
while [ $# -gt 0 ]; do
  case "$1" in
    --compile) bundle=$2; shift ;;
    --output-partial-info-plist) partial=$2; shift ;;
  esac
  shift
done
# The controls' Yo symbols (Resources/Symbols.xcassets, custom SF Symbols): compile-app-icon.sh compiles icons only, so
# when it compiles the app, the catalog goes into YoControls' own Assets.car here (the controls read their bundle's).
appex=${bundle:+$bundle/PlugIns/YoControls.appex}
if [ -n "$appex" ] && [ -d "$appex" ]; then
  "$REAL_ACTOOL" "$(dirname "$0")/../YoApp/Resources/Symbols.xcassets" --compile "$appex" --platform iphoneos \
    --minimum-deployment-target 26.0 --output-format human-readable-text --errors --warnings
  [ -f "$appex/Assets.car" ] || { echo "actool.sh: no Assets.car in $appex" >&2; exit 1; }
  # The iMessage sticker pack (no code, so not an xtool extension): YoStickers.appex assembled next to the others,
  # here, before ship.sh stamps, provisions and signs every PlugIns/*.appex.
  "$(dirname "$0")/stickers.sh" "$bundle"
fi
[ -n "$bundle" ] && [ -n "$partial" ] && [ -f "$partial" ] || exit 0
python3 -I - "$bundle/Info.plist" "$partial" <<'PY'
import plistlib, sys
built, partial = (plistlib.load(open(p, "rb")) for p in sys.argv[1:])
for key in ("CFBundleIcons", "CFBundleIcons~ipad"):
    alternates = built.get(key, {}).get("CFBundleAlternateIcons")
    if alternates and key in partial:
        compiled = partial[key].setdefault("CFBundleAlternateIcons", {})
        for name, entry in alternates.items():
            compiled.setdefault(name, entry)
plistlib.dump(partial, open(sys.argv[2], "wb"))
PY
