#!/usr/bin/env bash
# Yo Dev in the iOS Simulator, on a Mac with Xcode (and XcodeGen, rustup with aarch64-apple-ios-sim):
#   dev/sim.sh build              the core for the Simulator, then the app and its driver (dev/sim/project.yml)
#   dev/sim.sh device NAME        a Simulator of that name (iPhone 17 Pro, iOS 26), made once and booted
#   dev/sim.sh install NAME       the app built last onto it
#   dev/sim.sh drive NAME DIR     the driver on it (runs until `drive.py DIR quit`); drive.py sends it commands
#                                 (started again when XCTest ends it)
#   dev/sim.sh delete NAME        the Simulator gone again
# Yo Dev talks to the dev hub. Passkeys do not work in the Simulator (no team in the app's id for the passkey
# domain), nor does the iCloud Keychain sync; Face ID does (Features ▸ Face ID, or `simctl spawn … notifyutil`).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
derived=${YO_SIM_DERIVED:-$here/sim/build}
PATH="$HOME/.cargo/bin:$PATH"

udid() { xcrun simctl list devices -j | python3 -c 'import json,sys; n=sys.argv[1]; print(next((d["udid"] for r in json.load(sys.stdin)["devices"].values() for d in r if d["name"]==n), ""))' "$1"; }

case ${1:-} in
  build)
    YO_CORE_TARGET=aarch64-apple-ios-sim "$here/core.sh"
    (cd "$here/sim" && xcodegen generate --quiet)
    xcodebuild build-for-testing -quiet -project "$here/sim/YoSim.xcodeproj" -scheme YoSim \
      -destination 'generic/platform=iOS Simulator' -derivedDataPath "$derived"
    ;;
  device)
    u=$(udid "$2")
    if [ -z "$u" ]; then
      runtime=$(xcrun simctl list runtimes -j | python3 -c 'import json,sys; print([r["identifier"] for r in json.load(sys.stdin)["runtimes"] if r["identifier"].startswith("com.apple.CoreSimulator.SimRuntime.iOS-26")][-1])')
      u=$(xcrun simctl create "$2" com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro "$runtime")
    fi
    xcrun simctl bootstatus "$u" -b >/dev/null
    echo "$u"
    ;;
  install)
    xcrun simctl install "$(udid "$2")" "$derived/Build/Products/Debug-iphonesimulator/YoApp.app"
    ;;
  drive)
    # A failure XCTest records (an element gone while asked) ends the test: it starts again until told to quit.
    rm -f "$3/quit"
    until [ -f "$3/quit" ]; do
      TEST_RUNNER_YO_DRIVER_DIR=$3 xcodebuild test-without-building -project "$here/sim/YoSim.xcodeproj" -scheme YoSim \
        -destination "platform=iOS Simulator,id=$(udid "$2")" -derivedDataPath "$derived" \
        -test-timeouts-enabled NO -only-testing:YoDriver/Driver/testDrive || true
    done
    ;;
  delete)
    xcrun simctl delete "$(udid "$2")"
    ;;
  *)
    sed -n '2,10p' "$0" >&2
    exit 2
    ;;
esac
