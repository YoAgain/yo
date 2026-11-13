// The list of friends, as in 2014: one coloured bar per friend from edge to edge, the name large in the middle; a tap
// sends a Yo ("Sent Yo!" on the bar, then it goes on top). The order is the original's: whoever you last sent a Yo to
// or got one from is on top; each friend keeps their colour as they move (the original's went with the place). In the mockup a tap sends the Yo to yourself, as if that
// friend had sent it (Friends.replyDelay). Hold a bar to remove the friend; the last bar adds one.
//
// On top, the system's bar: profile (a sheet) on the left, the camera on the right, between them the title "Yo",
// a heading that becomes a glass pill once rows scroll behind it.
//
// A Yo arriving: the title gains a "!" (one per Yo in a burst) with a haptic; the sender's bar is on top at once, its
// words knock and it says "Yo" at its end, in the name's own type ("3×" before it), fading out within `yoSeconds`; then
// it is gone, as in the original (no unread mark). A photo or video waits in the same place as "Yo 📷" or "Yo 🎥": a
// finger on it plays them (HeldMedia), and what was shown is gone (as Snapchat began).
//
// Camera mode, the original's camera "context": the bars turn see-through over the live picture; a tap on a name
// takes a photo, holding it records a video. Either goes after `sendDelay`, a ring at the bar's left that undoes the
// send when tapped. The bottom has the photo library (left) and front/back (right), as the Camera app.
import AVFoundation
import PhotosUI
import SwiftUI

struct FriendList: View {
  /// How long a photo stays when its thumbnail is held.
  static let photoSeconds = 3.0  // Snapchat's first default
  /// How long a photo or video waits before it goes, time to undo it.
  static let sendDelay = 1.5
  /// How long a plain Yo shows on its bar.
  static let yoSeconds = 5.0
  /// How long of that it stands fully before it fades.
  static let yoSolidSeconds = 3.0

  @Environment(Inbox.self) private var inbox
  @Environment(Friends.self) private var friends

  // The list
  @State private var adding = false
  @State private var newName = ""
  @FocusState private var typing: Bool
  @State private var settings = false
  @State private var position = ScrollPosition(idType: String.self)
  /// Rows are behind the title: it wears its glass pill.
  @State private var scrolled = false

  // Sending
  /// What each bar says for a moment after a send ("Sent Yo!", "Not sent").
  @State private var sent: [Friend: String] = [:]
  /// Counts sends, for the haptic.
  @State private var sends = 0
  /// Photos and videos on their way out, per friend, each with when it started; any number at once (a tap on the bar
  /// sends another, the ring at its left undoes them).
  @State private var outgoing: [Friend: [Outgoing]] = [:]
  @State private var sendTasks: [UUID: Task<Void, Never>] = [:]

  struct Outgoing: Equatable {
    let id = UUID()
    let started = Date.now
  }

  // Camera mode
  @State private var cameraOn = false
  @State private var camera = Camera()
  @State private var recordingFor: Friend?
  @State private var libraryItem: PhotosPickerItem?
  /// A photo from the library, waiting to be sent: it stands in for the live picture.
  @State private var picked: Data?

  // Arriving
  /// One "!" per Yo in a burst, gone when the burst is over.
  @State private var bangs = 0
  @State private var cheers = 0
  @State private var lastCheer = Date.distantPast
  /// The plain Yos on bars now, by sender.
  @State private var landed: [String: Landing] = [:]
  /// Arrivals already shown.
  @State private var launched = 0
  /// The photos and videos playing while a thumbnail is held, and those already shown.
  @State private var viewing: [Arrival]?
  @State private var shown: [Arrival] = []

  struct Landing: Equatable {
    let key: String
    let at: Date
    var count: Int
  }

  var body: some View {
    reacting(
      navigation
        .sheet(isPresented: $settings) { SettingsView() }
        .overlay {
          if let viewing {
            HeldMedia(queue: viewing, photoSeconds: Self.photoSeconds) {
              shown.append($0)
            } finished: {
              closeViewer()
            }
            .transition(.opacity)
          }
        }
    )
  }

