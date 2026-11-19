import SwiftUI
import UIKit
import YoCore

enum ListMotion {
  static let expansion = Animation.spring(duration: 0.35, bounce: 0)
  static let flight = Animation.spring(duration: 0.35, bounce: 0.16)
}

/// A finger on a bar's photos: down (they show at once), then a short tap (they run on) or let go after holding.
enum ThumbnailTouch {
  case down, tapped, released
}

/// A friend's bar, reading its own `Bars.Bar`, so a tap redraws this bar alone.
struct LiveBar: View, @MainActor Equatable {
  // Closures reach live State; the observable row object redraws its own bar, so parent updates reuse unchanged bars.
  static func == (lhs: Self, rhs: Self) -> Bool {
    lhs.friend.key == rhs.friend.key
      && lhs.friend.name == rhs.friend.name && lhs.friend.color == rhs.friend.color
      && lhs.friend.user == rhs.friend.user && lhs.friend.group == rhs.friend.group
      && lhs.friend.blocked == rhs.friend.blocked && lhs.friend.gone == rhs.friend.gone
      && lhs.friend.keyChanged == rhs.friend.keyChanged && lhs.friend.request == rhs.friend.request
      && lhs.friend.undoFirst == rhs.friend.undoFirst && lhs.friend.undoSeconds == rhs.friend.undoSeconds
      && lhs.bar === rhs.bar
      && lhs.landing === rhs.landing && lhs.color == rhs.color && lhs.recording == rhs.recording
      && lhs.cameraOn == rhs.cameraOn
      && lhs.media == rhs.media && lhs.hint == rhs.hint && (lhs.hold == nil) == (rhs.hold == nil)
      && lhs.keepHeight == rhs.keepHeight
  }

  let friend: Friend
  let bar: FriendList.Bars.Bar
  let color: Color
  let cameraOn: Bool
  let recording: Bool
  let stop: () -> Void
  let media: [Arrival]
  let landing: FriendList.LandingSlot
  /// Another bar is being tapped: this one keeps its height.
  let keepHeight: Bool
  private var yo: FriendList.Landing? { landing.value }
  let hint: String?
  let hold: ((Bool, CGPoint?) -> Void)?
  let cancelHold: () -> Void
  let press: (Bool) -> Void
  let flyBegan: (UUID) -> Void
  let view: (ThumbnailTouch) -> Void
  let action: (CGPoint?) -> Void

  var body: some View {
    RowButton(
      // A request shows only "Yo Request from X": no count, no photo, not even its 📷 (what spam aims at).
      title, count: friend.request ? 0 : count, name: friend.group == nil ? friend.name : nil, color: color, cameraOn: cameraOn, recording: recording,
      outgoing: bar.outgoing.isEmpty ? (bar.sendingBatch?.items ?? bar.sentStack).last?.started
        : (bar.countdownStarted ?? .distantFuture),
      outgoingItems: bar.outgoing.isEmpty ? (bar.sendingBatch?.items ?? bar.sentStack) : bar.outgoing,
      undoAvailable: !bar.outgoing.isEmpty, sendSeconds: bar.seconds, stop: stop,
      media: friend.request ? [] : media, mediaMarkCount: bar.modelMarkCount, mediaSign: bar.modelMediaSign, yo: friend.request ? nil : yo, hint: hint, hold: hold, cancelHold: cancelHold, press: press, flyBegan: flyBegan, view: view, faded: friend.gone && !bar.hasNotice,
      icon: friend.group == nil ? nil : "person.3.fill", senders: bar.modelSenders, keepHeight: keepHeight,
      action: action)
      // Gone: still their name (whose bar it is), faded; a tap says "Not on Yo".
      .accessibilityValue(friend.gone ? productText(text: .notOnYo, lang: .english) : "")
  }

  private var title: String { bar.modelTitle.isEmpty ? friend.name : bar.modelTitle }
  private var count: Int { bar.modelCount }
}

