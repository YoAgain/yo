// The Live Activity of a Yo sent from outside the app (YoActivity.swift starts it). It cannot animate by itself, so
// "sending" is a steady faded "Yo", not a pulse.
import ActivityKit
import SwiftUI
import WidgetKit
import YoCore

struct YoSentLiveActivity: Widget {
  var body: some WidgetConfiguration {
    ActivityConfiguration(for: YoSentAttributes.self) { context in
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
        DynamicIslandExpandedRegion(.bottom) {
          // The region ends at the island's edge and cut the bar when it filled it.
          Bar(state: context.state, stale: context.isStale)
            .padding(.bottom, 6)
        }
      } compactLeading: {
        Word(state: context.state, stale: context.isStale, size: 16)
      } compactTrailing: {
        // Balanced around the camera. iOS 27's isDynamicIslandLimitedInWidth needs the iOS 27 SDK (CI: Xcode 26).
        Name(state: context.state, size: 16)
          .frame(maxWidth: 62, alignment: .leading)
      } minimal: {
        Text(verbatim: productText(text: .yo, lang: .english))
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
  static func friend(_ state: YoSentAttributes.ContentState) -> Color {
    state.color.map(Color.row) ?? .amethyst
  }
}

private struct Bar: View {
  let state: YoSentAttributes.ContentState
  var stale = false
  /// Concentric with the island: its corner radius (about 44 pt) minus the bar's half height (22 pt).
  static let inset: CGFloat = 22
  static let height: CGFloat = 44

  private var words: String {
    productActivityWords(outcome: state.outcome.product, place: .expanded, stale: stale,
      count: Int64(state.count), total: Int64(state.total), hasOthers: !state.others.isEmpty,
      back: state.back, lang: .english)
  }

  /// Owner's pick: white on the island's black, the brand colour only in "Yo!".
  private var line: Text {
    let plain = state.outcome == .failed || state.outcome == .nobody
    if plain || !words.hasSuffix(productText(text: .yoBang, lang: .english)) {
      return Text(verbatim: words).foregroundColor(plain ? .alizarin : .white)
    }
    let head = String(words.dropLast(3))
    return Text(verbatim: head).foregroundColor(.white) + Text(verbatim: productText(text: .yoBang, lang: .english)).foregroundColor(.amethyst)
  }

  var body: some View {
    HStack(spacing: 12) {
      line
        .fixedSize()  // the words never shrink; the name is what is cut
        .contentTransition(.numericText(value: Double(state.others.isEmpty ? state.count : state.total)))
        .opacity(state.outcome == .sending && !stale ? 0.45 : 1)
      Name(state: state, size: 30, tinted: false, all: true)
        .foregroundStyle(.white)
    }
    // Fixed size: the island's height is fixed, Dynamic Type would cut it.
    .font(.custom("Montserrat-Bold", fixedSize: 30))
    .lineLimit(1)
    .minimumScaleFactor(0.8)
    .padding(.horizontal, 6)
    .frame(maxWidth: .infinity)
    .frame(height: Self.height)
  }
}

/// `stale`: the sending process was suspended before it could report an outcome.
private struct Word: View {
  let state: YoSentAttributes.ContentState
  var stale = false
  let size: CGFloat
  var tinted = true
  /// The Lock Screen's wording.
  var long = false

  private var text: String {
    productActivityWords(outcome: state.outcome.product, place: long ? .lockScreen : .compact, stale: stale,
      count: Int64(state.count), total: Int64(state.total), hasOthers: !state.others.isEmpty,
      back: state.back, lang: .english)
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

/// `all`: everyone Yo'd while it showed, oldest first.
private struct Name: View {
  let state: YoSentAttributes.ContentState
  let size: CGFloat
  var tinted = true
  var all = false

  private var text: String {
    productActivityNames(name: state.name, others: state.others, all: all, nobody: state.outcome == .nobody)
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
