import SwiftUI

/// Keep a row's explanation beside its title, before any values or controls.
struct TitleWithInfo: View {
  let title: String
  let text: String
  @State private var showing = false

  var body: some View {
    HStack(spacing: 6) {
      Text(title)
      Button {
        showing = true
      } label: {
        Image(systemName: "info.circle")
          .font(.body)
          .foregroundStyle(.secondary)
      }
      .buttonStyle(.borderless)
      .tint(.secondary)
      .accessibilityLabel("About \(title)")
      .accessibilityIdentifier("info.\(title)")
      .popover(isPresented: $showing) {
        // The row around it may set a one-line limit; the explanation always wraps.
        Text(text)
          .font(.subheadline)
          .lineLimit(nil)
          .multilineTextAlignment(.leading)
          .fixedSize(horizontal: false, vertical: true)
          .frame(width: 280, alignment: .leading)
          .padding()
          .presentationCompactAdaptation(.popover)
      }
    }
  }
}
