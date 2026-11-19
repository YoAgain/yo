import SwiftUI

/// A row's symbol as Settings draws it: white on a small rounded square of colour.
struct Tile: LabelStyle {
  let color: Color
  init(_ color: Color) { self.color = color }

  func makeBody(configuration: Configuration) -> some View {
    HStack(spacing: 14) {
      configuration.icon
        .font(.system(size: 15, weight: .semibold))
        .foregroundStyle(.white)
        .frame(width: 29, height: 29)
        .background(color.gradient, in: .rect(cornerRadius: 7))
        .clipShape(.rect(cornerRadius: 7))
      configuration.title
    }
  }
}

struct SecurityRow: View {
  let icon: String
  let color: Color
  let title: String
  let value: String
  var done = false
  @Environment(\.dynamicTypeSize) private var typeSize

  var body: some View {
    // Accessibility sizes need the value beneath the title.
    HStack {
      Label {
        VStack(alignment: .leading) {
          Text(title).foregroundStyle(Color(.label))
          if typeSize.isAccessibilitySize { state }
        }
      } icon: {
        Image(systemName: icon)
      }
      .labelStyle(Tile(color))
      Spacer(minLength: 0)
      if !typeSize.isAccessibilitySize { state }
    }
  }

  private var state: some View {
    CheckValue(value: value, check: done ? .ok : .open)
  }
}

struct CheckValue: View {
  let value: String
  let check: Check

  var body: some View {
    HStack(spacing: 6) {
      Text(value).foregroundStyle(.secondary).lineLimit(1).minimumScaleFactor(0.6)
      Image(systemName: check.symbol).foregroundStyle(check.color).accessibilityHidden(true)
    }
  }
}

struct StatusRow: View {
  let title: String
  let value: String
  let check: Check

  var body: some View {
    LabeledContent {
      CheckValue(value: value, check: check)
    } label: {
      Text(title).lineLimit(1)
    }
  }
}