  private var navigation: some View {
    NavigationStack {
      ScrollView {
        LazyVStack(spacing: 0) {
          ForEach(friends.all) { friend in
            row(friend)
              .id(friend.name)
          }
          addRow
        }
        .scrollTargetLayout()
      }
      .scrollBounceBehavior(.basedOnSize)
      .scrollDismissesKeyboard(.interactively)
      .scrollPosition($position)
      // No bar behind the controls: they float as glass over the list, and at the top the background is the brand's
      // Amethyst exactly. The status bar keeps the system's soft edge, so the clock stays readable over the bars.
      .toolbarBackgroundVisibility(.hidden, for: .navigationBar)
      .scrollEdgeEffectStyle(.soft, for: .top)
      .onScrollGeometryChange(for: Bool.self) { $0.contentOffset.y + $0.contentInsets.top > 4 } action: { _, now in
        withAnimation(.smooth(duration: 0.3)) { scrolled = now }
      }
      // Amethyst behind everything; in camera mode the live picture or the picked photo.
      .background { backdrop.ignoresSafeArea() }
      .overlay(alignment: .bottom) {
        if cameraOn { cameraControls }
      }
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .principal) { title }
        ToolbarItem(placement: .topBarLeading) {
          // Your picture opens your profile and settings in a sheet, as Apple's own apps do.
          Button("Profile", systemImage: "person.fill") { settings = true }
        }
        // The camera; it turns red (Alizarin, the original's round button) while it is on.
        ToolbarItem(placement: .topBarTrailing) {
          if cameraOn {
            Button("Camera Off", systemImage: "camera.fill") { cameraOn = false }
              .buttonStyle(.glassProminent)
              .tint(.alizarin)
          } else {
            Button("Camera", systemImage: "camera") { cameraOn = true }
          }
        }
      }
    }
  }

  /// "Yo", one "!" per Yo in a burst (each out of a blur, as iOS changes text); in camera mode what to do under it.
  /// A heading at the top of the list; once rows pass behind it, a glass pill as the buttons beside it, so it stays
  /// legible. The pill grows with every "!".
  private var title: some View {
    VStack(spacing: 0) {
      HStack(spacing: 0) {
        Text("Yo")
        ForEach(0..<bangs, id: \.self) { _ in
          Text("!").transition(.blurReplace(.upUp))
        }
      }
      if cameraOn {
        Text(picked == nil ? "Tap: photo · Hold: video" : "Tap a name to send it")
          .font(.system(size: 11, weight: .semibold))
          .opacity(0.85)
          .fixedSize()
          .transition(.blurReplace)
      }
    }
    .font(.yo(Metrics.titleFont, relativeTo: .largeTitle))
    .foregroundStyle(.white)
    .lineLimit(1)
    .padding(.horizontal, 16)
    .fixedSize(horizontal: true, vertical: false)
    .frame(height: Metrics.titleHeight)
    .glassEffect(scrolled ? .regular : .identity, in: .capsule)
    .accessibilityAddTraits(.isHeader)
  }

  // MARK: Bars

  @ViewBuilder private func row(_ friend: Friend) -> some View {
    let waiting = inbox.media[friend.name] ?? []
    let bar = RowButton(
      rowTitle(friend),
      color: .row(friend.color).opacity(cameraOn ? 0.45 : 1),  // see-through over the camera
      recording: recordingFor == friend,
      outgoing: outgoing[friend]?.last?.started,
      stop: { cancelSending(friend) },
      media: waiting,
      yo: landed[friend.name],
      hint: cameraOn ? "Tap for a photo, hold for a video" : nil,
      hold: cameraOn && picked == nil ? { hold(friend, $0) } : nil,
      view: { see(friend, $0) }
    ) {
      send(to: friend)
    }
    if cameraOn || !waiting.isEmpty {
      bar  // holding records a video, or shows the photos: no menu then
    } else {
      bar
        .contextMenu {
          Button("Remove \(friend.name)", systemImage: "trash", role: .destructive) { friends.remove(friend) }
        }
    }
  }

  /// The bar's words: what is happening, else the name.
  private func rowTitle(_ friend: Friend) -> String {
    if recordingFor == friend { return "● Rec" }
    if let text = sent[friend] { return text }
    return friend.name
  }

  /// The last bar, as in the original: "+", and the name typed right there once tapped.
  @ViewBuilder private var addRow: some View {
    let color = Color.row(friends.nextColor).opacity(cameraOn ? 0.45 : 1)  // the colour the new friend will have
    if adding {
      TextField("", text: $newName, prompt: Text("Name").foregroundStyle(.white.opacity(0.6)))
        .font(.yo(Metrics.rowFont))
        .foregroundStyle(.white)
        .tint(.white)
        .multilineTextAlignment(.center)
        .textInputAutocapitalization(.words)
        .autocorrectionDisabled()
        .submitLabel(.done)
        .focused($typing)
        .padding(.horizontal)
        .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
        .background(color)
        .onSubmit(add)
        .onChange(of: typing) { _, focused in
          if !focused { add() }
        }
        .onAppear { typing = true }
    } else {
      RowButton("+", color: color) { adding = true }
        .accessibilityLabel("Add a friend")
    }
  }

  private func add() {
    friends.add(newName)
    newName = ""
    adding = false
  }

  // MARK: Camera mode

  @ViewBuilder private var backdrop: some View {
    if cameraOn, let picked, let image = UIImage(data: picked) {
      Image(uiImage: image).resizable().scaledToFill()
    } else if cameraOn {
      CameraPreview(session: camera.session)
    } else {
      Color.amethyst
    }
  }

  /// The photo library at the bottom left, front/back at the bottom right, as in the Camera app.
  private var cameraControls: some View {
    ZStack {
      let thumbnail = pickedThumbnail  // the picker's label closure is Sendable: take the value here
      PhotosPicker(selection: $libraryItem, matching: .images) {
        LibraryLabel(thumbnail: thumbnail)
      }
      .buttonStyle(.plain)
      .glassEffect(.regular.interactive(), in: .circle)
      .accessibilityLabel("Photo Library")
      .frame(maxWidth: .infinity, alignment: .leading)
      Button { camera.flip() } label: {
        Image(systemName: "arrow.triangle.2.circlepath")
          .font(.body.weight(.medium))
          .foregroundStyle(.white)
          .frame(width: 44, height: 44)
      }
      .buttonStyle(.plain)
      .glassEffect(.regular.interactive(), in: .circle)
      .accessibilityLabel("Front or Back Camera")
      .frame(maxWidth: .infinity, alignment: .trailing)
    }
    .padding(.horizontal, 20)
    .padding(.bottom, 8)
    .transition(.move(edge: .bottom).combined(with: .opacity))
  }

  private var pickedThumbnail: UIImage? {
    picked.flatMap(UIImage.init(data:))?.preparingThumbnail(of: CGSize(width: 120, height: 120))
  }

  private func switchCamera(_ on: Bool) async {
    guard on else { return camera.stop() }
    if await Camera.allowed() { camera.start() } else { cameraOn = false }
  }

  // MARK: Sending

  private func send(to friend: Friend) {
    let delay = Friends.replyDelay
    sends += 1
    let library = picked
    picked = nil
    libraryItem = nil
    guard cameraOn || library != nil else {
      toTop(friend.name)  // on top at once, "Sent Yo!" there
      showSent(to: friend, "Sent Yo!")
      Task { await inbox.fakeYo(from: friend.name, after: delay) }
      return
    }
    run(friend, "Sent 📷 Yo!") {
      if let library { return Prepared { await inbox.fakeYo(from: friend.name, after: delay, photo: library) } }
      guard let photo = await camera.photo() else { return nil }
      return Prepared {
        Library.saveIfWanted(photo: photo)  // only what the camera took, not the library's
        await inbox.fakeYo(from: friend.name, after: delay, photo: photo)
      }
    }
  }

  /// Holding a name in camera mode: a video from the moment it is held until it is let go.
  private func hold(_ friend: Friend, _ holding: Bool) {
    let delay = Friends.replyDelay
    if holding {
      guard recordingFor == nil else { return }  // one video at a time: a second finger records nothing
      recordingFor = friend
      camera.startVideo()
    } else if recordingFor == friend {
      recordingFor = nil
      sends += 1
      run(friend, "Sent 🎥 Yo!") {
        guard let video = await camera.stopVideo() else { return nil }
        return Prepared {
          Library.saveIfWanted(video: video)
          await inbox.fakeYo(from: friend.name, after: delay, video: video)
        } discard: {
          try? FileManager.default.removeItem(at: video)
        }
      }
    }
  }

  /// A photo or video on its way out: the ring at the bar's left closes in `sendDelay`; meanwhile `prepare` takes the
  /// photo or ends the video and hands back the send. Then it goes, unless the ring was tapped. Several can run for
  /// one friend at once.
  private func run(_ friend: Friend, _ text: String, prepare: @escaping @MainActor () async -> Prepared?) {
    let one = Outgoing()
    outgoing[friend, default: []].append(one)
    sendTasks[one.id] = Task {
      async let ready = prepare()
      try? await Task.sleep(for: .seconds(Self.sendDelay))
      let prepared = await ready
      sendTasks[one.id] = nil
      guard !Task.isCancelled, outgoing[friend]?.contains(one) == true else {
        prepared?.discard()  // undone: nothing of it stays behind
        return
      }
      outgoing[friend]?.removeAll { $0 == one }
      if outgoing[friend]?.isEmpty == true { outgoing[friend] = nil }
      guard let prepared else { return showSent(to: friend, "Not sent") }
      toTop(friend.name)  // it went: on top now
      showSent(to: friend, text)
      await prepared.send()
    }
  }

  /// A photo or video ready to go: what sends it, and what throws it away when the send is undone.
  struct Prepared {
    let send: @MainActor () async -> Void
    var discard: @MainActor () -> Void = {}
  }

  /// The ring tapped: every photo or video still on its way to this friend stays here.
  private func cancelSending(_ friend: Friend) {
    for one in outgoing[friend] ?? [] {
      sendTasks[one.id]?.cancel()
      sendTasks[one.id] = nil
    }
    outgoing[friend] = nil
    showSent(to: friend, "Not sent")
  }

  /// What the bar says after a send, for 1.5 seconds as in the original.
  private func showSent(to friend: Friend, _ text: String) {
    sent[friend] = text
    Task {
      try? await Task.sleep(for: .seconds(1.5))
      guard sent[friend] == text else { return }
      withAnimation(.snappy) { sent[friend] = nil }
    }
  }

  // MARK: Seeing

  /// A finger on a bar's thumbnail plays its photos and videos, oldest first. Held, letting go closes them; a short
  /// tap lets them run to the end (a tap on the picture skips). What was shown is gone.
  private func see(_ friend: Friend, _ touch: ThumbnailTouch) {
    switch touch {
    case .down:
      shown = []
      viewing = inbox.media[friend.name]
    case .tapped:
      break  // it runs on by itself
    case .released:
      closeViewer()
    }
  }

  private func closeViewer() {
    viewing = nil
    inbox.seen(shown)
    shown = []
  }
}

