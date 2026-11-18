// How "Sent Yo! to Mia" looks (YoActivity.swift starts it): in the Dynamic Island "Yo" on the left and the friend's
// initial in their bar's colour with a green check on the right (an orange "!" when it did not go); opened, the
// initial big with the whole line (it opens so by itself at first, then turns small, and is gone after ~4 s; a tap
// opens Yo); with another activity beside it, the initial alone; on the Lock Screen one row.
import ActivityKit
import SwiftUI
import WidgetKit

struct YoSentLiveActivity: Widget {
  var body: some WidgetConfiguration {
    ActivityConfiguration(for: YoSentAttributes.self) { context in
      // The Lock Screen (and the banner on an iPhone without the island): the same row, equal margins all round.
      Row(attributes: context.attributes, state: context.state, initial: 40, title: 18, mark: 22)
        .padding(16)
        .activityBackgroundTint(.black.opacity(0.6))
        .activitySystemActionForegroundColor(.white)
    } dynamicIsland: { context in
      DynamicIsland {
        // One row across the island's whole width, so the name reads at a glance.
        DynamicIslandExpandedRegion(.bottom) {
          Row(attributes: context.attributes, state: context.state, initial: 44, title: 20, mark: 24)
            .frame(maxHeight: .infinity)
        }
      } compactLeading: {
        Text(verbatim: context.attributes.back ? "Yo ↩︎" : "Yo")
          .font(.yo(15, relativeTo: .headline))
          .foregroundStyle(.amethyst)
      } compactTrailing: {
        HStack(spacing: 3) {
          Initial(attributes: context.attributes, size: 20)
          Mark(state: context.state, size: 13)
        }
      } minimal: {
        Initial(attributes: context.attributes, size: 20)
      }
      // The same margin on every side of the opened island.
      .contentMargins(.all, 16, for: .expanded)
      .keylineTint(.amethyst)
    }
  }
}

/// The initial, then the title with the mark at its end on one line (centred on it), and the reason under it only
/// when there is one; with no reason the title sits in the middle beside the initial.
private struct Row: View {
  let attributes: YoSentAttributes
  let state: YoSentAttributes.ContentState
  let initial: CGFloat
  let title: CGFloat
  let mark: CGFloat

  var body: some View {
    HStack(spacing: 12) {
      Initial(attributes: attributes, size: initial)
      VStack(alignment: .leading, spacing: 2) {
        HStack(spacing: 8) {
          Title(name: attributes.name, text: state.title, size: title)
          Spacer(minLength: 0)
          Mark(state: state, size: mark)
        }
        if !state.detail.isEmpty {
          Text(verbatim: state.detail)
            .font(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(1)
        }
      }
    }
  }
}

/// "Sent Yo! to Mia", one line in Montserrat Bold: "Yo" and the rest never shrink, a long name is cut at its end.
private struct Title: View {
  let name: String
  let text: String
  let size: CGFloat

  var body: some View {
    Group {
      if !name.isEmpty, let range = text.range(of: name) {
        HStack(spacing: 0) {
          Text(verbatim: String(text[..<range.lowerBound])).fixedSize()
          Text(verbatim: name).lineLimit(1).truncationMode(.tail)
          Text(verbatim: String(text[range.upperBound...])).fixedSize()
        }
      } else {
        Text(verbatim: text).lineLimit(1)
      }
    }
    .font(.yo(size, relativeTo: .headline))
    .foregroundStyle(.white)
  }
}

/// The friend's initial in a circle of their bar's colour; a grey "?" when there is no one.
private struct Initial: View {
  let attributes: YoSentAttributes
  let size: CGFloat

  var body: some View {
    Text(verbatim: attributes.initial)
      .font(.yo(size * 0.5))
      .foregroundStyle(.white)
      .frame(width: size, height: size)
      .background(attributes.name.isEmpty ? .gray : attributes.color.map(Color.row) ?? .amethyst, in: .circle)
  }
}

/// A green check when it went, an orange "!" when not, a grey "?" when there was no one to Yo back.
private struct Mark: View {
  let state: YoSentAttributes.ContentState
  let size: CGFloat

  var body: some View {
    switch state.outcome {
    case .sent: symbol("checkmark", .green)
    case .failed: symbol("exclamationmark", .orange)
    case .nobody: symbol("questionmark", .gray)
    }
  }

  private func symbol(_ name: String, _ color: Color) -> some View {
    Image(systemName: name).font(.system(size: size * 0.8, weight: .bold)).foregroundStyle(color)
  }
}
