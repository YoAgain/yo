import SwiftUI

struct LicensesPage: View {
  /// Read ahead by Help (LicenseContent.prepare), so the page is whole on its first frame.
  @State private var content = LicenseContent.loaded

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
            Text(part)
              .font(.system(.caption2, design: .monospaced))
              .multilineTextAlignment(.leading)
              .frame(maxWidth: .infinity, alignment: .leading)
              .textSelection(.enabled)
          }
        } else {
          ProgressView().frame(maxWidth: .infinity, minHeight: 80)
        }
      }
    }
    .tint(Color(.label))
    .task {
      if content == nil {
        await LicenseContent.prepare()
        content = LicenseContent.loaded
      }
    }
    .navigationTitle("About & Licenses")
    .sheetFit()
    .navigationBarTitleDisplayMode(.inline)
  }
}

/// The bundled paragraphs, read once off the main thread before the page is pushed.
struct LicenseContent: Sendable {
  let parts: [String]
  let source: URL?

  @MainActor private(set) static var loaded: Self?

  @MainActor static func prepare() async {
    if loaded == nil { loaded = await load() }
  }

  private static func load() async -> Self {
    await Task.detached(priority: .utility) {
      let text =
        Bundle.main.url(forResource: "Licenses", withExtension: "txt")
        .flatMap { try? String(contentsOf: $0, encoding: .utf8) } ?? ""
      let first = text.prefix { $0 != "\n" }
      let source =
        first.hasPrefix("Source: ")
        ? URL(string: String(first.dropFirst("Source: ".count).prefix { $0 != " " })) : nil
      // Licence files centre their headings with spaces: every line starts at the left edge instead.
      let parts = text.components(separatedBy: "\n\n").dropFirst()
        .map { $0.split(separator: "\n", omittingEmptySubsequences: false).map { $0.trimmingCharacters(in: .whitespaces) }.joined(separator: "\n") }
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { !$0.isEmpty }
      return Self(parts: parts, source: source)
    }.value
  }
}