/// The list's reactions to what changes (arrivals, the camera, the library), kept apart from its layout.
extension FriendList {
  fileprivate func reacting(_ content: some View) -> some View {
    feedback(changes(content))
  }

  private func changes(_ content: some View) -> some View {
    content
      .onChange(of: inbox.arrivals.count) { _, count in arrived(count) }
      .onChange(of: libraryItem) { _, item in load(item) }
      .onChange(of: cameraOn) { _, on in cameraSwitched(on) }
      .onDisappear { cameraOn = false }
  }

  private func feedback(_ content: some View) -> some View {
    animations(haptics(content))
  }

  private func haptics(_ content: some View) -> some View {
    content
      .sensoryFeedback(trigger: cheers) { _, _ in .impact(weight: .heavy, intensity: min(1, 0.6 + 0.1 * Double(bangs))) }
      .sensoryFeedback(.impact, trigger: sends)
      .sensoryFeedback(.start, trigger: recordingFor) { (_: Friend?, new: Friend?) in new != nil }
      .sensoryFeedback(.selection, trigger: viewing) { (old: [Arrival]?, new: [Arrival]?) in old == nil && new != nil }
  }

  private func animations(_ content: some View) -> some View {
    content
      .animation(.easeOut(duration: 0.15), value: viewing)
      .animation(.smooth, value: cameraOn)
      .animation(.smooth, value: picked)
      .preferredColorScheme(.dark)  // white status bar and bar items on purple
  }

