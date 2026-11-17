// The welcome's pages, as the original's welcome screen had them (YoWelcomeController, Login.storyboard, 2015): the
// logo first, then three cards that say in a few words what Yo is, swiped through above the same buttons, with the
// dots under them. Each card shows the app itself: a picture of the list taken from the app (Resources/Intro*@3x.png,
// from the Simulator), in a phone's frame, with a title and one line under it. The buttons stay where they are
// (Onboarding's welcome); no page has its own.
//
// A carousel: past the last card comes the logo again, and before the logo the last card, without a jump back. Like
// the original's banner, the pages go on by themselves (every 4 s, always forward), paused while the user swipes. The
// pictures are pictures only (VoiceOver reads the title and the line); when the text is large and the room small, the
// picture gives way, then the words scroll.
import SwiftUI

/// The welcome's swipeable pages: `first` (the logo) and then the intro cards, dots under them. They go on by
/// themselves every `advanceSeconds`, a full interval after the user last swiped or tapped a dot; never with VoiceOver.
struct Intro<First: View>: View {
  static var advanceSeconds: Double { 6 }

  @ViewBuilder let first: First
  /// Where the pages stand among the slots: the pages, with the last one again before the first and the first again
  /// after the last, so a swipe past either end goes on; once there, the real page takes over unseen.
  @State private var slot: Int? = 1
  /// When the user last swiped or tapped a dot: the pages wait a full interval after it.
  @State private var touched = Date.distantPast
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityVoiceOverEnabled) private var voiceOver

  private static var count: Int { IntroCard.all.count + 1 }
  /// The page a slot shows: slot 0 is the last page, slot count + 1 the first.
  private static func page(_ slot: Int) -> Int { (slot - 1 + count) % count }

  var body: some View {
    VStack(spacing: 0) {
      Spacer(minLength: 0)
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
      .frame(height: 360)  // the cards' own height: the dots follow right under them
      // Starting on the logo (slot 1 of count + 2), also when the position is not applied at the first layout: the
      // anchor 1 / (count + 1) of the content is the same point of the view.
      .defaultScrollAnchor(UnitPoint(x: 1 / Double(Self.count + 1), y: 0.5))
      // Come to rest on a copy at either end: the real page, unseen (the same picture).
      .onScrollPhaseChange { _, phase in
        if phase == .interacting { touched = .now }
        if phase == .idle, let now = slot, now == 0 || now == Self.count + 1 {
          var quiet = Transaction()
          quiet.disablesAnimations = true
          withTransaction(quiet) { slot = Self.page(now) + 1 }
        }
      }
      Dots(count: Self.count, page: Self.page(slot ?? 1)) { page in
        touched = .now
        withAnimation(.smooth(duration: 0.4)) { slot = page + 1 }
      }
      .padding(.top, 4)
      // On the logo the screen stays clean: the dots come with the first card.
      .opacity(Self.page(slot ?? 1) == 0 ? 0 : 1)
      .animation(.smooth(duration: 0.3), value: Self.page(slot ?? 1) == 0)
      Spacer(minLength: 0)
    }
    // Reduce Motion: the next page fades in rather than sliding.
    .task(id: voiceOver) {
      guard !voiceOver else { return }
      while (try? await Task.sleep(for: .seconds(Self.advanceSeconds))) != nil {
        guard Date.now.timeIntervalSince(touched) >= Self.advanceSeconds else { continue }
        withAnimation(reduceMotion ? .easeInOut(duration: 0.3) : .smooth(duration: 0.6)) {
          slot = (slot ?? 1) + 1
        }
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
        .frame(maxWidth: .infinity, maxHeight: .infinity)  // the logo stays centred, higher than the cards
    } else {
      IntroPage(card: IntroCard.all[page - 1])
    }
  }
}

/// The welcome's cards, available again from Help without the logo or account setup.
struct IntroHelp: View {
  @Environment(\.dismiss) private var dismiss
  @State private var page = 0
  @State private var pageHeight: CGFloat = 480

