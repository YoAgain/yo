// How a Yo from outside the app looks (YoActivity.swift starts it). Small: "Yo" in Yo's purple left of the camera
// ("2× Yo" for another to the same friend, "No Yo" in red when it did not go), the friend's name in their bar's colour
// right of it. Opened (on start and by a long press): under the camera the friend's bar as in the list, "Sent Yo!" and
// the name. On the Lock Screen one line on Yo's purple: "Yo" left, the name (or everyone Yo'd while it showed) right,
// in white. A Live Activity cannot animate by itself, so "sending" is a steady faded "Yo", not a pulse.
import ActivityKit
import SwiftUI
import WidgetKit

struct YoSentLiveActivity: Widget {
  var body: some WidgetConfiguration {
    ActivityConfiguration(for: YoSentAttributes.self) { context in
      // The Lock Screen (and the banner on an iPhone without the island): one line, "Yo" left, the name (or everyone
      // Yo'd while it showed) right.
      HStack(spacing: 12) {
        Word(state: context.state, stale: context.isStale, size: 30, tinted: false, long: true)
        Spacer(minLength: 0)
        Name(state: context.state, size: 28, tinted: false, all: true)
      }
      .padding(14)
      .activityBackgroundTint(.amethyst)
      .activitySystemActionForegroundColor(.white)
    } dynamicIsland: { context in
      DynamicIsland {
        // Opened (on start and by a long press): nothing beside the camera; under it the friend's bar as in the list,
        // with the list's words ("Sent Yo!") and the name. Its corners follow the island's (ContainerRelativeShape),
        // the same gap left, right and below (contentMargins), so the two curves run side by side.
        DynamicIslandExpandedRegion(.bottom) {
          // Its own room below: the region ends at the island's edge and cut the bar when it filled it.
          Bar(state: context.state, stale: context.isStale)
            .padding(.bottom, 6)
        }
      } compactLeading: {
        Word(state: context.state, stale: context.isStale, size: 16)
      } compactTrailing: {
        // About as wide as "Yo" on the other side at most (balanced around the camera), cut at its end. iOS 27's
        // isDynamicIslandLimitedInWidth needs the iOS 27 SDK; CI builds with Xcode 26 for now.
        Name(state: context.state, size: 16)
          .frame(maxWidth: 62, alignment: .leading)
      } minimal: {
        // "Yo" in the friend's colour (red when it did not go).
        Text(verbatim: "Yo")
          .font(.yo(14, relativeTo: .headline))
          .foregroundStyle(context.state.outcome == .failed ? .alizarin : Color.friend(context.state))
          .opacity(context.state.outcome == .sending && !context.isStale ? 0.45 : 1)
      }
      .contentMargins([.leading, .trailing, .bottom], Bar.inset, for: .expanded)
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

/// The opened island's bar: the friend's colour (red when it did not go, grey when there was nobody to answer), the
/// list's words in white and the name (or everyone Yo'd while it showed) after them, cut at its end.
private struct Bar: View {
  let state: YoSentAttributes.ContentState
  var stale = false
  /// The gap between the bar and the island's edge, the same on three sides.
  /// The gap to the island's edge. With the island's corner radius (about 44 pt) and the bar's half height (22 pt)
  /// it is concentric: 44 − 22 = 22, so the island's curve runs alongside the bar's ends. A shorter bar also stays
  /// inside the bottom region, which cut a taller one at its lower edge.
  static let inset: CGFloat = 22
  static let height: CGFloat = 44

  /// The list's words (YoWords in the app, which this extension cannot import).
  private var words: String {
    let count = state.others.isEmpty ? state.count : state.total
    return switch state.outcome {
    case .sending where stale: "Yo?"
    case .sending: count <= 1 ? "Sending Yo!" : "Sending \(count)× Yo!"
    case .sent: count <= 1 ? "Sent Yo!" : "Sent \(count)× Yo!"
    case .failed: "Not Sent"
    case .nobody: "No Yo to answer"
    }
  }

  private var color: Color {
    switch state.outcome {
    case .failed: .alizarin
    case .nobody: Color(hex: 0x7F8C8D)  // asbestos
    case .sending, .sent: .friend(state)
    }
  }

  var body: some View {
    HStack(spacing: 12) {
      Text(verbatim: words)
        .fixedSize()  // the words never shrink; the name is what is cut
        .contentTransition(.numericText(value: Double(state.others.isEmpty ? state.count : state.total)))
      Name(state: state, size: 20, tinted: false, all: true)
        .opacity(0.85)
    }
    // A fixed size (no Dynamic Type here): the island's height is fixed too, a larger text would be cut.
    .font(.custom("Montserrat-Bold", fixedSize: 20))
    .foregroundStyle(.white)
    .lineLimit(1)
    .minimumScaleFactor(0.8)
    .padding(.horizontal, 18)
    .frame(maxWidth: .infinity)
    .frame(height: Self.height)
    // A capsule: with the island's corners and the `inset` gap it runs alongside them (ContainerRelativeShape took the
    // region's own, wrong, corners).
    .background(color, in: Capsule())
  }
}

/// "Yo", "2× Yo" or "No Yo"; faded while it goes, "Yo?" when no outcome came in time (`stale`: the process that
/// sent it was suspended before it could say). In Yo's colours in the island, white on the Lock Screen's purple.
private struct Word: View {
  let state: YoSentAttributes.ContentState
  var stale = false
  let size: CGFloat
  var tinted = true
  /// The Lock Screen: room for "Yo back", "No Yo back" and, once there were others, how many Yos in all
  /// ("3 Yo").
  var long = false

  private var text: String {
    switch state.outcome {
    case .failed: "No Yo"
    case .nobody: long ? "No Yo back" : "No Yo"
    case .sending where stale: "Yo?"
    case .sending, .sent:
      if long && !state.others.isEmpty { "\(state.total) Yo" }
      else if state.count > 1 { "\(state.count)× Yo" }
      else if long && state.back { "Yo back" }
      else { "Yo" }
    }
  }

  private var color: Color {
    switch state.outcome {
    case .failed, .nobody: tinted ? .alizarin : Color(hex: 0xFFD6D0)
    case .sending, .sent: tinted ? .amethyst : .white
    }
  }

  var body: some View {
    Text(verbatim: text)
      .font(.yo(size, relativeTo: .headline))
      .foregroundStyle(color)
      .opacity(state.outcome == .sending && !stale ? 0.45 : 1)
      .lineLimit(1)
      .fixedSize()  // "Yo" never shrinks
      .contentTransition(.numericText(value: Double(long && !state.others.isEmpty ? state.total : state.count)))
      .animation(.default, value: text)
  }
}

/// The friend's name, one line, cut at its end; in their bar's colour in the island, white on the Lock Screen.
/// `all`: everyone Yo'd while it showed, oldest first ("JanG · Kevin · lina"); nothing when there was nobody ("No Yo
/// back" says it all).
private struct Name: View {
  let state: YoSentAttributes.ContentState
  let size: CGFloat
  var tinted = true
  var all = false

  private var text: String {
    if all && state.outcome == .nobody { return "" }
    if all && !state.others.isEmpty { return (state.others.reversed() + [state.name]).joined(separator: " · ") }
    return state.name
  }

  private var color: Color {
    if state.outcome == .nobody { return tinted ? .gray : .white.opacity(0.8) }
    return tinted ? .friend(state) : .white
  }

  var body: some View {
    if !text.isEmpty {  // no gap beside "No Yo back"
      Text(verbatim: text)
        .font(.custom("Montserrat-Bold", fixedSize: size))  // fixed: the island does not grow with Dynamic Type
        .foregroundStyle(color)
        .lineLimit(1)
        .truncationMode(.tail)
        .contentTransition(.opacity)
        .id(text)  // another friend: the name switches, it does not morph letter by letter
    }
  }
}