  /// New Yos: shown one after the other when several came at once (on opening the app).
  private func arrived(_ count: Int) {
    let new = inbox.arrivals.dropFirst(launched)
    launched = count
    for (i, arrival) in new.enumerated() {
      show(arrival, after: Double(i) * 0.35)
    }
  }

  /// One Yo: the title gains a "!" (with the haptic), the sender's bar goes on top (the list scrolls there if it is
  /// not on screen), and its Yo shows there until `yoSeconds` are over. A photo stays until seen (Inbox.media).
  private func show(_ arrival: Arrival, after delay: Double) {
    let name = arrival.from
    Task {
      try? await Task.sleep(for: .seconds(delay))
      cheers += 1
      lastCheer = .now
      toTop(name)  // on top at once, the Yo on it in the same moment
      withAnimation(.spring(duration: 0.3, bounce: 0.6)) {
        bangs += 1
        let count = (landed[name]?.count ?? 0) + 1
        landed[name] = Landing(key: arrival.id, at: .now, count: count)
      }
      try? await Task.sleep(for: .seconds(1.0))
      // The burst is over when no Yo came for a second: the "!"s fold away together.
      if Date.now.timeIntervalSince(lastCheer) >= 0.95 {
        withAnimation(.spring(duration: 0.35, bounce: 0.2)) { bangs = 0 }
      }
      try? await Task.sleep(for: .seconds(Self.yoSeconds - 1.0))
      if landed[name]?.key == arrival.id {
        withAnimation(.smooth) { _ = landed.removeValue(forKey: name) }
      }
    }
  }

