#!/usr/bin/env bash
# Yo Dev in the iOS Simulator, on a Mac with Xcode (and XcodeGen, rustup with aarch64-apple-ios-sim):
#   dev/sim.sh build              the core for the Simulator, then the app and its driver (dev/sim/project.yml)
#   dev/sim.sh device NAME        a Simulator of that name (iPhone 17 Pro, iOS 26), made once and booted
#   dev/sim.sh install NAME       the app built last onto it (with YoNotifyProbe)
#   dev/sim.sh drive NAME DIR     the driver on it (runs until `drive.py DIR quit`); drive.py sends it commands
#                                 (started again when XCTest ends it)
#   dev/sim.sh notify NAME        a push as the hub sends it, through the notification service's code (YoNotifyProbe:
#                                 the Simulator runs no notification service itself), shown as it made it
#   dev/sim.sh delete NAME        the Simulator gone again
# XCODEGEN names XcodeGen when it is not on the PATH; dev/local.env (ignored) may set it, and RUSTUP_TOOLCHAIN when
# the Mac's rustup has another toolchain than rust-toolchain.toml's. Yo Dev talks to the dev hub. Passkeys do not work
# in the Simulator (no team in the app's id for the passkey domain), nor does the iCloud Keychain sync; Face ID does
# (Features ▸ Face ID, or `simctl spawn … notifyutil`). See ../TESTING.md.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
[ -f "$here/local.env" ] && . "$here/local.env"
derived=${YO_SIM_DERIVED:-$here/sim/build}
# YO_SIM_CONFIG=Release: the app optimized, as on the iPhone (for measuring, TESTING.md); Debug by default.
config=${YO_SIM_CONFIG:-Debug}
PATH="$HOME/.cargo/bin:$PATH"

udid() { xcrun simctl list devices -j | python3 -c 'import json,sys; n=sys.argv[1]; print(next((d["udid"] for r in json.load(sys.stdin)["devices"].values() for d in r if d["name"]==n), ""))' "$1"; }

case ${1:-} in
  build)
    YO_CORE_TARGET=aarch64-apple-ios-sim "$here/core.sh"
    (cd "$here/sim" && "${XCODEGEN:-xcodegen}" generate --quiet)
    xcodebuild build-for-testing -quiet -project "$here/sim/YoSim.xcodeproj" -scheme YoSim -configuration "$config" \
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
    xcrun simctl install "$(udid "$2")" "$derived/Build/Products/$config-iphonesimulator/YoApp.app"
    xcrun simctl install "$(udid "$2")" "$derived/Build/Products/$config-iphonesimulator/YoNotifyProbe.app"
    ;;
  notify)
    made=$(xcrun simctl launch --console-pty --terminate-running-process "$(udid "$2")" app.justyo.ios.dev.probe \
      | tr -d '\r' | grep '^{' | tail -n 1)
    echo "$made"
    printf %s "$made" > "$derived/notify.apns"
    xcrun simctl push "$(udid "$2")" app.justyo.ios.dev "$derived/notify.apns"
    ;;
  drive)
    # A failure XCTest records (an element gone while asked) ends the test, and xcodebuild then does not always exit:
    # it is ended once the test is over, and the test starts again until told to quit. Its output: DIR/xcodebuild.log.
    rm -f "$3/quit"
    until [ -f "$3/quit" ]; do
      TEST_RUNNER_YO_DRIVER_DIR=$3 xcodebuild test-without-building -project "$here/sim/YoSim.xcodeproj" -scheme YoSim \
        -configuration "$config" \
        -destination "platform=iOS Simulator,id=$(udid "$2")" -derivedDataPath "$derived" \
        -test-timeouts-enabled NO -only-testing:YoDriver/Driver/testDrive > "$3/xcodebuild.log" 2>&1 &
      pid=$!
      while kill -0 "$pid" 2>/dev/null; do
        if grep -qE "Test Suite 'Selected tests' (passed|failed)" "$3/xcodebuild.log"; then
          sleep 3
          kill "$pid" 2>/dev/null
        fi
        sleep 1
      done
    done
    ;;
  delete)
    xcrun simctl delete "$(udid "$2")"
    ;;
  *)
    sed -n '2,12p' "$0" >&2
    exit 2
    ;;
esac
