import SwiftUI
import UIKit
import YoCore

/// Help, feedback to the Yo team, and this build's information.
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
    SettingsForm {
      Section {
        Button {
          showingHowTo = true
        } label: {
          HStack {
            Label("Tutorial", systemImage: "hand.tap.fill").labelStyle(Tile(.amethyst))
            Spacer()
            Chevron()
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          .contentShape(Rectangle())
        }
        .foregroundStyle(Color(.label))
        Button {
          showingMessage = true
        } label: {
          HStack {
            Label("Contact Us", systemImage: "envelope.fill").labelStyle(Tile(.amethyst))
            SupportBadge()
            Spacer()
            Chevron()
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          .contentShape(Rectangle())
        }
        .foregroundStyle(Color(.label))
      }
      Section {
        StatusPage(summaryOnly: true)
        NavigationLink(value: SettingsView.Page.licenses) {
          SettingsNavigationLabel {
            LabeledContent {
              Text("Yo \(Self.marketingVersion)")
            } label: {
              Label("Licenses", systemImage: "doc.text.fill").labelStyle(Tile(.gray))
            }
          }
        }
      }
    }
  }

  var body: some View {
    content
      .hitchScene("Help")
      .navigationTitle("Help")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .principal) { Text("Help").font(.headline) }
      }
      .toolbar(.visible, for: .navigationBar)
      .sheet(isPresented: $showingMessage) { MessagePage() }
      .sheet(isPresented: $showingHowTo) { IntroHelp() }
  }
}
