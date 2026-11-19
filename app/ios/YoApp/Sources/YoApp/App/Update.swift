import SwiftUI

/// Over everything while the hub refuses this build (426, `AppUpdate` in Hub.swift).
struct UpdateScreen: View {
  @Environment(\.openURL) private var openURL

  var body: some View {
    VStack(spacing: 20) {
      Spacer()
      Text("Yo").font(.yo(96)).foregroundStyle(.white)
      Text("Update Yo").font(.yo(30)).foregroundStyle(.white)
      Text("This version of Yo no longer works. Update to keep sending Yos.")
        .font(.body).foregroundStyle(.white.opacity(0.9)).multilineTextAlignment(.center)
      Spacer()
      // TestFlight until Yo is in the App Store. Yo Dev: push.sh.
      if !Hub.isDevApp, let link = URL(string: "https://testflight.apple.com/join/V3wa1DeX") {
        Button { openURL(link) } label: {
          Text("Update").font(.yo(22)).frame(maxWidth: .infinity)
        }
        .buttonStyle(.glass)
        .controlSize(.large)
      }
    }
    .padding(.horizontal, 32)
    .padding(.bottom, 24)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(.amethyst)
  }
}
