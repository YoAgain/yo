import SwiftUI

struct LicensesPage: View {
  /// The bundled text (Resources/Licenses.txt, no network), read off the main thread after the page is up.
  @State private var content: LicenseContent?

  var body: some View {
    ScrollView {
      VStack(spacing: 28) {
        card {
          Link(destination: URL(string: "https://justyo.co/p/imprint")!) {
            row("Yo \(HelpPage.appVersion) by Or Arbel")
          }
          Divider().padding(.horizontal, 16)
          row("Operated by Christopher Eller", secondary: true)
          Divider().padding(.horizontal, 16)
          Link(destination: URL(string: "https://justyo.co")!) {
            row("Website", value: "justyo.co")
          }
        }
        card {
          if let source = content?.source {
            Link(destination: source) {
              row("Source", value: String(source.lastPathComponent.prefix(12)))
            }
            Divider().padding(.horizontal, 16)
          }
          Text("Yo: MIT. libsignal: AGPL-3.0. Complete source: the commit above.")
            .font(.subheadline)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.vertical, 5)
        }
        // Thousands of lines: drawn lazily, as they scroll in.
        if let content {
          LazyVStack(alignment: .leading, spacing: 14) {
            ForEach(Array(content.parts.enumerated()), id: \.offset) { _, part in
              Text(part)
                .font(.system(.caption2, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
          }
          .padding(16)
          .background(Color(.secondarySystemGroupedBackground), in: .rect(cornerRadius: 26))
        } else {
          ProgressView().frame(maxWidth: .infinity, minHeight: 80)
        }
      }
      .padding(20)
    }
    .background(Color(.systemGroupedBackground))
    .task { if content == nil { content = await LicenseContent.load() } }
    .navigationTitle("About & Licenses")
    .navigationBarTitleDisplayMode(.inline)
  }

  private func card<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
    VStack(spacing: 0) { content() }
      .background(Color(.secondarySystemGroupedBackground), in: .rect(cornerRadius: 26))
  }

  private func row(_ title: String, value: String? = nil, secondary: Bool = false) -> some View {
    HStack {
      Text(title).foregroundStyle(secondary ? Color.secondary : Color(.label))
      Spacer(minLength: 8)
      if let value { Text(value).foregroundStyle(.secondary) }
    }
    .lineLimit(1)
    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
    .padding(.horizontal, 16)
    .padding(.vertical, 5)
    .contentShape(Rectangle())
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