/// The slot keeps its identity even while empty; the resting stack survives each new flight.
private struct OutgoingThumbnailSlot: View {
  let item: FriendList.Outgoing?
  let destination: CGPoint
  let fallbackOrigin: CGPoint
  var coordinateOrigin: CGPoint = .zero
  var side: CGFloat = 28
  let began: (UUID) -> Void
  @State private var restingImage: UIImage?
  @State private var restingVideo = false

  var body: some View {
    let top = destination
    ZStack {
      Group {
        if let restingImage { Image(uiImage: restingImage).resizable().scaledToFill() }
        else { Color.clear }
      }
        .frame(width: side, height: side).clipped()
        .overlay { VideoPlayGlyph(visible: restingVideo, side: side) }
        .position(top)
        .opacity(item == nil || restingImage == nil ? 0 : 1)
      if let item, let image = item.thumbnail {
        OutgoingThumbnail(image: image, video: item.text.contains(productMediaSign(kind: .video)), destination: top,
          origin: item.origin.map { CGPoint(x: $0.x - coordinateOrigin.x, y: $0.y - coordinateOrigin.y) } ?? fallbackOrigin, side: side, began: {
            Perf.mark("thumb.fly.start")
            began(item.id)
          })
          .id(item.id)
          .transition(.identity)
      }
    }
    .onChange(of: item, initial: true) { old, new in
      if old?.id != new?.id {
        if new == nil { restingImage = nil }
        else if let thumbnail = old?.thumbnail { restingImage = thumbnail; restingVideo = old?.text.contains(productMediaSign(kind: .video)) == true }
      }
    }
  }
}

/// A captured item flies once from the finger, then rests on top of the stack.
private struct OutgoingThumbnail: View {
  let image: UIImage
  var video = false
  let destination: CGPoint
  let origin: CGPoint
  var side: CGFloat = 28
  let began: () -> Void
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var landed = false

  var body: some View {
    let resting = landed || reduceMotion
    Image(uiImage: image)
      .resizable()
      .scaledToFill()
      .frame(width: side, height: side)
      .clipped()
      .overlay { VideoPlayGlyph(visible: video, side: side) }
      .scaleEffect(resting ? 1 : Metrics.rowHeight * 0.6 / side)
      .position(resting ? destination : origin)
      .opacity(reduceMotion && !landed ? 0 : 1)
      .task {
        withAnimation(reduceMotion ? .easeOut(duration: 0.18) : ListMotion.flight) {
          landed = true
        }
        began()
      }
  }
}

/// One friend's bar: tap to send. With `hold`, holding is reported as well (true when held long enough, false when let
/// go): camera mode records a video with it. A finger on the photos' thumbnail is reported through `view`.
struct RowButton: View {
  let title: String
  /// How many sends the words count ("Sent 3× Yo!").
  var count = 1
  /// A friend's name, for what came from them ("3× Yo from Mia"); nil for a group or the add row.
  var name: String?
  let color: Color
  var cameraOn = false
  var recording = false
  /// When the newest photo or video on its way out started; its ring closes in `sendSeconds`, a tap on it undoes.
  var outgoing: Date?
  var outgoingItems: [FriendList.Outgoing] = []
  var undoAvailable = true
  var sendSeconds = FriendList.sendDelay
  var stop: () -> Void = {}
  var mediaMarkCount = 0
  var mediaSign = ""
  var media: [Arrival] = []
  var yo: FriendList.Landing?
  private static let fade = FriendList.yoSeconds - FriendList.yoSolidSeconds
  var hint: String?
  var hold: ((Bool, CGPoint?) -> Void)?
  var cancelHold: () -> Void = {}
  var press: (Bool) -> Void = { _ in }
  var flyBegan: (UUID) -> Void = { _ in }
  var view: (ThumbnailTouch) -> Void = { _ in }
  var faded = false
  /// A group's bar: its sign, and who sent the Yos on it, a line each under the name.
  var icon: String?
  var senders: [RenderedSender] = []
  /// Another bar is being tapped: this one keeps the rows it had then (`frozen`), showing its newest line in one.
  var keepHeight = false
  @State private var frozen: Rows?
  let action: (CGPoint?) -> Void
  @State private var stationaryTouch = BarTouchGate()
  @State private var touchOrigin: CGPoint?
  @State private var holdOrigin: CGPoint?
  @State private var touchSpace = UUID()
  @State private var shutterDown = false
  @State private var holding = false
  @State private var viewing = false
  @State private var touchedAt = Date.distantPast
  @State private var recordingByAction = false
  @AppStorage(SettingKey.leftHanded) private var leftHanded = false

