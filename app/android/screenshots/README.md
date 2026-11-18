# Screenshot verification

`welcome.png` and `choose-name.png` are real captures from the optimized app on an API 28 AOSP x86_64 emulator,
using software CPU/GPU (`-accel off`, SwiftShader). The release artifact was signed locally with the SDK’s debug
key for this runtime check; no signing key is included here. The welcome, sign-up navigation, text input, generated
Android UniFFI bindings and dev-hub name-availability lookup ran without an AndroidRuntime crash. “ExampleName”
is synthetic; no Android account was registered by this UI check.

The API 37 AOSP ATD image also booted, but its SurfaceFlinger crashed with SIGTRAP in its scheduler, followed by
Android system-server restarts. Its package/activity services were unstable. No API 37 UI acceptance is claimed.
This host exposes no `/dev/kvm`.

The screenshots do not prove signed-in friends/groups, hardware camera/video, media viewing, biometric deletion,
Credential Manager providers, crash recovery or accessibility acceptance. Follow ../README.md on a KVM-capable
host or Android device to extend the captures with synthetic accounts. Never capture recovery keys, device
identifiers or personal account data into this public directory.
