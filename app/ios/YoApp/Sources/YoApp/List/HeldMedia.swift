import AVFoundation
import ImageIO
import SwiftUI
import UIKit
import YoCore

/// The finger holding a bar's media sign while its photos show, in screen points (nil once lifted).
@MainActor @Observable final class HeldFinger {
  var point: CGPoint?
  var barBottom = 0.0
}

/// Photos and videos, as Stories: full screen, oldest first, a line for each at the top filling up; a photo
/// `photoSeconds`, a video as long as it runs. A finger held pauses; a tap on the left third goes back, elsewhere on;
/// swiped down it closes and the unseen rest goes as if viewed. The finger that opened them, drawn down out of its bar,
/// zooms in around itself (`productHoldZoom`); moved, it pans; back at the bar or lifted, the picture springs back.
struct HeldMedia: View {
  let queue: [Arrival]
  let photoSeconds: Double
  let finger: HeldFinger
  let shows: (Arrival) -> Void
  let finished: () -> Void
  @State private var index = 0
  /// Counts each start (also of the same one again): the showing one's tasks go by it.
  @State private var round = 0
  @State private var seconds: Double?
  /// How much ran before the current stretch, and when it began (nil while paused): the line fills from the clock.
  @State private var ran = 0.0
  @State private var since: Date?
  @GestureState private var held = false  // also let go when the system takes the touch
  /// Where and when the finger came down: a new place is a new touch (one the system took leaves nothing behind).
  @State private var touched: (at: CGPoint, time: Date)?
  @State private var pulled = 0.0
  @State private var width = 393.0
  @State private var player: AVPlayer?
  /// The showing photo in full, read from its file (the list keeps only a small thumbnail).
  @State private var full: UIImage?
  /// The screen's frame (the picture fills it) and the zoom the held finger asks for.
  @State private var screen = CGRect.zero
  @State private var zoom = ProductHoldZoom(scale: 1, x: 0, y: 0)
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  private struct Run: Equatable {
    let round: Int
    let held: Bool
    let ready: Bool
  }

  init(
    queue: [Arrival], photoSeconds: Double, finger: HeldFinger, shows: @escaping (Arrival) -> Void,
    finished: @escaping () -> Void
  ) {
    self.queue = queue
    self.photoSeconds = photoSeconds
    self.finger = finger
    self.shows = shows
    self.finished = finished
  }

  @AppStorage("viewerMuted") private var muted = false