  init(
    _ title: String, count: Int = 1, name: String? = nil, color: Color, cameraOn: Bool = false, recording: Bool = false, outgoing: Date? = nil,
    outgoingItems: [FriendList.Outgoing] = [], undoAvailable: Bool = true,
    sendSeconds: Double = FriendList.sendDelay, stop: @escaping () -> Void = {},
    media: [Arrival] = [], mediaMarkCount: Int = 0, mediaSign: String = "", yo: FriendList.Landing? = nil, hint: String? = nil, hold: ((Bool, CGPoint?) -> Void)? = nil, cancelHold: @escaping () -> Void = {}, press: @escaping (Bool) -> Void = { _ in }, flyBegan: @escaping (UUID) -> Void = { _ in },
    view: @escaping (ThumbnailTouch) -> Void = { _ in }, faded: Bool = false, icon: String? = nil, senders: [RenderedSender] = [],
    keepHeight: Bool = false, action: @escaping (CGPoint?) -> Void
  ) {
    self.keepHeight = keepHeight
    self.icon = icon
    self.senders = senders
    self.title = title
    self.count = count
    self.name = name
    self.color = color
    self.cameraOn = cameraOn
    self.recording = recording
    self.outgoing = outgoing
    self.outgoingItems = outgoingItems
    self.undoAvailable = undoAvailable
    self.sendSeconds = sendSeconds
    self.stop = stop
    self.media = media
    self.mediaMarkCount = mediaMarkCount
    self.mediaSign = mediaSign
    self.yo = yo
    self.hint = hint
    self.hold = hold
    self.cancelHold = cancelHold
    self.press = press
    self.flyBegan = flyBegan
    self.view = view
    self.faded = faded
    self.action = action
  }

  var body: some View {
    ZStack(alignment: leftHanded ? .trailing : .leading) {
      // One button and label subtree across camera changes: stacks and flights keep their state.
      Button {
        guard stationaryTouch.isStationary, !cameraOn || !holding else { return }
        action(touchOrigin)
        touchOrigin = nil
      } label: { label }
      .buttonStyle(BarPress(cameraOn: cameraOn, touchDown: shutterDown, press: press))
      .disabled(!cameraOn && viewing)
      .accessibilityHint(hint ?? "")
      .accessibilityActions {
        if cameraOn && hold != nil {
          Button(recordingByAction ? "Stop and Send Video" : "Record Video") {
            recordingByAction.toggle()
            hold?(recordingByAction, nil)
          }
        }
      }
      .gesture(BarTouchObserver(gate: stationaryTouch, held: { down in
        guard let hold else { return }
        if down {
          guard stationaryTouch.isStationary else { return }
          holding = true
          holdOrigin = touchOrigin
          hold(true, holdOrigin)
        } else if holding {
          hold(false, holdOrigin)  // the finger lifted: moving while recording never cancels the video
          touchOrigin = nil
          Task {
            try? await Task.sleep(for: .milliseconds(300))
            holding = false
          }
        }
      }, observesHold: hold != nil, pressed: { down in
        if cameraOn { shutterDown = down }
      }) {
        touchOrigin = nil
        press(false)
        if holding {
          cancelHold()
          holding = false
        }
      })
      // Where the finger touched, for the photo's flight: a tap gesture, never a drag (a zero-distance drag on every bar
      // took the list's pan, and the list no longer scrolled).
      .simultaneousGesture(
        SpatialTapGesture(coordinateSpace: .named(touchSpace))
          .onEnded { value in touchOrigin = value.location }
      )
      .coordinateSpace(name: touchSpace)
      // What takes its own touches (the photo sign) lies over the bar: Buttons in a Button do not work.
      .overlay {
        line(touchable: true)
          // Received media stays visible; camera taps and holds reach the shutter underneath.
          .allowsHitTesting(!cameraOn)
          .accessibilityHidden(cameraOn)
      }
      // The undo ring, away from where the bar is tapped (a tap more is another photo, never an undo). A sibling of
      // the button, so its gestures and style cannot replace the stack's flight or countdown.
      if let outgoing {
        undoRing(since: outgoing)
          .padding(leftHanded ? .trailing : .leading, BarRow.margin)
          // In the sent line (the top one when two), not between both lines.
          .frame(height: Metrics.rowHeight)
          .frame(maxHeight: .infinity, alignment: .top)
      }
    }
    .onChange(of: keepHeight, initial: true) { _, keep in frozen = keep ? natural : nil }
  }

