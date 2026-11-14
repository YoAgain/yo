// The screen for when the hub says this build is too old (426, `AppUpdate` in Hub.swift): a breaking change on the
// hub, after which this build no longer works.
import SwiftUI

/// Over everything while `AppUpdate.needed`: what happened, and the way to the new build.
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
      // TestFlight for now (its link opens the TestFlight app); the App Store once Yo is there. Yo Dev: push.sh.
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