  var body: some View {
    ZStack {
      Color.black.ignoresSafeArea()
      if index < queue.count {
        let current = queue[index]
        // Inside a secure field's layer: screenshots and screen recordings show black where the picture is.
        Color.clear
          .overlay {
            ScreenshotProof {
              Color.clear
                .overlay {
                  GeometryReader { box in
                    if current.video != nil, let player {
                      PlayerLayer(player: player, screen: box.size)
                    } else if let photo = full ?? current.photo {
                      Image(uiImage: photo).resizable()
                        .aspectRatio(contentMode: ViewerFit.fills(photo.size, box.size) ? .fill : .fit)
                        .frame(width: box.size.width, height: box.size.height)
                    }
                  }
                }
                .clipped()
            }
            .scaleEffect(zoom.scale)
            .offset(x: zoom.x, y: zoom.y)
          }
          .clipped()
          .ignoresSafeArea()
          .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { screen = $0 }
          .allowsHitTesting(false)
        VStack(alignment: .leading, spacing: 10) {
          TimelineView(.animation(minimumInterval: 1.0 / 120, paused: since == nil)) { timeline in
            let running = progress(at: timeline.date)
            HStack(spacing: 3) {
              ForEach(queue.indices, id: \.self) { i in
                Capsule()
                  .fill(.white.opacity(0.35))
                  .overlay {
                    Capsule()
                      .fill(.white)
                      .scaleEffect(x: i < index ? 1 : i == index ? running : 0, anchor: .leading)
                  }
                  .frame(height: 3)
              }
            }
          }
          HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(current.from)
              .font(.yo(17, relativeTo: .headline))
              .lineLimit(1)
            if let at = current.sentAt {
              Text(Arrival.age(at))
                .font(.subheadline.weight(.semibold))
                .opacity(0.7)
                .fixedSize()
            }
            Spacer(minLength: 0)
            if current.video != nil {
              Image(systemName: muted ? "speaker.slash.fill" : "speaker.wave.2.fill")
                .font(.body.weight(.semibold))
                .contentTransition(.symbolEffect(.replace))
                .frame(width: 44, height: 32, alignment: .trailing)
                .contentShape(.rect)
                .highPriorityGesture(TapGesture().onEnded {
                  muted.toggle()
                  player?.isMuted = muted
                })
                .accessibilityLabel(muted ? "Sound Off" : "Sound On")
                .accessibilityAddTraits(.isButton)
            }
          }
          .foregroundStyle(.white)
          .shadow(color: .black.opacity(0.3), radius: 2)
          .padding(.horizontal, 4)
          .accessibilityElement(children: .combine)
          .accessibilityLabel(productNotificationLine(name: current.from, kind: .yo, request: false, lang: .english))
        }
        .padding(.horizontal, 8)
        .padding(.top, 4)
        .padding(.bottom, 40)
        .background {
          LinearGradient(colors: [.black.opacity(0.5), .clear], startPoint: .top, endPoint: .bottom)
            .ignoresSafeArea(edges: .top)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
      }
    }
    .offset(y: pulled)
    .statusBarHidden()
    .persistentSystemOverlays(.hidden)
    .contentShape(.rect)
    .onGeometryChange(for: Double.self) { $0.size.width } action: { width = $0 }
    // Any finger, also a second one while the first still holds the thumbnail underneath.
    .gesture(touches)
    // A touch the system took (no end): nothing of it stays.
    .onChange(of: held) { _, holding in
      if !holding && pulled != 0 { withAnimation(.snappy(duration: 0.25)) { pulled = 0 } }
    }
    .onChange(of: finger.point) { _, point in follow(point) }
    .task(id: round) { await load() }
    .task(id: Run(round: round, held: held, ready: seconds != nil)) { await run() }
    .onAppear { ViewerSound.open() }
    .onDisappear {
      player?.pause()
      ViewerSound.close()
    }
    .accessibilityAction(named: "Previous") { go(to: index - 1) }
    .accessibilityAction(named: "Next") { go(to: index + 1) }
    .accessibilityAction(.escape) { finished() }
  }

  private var touches: some Gesture {
    DragGesture(minimumDistance: 0)
      .updating($held) { _, held, _ in held = true }
      .onChanged { drag in
        if touched?.at != drag.startLocation { touched = (drag.startLocation, drag.time) }
        pulled = max(0, drag.translation.height) * 0.5  // the picture follows a swipe down, a little
      }
      .onEnded { drag in
        let quick = (touched.map { drag.time.timeIntervalSince($0.time) } ?? .infinity) < 0.3
        touched = nil
        let moved = drag.translation
        if moved.height > 90 && moved.height > abs(moved.width) { return swipedAway() }
        withAnimation(.snappy(duration: 0.25)) { pulled = 0 }
        guard quick, abs(moved.width) < 20, abs(moved.height) < 20 else { return }
        go(to: drag.startLocation.x < width / 3 ? index - 1 : index + 1)
      }
  }

  /// The zoom follows the finger as it moves, a little softly; lifted or back at the bar, it springs back. A light
  /// tick as it starts. Reduce Motion: no animation, the picture still zooms.
  private func follow(_ point: CGPoint?) {
    let next = point.map {
      productHoldZoom(
        fingerX: $0.x - screen.minX, fingerY: $0.y - screen.minY, barBottom: finger.barBottom - screen.minY,
        width: screen.width, height: screen.height)
    } ?? ProductHoldZoom(scale: 1, x: 0, y: 0)
    if zoom.scale == 1 && next.scale > 1 { Haptic.tap(.light) }
    let motion: Animation? = reduceMotion ? nil
      : next.scale == 1 ? .spring(duration: 0.35, bounce: 0.3) : .interactiveSpring(duration: 0.18)
    withAnimation(motion) { zoom = next }
  }