  /// A friend goes on top, and the list with them: to its very start, below the title (scrolling to the bar itself
  /// would leave it behind the bar of buttons). At once, as in the original: a bar sliding up first and its Yo coming
  /// after reads as two events.
  private func toTop(_ name: String) {
    var now = Transaction()
    now.disablesAnimations = true
    withTransaction(now) {
      friends.promote(name)
      position.scrollTo(edge: .top)
    }
  }

  /// The photo picked in the library; a load that finishes after another pick, a send or the camera going off does not
  /// land.
  private func load(_ item: PhotosPickerItem?) {
    guard let item else { return picked = nil }
    Task {
      let data = try? await item.loadTransferable(type: Data.self)
      if libraryItem == item { picked = data }
    }
  }

  private func cameraSwitched(_ on: Bool) {
    if !on {
      picked = nil
      libraryItem = nil
    }
    Task { await switchCamera(on) }
  }
}

/// The library button: the picked photo when there is one.
struct LibraryLabel: View {
  let thumbnail: UIImage?

  var body: some View {
    if let thumbnail {
      Image(uiImage: thumbnail)
        .resizable()
        .scaledToFill()
        .frame(width: 44, height: 44)
        .clipShape(.circle)
    } else {
      Image(systemName: "photo.on.rectangle")
        .font(.body.weight(.medium))
        .foregroundStyle(.white)
        .frame(width: 44, height: 44)
    }
  }
}

