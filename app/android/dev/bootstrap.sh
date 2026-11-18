#!/usr/bin/env bash
# Linux x86_64, no sudo. All downloaded tools and user state stay under ~/Android.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$HOME/Android/downloads" "$HOME/Android/sdk/cmdline-tools" "$HOME/Android/user"
cd "$HOME/Android/downloads"
if [ ! -x "$HOME/Android/jdk/bin/java" ]; then
  curl -fL --retry 3 'https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz' -o jdk.tar.gz
  echo 'd7f5d5cf865a2902b70ff549b97586a7d3f234e016f041ab447ea740c7d66d1c  jdk.tar.gz' | sha256sum -c -
  tar -xf jdk.tar.gz -C "$HOME/Android"
  ln -sfn "$HOME/Android/jdk-21.0.13+9" "$HOME/Android/jdk"
fi
if [ ! -x "$HOME/Android/sdk/cmdline-tools/latest/bin/sdkmanager" ]; then
  curl -fL --retry 3 https://dl.google.com/android/repository/commandlinetools-linux-13114758_latest.zip -o tools.zip
  echo '7ec965280a073311c339e571cd5de778b9975026cfcbe79f2b1cdcb1e15317ee  tools.zip' | sha256sum -c -
  mkdir -p tools
  unzip -q tools.zip -d tools
  mv tools/cmdline-tools "$HOME/Android/sdk/cmdline-tools/latest"
fi
. "$here/env.sh"
# sdkmanager exits once it has read all answers, so yes may receive SIGPIPE.
set +o pipefail
yes | "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --licenses
set -o pipefail
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" \
  'platforms;android-37.0' 'build-tools;37.0.0' 'ndk;29.0.14206865' 'platform-tools' 'emulator'
cd "$here/../../.."
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
