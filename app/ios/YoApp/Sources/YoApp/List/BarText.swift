import SwiftUI
import UIKit
import YoCore

/// A group's Yos by sender, a line each ("Kai: 2× Yo") under the bar's name.
struct RenderedSender {
  let user: String
  let name: String
  let count: Int
  let elapsed: UInt64
  let at: Date
}

struct SenderBlocks: View {
  let senders: [RenderedSender]
  var lead = BarRow.margin
  var trail = BarRow.margin
  var paused = false

  var body: some View {
    // Each line fades on its own as a bar's Yo; its place stays until the whole bar's Yos are over.
    VStack(spacing: 0) {
      ForEach(senders, id: \.user) { sender in
        BarRow(lead: lead, trail: trail) {
          // Only the name is ever cut, never its Yos.
          HStack(spacing: 0) {
            Text(sender.name).lineLimit(1).truncationMode(.tail)
            SteadyText(": " + productCounted(count: Int64(sender.count))).fixedSize()
          }
          .padding(.horizontal, 3)
        }
        .modifier(ClockFade(since: sender.at.addingTimeInterval(FriendList.yoSolidSeconds),
          seconds: FriendList.yoSeconds - FriendList.yoSolidSeconds, paused: paused,
          elapsed: Double(sender.elapsed) / 1000 - FriendList.yoSolidSeconds))
        .transition(.opacity)
      }
    }
    .font(.yo(Metrics.rowFont))  // as a bar's name
    .allowsHitTesting(false)  // a tap there sends a Yo
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(senders.map { "\($0.count == 1 ? "Yo" : "\($0.count) Yos") from \($0.name)" }.joined(separator: ", "))
  }
}

struct ClockFade: ViewModifier {
  let since: Date?
  let seconds: Double
  var paused = false
  var elapsed: Double = 0

  func body(content: Content) -> some View {
    TimelineView(.animation(minimumInterval: 1.0 / 60, paused: since == nil || paused)) { timeline in
      let fade = paused ? max(0, elapsed) / max(0.001, seconds) : since.map { max(0, timeline.date.timeIntervalSince($0)) / max(0.001, seconds) } ?? 0
      content.opacity(max(0, 1 - fade))
    }
  }
}

extension ProductText {
  /// The core's English copy (`yo_product::Text`), as the app shows it.
  var english: String { productText(text: self, lang: .english) }
}

enum BarSendText {
  static let sentYo = productText(text: .sentYo, lang: .english)
  static let sentPhoto = productText(text: .sentPhoto, lang: .english)
  static let sentVideo = productText(text: .sentVideo, lang: .english)
  static let notSent = productText(text: .notSent, lang: .english)
  static let canceled = productText(text: .canceled, lang: .english)
}

/// Words with a count that hold still as it changes ("23× Yo" to "24× Yo"): Montserrat has no tabular figures, so
/// each number gets a block as wide as its digits times the widest figure. Only the words are cut ("3× Yo from Mia…").
struct SteadyText: View {
  let text: String
  init(_ text: String) { self.text = text }
  @Environment(\.dynamicTypeSize) private var typeSize

  /// The widest digit at the bar's size, measured once per text size.
  @MainActor private static var slots: [DynamicTypeSize: CGFloat] = [:]
  @MainActor private static func slot(_ size: DynamicTypeSize) -> CGFloat {
    if let width = slots[size] { return width }
    let base = UIFont(name: "Montserrat-Bold", size: Metrics.rowFont) ?? .boldSystemFont(ofSize: Metrics.rowFont)
    let font = UIFontMetrics(forTextStyle: .title1)
      .scaledFont(for: base, compatibleWith: UITraitCollection(preferredContentSizeCategory: UIContentSizeCategory(size)))
    let width = (0...9).map { ceil(("\($0)" as NSString).size(withAttributes: [.font: font]).width) }.max() ?? 0
    slots[size] = width
    return width
  }

  /// The text in runs: each number whole, the words between them whole.
  private var parts: [(digit: Bool, text: String)] {
    var parts: [(digit: Bool, text: String)] = []
    for character in text {
      let digit = character.isNumber
      if let last = parts.last, last.digit == digit { parts[parts.count - 1].text.append(character) }
      else { parts.append((digit, String(character))) }
    }
    return parts
  }

  var body: some View {
    let slot = Self.slot(typeSize)
    HStack(spacing: 0) {
      ForEach(Array(parts.enumerated()), id: \.offset) { _, part in
        if part.digit {
          Text(part.text).fixedSize().frame(width: slot * CGFloat(part.text.count), alignment: .trailing)
        } else {
          Text(part.text).lineLimit(1).truncationMode(.tail)
        }
      }
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(text)
  }
}

/// One line of a bar: the first candidate that fits between `lead` and `trail`, in the middle when it fits there,
/// else moved aside as little as it takes. Only the last is cut with "…". Never smaller, nothing overlaps.
struct BarRow: Layout {
  static let margin = 20.0
  var lead = margin
  var trail = margin

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    CGSize(width: proposal.width ?? 393, height: Metrics.rowHeight)
  }

  func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
    guard !subviews.isEmpty else { return }
    let widths = subviews.map { $0.sizeThatFits(.unspecified).width }
    let low = bounds.minX + lead
    let high = bounds.maxX - trail
    let chosen = widths.indices.first { widths[$0] <= high - low + 0.5 } ?? subviews.count - 1
    let width = min(widths[chosen], max(0, high - low))
    // The middle, or as near it as the sides allow.
    let x = min(max(bounds.midX - width / 2, low), high - width)
    for index in subviews.indices {
      // The others stay outside the bar.
      subviews[index].place(
        at: CGPoint(x: index == chosen ? x : bounds.minX - 100_000, y: bounds.midY), anchor: .leading,
        proposal: ProposedViewSize(width: index == chosen ? width : widths[index], height: nil))
    }
  }
}
