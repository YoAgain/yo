// The list of friends, as in 2014: one coloured bar per friend from edge to edge, the name large in the middle; a tap
// sends a Yo ("Sent Yo!" on the bar, then it goes on top). The order is the original's: whoever you last sent a Yo to
// or got one from is on top; each friend keeps their colour as they move (the original's went with the place). In the
// mockup a tap sends the Yo to yourself, as if that friend had sent it, when the bar goes on top (showSent). Hold a bar
// to remove the friend; the last bar adds one.
//
// On top, the system's bar: profile (a sheet) on the left, the camera on the right, between them the title "Yo",
// a heading that becomes a glass pill once rows scroll behind it.
//
// A Yo arriving: the sound and a haptic (the title stays "Yo"); the sender's bar goes on top, its words knock (the
// first Yo of a run only) and it says "Yo" at its end, in the name's own type, one "!" for each Yo after the first
// ("Yo!!" for three). Each Yo stands `yoSolidSeconds` from when its bar is on screen; then the last "!" (the oldest)
// goes at once, and the last Yo fades out within `yoSeconds`; then it is gone, as in the original (no unread mark). A
// photo or video waits in the same place as "Yo 📷" or "Yo 🎥": a finger on it plays them (HeldMedia), and what was shown
// is gone (as Snapchat began).
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
  @Environment(AccountStore.self) private var accounts
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
  /// What each bar says for a moment after a send ("Sent Yo", "Not Sent", "Canceled").
  @State private var sent: [Friend: String] = [:]
  /// Sends whose 1.5 seconds are still running, by name; each tap starts them again (a bar under taps stays put).
  @State private var sentRounds: [String: Int] = [:]
  /// How many sends of the same kind in a row a bar counts ("Sent Yo!!" for three).
  @State private var sentCounts: [String: (text: String, count: Int)] = [:]
  /// The mockup's Yos back, by name: they come when the bar goes on top, so it comes in carrying them.
  @State private var replies: [String: [(at: Date, send: @MainActor () async -> Void)]] = [:]
  /// How often each friend went on top: a new identity each time, so the bar leaves its place and comes in on top.
  @State private var lifts: [String: Int] = [:]
  /// Senders waiting to go on top, oldest first: gathered for a moment and moved in one step, so many Yos at once do
  /// not shuffle the list bar by bar.
  @State private var rising: [String] = []
  /// The friends whose bars are on screen now.
  @State private var onScreen: Set<String> = []
  /// Counts sends, for the haptic.
  @State private var sends = 0
  /// Photos and videos on their way out, per friend, each with when it started; any number at once (a tap on the bar
  /// sends another, the ring at its left undoes them).
  @State private var outgoing: [Friend: [Outgoing]] = [:]
  /// Each photo or video of a stack being made ready (taken, or the video ended), by its id.
  @State private var preparing: [UUID: Task<Prepared?, Never>] = [:]
  /// A stack's 1.5 seconds, by name: each new photo starts them again; when they are over, the whole stack goes.
  @State private var stackRounds: [String: Int] = [:]

  struct Outgoing: Equatable {
    let id = UUID()
    let started = Date.now
    /// What the bar says meanwhile ("Sent 📷!"), the undo ring right after it.
    let text: String
  }

  // Camera mode
  @State private var cameraOn = false
  @State private var camera = Camera()
  @State private var recordingFor: Friend?
  @State private var libraryItem: PhotosPickerItem?
  /// A photo from the library, waiting to be sent: it stands in for the live picture.
  @State private var picked: Data?

  // Arriving
  /// Counts Yos arriving, for the haptic.
  @State private var cheers = 0
  /// Taps on the title.
  @State private var sayYo = 0
  /// The plain Yos on bars now, by sender.
  @State private var landed: [String: Landing] = [:]
  /// Arrivals already shown.
  @State private var launched = 0
  /// The photos and videos playing while a thumbnail is held, and those already shown.
  @State private var viewing: [Arrival]?
  @State private var shown: [Arrival] = []

  /// A friend with how often they went on top: their identity in the list.
  struct Lifted: Identifiable {
    let friend: Friend
    let lifts: Int
    var id: String { "\(friend.name)#\(lifts)" }
  }

  /// The Yos on a bar now, a stack: each new one adds a "!" and starts the stack's time again. Only when none came for
  /// `yoSolidSeconds` (counted while the bar is on screen) do the "!"s go, one quickly after the other, and the last
  /// Yo fades out slowly.
  struct Landing: Equatable {
    /// The first Yo of this run: it knocks; the ones after only add a "!".
    var first: String
    var count: Int
    /// When the stack's time last started; nil while its bar has not been on screen.
    var seen: Date?
    /// The last one is fading out since then.
    var fading: Date?
  }

  var body: some View {
    reacting(
      navigation
        // The list is always dark (white on purple); the profile follows the iPhone's own appearance.
        .sheet(isPresented: $settings) { SettingsView().preferredColorScheme(SystemAppearance.scheme) }
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
          // Going on top is a delete and an insert (as Signal's chat list does): the gap closes below, the bar comes in
          // from the top; it does not fly across the list.
          ForEach(friends.all.map { Lifted(friend: $0, lifts: lifts[$0.name] ?? 0) }) { lifted in
            row(lifted.friend)
              .transition(.asymmetric(insertion: .move(edge: .top).combined(with: .opacity), removal: .opacity))
          }
          addRow
        }
        .scrollTargetLayout()
      }
      .scrollBounceBehavior(.basedOnSize)
      .scrollDismissesKeyboard(.interactively)
      .scrollPosition($position)
      .onScrollTargetVisibilityChange(idType: String.self, threshold: 0.6) { ids in
        onScreen = Set(ids.map { String($0[..<($0.lastIndex(of: "#") ?? $0.endIndex)]) })
      }
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
          // The system's badge while the name is not secured: one thing to do (as Settings badges "Finish Setting Up").
          Button("Profile", systemImage: "person.fill") { settings = true }
            .badge(accounts.toDo)
        }
        // The camera; it turns red (Alizarin, the original's round button) while it is on.
        ToolbarItem(placement: .topBarTrailing) {
          if cameraOn {
            Button("Turn Off Camera", systemImage: "camera.fill") { cameraOn = false }
              .buttonStyle(.glassProminent)
              .tint(.alizarin)
          } else {
            Button("Camera", systemImage: "camera") { cameraOn = true }
          }
        }
      }
    }
  }

  /// "Yo", nothing more (a Yo shows on its bar); in camera mode what to do under it. A heading at the top of the list;
  /// once rows pass behind it, a glass pill as the buttons beside it.
  private var title: some View {
    VStack(spacing: 0) {
      Text("Yo")
      if cameraOn {
        Text(picked == nil ? "Tap a name for a photo. Hold for a video." : "Tap a name to send this photo.")
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
    .contentShape(.capsule)
    .onTapGesture {
      inbox.playYo()  // a tap on "Yo" says it
      sayYo += 1
    }
    .sensoryFeedback(.impact(weight: .light), trigger: sayYo)
    .accessibilityAddTraits(.isHeader)
    .accessibilityAction(named: "Say Yo") { inbox.playYo() }
  }

  // MARK: Bars

  @ViewBuilder private func row(_ friend: Friend) -> some View {
    let waiting = inbox.media[friend.name] ?? []
    let bar = RowButton(
      rowTitle(friend),
      count: rowCount(friend),
      color: .row(friend.color).opacity(cameraOn ? 0.45 : 1),  // see-through over the camera
      recording: recordingFor == friend,
      outgoing: outgoing[friend]?.last?.started,
      stop: { cancelSending(friend) },
      media: waiting,
      yo: landed[friend.name],
      hint: cameraOn ? "Takes and sends a photo. Use Record Video to record a video." : nil,
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
          Button("Remove \(friend.name)", systemImage: "trash", role: .destructive) {
            withAnimation(.snappy) { friends.remove(friend) }
          }
        }
    }
  }

  /// The bar's words: what is happening, else the name.
  private func rowTitle(_ friend: Friend) -> String {
    if recordingFor == friend { return "● Recording" }
    if let last = outgoing[friend]?.last { return last.text }
    if let text = sent[friend] { return text }
    return friend.name
  }

  /// How many the bar's words count ("Sent Yo!!" for three): photos on their way, or sends in a row.
  private func rowCount(_ friend: Friend) -> Int {
    if recordingFor == friend { return 1 }
    if let going = outgoing[friend], !going.isEmpty { return going.count }
    if sent[friend] != nil { return sentCounts[friend.name]?.count ?? 1 }
    return 1
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
        .accessibilityLabel("Add Friend")
    }
  }

  private func add() {
    withAnimation(.snappy) { friends.add(newName) }
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
      .accessibilityLabel("Switch Camera")
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
    let delay = Duration.zero  // the Yo back comes with the bar going on top
    sends += 1
    let library = picked
    picked = nil
    libraryItem = nil
    guard cameraOn || library != nil else {
      showSent(to: friend, "Sent Yo", thenOnTop: true) { await inbox.fakeYo(from: friend.name, after: delay) }
      return
    }
    run(friend, "Sent 📷") {
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
    let delay = Duration.zero
    if holding {
      guard recordingFor == nil else { return }  // one video at a time: a second finger records nothing
      recordingFor = friend
      camera.startVideo()
    } else if recordingFor == friend {
      recordingFor = nil
      sends += 1
      run(friend, "Sent 🎥") {
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

  /// A photo or video on its way out joins the friend's stack: every one in it waits until `sendDelay` has passed
  /// since the newest (each new one starts the time again, the ring after the words showing it); then the stack goes,
  /// in order. Meanwhile `prepare` takes the photo or ends the video. A tap on the ring keeps the whole stack here.
  private func run(_ friend: Friend, _ text: String, prepare: @escaping @MainActor () async -> Prepared?) {
    let one = Outgoing(text: text)
    outgoing[friend, default: []].append(one)
    preparing[one.id] = Task { await prepare() }
    let name = friend.name
    let round = (stackRounds[name] ?? 0) + 1
    stackRounds[name] = round
    Task {
      try? await Task.sleep(for: .seconds(Self.sendDelay))
      guard stackRounds[name] == round, let stack = outgoing[friend] else { return }  // another came, or undone
      stackRounds[name] = nil
      outgoing[friend] = nil
      var went = 0
      for one in stack {
        guard let prepared = await preparing.removeValue(forKey: one.id)?.value else { continue }
        went += 1
        showSent(to: friend, one.text, thenOnTop: true) { await prepared.send() }
      }
      if went == 0 { showSent(to: friend, "Not Sent") }
    }
  }

  /// A photo or video ready to go: what sends it, and what throws it away when the send is undone.
  struct Prepared {
    let send: @MainActor () async -> Void
    var discard: @MainActor () -> Void = {}
  }

  /// The ring tapped: the whole stack stays here, and nothing of it stays behind (a video's file is deleted).
  private func cancelSending(_ friend: Friend) {
    for one in outgoing[friend] ?? [] {
      if let task = preparing.removeValue(forKey: one.id) {
        Task { await task.value?.discard() }
      }
    }
    stackRounds[friend.name] = nil
    outgoing[friend] = nil
    showSent(to: friend, "Canceled")
  }

  /// What the bar says after a send, for 1.5 seconds as in the original; then a bar that sent goes on top. Another tap
  /// within them starts them again, so the bar never slides away under a finger tapping it again and again. Sends of
  /// the same kind in a row are counted, one "!" for each after the first ("Sent Yo", "Sent Yo!", …). `reply` is the
  /// mockup's Yo back, held until then.
  private func showSent(
    to friend: Friend, _ text: String, thenOnTop: Bool = false, reply: (@MainActor () async -> Void)? = nil
  ) {
    let name = friend.name
    let count = sentCounts[name].map { $0.text == text ? $0.count + 1 : 1 } ?? 1
    withAnimation(.snappy(duration: 0.2)) {  // a "!" comes in, or the number turns
      sentCounts[name] = (text, count)
      sent[friend] = text
    }
    if let reply { replies[name, default: []].append((.now, reply)) }
    let round = (sentRounds[name] ?? 0) + 1
    sentRounds[name] = round
    Task {
      try? await Task.sleep(for: .seconds(1.5))
      guard sentRounds[name] == round else { return }  // tapped again meanwhile
      sentRounds[name] = nil
      let pending = replies.removeValue(forKey: name) ?? []
      withAnimation(.snappy) {
        sent[friend] = nil
        sentCounts[name] = nil
      }
      if thenOnTop { rise(name) }
      // In the rhythm they were tapped: tapped fast, they come fast and their sounds cut each other off.
      let start = Date.now
      for reply in pending {
        let wait = reply.at.timeIntervalSince(pending[0].at) - Date.now.timeIntervalSince(start)
        if wait > 0 { try? await Task.sleep(for: .seconds(wait)) }
        await reply.send()
      }
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
      .onChange(of: onScreen) { startClocks() }
      .onChange(of: libraryItem) { _, item in load(item) }
      .onChange(of: cameraOn) { _, on in cameraSwitched(on) }
      .onDisappear { cameraOn = false }
  }

  private func feedback(_ content: some View) -> some View {
    animations(haptics(content))
  }

  private func haptics(_ content: some View) -> some View {
    content
      .sensoryFeedback(.impact(weight: .heavy), trigger: cheers)
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
      rise(name)  // on top as soon as no bar is being tapped
      withAnimation(.snappy(duration: 0.2)) {  // the words make room without a bounce
        if var landing = landed[name], landing.fading == nil {
          landing.count += 1  // one more "!"
          if landing.seen != nil {
            landing.seen = .now  // on screen: the stack's time starts again
            settle(name, landing.seen!)
          }
          landed[name] = landing
        } else {
          landed[name] = Landing(first: arrival.id, count: 1)  // a new run (also over one fading out): it knocks
        }
      }
      try? await Task.sleep(for: .milliseconds(150))  // the list has laid out where the bar is now
      startClocks()
    }
  }

  /// Stacks whose bars are on screen now start their time; one out of sight waits until it is seen.
  private func startClocks() {
    for (name, landing) in landed where onScreen.contains(name) && landing.seen == nil {
      let now = Date.now
      landed[name]?.seen = now
      settle(name, now)
    }
  }

  /// A stack's time, started at `since`: if no Yo came meanwhile, its "!"s go one by one (quickly; faster the more
  /// there are), then the last Yo fades out (`yoSeconds`). A new Yo starts it again.
  private func settle(_ name: String, _ since: Date) {
    Task {
      try? await Task.sleep(for: .seconds(Self.yoSolidSeconds))
      let step = min(0.2, 2.0 / Double(max(1, landed[name]?.count ?? 1)))
      while let landing = landed[name], landing.seen == since, landing.count > 1 {
        withAnimation(.easeOut(duration: 0.15)) { landed[name]?.count -= 1 }
        try? await Task.sleep(for: .seconds(step))
      }
      guard landed[name]?.seen == since else { return }  // another came
      let fading = Date.now
      landed[name]?.fading = fading
      try? await Task.sleep(for: .seconds(Self.yoSeconds - Self.yoSolidSeconds))
      if landed[name]?.fading == fading { _ = landed.removeValue(forKey: name) }
    }
  }

  /// A friend goes on top with the others coming within 0.3 seconds, in one step (the newest ends on top). While you
  /// are sending (taps or a stack of photos still in their time) the list does not move at all, so no bar slides away
  /// under your finger; it moves once you are done.
  private func rise(_ name: String) {
    rising.removeAll { $0 == name }
    rising.append(name)
    guard rising.count == 1 else { return }  // a step is already on its way
    Task {
      try? await Task.sleep(for: .seconds(0.3))
      while sending { try? await Task.sleep(for: .milliseconds(200)) }
      let names = rising
      rising = []
      toTop(names)
    }
  }

  /// A send still in its time: taps on a bar, or photos and videos on their way.
  private var sending: Bool {
    !sentRounds.isEmpty || !stackRounds.isEmpty || !outgoing.isEmpty
  }

  private func toTop(_ name: String) { toTop([name]) }

  /// Friends go on top, the last of `names` uppermost. At the top of the list the list stays at its very start, so the
  /// bar is in sight below the title; scrolled down, the view stays where it is (the title's "!" tells, and the Yo
  /// waits until it is seen).
  private func toTop(_ names: [String]) {
    let order = friends.all.map(\.name)
    let moving = names.filter { name in
      // Already in place: the top bars are these names in this order, newest first.
      order.firstIndex(of: name) != names.count - 1 - names.lastIndex(of: name)!
    }
    guard !moving.isEmpty else { return }
    withAnimation(.snappy(duration: 0.35)) {
      for name in names {
        if moving.contains(name) { lifts[name, default: 0] += 1 }
        friends.promote(name)
      }
      if !scrolled { position.scrollTo(edge: .top) }
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
  /// How many the words count: a "!" for each after the first (Bangs).
  var count = 1
  let color: Color
  var recording = false
  /// When the newest photo or video on its way out started; its ring closes in `sendSeconds`, a tap on it undoes.
  var outgoing: Date?
  var sendSeconds = FriendList.sendDelay
  var stop: () -> Void = {}
  var media: [Arrival] = []
  /// A Yo on this bar now: its words knock and it says "Yo" at its end ("Yo!!" for three), fading out within
  /// `yoSeconds`.
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
    _ title: String, count: Int = 1, color: Color, recording: Bool = false, outgoing: Date? = nil,
    stop: @escaping () -> Void = {},
    media: [Arrival] = [], yo: FriendList.Landing? = nil, hint: String? = nil, hold: ((Bool) -> Void)? = nil,
    view: @escaping (ThumbnailTouch) -> Void = { _ in }, action: @escaping () -> Void
  ) {
    self.title = title
    self.count = count
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
    // What takes its own touches (the undo ring, the photos) lies over the bar, in the same line (Buttons in a Button
    // do not work).
    .overlay { line(touchable: true) }
    .animation(.snappy, value: outgoing)
    .animation(.spring(duration: 0.4, bounce: 0.35), value: yo)
    .animation(.smooth, value: media)
  }

  /// A bar of the friend's colour from edge to edge, the name large in the middle (the original's), red while
  /// recording.
  private var label: some View {
    line(touchable: false)
      .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
      .background(recording ? Color.alizarin.opacity(0.85) : color)
      .contentShape(.rect)
  }

  /// The bar's line, sending and receiving alike (BarLine): the words; their "!"s and the undo ring after them; a Yo
  /// that came at the end. Laid out twice the same: in the bar the words and their "!"s, over it what takes its own
  /// touches (the ring, the Yo at the end), so each sits exactly where the other leaves it room.
  private func line(touchable: Bool) -> some View {
    BarLine {
      Text(title)
        .lineLimit(1)
        .contentTransition(.identity)  // other words come at once, never two drawn over each other
        .modifier(Knock(trigger: yo?.first))
        .opacity(touchable ? 0 : 1)
      HStack(spacing: 0) {
        Bangs(count: count).opacity(touchable ? 0 : 1)
        if let outgoing {
          Group {
            if touchable { undoRing(since: outgoing) } else { Color.clear }
          }
          .frame(width: Self.ring, height: Self.ring)
          .padding(.leading, 10)
        }
      }
      ZStack { mark(touchable: touchable) }  // always a part, so a missing "!" never takes its place
        .modifier(Knock(trigger: yo?.first))
        .opacity(touchable ? 1 : 0)
    }
    .font(.yo(Metrics.rowFont))
    .foregroundStyle(.white)
    .animation(nil, value: title)  // other words stand in their place at once; only "!"s, numbers and room animate
  }

  private static let ring = 30.0

  /// Something on its way out: a ring after the words closing like an upload; a tap on it undoes the send.
  private func undoRing(since start: Date) -> some View {
    Button(action: stop) {
      TimelineView(.animation) { timeline in
        let done = min(1, timeline.date.timeIntervalSince(start) / sendSeconds)
        ZStack {
          Circle().stroke(.white.opacity(0.35), lineWidth: 2.5)
          Circle()
            .trim(from: 0, to: 1 - done)
            .stroke(.white, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
            .rotationEffect(.degrees(-90))
          Image(systemName: "arrow.uturn.backward").font(.system(size: 11, weight: .heavy))
        }
        .foregroundStyle(.white)
        .frame(width: Self.ring, height: Self.ring)
      }
    }
    .buttonStyle(.plain)
    .contentShape(.circle.inset(by: -12))
    .accessibilityLabel("Undo Send")
    .transition(.scale.combined(with: .opacity))
  }

  /// At the bar's end, in the name's own type: "Yo 📷" or "Yo 🎥" for photos and videos not yet seen (a finger on it
  /// plays them at once), or "Yo" for one that just came; their "!"s after either.
  @ViewBuilder private func mark(touchable: Bool) -> some View {
    if let first = media.first {
      HStack(spacing: 0) {
        Text(first.video != nil ? "Yo 🎥" : "Yo 📷")
        Bangs(count: media.count)
      }
      .lineLimit(1)
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
      .allowsHitTesting(touchable)  // the copy in the bar under it is only for the layout
      .accessibilityHidden(!touchable)
      .transition(.scale.combined(with: .opacity))
    } else if let yo {
      // "Yo" and a "!" for each Yo after the first: a "!" goes quickly when its Yo is over, the last Yo fades slowly.
      TimelineView(.animation(paused: yo.fading == nil)) { timeline in
        let fade = yo.fading.map { timeline.date.timeIntervalSince($0) / (yoSeconds - Self.solid) } ?? 0
        HStack(spacing: 0) {
          Text("Yo")
          Bangs(count: yo.count)
        }
        .lineLimit(1)
        .opacity(max(0, 1 - fade))
      }
      .accessibilityLabel(yo.count == 1 ? "Yo" : "\(yo.count) Yos")
      .transition(.scale(scale: 0.3).combined(with: .opacity))
    }
  }
}

/// How many, the way Yo says it: one "!" for each after the first ("Yo", "Yo!", "Yo!!"), each its own, so one more
/// comes in on its own and the ones there stay (as text typed into a field, not the whole word drawn again). At most
/// five; beyond, the "!"s stay and how many more comes after them ("Yo!!!!! +3"), turning as a wheel, its digits all
/// as wide (nothing shifts).
struct Bangs: View {
  let count: Int

  static let most = 5

  var body: some View {
    let wanted = max(0, count - 1)
    let bangs = min(wanted, Self.most)
    HStack(spacing: 0) {
      ForEach(0..<bangs, id: \.self) { _ in
        Text("!").transition(.opacity.animation(.easeOut(duration: 0.15)))
      }
      if wanted > bangs {
        Text(" +\(wanted - bangs)")
          .monospacedDigit()
          .contentTransition(.numericText(value: Double(wanted - bangs)))
          .transition(.opacity)
      }
    }
  }
}

/// A bar's line, for sending and receiving alike: the words in the middle, what belongs to them (their "!"s, the undo
/// ring) right after them, a Yo that came at the bar's end. The words stay in the middle until they would run into the
/// end (or the edge): then they move left as far as needed, never past the margin; a name too long even then is cut
/// with "…". Nothing shrinks, nothing overlaps.
struct BarLine: Layout {
  static let margin = 20.0
  static let endPadding = 30.0
  static let gap = 14.0

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    let height = subviews.map { $0.sizeThatFits(.unspecified).height }.max() ?? 0
    return CGSize(width: proposal.width ?? 393, height: height)
  }

  func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
    // An empty part (no "!"s, no Yo at the end) is no subview at all: take what is there.
    guard let words = subviews.first else { return }
    let after = subviews.count > 1 ? subviews[1] : nil
    let end = subviews.count > 2 ? subviews[2] : nil
    let afterSize = after?.sizeThatFits(.unspecified) ?? .zero
    let endSize = end?.sizeThatFits(.unspecified) ?? .zero
    let endX = bounds.maxX - Self.endPadding - endSize.width
    let limit = endSize.width > 0 ? endX - Self.gap : bounds.maxX - Self.margin
    let room = max(0, limit - bounds.minX - Self.margin - afterSize.width)
    let wordsWidth = min(words.sizeThatFits(.unspecified).width, room)
    var x = bounds.midX - wordsWidth / 2
    x -= max(0, x + wordsWidth + afterSize.width - limit)
    x = max(bounds.minX + Self.margin, x)
    words.place(
      at: CGPoint(x: x, y: bounds.midY), anchor: .leading, proposal: ProposedViewSize(width: wordsWidth, height: nil))
    after?.place(at: CGPoint(x: x + wordsWidth, y: bounds.midY), anchor: .leading, proposal: .unspecified)
    end?.place(at: CGPoint(x: endX, y: bounds.midY), anchor: .leading, proposal: .unspecified)
  }
}

/// A Yo knocks: the words on a bar nudge left and right once, as a poke; the bar itself stays (moving it would show
/// what is behind it).
struct Knock: ViewModifier {
  let trigger: String?
  @State private var knocks = 0

  func body(content: Content) -> some View {
    content.keyframeAnimator(initialValue: 0.0, trigger: knocks) { words, x in
      words.offset(x: x)
    } keyframes: { _ in
      KeyframeTrack {
        SpringKeyframe(-9, duration: 0.09)
        SpringKeyframe(9, duration: 0.12)
        SpringKeyframe(0, duration: 0.18)
      }
    }
    .onChange(of: trigger) { _, new in
      if new != nil { knocks += 1 }
    }
    // A bar that comes in on top already carrying its Yo knocks as it arrives.
    .onAppear {
      if trigger != nil { knocks += 1 }
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