  /// A sheet like Secure Your Account: amethyst, grabber, a close button, the pages and their dots.
  var body: some View {
    NavigationStack {
      VStack(spacing: 0) {
        Spacer(minLength: 0)
        // The pages at their own height with their dots right under the words, the block centred.
        TabView(selection: $page) {
          ForEach(IntroCard.all.indices, id: \.self) { index in
            IntroPage(card: IntroCard.all[index], low: true).tag(index)
          }
        }
        .tabViewStyle(.page(indexDisplayMode: .never))
        .frame(height: pageHeight)
        .background {
          IntroPage(card: IntroCard.all[0], low: true).fixedSize(horizontal: false, vertical: true).hidden()
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { pageHeight = $0 }
        }
        Dots(count: IntroCard.all.count, page: page) { next in withAnimation(.smooth) { page = next } }
        Spacer(minLength: 0)
      }
      .foregroundStyle(.white)
      .background(Color.amethyst)
      .overlay(alignment: .topTrailing) {
        Button {
          dismiss()
        } label: {
          Image(systemName: "xmark")
            .font(.system(size: 17, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: 44, height: 44)
            .glassEffect(.regular.interactive(), in: .circle)
            .contentShape(.circle)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Close")
        .padding(.top, 20)
        .padding(.trailing, 16)
      }
      .toolbar(.hidden, for: .navigationBar)
    }
    .presentationBackground(Color.amethyst)
    .presentationDragIndicator(.visible)
    .presentationDetents([.large])
  }
}

/// Help & About's card on top: the tutorial's pages, smaller, swiped through on amethyst with their dots.
struct HelpHero: View {
  @State private var page = 0

  var body: some View {
    VStack(spacing: 0) {
      // The pages sit on a hidden first page that gives the height: laid out on the first frame, nothing measured
      // later and grown into place while the page slides in.
      IntroPage(card: IntroCard.all[0], low: true, zoom: 1, sizer: true)
        .fixedSize(horizontal: false, vertical: true)
        .hidden()
        .overlay {
          TabView(selection: $page) {
            ForEach(IntroCard.all.indices, id: \.self) { index in
              IntroPage(card: IntroCard.all[index], low: true, zoom: 1).tag(index)
            }
          }
          .tabViewStyle(.page(indexDisplayMode: .never))
        }
      Dots(count: IntroCard.all.count, page: page) { next in withAnimation(.smooth) { page = next } }
    }
    .padding(.top, 24)
    .foregroundStyle(.white)
    .frame(maxWidth: .infinity)
    .background(Color.amethyst, in: .rect(cornerRadius: 26))
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
  /// A hand over the picture showing what to do.
  var gesture: IntroGesture? = nil

  static let all: [IntroCard] = [
    IntroCard(
      title: "Tap to Send Yo", line: "", picture: "IntroTap",
      gesture: .tap(y: 0.456, yo: false)),  // the "Sent Yo" bar
    IntroCard(title: "Swipe for Pictures", line: "", picture: "IntroTap", gesture: .swipe),
    IntroCard(
      title: "Add Friends", line: "", picture: "IntroTap",
      gesture: .tap(y: 0.868, yo: false)),  // the "+" bar
  ]
}

/// What the tutorial's hand does: tap a bar (its centre, as a share of the picture's height) or swipe the camera in.
enum IntroGesture: Equatable {
  case tap(y: Double, yo: Bool)
  case swipe
}

private struct IntroPage: View {
  let card: IntroCard
  /// In the Help card: picture and words at their own height, no room around them (the card places them).
  var low = false
  /// The picture's scale there: bigger in the tutorial sheet, which has the room.
  var zoom: CGFloat = 1.35
  /// Only the page's size: the picture's room left empty (Help's card sizes its pages by it).
  var sizer = false

  var body: some View {
    if low {
      VStack(spacing: 24) {
        // The whole picture scaled, gestures and all.
        Group {
          if sizer {
            Color.clear.frame(width: 202, height: 190 * 743 / 540 + 6)
          } else {
            PhoneShot(picture: card.picture, gesture: card.gesture)
          }
        }
        .scaleEffect(zoom, anchor: .top)
        .frame(height: 190 * 743 / 540 * zoom, alignment: .top)
        words
      }
      .padding(.horizontal, 28)
    } else {
      roomy
    }
  }

  private var roomy: some View {
    VStack(spacing: 0) {
      Spacer(minLength: 12)
      // The picture when there is room for it beside the words; with very large text, the words alone; and if even
      // they do not fit, the words scroll.
      ViewThatFits(in: .vertical) {
        VStack(spacing: 24) {
          PhoneShot(picture: card.picture, gesture: card.gesture)
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
      if !card.line.isEmpty {
        Text(card.line)
          .font(.system(.body, weight: .medium))
          .opacity(0.85)
      }
    }
    .multilineTextAlignment(.center)
    .fixedSize(horizontal: false, vertical: true)
    .accessibilityElement(children: .combine)
  }
}

/// A picture of the app in the top of a phone: a black frame with round corners, fading out below.
private struct PhoneShot: View {
  let picture: String
  var gesture: IntroGesture? = nil
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @MainActor private static var cache: [String: UIImage] = [:]

  /// Decoded once and kept: a page pushed in shows its picture on the first frame, not a beat later.
  @MainActor private static func decoded(_ name: String) -> UIImage {
    if let image = cache[name] { return image }
    let image = UIImage(named: name).map { $0.preparingForDisplay() ?? $0 } ?? UIImage()
    cache[name] = image
    return image
  }

  var body: some View {
    // Loose bundle files (Resources/<name>@3x.png), not an asset catalog: Image(name) would not find them.
    Image(uiImage: Self.decoded(picture))
      .resizable()
      .scaledToFit()
      // TabView can propose an unspecified height; keep the screenshot’s intrinsic proportions
      // instead of letting the resizable image collapse to the black phone background.
      .frame(width: 190, height: 190 * 743 / 540)
      // Inside the phone's screen: clipped with the picture, faded with it.
      .overlay {
        switch gesture {
        case .swipe: swipeHint
        case .tap(let y, let yo): tapHint(y: y, yo: yo)
        case nil: EmptyView()
        }
      }
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

  /// The hint's loop: from rest to the gesture, eased, then back at once, over and over. A phase animator, not
  /// withAnimation(.repeatForever) on appear: that one's transaction caught the card's first layout during the
  /// push, so the card flew in offset and growing. Reduce Motion holds one still frame.
  private func looping<Hint: View>(
    still: Bool, delay: Double, curve: Animation,
    @ViewBuilder hint: @escaping (Bool) -> Hint
  ) -> some View {
    Group {
      if reduceMotion {
        hint(still)
      } else {
        PhaseAnimator([false, true]) { on in
          hint(on)
        } animation: { on in
          on ? curve.delay(delay) : nil
        }
      }
    }
    .frame(width: 190, height: 190 * 743 / 540)
  }

  /// A hand tapping a bar again and again: it presses in, a soft ring spreads, it lifts.
  private func tapHint(y: Double, yo: Bool) -> some View {
    let size = CGSize(width: 190, height: 190 * 743 / 540)
    let point = CGPoint(x: size.width * (yo ? 0.62 : 0.5), y: size.height * y)
    return looping(still: false, delay: 0.3, curve: .easeOut(duration: 0.9)) { swiping in
      ZStack {
        if yo {
          // The Yo rises straight up out of the tapped bar, as in the app.
          Text("Yo")
            .font(.yo(18))
            .foregroundStyle(.white.opacity(0.8))
            .opacity(swiping ? 0 : 1)
            .position(x: size.width * 0.5, y: point.y + (swiping ? -size.height * 0.3 : -10))
        }
        Circle()
          .stroke(.white.opacity(swiping ? 0 : 0.8), lineWidth: 3)
          .frame(width: swiping ? 64 : 18, height: swiping ? 64 : 18)
          .position(point)
        Image(systemName: "hand.point.up.left.fill")
          .font(.system(size: 40))
          .foregroundStyle(.white)
          .shadow(radius: 4)
          .scaleEffect(swiping ? 1 : 0.86)
          .position(x: point.x + 14, y: point.y + 26)
      }
    }
  }

  /// A hand sliding in from the right edge, the camera following it in.
  private var swipeHint: some View {
    let size = CGSize(width: 190, height: 190 * 743 / 540)
    return looping(still: true, delay: 0.4, curve: .easeInOut(duration: 1.4)) { swiping in
      ZStack(alignment: .leading) {
        Color.black.opacity(0.35)
          .overlay {
            Image(systemName: "camera.fill").font(.system(size: 34)).foregroundStyle(.white)
          }
          .frame(width: size.width * (swiping ? 0.6 : 0.08), height: size.height)
          .offset(x: size.width * (swiping ? 0.4 : 0.92))
        Image(systemName: "hand.point.up.left.fill")
          .font(.system(size: 40))
          .foregroundStyle(.white)
          .shadow(radius: 4)
          .offset(x: size.width * (swiping ? 0.32 : 0.82), y: size.height * 0.45)
      }
      .frame(width: size.width, height: size.height, alignment: .leading)
    }
  }
}
