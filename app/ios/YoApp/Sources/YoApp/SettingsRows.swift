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

/// A system button controlling sibling List rows, without DisclosureGroup's child inset.
struct SettingsExpansion<Content: View>: View {
  @Environment(\.eagerSettingsRows) private var eagerRows
  @Binding var expanded: Bool
  @ViewBuilder var content: () -> Content

  var body: some View {
    Button {
      withAnimation(.snappy) { expanded.toggle() }
    } label: {
      HStack {
        content().frame(maxWidth: .infinity, alignment: .leading)
        Chevron().rotationEffect(.degrees(expanded ? 90 : 0))
      }
      .frame(minHeight: eagerRows ? 44 : nil)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityValue(expanded ? "Expanded" : "Collapsed")
    .accessibilityHint("Double-tap to show or hide details")
  }
}

/// The disclosure arrow Settings draws after a row's count, so the row reads as tappable.
struct Chevron: View {
  var body: some View {
    Image(systemName: "chevron.right")
      .font(.footnote.weight(.semibold))
      .foregroundStyle(Color(.tertiaryLabel))
      .accessibilityHidden(true)
  }
}

/// A security row: its tile, its title, its state at the end, as Settings lists a setting.
struct SecurityRow: View {
  let icon: String
  let color: Color
  let title: String
  var info: String? = nil
  let value: String
  var done = false
  @Environment(\.dynamicTypeSize) private var typeSize

  var body: some View {
    // Accessibility sizes need the value beneath the title.
    HStack {
      Label {
        VStack(alignment: .leading) {
          if let info { TitleWithInfo(title: title, text: info) }
          else { Text(title).foregroundStyle(Color(.label)) }
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

  /// The state and its mark. Done: a green check; not yet: an empty circle, the step still to take.
  private var state: some View {
    CheckValue(value: value, check: done ? .ok : .open)
  }
}

/// A diagnostic value and its check share spacing and accessibility.
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

/// A plain title and value row.
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

/// Summary first; each tap advances to the next diagnostic value.
struct CyclingStatusRow: View {
  let title: String
  let check: Check
  let values: [String]
  var info: String? = nil
  var loading = false
  @State private var index = 0

  var body: some View {
    LabeledContent {
      if loading {
        ProgressView().controlSize(.small)
      } else {
        Button {
          index = (index + 1) % values.count
        } label: {
          CheckValue(value: values[index % values.count], check: check)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title + ", " + values[index % values.count])
        .accessibilityHint("Tap for the next detail")
      }
    } label: {
      if let info {
        TitleWithInfo(title: title, text: info)
      } else {
        Text(title).lineLimit(1)
      }
    }
  }
}

/// Disclosure content stays in sibling rows, with no nested list indentation.
@MainActor func flatGroup(
  _ title: String, expanded: Binding<Bool>, checks: [Check],
  info: String? = nil
) -> some View {
  SettingsExpansion(expanded: expanded) {
    LabeledContent {
      checkSummary(checks)
    } label: {
      if let info { TitleWithInfo(title: title, text: info) } else { Text(title) }
    }
  }
}

/// All good: just the check. Otherwise how many hold ("3 of 4") beside the worst mark.
@MainActor @ViewBuilder private func checkSummary(_ checks: [Check]) -> some View {
  let good = checks.filter { $0 == .ok }.count
  let check: Check = checks.contains(.problem) ? .problem : good == checks.count ? .ok : .waiting
  if good == checks.count {
    Image(systemName: check.symbol).foregroundStyle(check.color).accessibilityLabel("All good")
  } else {
    CheckValue(value: "\(good) of \(checks.count)", check: check)
  }
}
