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
      // The Lock Screen (and the banner on an iPhone without the island).
      HStack(spacing: 12) {
        Initial(attributes: context.attributes, size: 36)
        Lines(attributes: context.attributes, state: context.state, size: 18)
        Spacer(minLength: 0)
        Mark(state: context.state, size: 22)
      }
      .padding(16)
      .activityBackgroundTint(.black.opacity(0.6))
      .activitySystemActionForegroundColor(.white)
    } dynamicIsland: { context in
      DynamicIsland {
        // One row across the island's whole width, so the name reads at a glance.
        DynamicIslandExpandedRegion(.bottom) {
          HStack(spacing: 12) {
            Initial(attributes: context.attributes, size: 44)
            Lines(attributes: context.attributes, state: context.state, size: 20)
            Mark(state: context.state, size: 26)
          }
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
      .keylineTint(.amethyst)
    }
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

/// "Sent Yo! to Mia" (big enough to read at a glance), and under it "just now" or why it did not go.
private struct Lines: View {
  let attributes: YoSentAttributes
  let state: YoSentAttributes.ContentState
  let size: CGFloat

  var body: some View {
    VStack(alignment: .leading, spacing: 2) {
      Text(verbatim: state.title)
        .font(.yo(size, relativeTo: .headline))
        .foregroundStyle(.white)
        // "Yo" never shrinks: a long name is cut at its end.
        .lineLimit(1)
      Text(verbatim: state.detail)
        .font(.caption)
        .foregroundStyle(.secondary)
        .lineLimit(2)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
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
