// The welcome's pages, as the original's welcome screen had them (YoWelcomeController, Login.storyboard, 2015): the
// logo first, then three cards that say in a few words what Yo is, swiped through above the same buttons, with the
// dots under them. Each card shows the app itself: a picture of the list taken from the app (Resources/Intro*@3x.png,
// from the Simulator), in a phone's frame, with a title and one line under it. The buttons stay where they are
// (Onboarding's welcome); no page has its own.
//
// A carousel: past the last card comes the logo again, and before the logo the last card, without a jump back. Like
// the original's banner, the pages go on by themselves (every 4 s, always forward) until the user touches them. The
// pictures are pictures only (VoiceOver reads the title and the line); when the text is large and the room small, the
// picture gives way, then the words scroll.
import SwiftUI

/// The welcome's swipeable pages: `first` (the logo) and then the intro cards, dots under them. They go on by
/// themselves every `advanceSeconds` until a finger is on them (then they are the user's, for good); never with
/// Reduce Motion or VoiceOver.
struct Intro<First: View>: View {
  static var advanceSeconds: Double { 4 }

  @ViewBuilder let first: First
  /// Where the pages stand among the slots: the pages, with the last one again before the first and the first again
  /// after the last, so a swipe past either end goes on; once there, the real page takes over unseen.
  @State private var slot: Int? = 1
  /// Touched or swiped once: no more going on by themselves.
  @State private var touched = false
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityVoiceOverEnabled) private var voiceOver

  private static var count: Int { IntroCard.all.count + 1 }
  /// The page a slot shows: slot 0 is the last page, slot count + 1 the first.
  private static func page(_ slot: Int) -> Int { (slot - 1 + count) % count }

  var body: some View {
    VStack(spacing: 0) {
      ScrollView(.horizontal) {
        LazyHStack(spacing: 0) {
          ForEach(0..<(Self.count + 2), id: \.self) { slot in
            content(Self.page(slot)).containerRelativeFrame(.horizontal).id(slot)
          }
        }
        .scrollTargetLayout()
      }
      .scrollTargetBehavior(.paging)
      .scrollIndicators(.hidden)
      .scrollPosition(id: $slot)
      // Come to rest on a copy at either end: the real page, unseen (the same picture).
      .onScrollPhaseChange { _, phase in
        if phase == .interacting { touched = true }
        if phase == .idle, let now = slot, now == 0 || now == Self.count + 1 {
          var quiet = Transaction()
          quiet.disablesAnimations = true
          withTransaction(quiet) { slot = Self.page(now) + 1 }
        }
      }
      Dots(count: Self.count, page: Self.page(slot ?? 1)) { page in
        touched = true
        withAnimation(.smooth(duration: 0.4)) { slot = page + 1 }
      }
      .padding(.bottom, 20)
    }
    // A finger down anywhere on the pages (or the dots), before it even swipes.
    .simultaneousGesture(DragGesture(minimumDistance: 0).onChanged { _ in touched = true })
    .task(id: touched || reduceMotion || voiceOver) {
      guard !touched, !reduceMotion, !voiceOver else { return }
      while (try? await Task.sleep(for: .seconds(Self.advanceSeconds))) != nil, !touched {
        withAnimation(.smooth(duration: 0.6)) { slot = (slot ?? 1) + 1 }
      }
    }
  }

  @ViewBuilder private func content(_ page: Int) -> some View {
    if page == 0 {
      // The logo shrinks to the room left over (large text grows the buttons under it), never cut or wrapped.
      first
        .lineLimit(1)
        .minimumScaleFactor(0.3)
        .padding(.horizontal, 28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    } else {
      IntroPage(card: IntroCard.all[page - 1])
    }
  }
}

/// The pages' dots, as the system's page control: the one showing white, a tap on one goes there.
private struct Dots: View {
  let count: Int
  let page: Int
  let go: (Int) -> Void

  var body: some View {
    HStack(spacing: 0) {
      ForEach(0..<count, id: \.self) { index in
        Circle()
          .fill(.white.opacity(index == page ? 1 : 0.4))
          .frame(width: 8, height: 8)
          .frame(width: 24, height: 44)
          .contentShape(.rect)
          .onTapGesture { go(index) }
      }
    }
    .animation(.smooth(duration: 0.2), value: page)
    .accessibilityElement()
    .accessibilityLabel("Page \(page + 1) of \(count)")
    .accessibilityAdjustableAction { direction in
      switch direction {
      case .increment: go((page + 1) % count)
      case .decrement: go((page - 1 + count) % count)
      @unknown default: break
      }
    }
  }
}

/// One page's words and its picture.
struct IntroCard {
  let title: String
  let line: String
  /// The app's list as it looks for this (Resources/<name>@3x.png).
  let picture: String

  static let all: [IntroCard] = [
    IntroCard(title: "Tap a Name", line: "That’s a Yo. Nothing to type.", picture: "IntroTap"),
    IntroCard(title: "Yo Back", line: "A Yo comes in. Tap their name to Yo back.", picture: "IntroYoBack"),
    IntroCard(title: "Add Friends by Name", line: "YoGPT is there from the start. It Yos back.", picture: "IntroAdd"),
  ]
}

private struct IntroPage: View {
  let card: IntroCard

  var body: some View {
    VStack(spacing: 0) {
      Spacer(minLength: 12)
      // The picture when there is room for it beside the words; with very large text, the words alone; and if even
      // they do not fit, the words scroll.
      ViewThatFits(in: .vertical) {
        VStack(spacing: 24) {
          PhoneShot(picture: card.picture)
          words
        }
        words
        ScrollView { words }.scrollBounceBehavior(.basedOnSize)
      }
      Spacer(minLength: 12)
    }
    .padding(.horizontal, 28)
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

/// A picture of the app in the top of a phone: a black frame with round corners, fading out below.
private struct PhoneShot: View {
  let picture: String

  var body: some View {
    Image(uiImage: UIImage(named: picture) ?? UIImage())
      .resizable()
      .scaledToFit()
      .frame(width: 190)
      .clipShape(UnevenRoundedRectangle(topLeadingRadius: 26, topTrailingRadius: 26))
      .padding([.top, .horizontal], 6)
      .background(.black, in: UnevenRoundedRectangle(topLeadingRadius: 32, topTrailingRadius: 32))
      .mask {
        LinearGradient(
          stops: [.init(color: .black, location: 0.8), .init(color: .clear, location: 1)],
          startPoint: .top, endPoint: .bottom)
      }
      .accessibilityHidden(true)
  }
}
