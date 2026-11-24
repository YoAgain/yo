#!/usr/bin/env bash
# A release from this machine, as the Release workflow (.github/workflows/release.yml) does it in Actions:
#   dev/release.sh 1.0.0-RC37 notes.md
# notes.md: the release text (a headline line, then a few user-facing bullets, as the earlier RCs); required, so no
# release goes out with only GitHub's generated changelog.
# Bumps app/android/version.properties (versionCode +1, versionName the version) in one signed commit on top of
# origin/main, pushes it to main, publishes the GitHub release v<version> on it (pre-release for -RC), which runs the
# release workflows, and sends the release card from the sibling yo-hub checkout (release-yo) when it is there.
# YO_GH_USER picks the gh account that pushes and releases (default: gh's active one); YO_HUB_DIR the yo-hub checkout.
# Only version.properties is committed; other changes in the checkout stay as they are.
set -euo pipefail
version=${1:?usage: dev/release.sh x.y.z[-RCn] NOTES.md}
notes=${2:?usage: dev/release.sh x.y.z[-RCn] NOTES.md (the release text)}
[ -s "$notes" ] || { echo "$notes is empty or missing" >&2; exit 1; }
notes=$(cd "$(dirname "$notes")" && pwd)/$(basename "$notes")
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-RC[0-9]+)?$ ]] || { echo "version '$version' is not x.y.z or x.y.z-RCn" >&2; exit 1; }
tag="v$version"
cd "$(dirname "$0")/.."
file=app/android/version.properties
user=${YO_GH_USER:-$(gh api user -q .login)}
token=$(gh auth token --user "$user")
git() { command git -c credential.helper= -c "credential.helper=!f() { echo username=$user; echo password=$token; }; f" "$@"; }

[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || { echo "not on main" >&2; exit 1; }
git diff --quiet HEAD -- "$file" || { echo "$file has local changes" >&2; exit 1; }
git fetch -q origin main --tags
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] \
  || { echo "HEAD is not origin/main; pull or push first" >&2; exit 1; }
! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || { echo "$tag exists already" >&2; exit 1; }

code=$(sed -n 's/^versionCode=//p' "$file")
code=$((code + 1))
sed -i -e "s/^versionCode=.*/versionCode=$code/" -e "s/^versionName=.*/versionName=$version/" "$file"
git commit -q -S --only -m "Release $version: version.properties versionCode $code, versionName $version" -- "$file"
git push origin HEAD:main
sha=$(git rev-parse HEAD)
prerelease=()
[[ "$version" == *-RC* ]] && prerelease=(--prerelease)
GH_TOKEN=$token gh release create "$tag" --repo YoAgain/yo --target "$sha" --title "$tag" --notes-file "$notes" --generate-notes "${prerelease[@]}"
echo "released $tag (versionCode $code) on $sha"

hub=${YO_HUB_DIR:-../yo-hub}
if [ -x "$hub/release-yo" ]; then (cd "$hub" && ./release-yo "$tag"); else echo "no $hub/release-yo: release card not sent"; fi
