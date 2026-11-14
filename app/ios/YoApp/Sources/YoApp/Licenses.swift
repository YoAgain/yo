// Settings › Licenses: the commit this build is made from, with a link to it (the app links libsignal under the
// AGPL-3.0, so that commit is its complete corresponding source), then every licence as dev/licenses.sh wrote them
// into Licenses.txt.
import SwiftUI

struct LicensesPage: View {
  @State private var parts: [String] = []
  @State private var source: URL?

  var body: some View {
    List {
      Section {
        if let source {
          Link(destination: source) {
            LabeledContent("Source", value: String(source.lastPathComponent.prefix(12)))
          }
        }
      } footer: {
        Text("Yo is MIT licensed. The app includes libsignal, licensed under the AGPL-3.0; this build’s complete source is the commit above.")
      }
      Section {
        ForEach(Array(parts.enumerated()), id: \.offset) { _, part in
          Text(part)
            .font(.system(.caption2, design: .monospaced))
            .textSelection(.enabled)
        }
      }
    }
    .navigationTitle("Licenses")
    .navigationBarTitleDisplayMode(.inline)
    .task { await load() }
  }

  /// The file in parts of a paragraph each, so the list draws only what is on the screen.
  private func load() async {
    let text = await Task.detached {
      Bundle.main.url(forResource: "Licenses", withExtension: "txt").flatMap { try? String(contentsOf: $0, encoding: .utf8) }
    }.value ?? ""
    let first = text.prefix { $0 != "\n" }
    source = first.hasPrefix("Source: ")
      ? URL(string: String(first.dropFirst("Source: ".count).prefix { $0 != " " })) : nil
    parts = text.components(separatedBy: "\n\n").dropFirst().filter { !$0.isEmpty }
  }
}
