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
  ],
  targets: [
    .target(name: "YoApp"),
  ],
  swiftLanguageModes: [.v6]
)
