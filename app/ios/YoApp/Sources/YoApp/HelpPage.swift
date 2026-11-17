import SwiftUI
import UIKit
import YoCore

/// Help & About: the tutorial card, How to Use and Contact Us, then this build's information and licenses.
@MainActor
struct HelpPage: View {
  @State private var showingMessage = false
  @State private var showingHowTo = false

  static var marketingVersion: String {
    Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?"
  }

  /// "1.2 (34)": CFBundleShortVersionString and CFBundleVersion.
  static var appVersion: String {
    let info = Bundle.main.infoDictionary
    let short = info?["CFBundleShortVersionString"] as? String ?? "?"
    let build = info?["CFBundleVersion"] as? String ?? "?"
    return String(bytes: "\(short) (\(build))".utf8.prefix(64), encoding: .utf8) ?? "?"
  }

  private var content: some View {
    SettingsForm(profileFirst: true) {
      Section {
        HelpHero().padding(.top, 20)
      }
      Section("Help") {
        Button {
          showingHowTo = true
        } label: {
          SettingsNavigationLabel {
            Label("How to Use", systemImage: "hand.tap.fill").labelStyle(Tile(.amethyst))
          }
        }
        Button {
          showingMessage = true
        } label: {
          SettingsNavigationLabel {
            HStack {
              Label("Contact Us", systemImage: "envelope.fill").labelStyle(Tile(.blue))
              SupportBadge()
            }
          }
        }
      }
      Section("About") {
        NavigationLink(value: SettingsView.Page.licenses) {
          SettingsNavigationLabel {
            LabeledContent {
              Text("Yo \(Self.marketingVersion)")
            } label: {
              Label("About & Licenses", systemImage: "info.circle.fill").labelStyle(Tile(.gray))
            }
          }
        }
      }
    }
  }

  var body: some View {
    content
      .hitchScene("Help & About")
      .navigationTitle("Help & About")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .principal) { Text("Help & About").font(.headline) }
      }
      .toolbar(.visible, for: .navigationBar)
      .sheet(isPresented: $showingMessage) { MessagePage() }
      .sheet(isPresented: $showingHowTo) { IntroHelp() }
  }
}