  /// Swiped away: the unseen rest goes too, as if viewed (view once). A light tap says they are gone.
  private func swipedAway() {
    player?.pause()
    let rest = queue.indices.contains(index) ? Array(queue[index...]) : []
    if rest.count > 1 || (rest.count == 1 && seconds == nil) { Haptic.tap(.light) }
    for arrival in rest { shows(arrival) }
    finished()
  }

  private func progress(at date: Date) -> Double {
    guard let seconds, seconds > 0 else { return 0 }
    return min(1, (ran + (since.map { date.timeIntervalSince($0) } ?? 0)) / seconds)
  }

  private func go(to next: Int) {
    index = max(0, next)
    seconds = nil
    ran = 0
    since = nil
    round += 1
  }

  /// Reported as seen only once it shows: one closed while loading stays. Past the last: closed.
  private func load() async {
    guard index < queue.count else { return finished() }
    let current = queue[index]
    player?.pause()
    // Only downloaded ones are opened (`FriendList.see`); one still loading is passed by, never waited for.
    guard current.pending == nil else { return go(to: index + 1) }
    guard let video = current.video else {
      player = nil
      full = nil
      if let file = current.file {
        full = await Task.detached(priority: .userInitiated) { Perf.timed("media.viewer.decode") { Self.fitted(file) } }.value
      }
      guard !Task.isCancelled else { return }
      Perf.timed("media.viewer.publish") {
        shows(current)
        seconds = photoSeconds
      }
      return
    }
    let player = Perf.timed("media.viewer.player") { AVPlayer(url: video) }
    player.isMuted = muted
    self.player = player
    let length = (try? await AVURLAsset(url: video).load(.duration).seconds) ?? 0
    guard !Task.isCancelled else { return }
    shows(current)
    // A video is 10 s at most (spec 3.2): a longer one, from a sender that ignores it, is cut there. The bar runs over
    // its real length, not the sender's word.
    seconds = length > 0 ? min(length, Self.longestVideo) : photoSeconds
  }

  static let longestVideo = MediaFile.longestVideo + 0.5

  /// At most 2048 px on the longer side, whatever the sender said (spec 3.2: the receiver checks the bytes).
  nonisolated static func fitted(_ file: URL) -> UIImage? {
    guard let source = CGImageSourceCreateWithURL(file as CFURL, nil) else { return nil }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceThumbnailMaxPixelSize: 2048,
      kCGImageSourceShouldCacheImmediately: true,
    ]
    return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
  }

  private func run() async {
    guard let seconds else { return }
    if held {
      player?.pause()
      if let since { ran += Date.now.timeIntervalSince(since) }
      since = nil
      return
    }
    since = .now
    player?.play()
    try? await Task.sleep(for: .seconds(max(0, seconds - ran)))
    guard !Task.isCancelled else { return }
    go(to: index + 1)
  }
}

/// Content left out of screenshots and screen recordings: hosted in the layer a secure text field draws its text in,
/// which iOS blanks in captures. Where that layer is not found (a later iOS), the content shows as usual.
struct ScreenshotProof<Content: View>: UIViewRepresentable {
  @ViewBuilder var content: () -> Content