  /// The friend's colour from edge to edge, the name large in the middle, red while recording.
  private var label: some View {
    line(touchable: false)
      .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
      .background {
        ZStack(alignment: .leading) {
          // Pressed in camera mode: more see-through (the shutter's flash), also while recording.
          Color.clear.modifier(ListColor(color: color, pressed: cameraOn && (shutterDown || recording)))
          if recording { RecordingFill(seconds: MediaFile.longestVideo, fromRight: leftHanded) }
        }
      }
      .contentShape(.rect)
  }

  /// The bar's words, each line in the middle (BarRow). Drawn twice: the words in the bar, over it the photo sign.
  private func line(touchable: Bool) -> some View {
    Group {
      if touchable { signLayer } else { words }
    }
    .font(.yo(Metrics.rowFont))  // counts held still by SteadyText
    .foregroundStyle(.white)
    .animation(nil, value: title)  // other words stand in their place at once
    // A new count ("10×" to "11×") swaps in one frame, no crossfade of the old digits under the new.
    .contentTransition(.identity)
    .animation(nil, value: count)
    .animation(nil, value: receivedCount)
  }

  private var hasSign: Bool { !media.isEmpty && !mediaSign.isEmpty }
  private var received: Bool { icon == nil && name != nil && (!media.isEmpty || yo != nil) }
  /// What came in and what the bar says otherwise: a line each (kept while another bar is tapped, `keepHeight`).
  private var twoRows: Bool { rows.two }
  private var senderRows: Int { rows.senders }

  struct Rows: Equatable {
    var two: Bool
    var senders: Int
  }
  private var natural: Rows {
    Rows(two: received && title != name,
      senders: icon != nil && !senders.isEmpty && (yo != nil || !media.isEmpty) ? senders.count : 0)
  }
  private var rows: Rows { keepHeight ? (frozen ?? natural) : natural }
  /// Photos still in their countdown (undo possible): "Sending", not yet "Sent".
  private var sendingMedia: Bool {
    undoAvailable && outgoingItems.contains { productMediaKind(label: $0.text) != .yo }
  }
  /// Photos waiting count as Yos.
  private var receivedCount: Int { !media.isEmpty ? max(1, mediaMarkCount, media.count) : (yo?.total ?? 1) }

  /// Room kept at the bar's sides: the outgoing thumbnail on its side (every line), the photo sign on its.
  private func room(sign: Bool) -> (lead: CGFloat, trail: CGFloat) {
    let thumb = outgoing == nil ? BarRow.margin : BarRow.margin + Self.ring + 12
    let end = sign ? BarRow.margin + Self.sign + 8 : BarRow.margin
    return leftHanded ? (end, thumb) : (thumb, end)
  }

