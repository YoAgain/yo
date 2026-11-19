// The welcome's pages, after the original's welcome screen (YoWelcomeController, 2015): the logo, then cards showing
// the app in a phone frame (Resources/Intro*@3x.png). An endless carousel that advances by itself, paused by swipes.
import SwiftUI

/// Advances every `advanceSeconds`, a full interval after the last swipe or dot tap; never with VoiceOver.
struct Intro<First: View>: View {
  private static var advanceSeconds: Double { 6 }

  @ViewBuilder let first: First
  /// Slots: the last page again before the first and the first again after the last, so swipes past either end go on.
  @State private var slot: Int? = 1
  @State private var touched = Date.distantPast
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityVoiceOverEnabled) private var voiceOver

  private static var count: Int { IntroCard.all.count + 1 }
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
      .frame(height: 360)
      // Start on the logo even when the scroll position is not applied at the first layout.
      .defaultScrollAnchor(UnitPoint(x: 1 / Double(Self.count + 1), y: 0.5))
      // Come to rest on a copy at either end: jump to the real page, unseen.
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
      .opacity(Self.page(slot ?? 1) == 0 ? 0 : 1)
      .animation(.smooth(duration: 0.3), value: Self.page(slot ?? 1) == 0)
      Spacer(minLength: 0)
    }
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
      // The logo shrinks to the room left (large text grows the buttons), never cut or wrapped.
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

struct HelpHero: View {
  @State private var page = 0

  var body: some View {
    VStack(spacing: 0) {
      // A hidden first page gives the height on the first frame, so nothing grows into place while sliding in.
      IntroPage(card: IntroCard.all[0], low: true, zoom: 1.25, sizer: true)
        .fixedSize(horizontal: false, vertical: true)
        .hidden()
        .overlay {
          TabView(selection: $page) {
            ForEach(IntroCard.all.indices, id: \.self) { index in
              IntroPage(card: IntroCard.all[index], low: true, zoom: 1.25).tag(index)
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

struct IntroCard {
  let title: String
  let picture: String
  var gesture: IntroGesture? = nil

  static let all: [IntroCard] = [
    IntroCard(title: "Tap to Send Yo", picture: "IntroTap", gesture: .tap(y: 0.456)),  // the "Sent Yo" bar
    IntroCard(title: "Swipe for Pictures", picture: "IntroTap", gesture: .swipe),
    IntroCard(title: "Add Friends", picture: "IntroTap", gesture: .tap(y: 0.868)),  // the "+" bar
  ]
}

/// Tap a bar (its centre, as a share of the picture's height) or swipe the camera in.
enum IntroGesture: Equatable {
  case tap(y: Double)
  case swipe
}

private struct IntroPage: View {
  let card: IntroCard
  /// In the Help card: picture and words at their own height, no room around them.
  var low = false
  var zoom: CGFloat = 1.35
  /// Only the page's size, the picture's room left empty (Help's card sizes its pages by it).
  var sizer = false

  var body: some View {
    if low {
      VStack(spacing: 24) {
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
      // Picture and words when there is room; with very large text the words alone, else scrolling.
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
    Text(card.title)
      .font(.yo(28, relativeTo: .title))
      .accessibilityAddTraits(.isHeader)
      .multilineTextAlignment(.center)
      .fixedSize(horizontal: false, vertical: true)
  }
}

/// A picture of the app in the top of a phone frame, fading out below.
private struct PhoneShot: View {
  let picture: String
  var gesture: IntroGesture? = nil
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @MainActor private static var cache: [String: UIImage] = [:]

  /// Decoded once: a page pushed in shows its picture on the first frame.
  @MainActor private static func decoded(_ name: String) -> UIImage {
    if let image = cache[name] { return image }
    let image = UIImage(named: name).map { $0.preparingForDisplay() ?? $0 } ?? UIImage()
    cache[name] = image
    return image
  }

  var body: some View {
    // Loose bundle files, not an asset catalog: Image(name) would not find them.
    Image(uiImage: Self.decoded(picture))
      .resizable()
      .scaledToFit()
      // TabView can propose an unspecified height: keep the screenshot's proportions.
      .frame(width: 190, height: 190 * 743 / 540)
      .overlay {
        switch gesture {
        case .swipe: swipeHint
        case .tap(let y): tapHint(y: y)
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

  /// A PhaseAnimator, not withAnimation(.repeatForever) on appear: that one's transaction caught the card's first
  /// layout during the push. Reduce Motion holds one still frame.
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

  private func tapHint(y: Double) -> some View {
    let size = CGSize(width: 190, height: 190 * 743 / 540)
    let point = CGPoint(x: size.width * 0.5, y: size.height * y)
    return looping(still: false, delay: 0.3, curve: .easeOut(duration: 0.9)) { swiping in
      ZStack {
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