/// A finger on a bar's photos: down (they show at once), then a short tap (they run on) or let go after holding.
enum ThumbnailTouch {
  case down, tapped, released
}

/// One friend's bar: tap to send. With `hold`, holding is reported as well (true when held long enough, false when let
/// go): camera mode records a video with it. A finger on the photos' thumbnail is reported through `view`.
struct RowButton: View {
  let title: String
  let color: Color
  var recording = false
  /// When the newest photo or video on its way out started; its ring closes in `sendSeconds`, a tap on it undoes.
  var outgoing: Date?
  var sendSeconds = FriendList.sendDelay
  var stop: () -> Void = {}
  var media: [Arrival] = []
  /// A Yo on this bar now: its words knock and it says "Yo" at its end ("3×" before it), fading out within `yoSeconds`.
  var yo: FriendList.Landing?
  var yoSeconds = FriendList.yoSeconds
  private static let solid = FriendList.yoSolidSeconds
  var hint: String?
  var hold: ((Bool) -> Void)?
  var view: (ThumbnailTouch) -> Void = { _ in }
  let action: () -> Void
  @State private var pressed = false
  @State private var viewing = false
  @State private var touchedAt = Date.distantPast
  @State private var recordingByAction = false

  init(
    _ title: String, color: Color, recording: Bool = false, outgoing: Date? = nil, stop: @escaping () -> Void = {},
    media: [Arrival] = [], yo: FriendList.Landing? = nil, hint: String? = nil, hold: ((Bool) -> Void)? = nil,
    view: @escaping (ThumbnailTouch) -> Void = { _ in }, action: @escaping () -> Void
  ) {
    self.title = title
    self.color = color
    self.recording = recording
    self.outgoing = outgoing
    self.stop = stop
    self.media = media
    self.yo = yo
    self.hint = hint
    self.hold = hold
    self.view = view
    self.action = action
  }

  var body: some View {
    Group {
      if let hold {
        label
          .overlay(.white.opacity(pressed ? 0.18 : 0))
          .scaleEffect(pressed ? 0.985 : 1)
          .animation(.smooth(duration: 0.15), value: pressed)
          .onTapGesture(perform: action)
          .onLongPressGesture(minimumDuration: 0.3, maximumDistance: 60) {
            hold(true)
          } onPressingChanged: { pressing in
            pressed = pressing
            if !pressing { hold(false) }
          }
          .accessibilityAddTraits(.isButton)
          .accessibilityHint(hint ?? "")
          // VoiceOver has no hold: recording starts and stops as an action.
          .accessibilityAction(named: recordingByAction ? "Stop and Send Video" : "Record Video") {
            recordingByAction.toggle()
            hold(recordingByAction)
          }
      } else {
        Button(action: action) { label }
          .buttonStyle(RowPress())
          .disabled(viewing)  // a finger on the photos is not a press on the bar
      }
    }
    .overlay(alignment: .trailing) {
      mark.modifier(Knock(trigger: yo?.key)).padding(.trailing, 30)
    }
    .overlay(alignment: .leading) {
      if let outgoing { undoRing(since: outgoing) }
    }
    .animation(.snappy, value: outgoing)
    .animation(.spring(duration: 0.4, bounce: 0.35), value: yo)
    .animation(.smooth, value: media)
  }

  /// A bar of the friend's colour from edge to edge, the name large in the middle (the original's), red while
  /// recording.
  private var label: some View {
    Text(title)
      .font(.yo(Metrics.rowFont))
      .lineLimit(1)
      .minimumScaleFactor(0.5)
      .foregroundStyle(.white)
      .modifier(Knock(trigger: yo?.key))
      .padding(.horizontal, 104)  // room for the undo ring on the left and "Yo 📷" on the right
      .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
      .background(recording ? Color.alizarin.opacity(0.85) : color)
      .contentShape(.rect)
  }

