import SwiftUI

struct LicensesPage: View {
  @State private var content: LicenseContent?

  var body: some View {
    Form {
      Section {
        Link("Yo \(HelpPage.appVersion) by Or Arbel", destination: URL(string: "https://justyo.co/p/imprint")!)
        Text("Operated by Christopher Eller").foregroundStyle(.secondary)
        Link(destination: URL(string: "https://justyo.co")!) { LabeledContent("Website", value: "justyo.co") }
      }
      Section {
        if let source = content?.source {
          Link(destination: source) { LabeledContent("Source", value: String(source.lastPathComponent.prefix(12))) }
        }
      } footer: {
        Text("Yo: MIT. libsignal: AGPL-3.0. Complete source: the commit above.")
      }
      Section {
        if let content {
          // Thousands of lines: one row per paragraph, drawn as they scroll in.
          ForEach(Array(content.parts.enumerated()), id: \.offset) { _, part in
            Text(part).font(.system(.caption2, design: .monospaced)).textSelection(.enabled)
          }
        } else {
          ProgressView().frame(maxWidth: .infinity, minHeight: 80)
        }
      }
    }
    .tint(Color(.label))
    .task { if content == nil { content = await LicenseContent.load() } }
    .navigationTitle("About & Licenses")
    .navigationBarTitleDisplayMode(.inline)
  }
}

/// Load the bundled paragraphs before the push so the source row is present in its first frame.
struct LicenseContent: Sendable {
  let parts: [String]
  let source: URL?

  static func load() async -> Self {
    await Task.detached(priority: .utility) {
      let text =
        Bundle.main.url(forResource: "Licenses", withExtension: "txt")
        .flatMap { try? String(contentsOf: $0, encoding: .utf8) } ?? ""
      let first = text.prefix { $0 != "\n" }
      let source =
        first.hasPrefix("Source: ")
        ? URL(string: String(first.dropFirst("Source: ".count).prefix { $0 != " " })) : nil
      return Self(
        parts: text.components(separatedBy: "\n\n").dropFirst().filter { !$0.isEmpty },
        source: source)
    }.value
  }
}