  private var words: some View {
    VStack(spacing: 0) {
      if received && !twoRows {
        receivedRow
      } else {
        let room = room(sign: hasSign && !twoRows)
        // A group's sign only beside its name: "Sent 5× Yo!" has the room to itself.
        let groupSign = title == BarSendText.sentYo ? nil : icon
        BarRow(lead: room.lead, trail: room.trail) {
          // A group: "#" as part of its name ("#Hey"), in the name's own font (usernames can't hold "#" or "@").
          SteadyText(title == BarSendText.sentYo ? productSendWords(mark: productCounted(count: Int64(count)), sending: sendingMedia, lang: .english)
            : groupSign != nil ? "#" + title : title)
        }
        if twoRows {
          if received { receivedRow.transition(.opacity) }
          else { Color.clear.frame(height: Metrics.rowHeight) }
        }
      }
      if senderRows > 0 {
        let room = room(sign: false)
        let shown = icon != nil && !senders.isEmpty && (yo != nil || !media.isEmpty) ? Array(senders.prefix(senderRows)) : []
        SenderBlocks(senders: shown, lead: room.lead, trail: room.trail, paused: yo?.paused ?? cameraOn)
          .modifier(ClockFade(since: yo?.fading, seconds: Self.fade, paused: yo?.paused ?? false,
            elapsed: yo?.fadeElapsed ?? 0))
          .transition(.opacity)
        if shown.count < senderRows {
          Color.clear.frame(height: Metrics.rowHeight * CGFloat(senderRows - shown.count))
        }
      }
    }
    .opacity(faded ? 0.55 : 1)
  }

  private var receivedRow: some View {
    let room = room(sign: hasSign)
    let lines = productArrivalLines(count: Int64(receivedCount), name: name ?? "", lang: .english)
    let cuts = Array(lines.dropFirst().dropLast().enumerated())
    return BarRow(lead: room.lead, trail: room.trail) {
      SteadyText(lines[0]).padding(.horizontal, 3)
      ForEach(cuts, id: \.offset) { _, line in
        SteadyText(line).padding(.horizontal, 3)
      }
      SteadyText(lines[lines.count - 1]).padding(.horizontal, 3)
    }
    // A plain Yo fades out; photos and videos stay until seen.
    .modifier(ClockFade(since: media.isEmpty ? yo?.fading : nil, seconds: Self.fade,
      paused: media.isEmpty && (yo?.paused ?? false), elapsed: media.isEmpty ? (yo?.fadeElapsed ?? 0) : 0))
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(media.isEmpty
      ? "\(receivedCount == 1 ? "Yo" : "\(receivedCount) Yos") from \(name ?? "")"
      : (media.count == 1 ? "Yo with a photo or video from \(name ?? "")"
        : "\(media.count) Yos with photos or videos from \(name ?? "")"))
  }

  private var signLayer: some View {
    VStack(spacing: 0) {
      if twoRows { Color.clear.frame(height: Metrics.rowHeight).allowsHitTesting(false) }
      HStack(spacing: 0) {
        if !leftHanded { Spacer(minLength: 0) }
        if hasSign { sign }
        if leftHanded { Spacer(minLength: 0) }
      }
      .padding(.horizontal, BarRow.margin)
      .frame(height: Metrics.rowHeight)
      Spacer(minLength: 0)
    }
    .frame(maxHeight: .infinity, alignment: .top)
    .animation(nil, value: media.isEmpty)
  }

  private static let ring = 56.0
  private static let sign = 40.0  // the sign at the words' own size

  /// "📷" or "🎥" while media waits unseen: a finger on it shows them; only the sign takes the touch.
  @ViewBuilder private var sign: some View {
    if !mediaSign.isEmpty {
      Text(mediaSign)
        .font(.system(size: Metrics.rowFont))  // as tall as the words
        .frame(width: Self.sign, height: Self.sign)
        .scaleEffect(viewing ? 0.88 : 1)
        .contentShape(.rect.inset(by: -10))  // easy to hit
        .highPriorityGesture(
          DragGesture(minimumDistance: 0)
            .onChanged { _ in
              if !viewing {
                viewing = true
                touchedAt = .now
                view(.down)
              }
            }
            .onEnded { _ in
              viewing = false
              view(Date.now.timeIntervalSince(touchedAt) < 0.35 ? .tapped : .released)
            }
        )
        .accessibilityAddTraits(.isButton)
        .accessibilityLabel(media.count == 1 ? "1 photo or video" : "\(media.count) photos or videos")
        .accessibilityAction { view(.down); view(.tapped) }  // opens them, running to the end
        .transition(.scale.combined(with: .opacity))
    }
  }

