// How a Yo from outside the app looks (YoActivity.swift starts it): two words around the camera, nothing else. On the
// left "Yo" in Yo's purple (faded while it goes; "2× Yo" for another to the same friend; "No Yo" in red when it did
// not go), on the right the friend's name in their bar's colour. Opened (by a long press only: it never opens by
// itself) the same two words bigger, beside the camera; with another activity beside it, "Yo" alone. On the Lock
// Screen the same two words in white on Yo's purple, and under them who else got one while it showed. A Live Activity cannot animate by itself, so "sending" is a
// steady faded "Yo", not a pulse.
import ActivityKit
import SwiftUI
import WidgetKit

struct YoSentLiveActivity: Widget {
  var body: some WidgetConfiguration {
    ActivityConfiguration(for: YoSentAttributes.self) { context in
      // The Lock Screen (and the banner on an iPhone without the island): the newest big, under it who else
      // ("+ Kevin, lina") or whose did not go ("No Yo to Kevin").
      VStack(alignment: .leading, spacing: 4) {
        HStack(spacing: 12) {
          Word(state: context.state, size: 28, tinted: false, long: true)
          Spacer(minLength: 0)
          Name(state: context.state, size: 24, tinted: false)
        }
        if let line = Others.line(context.state) {
          Text(verbatim: line)
            .font(.yo(15, relativeTo: .subheadline))
            .foregroundStyle(.white.opacity(0.8))
            .lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .trailing)
        }
      }
      .padding(14)
      .activityBackgroundTint(.amethyst)
      .activitySystemActionForegroundColor(.white)
    } dynamicIsland: { context in
      DynamicIsland {
        // Beside the camera, centred on it; nothing under it, so the island opens no higher than it must.
        DynamicIslandExpandedRegion(.leading) {
          Word(state: context.state, size: 26, tinted: true)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .trailing)
        }
        DynamicIslandExpandedRegion(.trailing) {
          Name(state: context.state, size: 26, tinted: true)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
      } compactLeading: {
        Word(state: context.state, size: 16, tinted: true, compact: true)
      } compactTrailing: {
        CompactName(state: context.state)
      } minimal: {
        // "Yo" in the friend's colour (red when it did not go).
        Text(verbatim: "Yo")
          .font(.yo(14, relativeTo: .headline))
          .foregroundStyle(context.state.outcome == .failed ? .alizarin : Color.friend(context.state))
          .opacity(context.state.outcome == .sending ? 0.4 : 1)
      }
      .keylineTint(.amethyst)
    }
  }
}

extension Color {
  /// The friend's bar colour; Yo's purple for one not on the list (or nobody).
  static func friend(_ state: YoSentAttributes.ContentState) -> Color {
    state.color.map(Color.row) ?? .amethyst
  }
}

/// "Yo", "2× Yo" or "No Yo"; faded while it goes. `tinted`: in Yo's colours (the island), else white (the Lock
/// Screen's purple).
private struct Word: View {
  let state: YoSentAttributes.ContentState
  let size: CGFloat
  let tinted: Bool
  /// The Lock Screen: room for "Yo back".
  var long = false
  /// The small island: once there were others, how many Yos in all ("3 Yo").
  var compact = false

  private var text: String {
    switch state.outcome {
    case .failed, .nobody: "No Yo"
    case .sending, .sent:
      if compact && !state.others.isEmpty { "\(state.total) Yo" }
      else if state.count > 1 { "\(state.count)× Yo" }
      else if long && state.back { "Yo back" }
      else { "Yo" }
    }
  }

  private var color: Color {
    switch state.outcome {
    case .failed: tinted ? .alizarin : .white
    case .nobody: tinted ? .gray : .white
    case .sending, .sent: tinted ? .amethyst : .white
    }
  }

  var body: some View {
    Text(verbatim: text)
      .font(.yo(size, relativeTo: .headline))
      .foregroundStyle(color)
      .opacity(state.outcome == .sending ? 0.4 : 1)
      .lineLimit(1)
      .fixedSize()  // "Yo" never shrinks
      .contentTransition(.numericText(value: Double(compact ? state.total : state.count)))
  }
}

/// The friend's name, one line, cut at its end; in their bar's colour in the island, white on the Lock Screen.
private struct Name: View {
  let state: YoSentAttributes.ContentState
  let size: CGFloat
  let tinted: Bool

  private var color: Color {
    if state.outcome == .nobody { return tinted ? .gray : .white.opacity(0.8) }
    return tinted ? .friend(state) : .white
  }

  var body: some View {
    Text(verbatim: state.name)
      .font(.yo(size, relativeTo: .headline))
      .foregroundStyle(color)
      .lineLimit(1)
      .truncationMode(.tail)
      .contentTransition(.opacity)
      .id(state.name)  // another friend: the name switches, it does not morph letter by letter
  }
}

/// The compact island's name: about as wide as "Yo" on the other side at most (balanced around the camera), cut at
/// its end; none where the island is limited in width (another app's activity beside it).
private struct CompactName: View {
  let state: YoSentAttributes.ContentState

  var body: some View {
    if #available(iOS 27.0, *) {
      Limited { name }
    } else {
      name
    }
  }

  private var name: some View {
    Name(state: state, size: 16, tinted: true).frame(maxWidth: 64, alignment: .leading)
  }

  @available(iOS 27.0, *)
  private struct Limited<Content: View>: View {
    @Environment(\.isDynamicIslandLimitedInWidth) private var limited
    @ViewBuilder let content: Content

    var body: some View {
      if !limited { content }
    }
  }
}

/// Under the Lock Screen's row: whose Yo did not go, else who else got one meanwhile.
private enum Others {
  static func line(_ state: YoSentAttributes.ContentState) -> String? {
    if let missed = state.missed { return "No Yo to \(missed)" }
    return state.others.isEmpty ? nil : "+ " + state.others.joined(separator: ", ")
  }
}
