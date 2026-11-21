import SwiftUI
import UIKit
import YoCore

@MainActor
struct HelpPage: View {
  static var marketingVersion: String {
    Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?"
  }

  /// "1.2 (34)": CFBundleShortVersionString and CFBundleVersion.
  static var appVersion: String {
    let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "?"
    return String(bytes: "\(marketingVersion) (\(build))".utf8.prefix(64), encoding: .utf8) ?? "?"
  }

  /// As people read it: "1.0.0 RC19" for a release candidate (CI writes YoReleaseName from the tag), else
  /// "1.0.0 (build 202611211230)" (a local build is always 1 and shows just "1.0.0").
  static var shownVersion: String {
    if let rc = Bundle.main.infoDictionary?["YoReleaseName"] as? String { return "\(marketingVersion) \(rc)" }
    let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
    return build == "1" ? marketingVersion : "\(marketingVersion) (build \(build))"
  }

  /// The Help row's short form, one line beside the title: "1.0.0 RC19", else "1.0.0 dev" (the build number is on the
  /// About & Licenses page).
  static var rowVersion: String {
    if let rc = Bundle.main.infoDictionary?["YoReleaseName"] as? String { return "\(marketingVersion) \(rc)" }
    return (Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1") == "1" ? marketingVersion : "\(marketingVersion) dev"
  }

  var body: some View {
    Form {
      Section {
        HelpHero().listRowInsets(EdgeInsets()).listRowBackground(Color.clear)
      }
      Section {
        StatusPage(summaryOnly: true)
        NavigationLink(value: SettingsView.Page.licenses) {
          // A settings row: the label leading, the version as its value.
          LabeledContent {
            Text(Self.rowVersion)
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
  }
}
