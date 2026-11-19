import SwiftUI
import UIKit
import YoCore

/// The bar's font (Montserrat Bold, scaled as `.yo` scales it), for measuring what the core places.
@MainActor func barFont(_ size: CGFloat, _ typeSize: DynamicTypeSize) -> UIFont {
  let base = UIFont(name: "Montserrat-Bold", size: size) ?? .boldSystemFont(ofSize: size)
  return UIFontMetrics(forTextStyle: .title1)
    .scaledFont(for: base, compatibleWith: UITraitCollection(preferredContentSizeCategory: UIContentSizeCategory(typeSize)))
}

func textWidth(_ text: String, _ font: UIFont) -> CGFloat {
  ceil((text as NSString).size(withAttributes: [.font: font]).width)
}

/// One sender's Yos ("3× Yo from MIA"): at the bar's size when the whole line fits, else at the small size over
/// one or two lines ("3× Yo from" / "MAXIMILIAN_MUELL"). The name is never cut.
struct ArrivalWords: View {
  let count: Int
  let name: String
  var lead = BarRow.margin
  var trail = BarRow.margin
  @Environment(\.dynamicTypeSize) private var typeSize

  var body: some View {
    GeometryReader { geometry in
      let line = productArrivalLines(count: Int64(count), name: name, lang: .english)[0]
      // SteadyText's digits take a little more than their own width; its padding 3 on each side.
      if textWidth(line, barFont(Metrics.rowFont, typeSize)) + 16 <= geometry.size.width - lead - trail {
        BarRow(lead: lead, trail: trail) { SteadyText(line).padding(.horizontal, 3) }
      } else {
        SenderFlow(senders: [ListSender(user: "", name: name, count: UInt32(clamping: count))], lead: lead, trail: trail)
      }
    }
    .frame(height: Metrics.rowHeight)
  }
}

/// A bar's senders, newest first, over two lines at the small size ("2× Yo from TOM, 3× ANNA," / "4× PETER,
/// LUKAS +2"). The core places the names (`listFlowTwoLines`); this only measures the pieces and draws the lines.
struct SenderFlow: View {
  let senders: [ListSender]
  var lead = BarRow.margin
  var trail = BarRow.margin
  @Environment(\.dynamicTypeSize) private var typeSize

  var body: some View {
    GeometryReader { geometry in
      let lines = lines(room: max(0, geometry.size.width - lead - trail))
      let room = max(0, geometry.size.width - lead - trail)
      VStack(spacing: 2) {
        Text(lines[0])
        if lines.count > 1, !lines[1].isEmpty { Text(lines[1]) }
      }
      .font(.yo(Metrics.rowSmallFont))
      .lineLimit(1)
      // Only a name wider than the room even alone (the core places it whole): that line shrinks to fit.
      .minimumScaleFactor(0.5)
      .frame(width: room, height: geometry.size.height)
      .offset(x: lead)
    }
    .frame(height: Metrics.rowHeight)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(senders.map { "\($0.count == 1 ? "Yo" : "\($0.count) Yos") from \($0.name)" }
      .joined(separator: ", "))
  }

  private func lines(room: CGFloat) -> [String] {
    let font = barFont(Metrics.rowSmallFont, typeSize)
    func width(_ text: String) -> Float { Float(textWidth(text, font)) }
    let flow = listSenderFlow(senders: senders, lang: .english)
    let widths = ListFlowWidths(prefix: width(flow.prefix), tokens: flow.tokens.map(width),
      separator: width(flow.separator), comma: width(flow.comma), more: flow.more.map(width))
    return listFlowTwoLines(flow: flow, widths: widths, room: Float(room))
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