  /// Something on its way out: a ring at the bar's left closing like an upload; a tap on it undoes the send.
  private func undoRing(since start: Date) -> some View {
    Button(action: stop) {
      TimelineView(.animation) { timeline in
        let done = min(1, timeline.date.timeIntervalSince(start) / sendSeconds)
        ZStack {
          Circle().stroke(.white.opacity(0.35), lineWidth: 3)
          Circle()
            .trim(from: 0, to: 1 - done)
            .stroke(.white, style: StrokeStyle(lineWidth: 3, lineCap: .round))
            .rotationEffect(.degrees(-90))
          Image(systemName: "arrow.uturn.backward").font(.system(size: 13, weight: .bold))
        }
        .foregroundStyle(.white)
        .frame(width: 36, height: 36)
      }
    }
    .buttonStyle(.plain)
    .contentShape(.circle.inset(by: -10))
    .accessibilityLabel("Undo Send")
    .padding(.leading, 18)
    .transition(.scale.combined(with: .opacity))
  }

  /// At the bar's end, in the name's own type: "Yo 📷" or "Yo 🎥" for photos and videos not yet seen (a finger on it
  /// plays them at once), or "Yo" for one that just came; a count before either.
  @ViewBuilder private var mark: some View {
    if let first = media.first {
      HStack(alignment: .firstTextBaseline, spacing: 6) {
        if media.count > 1 { count(media.count) }
        Text(first.video != nil ? "Yo 🎥" : "Yo 📷").font(.yo(Metrics.rowFont)).foregroundStyle(.white)
      }
      .scaleEffect(viewing ? 0.92 : 1)
      .contentShape(.rect.inset(by: -12))  // easy to hit
      // A finger down shows them at once; a short tap lets them run, holding closes on letting go.
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
    } else if let yo {
      // Full for a moment, then slowly fading out.
      TimelineView(.animation) { timeline in
        let age = timeline.date.timeIntervalSince(yo.at)
        HStack(alignment: .firstTextBaseline, spacing: 6) {
          if yo.count > 1 { count(yo.count) }
          Text("Yo").font(.yo(Metrics.rowFont)).foregroundStyle(.white)
        }
        .opacity(age < Self.solid ? 1 : max(0, 1 - (age - Self.solid) / (yoSeconds - Self.solid)))
      }
      .accessibilityLabel(yo.count == 1 ? "Yo" : "\(yo.count) Yos")
      .transition(.scale(scale: 0.3).combined(with: .opacity))
    }
  }

  private func count(_ n: Int) -> some View {
    Text("\(n)×")
      .font(.yo(19, relativeTo: .headline))
      .foregroundStyle(.white)
      .contentTransition(.numericText())
  }
}

/// A Yo knocks: the words on a bar nudge sideways twice, as a poke; the bar itself stays (moving it would show what is
/// behind it).
struct Knock: ViewModifier {
  let trigger: String?

  func body(content: Content) -> some View {
    content.keyframeAnimator(initialValue: 0.0, trigger: trigger) { words, x in
      words.offset(x: x)
    } keyframes: { _ in
      KeyframeTrack {
        SpringKeyframe(10, duration: 0.08)
        SpringKeyframe(-6, duration: 0.09)
        SpringKeyframe(5, duration: 0.09)
        SpringKeyframe(-2, duration: 0.08)
        SpringKeyframe(8, duration: 0.09)
        SpringKeyframe(-4, duration: 0.09)
        SpringKeyframe(0, duration: 0.2)
      }
    }
  }
}

