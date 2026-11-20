import SwiftUI
import UIKit
import YoCore

@MainActor
struct HelpPage: View {
  @State private var showingMessage = false

  static var marketingVersion: String {
    Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?"
  }

  /// "1.2 (34)": CFBundleShortVersionString and CFBundleVersion.
  static var appVersion: String {
    let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "?"
    return String(bytes: "\(marketingVersion) (\(build))".utf8.prefix(64), encoding: .utf8) ?? "?"
  }

  /// "1.2" as people read it; a TestFlight build adds its number, "1.2 (34)" (a local build is always 1).
  static var shownVersion: String {
    let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
    return build == "1" ? marketingVersion : "\(marketingVersion) (\(build))"
  }

  var body: some View {
    Form {
      Section {
        HelpHero().listRowInsets(EdgeInsets()).listRowBackground(Color.clear)
      }
      Section {
        Button {
          showingMessage = true
        } label: {
          HStack {
            Label("Contact Us", systemImage: "envelope.fill").labelStyle(Tile(.blue))
            SupportBadge()
          }
        }
        .tint(Color(.label))
      }
      Section {
        StatusPage(summaryOnly: true)
        NavigationLink(value: SettingsView.Page.licenses) {
          // A settings row: the label leading, the version as its value.
          LabeledContent {
            Text(Self.marketingVersion)
          } label: {
            Label("About & Licenses", systemImage: "info.circle.fill").labelStyle(Tile(.gray))
          }
        }
      }
    }
    .hitchScene("Help & About")
    // About & Licenses opens whole, its text already read.
    .task { await LicenseContent.prepare() }
    .navigationTitle("Help & About")
    .navigationBarTitleDisplayMode(.inline)
    .sheet(isPresented: $showingMessage) { MessagePage() }
  }
}
