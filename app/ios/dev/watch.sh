#!/usr/bin/env bash
# The watch app and its widgets need the watchOS SDK. Xcode builds them separately from xtool's iPhone app.
# After ship.sh and stickers.sh --resign, sign them inside out and seal the iPhone app over both.
#   dev/watch.sh build
#   dev/watch.sh embed APP SIGN_DIR IPA
set -euo pipefail
project=$(cd "$(dirname "$0")/.." && pwd)/YoWatch
built=$project/build/Build/Products/Release-watchos/YoWatch.app
case ${1:-} in
  build)
    cd "$project"
    xcodegen generate
    xcodebuild -project YoWatch.xcodeproj -scheme YoWatch -configuration Release -sdk watchos \
      -destination 'generic/platform=watchOS' -derivedDataPath "$project/build" CODE_SIGNING_ALLOWED=NO build
    [ -d "$built/PlugIns/YoWatchWidgets.appex" ] || { echo "Missing watch app or widgets in $built" >&2; exit 1; }
    echo "$built"
    exit 0
    ;;
  embed) ;;
  *) echo "usage: watch.sh build | embed APP SIGN_DIR IPA" >&2; exit 2 ;;
esac

app=${2:?} sign=${3:?} ipa=$(cd "$(dirname "${4:?}")" && pwd)/$(basename "$4")
watch=$app/Watch/YoWatch.app
widgets=$watch/PlugIns/YoWatchWidgets.appex
[ -f "$app/Info.plist" ] && [ -d "$built/PlugIns/YoWatchWidgets.appex" ] || { echo "Missing app or watch build" >&2; exit 1; }
mkdir -p "$app/Watch"
rm -rf "$watch"
cp -a "$built" "$watch"

"${PY:-$HOME/pymobile3-venv/bin/python}" -I - "$app" "$watch" "$widgets" "$sign" "${OAD_TOOLS:?}" <<'PY'
import fnmatch, plistlib, sys
from pathlib import Path

app, watch, widgets, sign = map(Path, sys.argv[1:5])
sys.path.insert(0, sys.argv[5])
import asc

def info(bundle):
    return plistlib.loads((bundle / "Info.plist").read_bytes())

phone = info(app)
watch_id = phone["CFBundleIdentifier"] + ".watchkitapp"
versions = {k: phone[k] for k in ("CFBundleShortVersionString", "CFBundleVersion")}
if info(watch).get("WKCompanionAppBundleIdentifier") != phone["CFBundleIdentifier"]:
    sys.exit("watch.sh: watch companion does not match the iPhone app")
team_key = "com.apple.developer.team-identifier"
team = plistlib.loads((sign / "entitlements.plist").read_bytes())[team_key]
cert_id, _, _ = asc.certificate()
for bundle, ident in ((watch, watch_id), (widgets, watch_id + ".widgets")):
    data = info(bundle)
    if data["CFBundleIdentifier"] != ident:
        sys.exit(f"watch.sh: expected {ident} in {bundle}/Info.plist")
    (bundle / "Info.plist").write_bytes(plistlib.dumps(dict(data, **versions), fmt=plistlib.FMT_BINARY))
    print(f"Provisioning {ident} for {versions['CFBundleShortVersionString']} ({versions['CFBundleVersion']})", flush=True)
    asc.bundle_id(ident)
    raw = asc.profile(ident, cert_id)
    profile = asc.profile_payload(raw)
    granted = profile["Entitlements"]
    group = f"{team}.{watch_id}"
    if granted[team_key] != team or not granted["application-identifier"].endswith("." + ident):
        sys.exit(f"watch.sh: wrong team or application identifier in profile for {ident}")
    if not any(fnmatch.fnmatchcase(group, g) for g in granted.get("keychain-access-groups", [])):
        sys.exit(f"watch.sh: profile for {ident} does not grant {group}")
    if (sign / "cert.der").read_bytes() not in profile["DeveloperCertificates"]:
        sys.exit(f"watch.sh: profile for {ident} does not include the signing certificate")
    ent = {k: granted[k] for k in ("application-identifier", team_key, "beta-reports-active") if k in granted}
    ent.update({"get-task-allow": False, "keychain-access-groups": [group]})
    (bundle / "embedded.mobileprovision").write_bytes(raw)
    (sign / f"{bundle.stem}-entitlements.plist").write_bytes(plistlib.dumps(ent))
    print(f"{ident}: {profile['Name']} ({profile['UUID']})")
PY
team=$(python3 -I -c 'import plistlib,sys; print(plistlib.load(open(sys.argv[1],"rb"))["com.apple.developer.team-identifier"])' "$sign/entitlements.plist")
seal() {
  rcodesign sign --shallow --pem-file "$sign/key.pem" --certificate-der-file "$sign/cert.der" --team-name "$team" \
    --entitlements-xml-file "$sign/$2" "$1"
}
seal "$widgets" YoWatchWidgets-entitlements.plist
seal "$watch" YoWatch-entitlements.plist
seal "$app" entitlements.plist

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/Payload"
cp -a "$app" "$stage/Payload/"
rm -f "$ipa"
(cd "$stage" && zip -qry "$ipa" Payload)
echo "watch.sh: $(basename "$ipa") packed with the signed watch app and widgets"
