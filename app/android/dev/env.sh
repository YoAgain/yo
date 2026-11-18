#!/usr/bin/env bash
export JAVA_HOME="${JAVA_HOME:-$HOME/Android/jdk}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/sdk}"
export ANDROID_USER_HOME="${ANDROID_USER_HOME:-$HOME/Android/user}"
export ANDROID_AVD_HOME="${ANDROID_AVD_HOME:-$HOME/Android/avd}"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$HOME/.gradle}"
export PATH="$JAVA_HOME/bin:$HOME/.cargo/bin:$ANDROID_HOME/platform-tools:$PATH"
