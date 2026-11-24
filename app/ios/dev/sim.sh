#!/usr/bin/env bash
# Yo Dev in the iOS Simulator, on a Mac with Xcode (and XcodeGen, protoc, rustup with aarch64-apple-ios-sim):
#   dev/sim.sh build              the core for the Simulator, then the app and its driver (dev/sim/project.yml)
#   dev/sim.sh device NAME        a Simulator of that name (iPhone 17 Pro, iOS 26), made once and booted
#   dev/sim.sh install NAME       the app built last onto it (with YoNotifyProbe)
#   dev/sim.sh drive NAME DIR     the driver on it (runs until `drive.py DIR quit`); drive.py sends it commands
#                                 (started again when XCTest ends it)
#   dev/sim.sh notify NAME        a push as the hub sends it, through the notification service's code (YoNotifyProbe:
#                                 the Simulator runs no notification service itself), shown as it made it
#   dev/sim.sh watch-build        the watch app and widgets for Yo Dev
#   dev/sim.sh watch-test WATCH   run the watch unit tests on the named watch Simulator
#   dev/sim.sh watch-device IPHONE WATCH   create and boot WATCH, paired with the named iPhone
#   dev/sim.sh watch-install IPHONE WATCH  embed the last watch build in Yo Dev and install both apps
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
    "$here/licenses.sh"
    (cd "$here/sim" && "${XCODEGEN:-xcodegen}" generate --quiet)
    xcodebuild build-for-testing -quiet -project "$here/sim/YoSim.xcodeproj" -scheme YoSim -configuration "$config" \
      -destination 'generic/platform=iOS Simulator' -derivedDataPath "$derived"
    ;;
  watch-build|watch-test)
    action=build
    destination='generic/platform=watchOS Simulator'
    if [ "$1" = watch-test ]; then
      watch=$(udid "${2:?usage: sim.sh watch-test WATCH}")
      [ -n "$watch" ] || { echo "No watch Simulator named $2" >&2; exit 1; }
      action=test
      destination="platform=watchOS Simulator,id=$watch"
    fi
    project=$here/../YoWatch
    (cd "$project" && "${XCODEGEN:-xcodegen}" generate)
    xcodebuild -project "$project/YoWatch.xcodeproj" -scheme YoWatch -configuration Debug -sdk watchsimulator \
      -destination "$destination" -derivedDataPath "$derived-watch" \
      YO_COMPANION_BUNDLE=app.justyo.ios.dev YO_WATCH_BUNDLE=app.justyo.ios.dev.watchkitapp CODE_SIGN_IDENTITY=- "$action"
    ;;
  watch-device)
    phone=$(udid "${2:?usage: sim.sh watch-device IPHONE WATCH}")
    [ -n "$phone" ] || { echo "No iPhone Simulator named $2" >&2; exit 1; }
    watch=$(udid "${3:?usage: sim.sh watch-device IPHONE WATCH}")
    if [ -z "$watch" ]; then
      model=$(xcrun simctl list -j | python3 -c '
import json, sys
listing = json.load(sys.stdin)
runtimes = [r for r in listing["runtimes"] if r["identifier"].split(".")[-1].startswith("watchOS-") and r["isAvailable"]]
if not runtimes:
    sys.exit("No available watchOS Simulator runtime")
runtime = max(runtimes, key=lambda r: tuple(map(int, r["version"].split("."))))
supported = {d["identifier"] for d in runtime.get("supportedDeviceTypes", [])}
types = [d for d in listing["devicetypes"] if d["name"].startswith("Apple Watch") and
         (not supported or d["identifier"] in supported)]
if not types:
    sys.exit("No Apple Watch device type for " + runtime["name"])
device = max(types, key=lambda d: (d["name"] == "Apple Watch Series 11 (46mm)", d.get("minRuntimeVersion", 0)))
print(device["identifier"], runtime["identifier"])')
      read -r device runtime <<< "$model"
      watch=$(xcrun simctl create "$3" "$device" "$runtime")
    fi
    pair=$(xcrun simctl list pairs -j | python3 -c '
import json, sys
print(next((key for key, pair in json.load(sys.stdin)["pairs"].items()
            if pair["watch"]["udid"] == sys.argv[1] and pair["phone"]["udid"] == sys.argv[2]), ""))' "$watch" "$phone")
    if [ -z "$pair" ]; then pair=$(xcrun simctl pair "$watch" "$phone"); fi
    xcrun simctl bootstatus "$phone" -b >/dev/null
    xcrun simctl bootstatus "$watch" -b >/dev/null
    echo "Watch $watch, pair $pair"
    ;;
  watch-install)
    phone=$(udid "${2:?usage: sim.sh watch-install IPHONE WATCH}")
    watch=$(udid "${3:?usage: sim.sh watch-install IPHONE WATCH}")
    [ -n "$phone" ] || { echo "No iPhone Simulator named $2" >&2; exit 1; }
    [ -n "$watch" ] || { echo "No watch Simulator named $3" >&2; exit 1; }
    app=$derived/Build/Products/$config-iphonesimulator/YoApp.app
    watch_app=$derived-watch/Build/Products/Debug-watchsimulator/YoWatch.app
    [ -d "$app" ] || { echo "Build Yo Dev with sim.sh build first" >&2; exit 1; }
    [ -d "$watch_app" ] || { echo "Build YoWatch with sim.sh watch-build first" >&2; exit 1; }
    mkdir -p "$app/Watch"
    rm -rf "$app/Watch/YoWatch.app"
    ditto "$watch_app" "$app/Watch/YoWatch.app"
    codesign -f -s - --preserve-metadata=entitlements "$app"
    xcrun simctl install "$phone" "$app"
    xcrun simctl bootstatus "$watch" -b
    xcrun simctl install "$watch" "$watch_app"
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
    sed -n '/^# Yo Dev/,/^# XCODEGEN/{ /^# XCODEGEN/!p; }' "$0" >&2
    exit 2
    ;;
esac
