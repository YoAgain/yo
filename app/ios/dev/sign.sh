#!/usr/bin/env bash
# Sign a built YoApp.app for development and pack it as an .ipa, like omarchy-apple-dev's tools/sign-dev.sh, but
# with the app's own entitlements (YoApp/Entitlements.plist) on top of the profile's: a development profile allows
# any associated domain ("*"), the app must name the real one.
#   dev/sign.sh APP.app OUT.ipa [DEV_DIR]      DEV_DIR: default ~/.config/omarchy-apple-dev/development
set -euo pipefail
app=${1:?usage: sign.sh APP.app OUT.ipa [DEV_DIR]}
ipa=$(realpath -m "${2:?usage: sign.sh APP.app OUT.ipa [DEV_DIR]}")
dev=${3:-$HOME/.config/omarchy-apple-dev/development}
here=$(cd "$(dirname "$0")" && pwd)
own=$here/../YoApp/Entitlements.plist

stage=$(mktemp -d)
mkdir "$stage/Payload"
cp -a "$app" "$stage/Payload/"
chmod -R u+w "$stage/Payload"   # SwiftPM outputs are read-only; rcodesign writes into the bundle
app=$stage/Payload/$(basename "$app")

bid=$(python3 -I -c 'import plistlib,sys; print(plistlib.load(open(sys.argv[1],"rb"))["CFBundleIdentifier"])' "$app/Info.plist")
prof=$dev/$bid.mobileprovision
[ -f "$prof" ] || { echo "no profile for $bid in $dev" >&2; exit 1; }
ent=$stage/entitlements.plist
team=$(python3 -I - "$prof" "$own" "$ent" "$app/Info.plist" <<'PY'
import plistlib, sys
raw = open(sys.argv[1], "rb").read()
profile = plistlib.loads(raw[raw.index(b"<?xml"):raw.rindex(b"</plist>") + 8])
entitlements = dict(profile["Entitlements"])
# Expand the profile's wildcard to this app's identifier. A literal PREFIX.* as the default keychain group
# is shared by development apps and is not a stable app identity across installations.
info = plistlib.load(open(sys.argv[4], "rb"))
prefix = profile["ApplicationIdentifierPrefix"][0]
identifier = prefix + "." + info["CFBundleIdentifier"]
entitlements["application-identifier"] = identifier
if info["CFBundleIdentifier"].endswith(".dev"):
    groups = entitlements.get("keychain-access-groups", [])
    if identifier not in groups and prefix + ".*" not in groups:
        sys.exit("the profile does not allow this app's keychain group")
    # Keep the profile's grants after our concrete default, so an older state's key can be migrated.
    entitlements["keychain-access-groups"] = [identifier] + [g for g in groups if g != identifier]
    info["YoKeychainAccessGroup"] = identifier
    plistlib.dump(info, open(sys.argv[4], "wb"))

own = plistlib.load(open(sys.argv[2], "rb"))
for key, value in own.items():
    if key in ("com.apple.developer.usernotifications.communication", "com.apple.developer.siri") and not entitlements.get(key):
        print(f"sign.sh: {key} not on the app id, signed without it", file=sys.stderr)
        entitlements.pop(key, None)
        continue
    if key not in entitlements:
        sys.exit(f"the profile does not allow {key}: enable it for the app id and make the profile again")
    entitlements[key] = value
# A development build reads its domains' association files straight from the domain, not through Apple's CDN (which
# keeps an old one up to a day): "?mode=developer", honoured with Settings > Developer > Associated Domains Development.
domains = entitlements.get("com.apple.developer.associated-domains")
if isinstance(domains, list):
    entitlements["com.apple.developer.associated-domains"] = [
        d if "?mode=" in d else d + "?mode=developer" for d in domains]
plistlib.dump(entitlements, open(sys.argv[3], "wb"))
print(profile["TeamIdentifier"][0])
PY
)
[[ "$team" =~ ^[A-Z0-9]{10}$ ]] || { echo "bad team id '$team'" >&2; exit 1; }
cp "$prof" "$app/embedded.mobileprovision"
rcodesign sign --pem-file "$dev/key.pem" --certificate-der-file "$dev/cert.der" \
  --team-name "$team" --entitlements-xml-file "$ent" "$app" >/dev/null
rm -f "$ipa" "$ent"
(cd "$stage" && zip -qry "$ipa" Payload)
echo "wrote $ipa"