/// The bar under the finger: a quick light-up.
struct RowPress: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .overlay(.white.opacity(configuration.isPressed ? 0.18 : 0))
      .scaleEffect(configuration.isPressed ? 0.985 : 1)
      .animation(.smooth(duration: 0.15), value: configuration.isPressed)
  }
}

/// Photos and videos while their thumbnail is held, as Stories: the whole screen, one after the other (oldest first),
/// a line for each at the top filling up; a photo at most `photoSeconds`, a video as long as it runs. A tap skips to
/// the next. Each is reported when it shows; at the end it closes. Held, letting go closes it sooner (FriendList).
struct HeldMedia: View {
  let queue: [Arrival]
  let photoSeconds: Double
  let shows: (Arrival) -> Void
  let finished: () -> Void
  @State private var index = 0
  /// When the one showing started, and how long it runs: its line fills from the clock, so skipping never leaves a
  /// line half way or running on.
  @State private var started: Date?
  @State private var seconds = 3.0
  @State private var player: AVPlayer?

  init(
    queue: [Arrival], photoSeconds: Double, shows: @escaping (Arrival) -> Void, finished: @escaping () -> Void
  ) {
    self.queue = queue
    self.photoSeconds = photoSeconds
    self.shows = shows
    self.finished = finished
  }

  // As iOS shows media full screen (Photos, Stories): the picture edge to edge, the status bar and the home indicator
  // out of the way; what is drawn over it (the lines, who it is from) inside the safe area, under the Dynamic Island.
  var body: some View {
    ZStack {
      Color.black.ignoresSafeArea()
      if index < queue.count {
        let current = queue[index]
        Group {
          if current.video != nil, let player {
            PlayerLayer(player: player)
          } else if let photo = current.photo {
            Image(uiImage: photo).resizable().scaledToFit().frame(maxWidth: .infinity, maxHeight: .infinity)
          }
        }
        .ignoresSafeArea()
        VStack(spacing: 10) {
          TimelineView(.animation) { timeline in
            let running = started.map { min(1, timeline.date.timeIntervalSince($0) / seconds) } ?? 0
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
          Text("Yo from \(current.from)")
            .font(.yo(17, relativeTo: .headline))
            .foregroundStyle(.white)
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .glassEffect(.regular, in: .capsule)
        }
        .padding(.horizontal, 4)
        .padding(.top, 4)
        .frame(maxHeight: .infinity, alignment: .top)
      }
    }
    .statusBarHidden()
    .persistentSystemOverlays(.hidden)
    .contentShape(.rect)
    // A tap goes to the next, also with a second finger while the first still holds the thumbnail underneath.
    .onTapGesture { index += 1 }
    .task(id: index) { await play(index) }
    .onDisappear { player?.pause() }
  }

  private func play(_ index: Int) async {
    guard index < queue.count else { return finished() }
    let current = queue[index]
    shows(current)
    var seconds = photoSeconds
    if let video = current.video {
      let player = AVPlayer(url: video)
      self.player = player
      player.play()
      if let length = try? await AVURLAsset(url: video).load(.duration).seconds, length > 0 { seconds = length }
    } else {
      player?.pause()
      player = nil
    }
    self.seconds = seconds
    started = .now
    try? await Task.sleep(for: .seconds(seconds))
    guard !Task.isCancelled else { return }
    self.index += 1
  }
}

/// A video without controls, filling the screen.
struct PlayerLayer: UIViewRepresentable {
  let player: AVPlayer

  func makeUIView(context: Context) -> PlayerView {
    let view = PlayerView()
    view.playerLayer.player = player
    view.playerLayer.videoGravity = .resizeAspect
    return view
  }

  func updateUIView(_ view: PlayerView, context: Context) {
    view.playerLayer.player = player
  }

  final class PlayerView: UIView {
    override class var layerClass: AnyClass { AVPlayerLayer.self }
    var playerLayer: AVPlayerLayer { layer as! AVPlayerLayer }
  }
}

#Preview {
  FriendList()
    .environment(Inbox())
    .environment(Friends())
}
