// The welcome's pages, as the original's welcome screen had them (YoWelcomeController, Login.storyboard, 2015): the
// logo first, then cards of coloured bars that say in a few words what Yo is ("Jerry's GF / Yo's to say, / "Hi! I'm /
// thinking / of you." / #shecares"), swiped through above the same buttons, with the dots under them. Here the cards
// show what this app does: a tap on a name, a Yo back, the camera, a friend by name, nobody else reading along. The
// buttons stay where they are (Onboarding's welcome); no page has its own.
//
// Each page: a small card of bars as the list draws them, a title and one line under it. The card is a picture only
// (VoiceOver reads the title and the line); when the text is large and the room small, the card gives way, then the
// words scroll.
import SwiftUI

/// The welcome's swipeable pages: `first` (the logo) and then the intro cards, the system's page dots under them.
struct Intro<First: View>: View {
  @ViewBuilder let first: First
  @State private var page = 0

  var body: some View {
    TabView(selection: $page) {
      // The logo shrinks to the room left over (large text grows the buttons under it), never cut or wrapped.
      first
        .lineLimit(1)
        .minimumScaleFactor(0.3)
        .padding(.horizontal, 28)
        .padding(.bottom, 36)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .tag(0)
      ForEach(Array(IntroCard.all.enumerated()), id: \.offset) { index, card in
        IntroPage(card: card, shown: page == index + 1)
          .tag(index + 1)
      }
    }
    .tabViewStyle(.page(indexDisplayMode: .always))
    .indexViewStyle(.page(backgroundDisplayMode: .interactive))
  }
}

/// One page's words and its card.
struct IntroCard {
  let title: String
  let line: String
  let bars: [IntroBar]
  /// The camera's page: the bars see-through over a picture, as in camera mode.
  var camera = false

  static let all: [IntroCard] = [
    IntroCard(
      title: "Tap a Name",
      line: "That's a Yo. Nothing to type.",
      bars: [
        IntroBar("Marissa", color: 0),
        IntroBar("Peter", color: 1, then: "Sent Yo!"),
        IntroBar("Jenny", color: 2),
      ]),
    IntroCard(
      title: "Yo Back",
      line: "A Yo comes in. Tap their name to answer.",
      bars: [
        IntroBar("Peter", color: 3, end: "Yo!"),
        IntroBar("Marissa", color: 4),
        IntroBar("Jenny", color: 5),
      ]),
    IntroCard(
      title: "Photo or Video",
      line: "Camera on: tap a name for a photo, hold it for a video.",
      bars: [
        IntroBar("Melanie", color: 6, then: "Sent 📷"),
        IntroBar("Donald", color: 7, then: "Sent 🎥", offset: 1),
        IntroBar("Jenny", color: 0),
      ],
      camera: true),
    IntroCard(
      title: "Add Friends by Name",
      line: "YoGPT is there from the start. It Yos back.",
      bars: [
        IntroBar("YoGPT", color: 1, end: "Yo"),
        IntroBar("Jenny", color: 2),
        IntroBar("+", color: 3),
      ]),
    IntroCard(
      title: "Just You Two",
      line: "End-to-end encrypted. No phone number.",
      bars: [
        IntroBar("Just", color: 4),
        IntroBar("you", color: 5),
        IntroBar("two. 🔒", color: 6),
      ]),
  ]
}

/// A bar on a card: its words, maybe a Yo at its end, maybe words it turns into now and then (as a sent Yo does).
struct IntroBar {
  let text: String
  let color: Int
  var end: String?
  var then: String?
  /// Which beat it turns on (two bars on one card take turns).
  var offset = 0

  init(_ text: String, color: Int, end: String? = nil, then: String? = nil, offset: Int = 0) {
    self.text = text
    self.color = color
    self.end = end
    self.then = then
    self.offset = offset
  }
}

private struct IntroPage: View {
  let card: IntroCard
  /// On screen: only then the card plays.
  let shown: Bool

  var body: some View {
    VStack(spacing: 0) {
      Spacer(minLength: 12)
      // The card when there is room for it beside the words; with very large text, the words alone; and if even they
      // do not fit, the words scroll.
      ViewThatFits(in: .vertical) {
        VStack(spacing: 28) {
          IntroCardView(card: card, playing: shown)
          words
        }
        words
        ScrollView { words }.scrollBounceBehavior(.basedOnSize)
      }
      Spacer(minLength: 12)
    }
    .padding(.horizontal, 28)
    .padding(.bottom, 36)  // clear of the page dots
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }

  private var words: some View {
    VStack(spacing: 10) {
      Text(card.title)
        .font(.yo(28, relativeTo: .title))
        .accessibilityAddTraits(.isHeader)
      Text(card.line)
        .font(.system(.body, weight: .medium))
        .opacity(0.85)
    }
    .multilineTextAlignment(.center)
    .fixedSize(horizontal: false, vertical: true)
    .accessibilityElement(children: .combine)
  }
}

/// The card: bars from edge to edge in rounded corners, as the original's welcome cards. A bar with `then` turns into
/// it for a moment every few seconds, while the page is on screen and motion is welcome; else it shows it still.
private struct IntroCardView: View {
  let card: IntroCard
  let playing: Bool
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  private static let beat = 1.6

  var body: some View {
    Group {
      if playing && !reduceMotion {
        TimelineView(.periodic(from: .now, by: Self.beat)) { context in
          bars(beat: Int(context.date.timeIntervalSinceReferenceDate / Self.beat))
        }
      } else {
        bars(beat: nil)
      }
    }
    .frame(maxWidth: 300)
    .background {
      if card.camera {
        // A picture behind see-through bars, as camera mode lays the list over the live one.
        LinearGradient(
          colors: [Color(hex: 0xF39C12), Color(hex: 0xE74C3C), Color(hex: 0x2C3E50)],
          startPoint: .topLeading, endPoint: .bottomTrailing)
      }
    }
    .clipShape(.rect(cornerRadius: 18))
    .accessibilityHidden(true)
  }

  /// `beat` nil: the still picture, every bar with `then` showing it.
  private func bars(beat: Int?) -> some View {
    let turned = beat.map { ($0 % 4) } // four beats: 0 and 2 plain, 1 the first bar's turn, 3 the second's
    return VStack(spacing: 0) {
      ForEach(Array(card.bars.enumerated()), id: \.offset) { _, bar in
        let on = bar.then != nil && (turned.map { $0 == 1 + 2 * bar.offset } ?? true)
        IntroBarView(bar: bar, on: on, seeThrough: card.camera)
      }
    }
    .animation(.smooth(duration: 0.3), value: turned)
  }
}

private struct IntroBarView: View {
  let bar: IntroBar
  let on: Bool
  let seeThrough: Bool

  var body: some View {
    // A fixed size: the card is a picture, not text to read (the page's words grow with Dynamic Type).
    Text(on ? bar.then ?? bar.text : bar.text)
      .font(.custom("Montserrat-Bold", fixedSize: 24))
      .lineLimit(1)
      .minimumScaleFactor(0.5)
      .contentTransition(.opacity)
      .padding(.horizontal, 56)
      .frame(maxWidth: .infinity)
      .frame(height: 54)
      .overlay(alignment: .trailing) {
        if let end = bar.end {
          Text(end)
            .font(.custom("Montserrat-Bold", fixedSize: 20))
            .padding(.trailing, 18)
        }
      }
      .background(Color.row(bar.color).opacity(seeThrough ? 0.45 : 1))
  }
}
