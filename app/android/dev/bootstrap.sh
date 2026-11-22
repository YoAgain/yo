#!/usr/bin/env bash
# Linux x86_64, no sudo. All downloaded tools and user state stay under ~/Android.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$HOME/Android/downloads" "$HOME/Android/sdk/cmdline-tools" "$HOME/Android/user"
cd "$HOME/Android/downloads"
jdk=jdk-25.0.5.1+2
if [ ! -x "$HOME/Android/$jdk/bin/java" ]; then
  curl -fL --retry 3 'https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5.1%2B2/OpenJDK25U-jdk_x64_linux_hotspot_25.0.5.1_2.tar.gz' -o jdk.tar.gz
  echo '760f91b22c3efc9bd1b64ff950598231be82928cc807e30691a0e7a2995a62c0  jdk.tar.gz' | sha256sum -c -
  tar -xf jdk.tar.gz -C "$HOME/Android"
fi
ln -sfn "$HOME/Android/$jdk" "$HOME/Android/jdk"
if [ ! -x "$HOME/Android/sdk/cmdline-tools/latest/bin/sdkmanager" ]; then
  curl -fL --retry 3 https://dl.google.com/android/repository/commandlinetools-linux-16111833_latest.zip -o tools.zip
  echo '0877a1d048fe4a24efe2eff536ca4223f7adeb58648bb81909d33c446918cfa8  tools.zip' | sha256sum -c -
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
  'platforms;android-37.2' 'build-tools;37.0.0' 'ndk;30.0.16248370' 'platform-tools' 'emulator'
cd "$here/../../.."
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