  /// A ring closing like an upload (a photo stack with a line under it instead); a tap undoes the send.
  private func undoRing(since start: Date) -> some View {
    Button(action: stop) {
      TimelineView(.animation(minimumInterval: 1.0 / 120)) { timeline in
        let done = min(1, max(0, timeline.date.timeIntervalSince(start) / max(0.001, sendSeconds)))
        ZStack {
          if outgoingItems.contains(where: { productMediaKind(label: $0.text) != .yo }) {
            GeometryReader { geometry in
              OutgoingThumbnailSlot(
                item: outgoingItems.last(where: { $0.thumbnail != nil }),
                destination: CGPoint(x: Self.ring / 2, y: Self.ring / 2),
                fallbackOrigin: CGPoint(x: Self.ring / 2, y: Self.ring / 2),
                coordinateOrigin: geometry.frame(in: .named(touchSpace)).origin,
                side: Self.ring, began: flyBegan)
            }
            .clipShape(Rectangle())  // square, like the bars: no rounded corners in the list
            Rectangle().fill(.white)
              .frame(width: Self.ring * (1 - done), height: 3)
              .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
              .offset(y: 3)  // right under the photo, touching it
          } else {
          Circle().stroke(.white.opacity(0.35), lineWidth: 3)
          Circle()
            .trim(from: 0, to: 1 - done)
            .stroke(.white, style: StrokeStyle(lineWidth: 3, lineCap: .round))
            .rotationEffect(.degrees(-90))
          Image(systemName: "arrow.uturn.backward").font(.system(size: 18, weight: .heavy))
          }
        }
        .foregroundStyle(.white)
        .frame(width: Self.ring, height: Self.ring)
      }
      .transaction { $0.animation = nil }
    }
    .buttonStyle(.plain)
    .contentShape(.rect.inset(by: -12))
    .disabled(!undoAvailable)
    .accessibilityLabel(undoAvailable ? "Undo Send" : productText(text: .sending, lang: .english))
    .transition(.scale.combined(with: .opacity))
  }
}

/// Recording: red grows from the bar's leading edge, reaching the other edge at the longest video.
struct RecordingFill: View {
  let seconds: Double
  var fromRight = false
  @State private var start = Date.now

  var body: some View {
    TimelineView(.animation(minimumInterval: 1.0 / 120)) { context in
      let elapsed = context.date.timeIntervalSince(start)
      Canvas { context, size in
        let width = size.width * min(1, elapsed / seconds)
        let x = fromRight ? size.width - width : 0
        context.fill(Path(CGRect(x: x, y: 0, width: width, height: size.height)), with: .color(.alizarin.opacity(0.85)))
      }
    }
    .allowsHitTesting(false)
    .accessibilityHidden(true)
  }
}

/// One style in both modes keeps the label alive across camera changes.
private struct BarPress: ButtonStyle {
  let cameraOn: Bool
  let touchDown: Bool
  let press: (Bool) -> Void

  func makeBody(configuration: Configuration) -> some View {
    let pressed = cameraOn ? touchDown : configuration.isPressed
    // In camera mode no press look: the colour plane goes see-through under the finger (ListColor's `pressed`).
    configuration.label
      // In the list a quick, light flash.
      .overlay(.white.opacity(!cameraOn && configuration.isPressed ? 0.18 : 0))
      .animation(.easeOut(duration: 0.15), value: configuration.isPressed)
      .onChange(of: pressed) { _, pressed in
        if pressed { Haptic.prepare() }
        press(pressed)
      }
  }
}

/// Row-local observation keeps flight layering from invalidating the whole list on every send.
struct FlightRowLayer: ViewModifier {
  let bar: FriendList.Bars.Bar
  func body(content: Content) -> some View {
    content.zIndex(bar.hasNotice || !bar.outgoing.isEmpty ? 10 : 0)
  }
}