  func makeUIView(context: Context) -> UIView {
    let container = UIView()
    container.backgroundColor = .clear
    let host = context.coordinator.host
    host.view.backgroundColor = .clear
    host.view.translatesAutoresizingMaskIntoConstraints = false
    host.safeAreaRegions = []

    let field = UITextField()
    field.isSecureTextEntry = true
    field.isUserInteractionEnabled = false
    field.translatesAutoresizingMaskIntoConstraints = false
    container.addSubview(field)
    pin(field, to: container)
    field.layoutIfNeeded()
    let canvas = field.subviews.first { String(describing: type(of: $0)).contains("Canvas") }
      ?? field.subviews.first
    if let canvas {
      canvas.translatesAutoresizingMaskIntoConstraints = false
      pin(canvas, to: field)
      canvas.addSubview(host.view)
      pin(host.view, to: canvas)
    } else {
      field.removeFromSuperview()
      container.addSubview(host.view)
      pin(host.view, to: container)
    }
    return container
  }

  func updateUIView(_ view: UIView, context: Context) {
    context.coordinator.host.rootView = AnyView(content())
  }

  func makeCoordinator() -> Coordinator { Coordinator(host: UIHostingController(rootView: AnyView(content()))) }

  final class Coordinator {
    let host: UIHostingController<AnyView>
    init(host: UIHostingController<AnyView>) { self.host = host }
  }

  private func pin(_ view: UIView, to parent: UIView) {
    NSLayoutConstraint.activate([
      view.leadingAnchor.constraint(equalTo: parent.leadingAnchor),
      view.trailingAnchor.constraint(equalTo: parent.trailingAnchor),
      view.topAnchor.constraint(equalTo: parent.topAnchor),
      view.bottomAnchor.constraint(equalTo: parent.bottomAnchor),
    ])
  }
}

/// Videos play their sound as Stories do, the ringer switch on silent too: `.playback` while the viewer is open,
/// `.ambient` again after. The camera's own session is left as it is.
enum ViewerSound {
  private static let queue = DispatchQueue(label: "yo.viewer-sound", qos: .userInitiated)
  nonisolated(unsafe) private static var switched = false  // only on `queue`

  static func open() {
    queue.async {
      let session = AVAudioSession.sharedInstance()
      guard [.ambient, .soloAmbient].contains(session.category) else { return }
      do {
        try session.setCategory(.playback, mode: .moviePlayback)
        try session.setActive(true)
        switched = true
      } catch { Diagnostics.record(error) }
    }
  }

  static func close() {
    queue.async {
      guard switched else { return }
      switched = false
      let session = AVAudioSession.sharedInstance()
      try? session.setActive(false, options: .notifyOthersOnDeactivation)  // music stopped for the video goes on
      try? session.setCategory(.ambient)
    }
  }
}

/// Whether a picture fills the screen or shows whole: one of about the screen's own shape (a phone's portrait photo
/// or video) fills it, losing only its rounded corners; any other shows whole, black above and below or beside.
enum ViewerFit {
  static func fills(_ picture: CGSize, _ screen: CGSize) -> Bool {
    guard picture.width > 0, picture.height > 0, screen.width > 0, screen.height > 0 else { return false }
    let ratio = (picture.width / picture.height) / (screen.width / screen.height)
    return ratio > 0.85 && ratio < 1.18
  }
}

/// A video without controls: filling the screen when its shape is about the screen's, else whole (ViewerFit).
struct PlayerLayer: UIViewRepresentable {
  let player: AVPlayer
  var screen: CGSize = .zero

  func makeUIView(context: Context) -> PlayerView {
    let view = PlayerView()
    view.playerLayer.player = player
    view.playerLayer.videoGravity = .resizeAspect
    return view
  }

  func updateUIView(_ view: PlayerView, context: Context) {
    view.playerLayer.player = player
    let size = player.currentItem?.presentationSize ?? .zero
    view.playerLayer.videoGravity = ViewerFit.fills(size, screen) ? .resizeAspectFill : .resizeAspect
  }

  final class PlayerView: UIView {
    override class var layerClass: AnyClass { AVPlayerLayer.self }
    var playerLayer: AVPlayerLayer { layer as! AVPlayerLayer }
  }
}
