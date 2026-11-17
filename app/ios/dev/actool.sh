#!/usr/bin/env bash
# actool for the TestFlight build (testflight.yml: ACTOOL=this, REAL_ACTOOL=Xcode's actool). omarchy-apple-dev's
# compile-app-icon.sh merges actool's partial Info.plist into the app's key by key, so its CFBundleIcons (the primary
# icon alone) replaces ours and the icons to choose from in Appearance (CFBundleAlternateIcons, loose PNGs at the
# bundle root) are lost. Alternate icon sets in the asset catalog fail ship.sh's Assets.car check, so the alternates
# are carried over from the built Info.plist into the partial one here instead, before it is merged.
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
[ -n "$bundle" ] && [ -n "$partial" ] && [ -f "$partial" ] || exit 0
python3 -I - "$bundle/Info.plist" "$partial" <<'PY'
import plistlib, sys
built, partial = (plistlib.load(open(p, "rb")) for p in sys.argv[1:])
for key in ("CFBundleIcons", "CFBundleIcons~ipad"):
    alternates = built.get(key, {}).get("CFBundleAlternateIcons")
    if alternates and key in partial:
        partial[key].setdefault("CFBundleAlternateIcons", alternates)
plistlib.dump(partial, open(sys.argv[2], "wb"))
PY
