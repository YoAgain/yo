// swift-tools-version: 6.2
// YoApp: the iOS app (an xtool project), SwiftUI only, iOS 26, Swift 6.
// Build on Linux with `xtool dev build`; install with `xtool dev run` (USB) or ../dev/push.sh (network).
import PackageDescription

let package = Package(
  name: "YoApp",
  platforms: [.iOS(.v26)],
  products: [
    // An xtool project's first library product is the app.
    .library(name: "YoApp", targets: ["YoApp"]),
    // The extensions (xtool.yml extensions): the share sheet, and the notification service. Both open the app's state
    // with the core (SharedState.swift); the app's files they use are linked into Sources/YoShare and Sources/YoNotify.
    .library(name: "YoShare", targets: ["YoShare"]),
    .library(name: "YoNotify", targets: ["YoNotify"]),
  ],
  targets: [
    .target(name: "YoApp", dependencies: ["YoCore"]),
    .target(name: "YoShare", dependencies: ["YoCore"]),
    .target(name: "YoNotify", dependencies: ["YoCore"]),
    // The Rust core (core/bindings) through UniFFI: `import YoCore`. ../dev/core.sh makes both into build/core
    // (ignored); run it before the first build and after a change in core/ (dev/push.sh does).
    .target(name: "YoCore", dependencies: ["yoFFI"], path: "build/core/swift"),
    .binaryTarget(name: "yoFFI", path: "build/core/yoFFI.xcframework"),
  ],
  swiftLanguageModes: [.v6]
)
