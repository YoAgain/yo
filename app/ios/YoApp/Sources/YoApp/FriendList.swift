// The list of friends, as in 2014: one coloured bar per friend from edge to edge, the name large in the middle; a tap
// sends a Yo ("Sent Yo!" on the bar, then it goes on top). The order is the original's: whoever you last sent a Yo to
// or got one from is on top; each friend keeps their colour as they move (the original's went with the place). Hold a
// bar to report, block or remove the friend, or swipe it left for Block and Remove; the last bar adds one.
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
//
// An invite link (justyo.co/NAME, Invite.swift) adds its name as if typed in Add Friend; one already on the list says
// so on its bar. The first Yo ever sent from this phone says what it does, as the original's did (2015).
import AVFoundation
import PhotosUI
import SwiftUI
import UserNotifications
import YoCore

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
  @Environment(MediaOutbox.self) private var outbox

  // The list
  @State private var adding = false
  /// "+" tapped: it opens into Add Friend, Add Group and Invite.
  @State private var addMenu = false
  @State private var newGroup = false
  @State private var newName = ""
  /// What the empty field says for a moment instead of "Name": "yogpt…" while the hub is asked who it is, "No Such
  /// User" when nobody has that name (it is not added, as in the original), "That’s You" for one's own (a Yo to
  /// oneself has no chat).
  @State private var hint: String?
  /// Names submitted (and hints shown): only the newest one's answer and hint touch the field.
  @State private var submitted = 0
  @FocusState private var typing: Bool
  @State private var settings = false
  @State private var position = ScrollPosition(idType: String.self)
  /// Rows are behind the title: it wears its glass pill.
  @State private var scrolled = false
  /// The bar swiped open (its Block and Remove showing), by `Friend.key`: one at a time.
  @State private var swiped: UUID?

  // Sending
  /// What the bars say while sending and what is on its way out: in an object of its own, read and written by the
  /// tasks that run the 1.5 seconds. Kept in plain @State, those tasks read the values from before a tap once a
  /// photo had been picked from the library, and a stack of photos never went (its undo ring stayed).
  @State private var bars = Bars()

  // Blocking and reporting
  /// The friend the Block dialog asks about, and whether it follows a report.
  @State private var blocking: (friend: Friend, reported: Bool)?
  /// What went wrong reporting or blocking: the alert's title and message.
  @State private var problem: (title: String, message: String)?
  /// The friends whose bars are on screen now, by `Friend.key` (a new spelling leaves them on screen).
  @State private var onScreen: Set<String> = []
  /// The first Yo sent: its alert, by whether notifications are allowed.
  @State private var firstYo: Notifications?
  enum Notifications { case allowed, notAsked, denied }
  private static let firstYoKey = "firstYo.shown"

  /// The bars' sending: what each says after a send, and the photos and videos on their way out.
  @MainActor @Observable final class Bars {
    /// One bar's: read only by that bar (`LiveBar`), so a tap redraws its bar alone, not the whole list.
    @MainActor @Observable final class Bar {
      /// What the bar says for a moment after a send ("Sent Yo", "Not Sent", "Canceled"), and how many sends of that
      /// kind in a row ("Sent Yo!!" for three).
      var sent: (text: String, count: Int)?
      /// Photos and videos on their way out, each with when it started; any number at once (a tap on the bar sends
      /// another, the ring at its left undoes them).
      var outgoing: [Outgoing] = []
    }

    /// Each friend's bar state, by `Friend.key`, made when first asked for.
    @ObservationIgnored private var each: [UUID: Bar] = [:]
    func of(_ friend: Friend) -> Bar {
      if let bar = each[friend.key] { return bar }
      let bar = Bar()
      each[friend.key] = bar
      return bar
    }
    /// Photos or videos on their way out on any bar.
    var outgoing: Bool { each.values.contains { !$0.outgoing.isEmpty } }

    /// Sends whose 1.5 seconds are still running; each tap starts them again (a bar under taps stays put).
    @ObservationIgnored var sentRounds: [UUID: Int] = [:]
    /// Photos and videos whose time is over, by friend: they go when the bar's words are over, in the rhythm they
    /// were taken.
    @ObservationIgnored var queued: [UUID: [(at: Date, send: @MainActor () async -> Void)]] = [:]
    /// Each photo or video of a stack being made ready (taken, or the video ended), by its id.
    @ObservationIgnored var preparing: [UUID: Task<Prepared?, Never>] = [:]
    /// A stack's 1.5 seconds: each new photo starts them again; when they are over, the whole stack goes.
    @ObservationIgnored var stackRounds: [UUID: Int] = [:]
    /// Per friend, the last stack letting its Yos go: the next waits for it, so they go in the order taken.
    @ObservationIgnored var releasing: [UUID: Task<Void, Never>] = [:]
    /// How often each friend went on top (by `Friend.key`): a new identity each time, so the bar leaves its place and
    /// comes in on top. Only here: a new spelling or user id never changes a bar's identity.
    var lifts: [UUID: Int] = [:]
    /// Senders waiting to go on top, oldest first: gathered for a moment and moved in one step, so many Yos at once
    /// do not shuffle the list bar by bar.
    @ObservationIgnored var rising: [String] = []
  }

  struct Outgoing: Equatable {
    let id = UUID()
    let started = Date.now
    /// What the bar says meanwhile ("Sent 📷!"), the undo ring at the bar's left.
    let text: String
  }

  // Camera mode
  @State private var cameraOn = false
  @State private var camera = Camera()
  /// The camera's zooms (CameraZoom) and the one chosen.
  @State private var zooms: [Double] = []
  @State private var zoom = 1.0
  @State private var recordingFor: Friend?
  @State private var libraryItem: PhotosPickerItem?
  /// A photo from the library, waiting to be sent: it stands in for the live picture.
  @State private var picked: Picked?

  /// A photo from the library: what is sent, and its pictures for the screen, made once (a 48-megapixel photo decoded
  /// at each redraw kept the list from answering).
  struct Picked: Equatable {
    let data: Data
    let image: UIImage
    let thumbnail: UIImage?

    /// Off the main actor: decoded and scaled down for the screen and the library button.
    static func make(_ data: Data) async -> Picked? {
      await Task.detached(priority: .userInitiated) {
        guard let full = UIImage(data: data) else { return nil }
        let screen = CGSize(width: 1320, height: 2868)  // the largest iPhone's screen in pixels
        let image = full.size.width * full.size.height > screen.width * screen.height
          ? full.preparingThumbnail(of: screen) ?? full : full
        return Picked(data: data, image: image, thumbnail: full.preparingThumbnail(of: CGSize(width: 120, height: 120)))
      }.value
    }
  }

  // Arriving
  /// Counts Yos arriving, for the haptic.
  @State private var cheers = 0
  /// The plain Yos on bars now, by sender.
  @State private var landed: [String: Landing] = [:]
  /// The stacks' running clocks, one per sender (a new Yo replaces it): not one waiting task per Yo.
  @State private var clocks = Clocks()
  /// The first friend whose new key is still to be said (Friends.keyAlerts).
  private var keyChanged: Friend? {
    friends.keyAlerts.lazy.compactMap { key in friends.all.first { $0.key == key } }.first
  }

  /// The photos and videos playing while a thumbnail is held, and those already shown.
  @State private var viewing: [Arrival]?
  @State private var shown: [Arrival] = []

  /// A friend with how often they went on top: their identity in the list.
  struct Lifted: Identifiable {
    let friend: Friend
    let lifts: Int
    var id: String { "\(friend.key)#\(lifts)" }
  }

  /// Tasks the list keeps for itself, not drawn: replacing one is no change to the view.
  @MainActor final class Clocks {
    /// Each stack's clock, by sender: a new Yo cancels it and starts another.
    var settling: [String: Task<Void, Never>] = [:]
    /// The stacks' clocks start once the list has laid out where the bars are now (one wait for a whole batch).
    var starting = false
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
        .sheet(isPresented: $newGroup) { NewGroupSheet().preferredColorScheme(SystemAppearance.scheme) }
        .confirmationDialog(
          blocking?.reported == true ? "Thanks for Reporting" : "Block \(blocking?.friend.name ?? "")?",
          isPresented: Binding(get: { blocking != nil }, set: { if !$0 { blocking = nil } }),
          titleVisibility: .visible, presenting: blocking
        ) { blocking in
          Button("Block \(blocking.friend.name)", role: .destructive) { block(blocking.friend) }
          Button(blocking.reported ? "Not Now" : "Cancel", role: .cancel) {}
        } message: { blocking in
          Text(
            blocking.reported
              ? "Block \(blocking.friend.name) too? You won’t get their Yos."
              : "You won’t get their Yos. Unblock them in your profile.")
        }
        .alert(
          problem?.title ?? "", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })
        ) {
          Button("OK", role: .cancel) {}
        } message: {
          Text(problem?.message ?? "")
        }
        .alert(
          "First Yo Sent!", isPresented: Binding(get: { firstYo != nil }, set: { if !$0 { firstYo = nil } }),
          presenting: firstYo
        ) { notifications in
          if notifications == .allowed {
            Button("Awesome", role: .cancel) {}
          } else {
            Button("Turn On Notifications") { Task { await turnOnNotifications(notifications) } }
            Button("Not Now", role: .cancel) {}
          }
        } message: { notifications in
          let me = accounts.account?.username ?? "you"
          Text(
            "They’ll get “Yo from \(me)” with a sound that says Yo."
              + (notifications == .allowed ? "" : " To get a Yo back, turn on notifications."))
        }
        // A contact's account key changed (spec 3.2): said once; Contacts shows it until their details are opened.
        .alert(
          "\(keyChanged?.name ?? "") Has a New Key",
          isPresented: Binding(
            get: { keyChanged != nil }, set: { if !$0, let friend = keyChanged { friends.keyAlertShown(friend) } }),
          presenting: keyChanged
        ) { _ in
          Button("OK", role: .cancel) {}
        } message: { friend in
          Text(
            "\(friend.name) may have a new iPhone. If you didn’t expect this, compare safety numbers in Profile > "
              + "Contacts.")
        }
        .overlay {
          if let viewing {
            HeldMedia(queue: viewing, photoSeconds: Self.photoSeconds) { arrival in
              guard !shown.contains(where: { $0.id == arrival.id }) else { return }  // stepped back to: once
              shown.append(arrival)
              inbox.viewed(arrival)
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
          ForEach(friends.shown.map { Lifted(friend: $0, lifts: bars.lifts[$0.key] ?? 0) }) { lifted in
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
      .onScrollPhaseChange { _, phase in
        if phase != .idle && swiped != nil { withAnimation(.snappy) { swiped = nil } }  // scrolling closes it
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
          // Your picture opens your profile and settings in a sheet, as Apple's own apps do. No badge: the iCloud
          // Keychain keeps the account; securing it further is offered in the profile itself.
          Button("Profile", systemImage: "person.fill") { settings = true }
        }
        // The camera mode: the live picture behind the list, a tap on a name sends a photo, holding it a video.
        ToolbarItem(placement: .topBarTrailing) {
          Button(cameraOn ? "Close Camera" : "Camera", systemImage: cameraOn ? "xmark" : "camera.fill") {
            cameraOn.toggle()
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
      Haptic.tap(.light)
      inbox.playYo()  // a tap on "Yo" says it
    }
    .accessibilityAddTraits(.isHeader)
    .accessibilityAction(named: "Say Yo") { inbox.playYo() }
  }

  // MARK: Bars

  @ViewBuilder private func row(_ friend: Friend) -> some View {
    let waiting = inbox.media(from: friend.name) ?? []
    let bar = LiveBar(
      friend: friend,
      bar: bars.of(friend),
      color: .row(friend.color).opacity(cameraOn ? 0.45 : 1),  // see-through over the camera
      recording: recordingFor == friend,
      stop: { cancelSending(friend) },
      media: waiting,
      yo: landed[friend.name],
      hint: cameraOn ? "Takes and sends a photo. Use Record Video to record a video." : nil,
      hold: cameraOn && picked == nil ? { hold(friend, $0) } : nil,
      view: { see(friend, $0) }
    ) {
      send(to: friend)
    }
    // Swiped left (not in camera mode, where the bars take photos): Block and Remove underneath, as in Mail.
    SwipeActions(
      open: Binding(
        get: { swiped == friend.key },
        set: { open in swiped = open ? friend.key : swiped == friend.key ? nil : swiped }),
      enabled: !cameraOn,
      began: { if swiped != friend.key { withAnimation(.snappy) { swiped = nil } } },  // another one closes
      block: { blocking = (friend, false) },
      remove: { remove(friend) }
    ) {
      if cameraOn || !waiting.isEmpty {
        bar  // holding records a video, or shows the photos: no menu then
      } else {
        bar
          .contextMenu {
            Menu("Report \(friend.name)", systemImage: "exclamationmark.bubble") {
              Button("Spam") { report(friend, .spam) }
              Button("Abuse") { report(friend, .abuse) }
              Button("Something Else") { report(friend, .other) }
            }
            Button("Block \(friend.name)", systemImage: "hand.raised") { blocking = (friend, false) }
            Button("Remove \(friend.name)", systemImage: "trash", role: .destructive) { remove(friend) }
          }
      }
    }
  }

  private func remove(_ friend: Friend) {
    withAnimation(.snappy) {
      swiped = nil
      friends.remove(friend)
    }
  }

  /// Reports a friend to Yo, then offers to block them.
  private func report(_ friend: Friend, _ reason: ReportReason) {
    Task {
      do {
        try await accounts.report(friend.user ?? friend.name, reason: reason)
        blocking = (friend, true)
      } catch {
        problem = ("Unable to Report", error.localizedDescription)
      }
    }
  }

  /// Blocks a friend: off the list, their Yos no longer shown (spec 4.8).
  private func block(_ friend: Friend) {
    Task {
      do {
        try await accounts.setBlocked(friend.user ?? friend.name, true)
        withAnimation(.snappy) { friends.setBlocked(friend, true) }
      } catch {
        problem = ("Unable to Block", error.localizedDescription)
      }
    }
  }

  /// The last bar, as in the original: "+". Tapped, it opens into three: Add Friend (the name typed right there),
  /// Add Group and Invite; untouched for a while, it closes again.
  @ViewBuilder private var addRow: some View {
    let color = Color.row(friends.nextColor).opacity(cameraOn ? 0.45 : 1)  // the colour the new friend will have
    if addMenu && !adding {
      RowButton("Add Friend", color: color) {
        addMenu = false
        adding = true
      }
      RowButton("Add Group", color: Color.row(friends.nextColor + 1).opacity(cameraOn ? 0.45 : 1)) {
        addMenu = false
        newGroup = true
      }
      ShareLink(
        item: accounts.account.map { Invite.link(for: $0.username) } ?? URL(string: "https://justyo.co")!,
        message: Text("Yo! Add me on Yo:"),
        // What the share sheet shows at its top: whose link, and the app's icon (not a bare "justyo.co").
        preview: SharePreview(
          "Add \(accounts.account?.username ?? "Me") on Yo",
          image: Image(uiImage: UIImage(named: "IconStandard") ?? UIImage()))
      ) {
        Text("Invite")
          .font(.yo(Metrics.rowFont))
          .foregroundStyle(.white)
          .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
          .background(Color.row(friends.nextColor + 2).opacity(cameraOn ? 0.45 : 1))
      }
      .buttonStyle(.plain)
      .simultaneousGesture(TapGesture().onEnded { addMenu = false })
      .task {
        try? await Task.sleep(for: .seconds(8))
        withAnimation(.snappy) { addMenu = false }
      }
    } else if adding {
      TextField("", text: $newName, prompt: Text(hint ?? "Name").foregroundStyle(.white.opacity(0.6)))
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
        // Only what a name may hold, as the sign-up field keeps it.
        .onChange(of: newName) { _, typed in
          let clean = Username.clean(typed)
          if clean != typed { newName = clean }
        }
        .onChange(of: typing) { _, focused in
          if !focused { add() }
        }
        .onAppear { typing = true }
    } else {
      RowButton("+", color: color) {
        withAnimation(.snappy) {
          swiped = nil
          addMenu = true
        }
      }
      .accessibilityLabel("Add")
    }
  }

  /// The name typed in Add Friend.
  private func add() {
    if hint != nil && newName.isEmpty { return }  // the hint stays a moment, then the bar closes
    let name = Username.clean(newName)
    newName = ""
    guard !name.isEmpty else {
      adding = false
      return
    }
    submitted += 1
    let this = submitted
    hint = "\(name)…"
    Task {
      let problem = await addFriend(name)
      guard submitted == this else { return }  // another name came meanwhile: the field is its
      if let problem { return say(problem) }
      hint = nil
      if newName.isEmpty && !typing { adding = false }
    }
  }

  /// The name of an invite link: added the same way; what is wrong shows in Add Friend's field.
  private func invited(_ name: String) async {
    guard let problem = await addFriend(name) else { return }
    withAnimation(.snappy) {
      addMenu = false
      adding = true
    }
    say(problem)
  }

  /// A name to add, typed or from a link, once the hub said who it is: added as its owner writes it ("yogpt" becomes
  /// "YoGPT"), on top; nobody by that name: not added; no answer (offline): added as typed, looked up again when the
  /// list opens. One already on the list is not added again: its bar says so. What to say instead ("That’s You",
  /// "No Such User"), or nil.
  private func addFriend(_ name: String) async -> String? {
    if let me = accounts.account?.username, Username.skeleton(name) == Username.skeleton(me) { return "That’s You" }
    let client = accounts.client
    let answer = await inbox.lookUp(name, accounts: accounts)
    guard client === accounts.client else { return nil }  // signed out meanwhile: not this account's friend
    var user: String?
    var spelled = name
    switch answer {
    case .noSuchUser: return "No Such User"
    case .found(let found, let owners):
      user = found
      spelled = owners ?? spelled
    case .unknown: break
    }
    let skeleton = Username.skeleton(spelled)
    if let known = friends.shown.first(where: {
      !$0.gone && (Username.skeleton($0.name) == skeleton || (user != nil && $0.user == user))
    }) {
      withAnimation(.snappy) {
        position.scrollTo(id: Lifted(friend: known, lifts: bars.lifts[known.key] ?? 0).id, anchor: .center)
      }
      showSent(to: known, "Already Added")
      return nil
    }
    withAnimation(.snappy) {
      friends.add(spelled, user: user)
      if !scrolled { position.scrollTo(edge: .top) }
    }
    return nil
  }

  /// A hint in the empty field for a moment; then the bar closes unless something is typed.
  private func say(_ text: String) {
    submitted += 1
    let this = submitted
    hint = text
    typing = true  // the keyboard stays, to type the name again
    Task {
      try? await Task.sleep(for: .seconds(1.5))
      guard submitted == this else { return }
      hint = nil
      if newName.isEmpty && !typing { adding = false }
    }
  }

  // MARK: Camera mode

  @ViewBuilder private var backdrop: some View {
    if cameraOn, let picked {
      Image(uiImage: picked.image).resizable().scaledToFill()
    } else if cameraOn {
      CameraPreview(session: camera.session)
    } else {
      Color.amethyst
    }
  }

  /// The photo library at the bottom left, front/back at the bottom right, as in the Camera app.
  private var cameraControls: some View {
    ZStack {
      let thumbnail = picked?.thumbnail  // the picker's label closure is Sendable: take the value here
      PhotosPicker(selection: $libraryItem, matching: .images) {
        LibraryLabel(thumbnail: thumbnail)
      }
      .buttonStyle(.plain)
      .glassEffect(.regular.interactive(), in: .circle)
      .accessibilityLabel("Photo Library")
      .frame(maxWidth: .infinity, alignment: .leading)
      if picked == nil {  // a picked photo goes as it is
        CameraZoom(choices: zooms, chosen: $zoom).onChange(of: zoom) { camera.zoom(to: zoom) }
      }
      Button {
        camera.flip()
        Task { await readZooms() }
      } label: {
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

  private func switchCamera(_ on: Bool) async {
    guard on else { return camera.stop() }
    // Asked first: closed meanwhile, it stays closed.
    if await Camera.allowed() { if cameraOn { camera.start() } } else { cameraOn = false }
    await readZooms()
  }

  /// The zooms of the camera now in use, at "1" (as it starts, and after a flip).
  private func readZooms() async {
    zooms = await camera.zoomChoices()
    zoom = 1
  }

  // MARK: Sending

  private func send(to friend: Friend) {
    Perf.mark("tap.action")
    defer { PerfProbe.frame("tap") }
    // A tap while a bar is swiped open closes it, as in Mail; it sends nothing.
    guard swiped == nil else { return withAnimation(.snappy) { swiped = nil } }
    // Here, as the finger lifts: not after the list has drawn the change. A photo: the shutter's click.
    if cameraOn && picked == nil { Haptic.shutter() } else { Haptic.tap() }
    let library = picked?.data
    picked = nil
    libraryItem = nil
    guard cameraOn || library != nil else {
      showSent(to: friend, "Sent Yo", thenOnTop: true)
      Task {
        if let problem = await inbox.send(to: friend, accounts: accounts, friends: friends) {
          showSent(to: friend, problem)
        } else {
          await sentYo()
        }
      }
      return
    }
    let client = accounts.client  // the account tapped in: a photo waiting its 1.5 seconds never goes from the next one
    let at = Date.now
    run(friend, "Sent 📷") {
      var taken = library
      if taken == nil { taken = await camera.photo() }
      guard let taken, let file = await Task.detached(operation: { MediaFile.photo(taken) }).value else { return nil }
      return await stage(file, to: friend, from: client, at: at, keep: library == nil)
    }
  }

  /// The first Yo ever sent from this phone: what the friend gets, once "Sent Yo" was seen; and the way to get a Yo
  /// back while notifications are off (asked for with the list already: here only when the answer was no).
  private func sentYo() async {
    guard !UserDefaults.standard.bool(forKey: Self.firstYoKey) else { return }
    UserDefaults.standard.set(true, forKey: Self.firstYoKey)
    let status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    try? await Task.sleep(for: .seconds(1))
    firstYo =
      switch status {
      case .authorized, .provisional, .ephemeral: .allowed
      case .notDetermined: .notAsked
      default: .denied
      }
  }

  private func turnOnNotifications(_ notifications: Notifications) async {
    if notifications == .notAsked {
      await inbox.askForNotifications()
    } else if let url = URL(string: UIApplication.openNotificationSettingsURLString) {
      await UIApplication.shared.open(url)
    }
  }

  /// A photo or video taken: into the library at once (`keep`, as Settings want it; not one picked from there), and
  /// into the outbox, whose upload goes during the undo time; its Yo goes when that is over (`Prepared.send`), and an
  /// undone one is taken out again. Only from `client`, the account the gesture began in.
  private func stage(
    _ file: MediaFile, to friend: Friend, from client: YoClient?, at: Date, keep: Bool = true
  ) async -> Prepared? {
    if keep { Task.detached(priority: .utility) { file.keepIfWanted() } }
    let id = UUID()
    guard let client, client === accounts.client,
      await outbox.send(
        file, to: friend.user ?? friend.name, from: client, accounts: accounts, id: id, held: true, at: at)
    else { return nil }
    let outbox = outbox
    return Prepared(
      send: {
        outbox.release(id)
        Stats.sentYo(to: friend.name)
      },
      undo: { outbox.discard(id) })
  }

  /// Holding a name in camera mode: a video from the moment it is held until it is let go.
  private func hold(_ friend: Friend, _ holding: Bool) {
    if holding {
      guard recordingFor == nil else { return }  // one video at a time: a second finger records nothing
      recordingFor = friend
      camera.startVideo()
    } else if recordingFor == friend {
      recordingFor = nil
      Haptic.tap()
      let client = accounts.client
      let at = Date.now
      run(friend, "Sent 🎥") {
        guard let recording = await camera.stopVideo(), let file = await MediaFile.video(recording) else { return nil }
        return await stage(file, to: friend, from: client, at: at)
      }
    }
  }

  /// A photo or video on its way out joins the friend's stack: every one in it waits until `sendDelay` has passed
  /// since the newest (each new one starts the time again, the ring after the words showing it); then the stack goes,
  /// in order. Meanwhile `prepare` takes the photo or ends the video. A tap on the ring keeps the whole stack here.
  private func run(_ friend: Friend, _ text: String, prepare: @escaping @MainActor () async -> Prepared?) {
    let bars = bars  // the object itself into the task, not this view's copy of the state
    let one = Outgoing(text: text)
    let bar = bars.of(friend)
    bar.outgoing.append(one)
    bars.preparing[one.id] = Task { await prepare() }
    let key = friend.key
    let round = (bars.stackRounds[key] ?? 0) + 1
    bars.stackRounds[key] = round
    Task {
      try? await Task.sleep(for: .seconds(Self.sendDelay))
      guard bars.stackRounds[key] == round, !bar.outgoing.isEmpty else { return }  // another came, or undone
      let stack = bar.outgoing
      bars.stackRounds[key] = nil
      bar.outgoing = []
      // After the stack before (a video still being finished): the friend gets them in the order taken.
      let before = bars.releasing[key]
      let releasing = Task {
        await before?.value
        var went = 0
        for one in stack {
          guard let prepared = await bars.preparing.removeValue(forKey: one.id)?.value else { continue }
          went += 1
          prepared.send()  // uploaded meanwhile (or uploading): its Yo goes now
          showSent(to: friend, one.text, thenOnTop: true)
        }
        if went == 0 { showSent(to: friend, "Not Sent") }
      }
      bars.releasing[key] = releasing
      await releasing.value
      if bars.releasing[key] == releasing { bars.releasing[key] = nil }
    }
  }

  /// A photo or video in the outbox, held: `send` lets its Yo go, `undo` takes it out again.
  struct Prepared {
    let send: @MainActor () -> Void
    let undo: @MainActor () -> Void
  }

  /// The ring tapped: the whole stack stays here.
  private func cancelSending(_ friend: Friend) {
    let bar = bars.of(friend)
    for one in bar.outgoing {
      // Taken out of the outbox once it is in (still being taken or sealed: as soon as it is).
      if let preparing = bars.preparing.removeValue(forKey: one.id) { Task { await preparing.value?.undo() } }
    }
    bars.stackRounds[friend.key] = nil
    bar.outgoing = []
    showSent(to: friend, "Canceled")
  }

  /// What the bar says after a send, for 1.5 seconds as in the original; then a bar that sent goes on top. Another tap
  /// within them starts them again, so the bar never slides away under a finger tapping it again and again. Sends of
  /// the same kind in a row are counted, one "!" for each after the first ("Sent Yo", "Sent Yo!", …). `then` is what
  /// goes once they are over (a photo or video).
  private func showSent(
    to friend: Friend, _ text: String, thenOnTop: Bool = false, then: (@MainActor () async -> Void)? = nil
  ) {
    let bars = bars  // the object itself into the task, not this view's copy of the state
    let key = friend.key  // the friend's, also if their spelling changes meanwhile
    let bar = bars.of(friend)
    let count = bar.sent.map { $0.text == text ? $0.count + 1 : 1 } ?? 1
    withAnimation(.snappy(duration: 0.2)) {  // a "!" comes in, or the number turns
      bar.sent = (text, count)
    }
    if let then { bars.queued[key, default: []].append((.now, then)) }
    let round = (bars.sentRounds[key] ?? 0) + 1
    bars.sentRounds[key] = round
    Task {
      try? await Task.sleep(for: .seconds(1.5))
      guard bars.sentRounds[key] == round else { return }  // tapped again meanwhile
      bars.sentRounds[key] = nil
      let pending = bars.queued.removeValue(forKey: key) ?? []
      withAnimation(.snappy) { bar.sent = nil }
      if thenOnTop, let now = friends.all.first(where: { $0.key == key }) { rise(now.name) }
      // In the rhythm they were tapped: tapped fast, they come fast and their sounds cut each other off.
      let start = Date.now
      for one in pending {
        let wait = one.at.timeIntervalSince(pending[0].at) - Date.now.timeIntervalSince(start)
        if wait > 0 { try? await Task.sleep(for: .seconds(wait)) }
        await one.send()
      }
    }
  }

  // MARK: Seeing

  /// A finger on a bar's thumbnail plays its photos and videos, oldest first. Held, letting go closes them; a short
  /// tap lets them run to the end (held: paused; a tap on the left third: back; elsewhere: on). What was shown is gone.
  private func see(_ friend: Friend, _ touch: ThumbnailTouch) {
    switch touch {
    case .down:
      shown = []
      viewing = inbox.media(from: friend.name)
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
      .onChange(of: inbox.arrived, initial: true) { arrived() }
      .onAppear { PerfProbe.install() }
      // While the list is on screen: Yos come in from the hub.
      .task { await inbox.listen(accounts: accounts, friends: friends) }
      .onChange(of: onScreen) { startClocks() }
      .onChange(of: libraryItem) { _, item in load(item) }
      .onChange(of: cameraOn) { _, on in cameraSwitched(on) }
      .onDisappear { cameraOn = false }
      // An invite link's name, once the hub can be asked: added once (Invite.swift).
      .task(id: "\(Invite.shared.pending ?? "")/\(accounts.client != nil)") {
        guard let name = Invite.shared.pending, accounts.client != nil else { return }
        Invite.shared.pending = nil
        await invited(name)
      }
  }

  private func feedback(_ content: some View) -> some View {
    animations(haptics(content))
  }

  private func haptics(_ content: some View) -> some View {
    content
      .sensoryFeedback(.impact(weight: .heavy), trigger: cheers)
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

  /// New Yos, as many as came since the last batch (`Inbox.batchSeconds`): one step for all of them, whatever their
  /// number. One haptic; each sender's bar goes on top once and counts all its Yos at once (one "!" for each); then
  /// the clocks start once.
  private func arrived() {
    let batch = inbox.takeArrivals()
    guard !batch.isEmpty else { return }
    var senders: [String] = []  // by their newest Yo, the newest last (it ends on top)
    var runs: [String: (first: String, count: Int)] = [:]
    for arrival in batch {
      if let at = senders.firstIndex(of: arrival.from) { senders.remove(at: at) }
      senders.append(arrival.from)
      runs[arrival.from, default: (arrival.id, 0)].count += 1
    }
    cheers += 1
    Perf.mark("arrived \(batch.count)")
    PerfProbe.frame("arrived")
    for name in senders { rise(name) }  // on top as soon as no bar is being tapped, all in one step
    withAnimation(.snappy(duration: 0.2)) {  // the words make room without a bounce
      for name in senders {
        guard let run = runs[name] else { continue }
        land(name, run.first, count: run.count)
      }
    }
    guard !clocks.starting else { return }
    clocks.starting = true
    Task {
      try? await Task.sleep(for: .milliseconds(150))  // the list has laid out where the bars are now
      clocks.starting = false
      startClocks()
    }
  }

  /// Yos on a sender's bar: the stack counts them, and one on screen starts its time again. A new run (also over one
  /// fading out) knocks. A photo stays until seen (Inbox.media(from:)).
  private func land(_ name: String, _ first: String, count: Int) {
    if var landing = landed[name], landing.fading == nil {
      landing.count += count  // more "!"s
      if landing.seen != nil {
        let now = Date.now
        landing.seen = now  // on screen: the stack's time starts again
        settle(name, now)
      }
      landed[name] = landing
    } else {
      landed[name] = Landing(first: first, count: count)
    }
  }

  /// Stacks whose bars are on screen now start their time; one out of sight waits until it is seen.
  private func startClocks() {
    let seen = Set(friends.shown.filter { onScreen.contains($0.key.uuidString) }.map(\.name))  // as named now
    for (name, landing) in landed where seen.contains(name) && landing.seen == nil {
      let now = Date.now
      landed[name]?.seen = now
      settle(name, now)
    }
  }

  /// A stack's time, started at `since`: if no Yo came meanwhile, its "!"s go one by one (quickly; faster the more
  /// there are), then the last Yo fades out (`yoSeconds`). A new Yo starts it again.
  private func settle(_ name: String, _ since: Date) {
    clocks.settling[name]?.cancel()  // the clock before is over: one per stack, not one per Yo
    clocks.settling[name] = Task {
      try? await Task.sleep(for: .seconds(Self.yoSolidSeconds))
      guard !Task.isCancelled else { return }
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
    let bars = bars  // the object itself into the task, not this view's copy of the state
    bars.rising.removeAll { $0 == name }
    bars.rising.append(name)
    guard bars.rising.count == 1 else { return }  // a step is already on its way
    Task {
      try? await Task.sleep(for: .seconds(0.3))
      while sending { try? await Task.sleep(for: .milliseconds(200)) }
      let names = bars.rising
      bars.rising = []
      toTop(names)
    }
  }

  /// A send still in its time: taps on a bar, or photos and videos on their way.
  private var sending: Bool {
    !bars.sentRounds.isEmpty || !bars.stackRounds.isEmpty || bars.outgoing
  }

  /// Friends go on top, the last of `names` uppermost. At the top of the list the list stays at its very start, so the
  /// bar is in sight below the title; scrolled down, the view stays where it is (the Yo waits on its bar until it is
  /// seen).
  private func toTop(_ names: [String]) {
    // By the friend, not the name: a name can change its spelling between a Yo landing and this step, and a name
    // not found would count as "moving" and lift a bar that is already on top.
    let shown = friends.shown
    let skeleton = { (name: String) in Username.skeleton(name) }
    var keys: [UUID] = []
    for name in names {
      guard let key = (shown.first { $0.name == name } ?? shown.first { skeleton($0.name) == skeleton(name) })?.key
      else { continue }
      keys.removeAll { $0 == key }
      keys.append(key)
    }
    let order = shown.map(\.key)  // as on screen: a blocked friend above would make it "move"
    // Already in place: the top bars are these friends in this order, newest first.
    let moving = keys.enumerated().filter { i, key in order.firstIndex(of: key) != keys.count - 1 - i }.map(\.element)
    guard !moving.isEmpty else { return }
    withAnimation(.snappy(duration: 0.35)) {
      for key in keys {
        if moving.contains(key) { bars.lifts[key, default: 0] += 1 }
        if let name = friends.all.first(where: { $0.key == key })?.name { friends.promote(name) }
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
      var made: Picked?
      if let data { made = await Picked.make(data) }
      if libraryItem == item { picked = made }
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

/// A friend's bar as it is now: what it says after a send and what is on its way out (`FriendList.Bars.Bar`) are
/// read here, so a tap redraws this bar alone and not the whole list.
struct LiveBar: View {
  let friend: Friend
  let bar: FriendList.Bars.Bar
  let color: Color
  let recording: Bool
  let stop: () -> Void
  let media: [Arrival]
  let yo: FriendList.Landing?
  let hint: String?
  let hold: ((Bool) -> Void)?
  let view: (ThumbnailTouch) -> Void
  let action: () -> Void

  var body: some View {
    RowButton(
      title, count: count, color: color, recording: recording, outgoing: bar.outgoing.last?.started, stop: stop,
      media: media, yo: yo, hint: hint, hold: hold, view: view, faded: friend.gone && bar.sent == nil,
      action: action)
      // Gone: still their name (whose bar it is), faded; a tap says "Not on Yo".
      .accessibilityValue(friend.gone ? "Not on Yo" : "")
  }

  /// The bar's words: what is happening, else the name.
  private var title: String {
    if recording { return "● Recording" }
    if let last = bar.outgoing.last { return last.text }
    if let sent = bar.sent { return sent.text }
    return friend.name
  }

  /// How many the words count ("Sent Yo!!" for three): photos on their way, or sends in a row.
  private var count: Int {
    if recording { return 1 }
    if !bar.outgoing.isEmpty { return bar.outgoing.count }
    return bar.sent?.count ?? 1
  }
}

/// A tap's haptic, played in the tap's own handler from a generator kept ready: `.sensoryFeedback` plays only in
/// SwiftUI's next update, once the list has been rebuilt for the change it watches (8 to 10 ms after the tap in the
/// Simulator, later when the list is busy), and a generator not prepared starts late.
@MainActor enum Haptic {
  private static let medium = UIImpactFeedbackGenerator(style: .medium)
  private static let light = UIImpactFeedbackGenerator(style: .light)
  private static let rigid = UIImpactFeedbackGenerator(style: .rigid)

  /// A photo taken: short and hard, as the Camera app's shutter.
  static func shutter() {
    rigid.impactOccurred(intensity: 1)
    Perf.mark("tap.haptic")
    rigid.prepare()
  }

  static func tap(_ style: UIImpactFeedbackGenerator.FeedbackStyle = .medium) {
    let generator = style == .light ? light : medium
    generator.impactOccurred()
    Perf.mark("tap.haptic")
    generator.prepare()  // ready for the next tap
  }

  /// A finger is down: the next `tap` plays without the engine's start.
  static func prepare() {
    medium.prepare()
    rigid.prepare()
  }
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
  /// The words faded (a friend no longer on Yo).
  var faded = false
  let action: () -> Void
  @State private var pressed = false
  @State private var viewing = false
  @State private var touchedAt = Date.distantPast
  @State private var recordingByAction = false

  init(
    _ title: String, count: Int = 1, color: Color, recording: Bool = false, outgoing: Date? = nil,
    stop: @escaping () -> Void = {},
    media: [Arrival] = [], yo: FriendList.Landing? = nil, hint: String? = nil, hold: ((Bool) -> Void)? = nil,
    view: @escaping (ThumbnailTouch) -> Void = { _ in }, faded: Bool = false, action: @escaping () -> Void
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
    self.faded = faded
    self.action = action
  }

  var body: some View {
    Group {
      if let hold {
        // The camera's shutter: pressed, the bar goes back and darkens; let go, it springs out again. Once a video
        // records (held), it is out again, red, filling as the video's time runs.
        let down = pressed && !recording
        label
          .brightness(down ? -0.1 : 0)
          .scaleEffect(down ? 0.94 : 1)
          .animation(down ? .easeOut(duration: 0.08) : .spring(response: 0.3, dampingFraction: 0.55), value: down)
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
    // do not work). The ring stands at the bar's very left, away from where the bar was tapped: a tap more is
    // another photo, never an undo by mistake.
    .overlay { line(touchable: true) }
    .overlay(alignment: .leading) {
      if let outgoing {
        undoRing(since: outgoing)
          .padding(.leading, BarLine.margin)
      }
    }
    .animation(.snappy, value: outgoing)
    .animation(.spring(duration: 0.4, bounce: 0.35), value: yo)
    .animation(.smooth, value: media)
  }

  /// A bar of the friend's colour from edge to edge, the name large in the middle (the original's), red while
  /// recording.
  private var label: some View {
    line(touchable: false)
      .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
      .background {
        ZStack(alignment: .leading) {
          recording ? Color.alizarin.opacity(0.85) : color
          if recording { RecordingFill(seconds: Camera.longestVideo) }
        }
      }
      .contentShape(.rect)
  }

  /// The bar's line, sending and receiving alike (BarLine): the words and their "!"s; a Yo that came at the end; room
  /// at the start for the undo ring while something is on its way out. Laid out twice the same: in the bar the words
  /// and their "!"s, over it what takes its own touches (the Yo at the end), so each sits exactly where the other
  /// leaves it room.
  private func line(touchable: Bool) -> some View {
    BarLine(start: outgoing == nil ? BarLine.margin : BarLine.margin + Self.ring + BarLine.gap) {
      Text(title)
        .lineLimit(1)
        .minimumScaleFactor(0.5)  // a long name, or large text, gets smaller before it is cut with "…"
        .contentTransition(.identity)  // other words come at once, never two drawn over each other
        .modifier(Knock(trigger: yo?.first))
        .opacity(touchable ? 0 : faded ? 0.55 : 1)
        .accessibilityHidden(touchable)
      Bangs(count: count).opacity(touchable ? 0 : 1).accessibilityHidden(touchable)
      ZStack { mark(touchable: touchable) }  // always a part, so a missing "!" never takes its place
        .modifier(Knock(trigger: yo?.first))
        .opacity(touchable ? 1 : 0)
        .accessibilityHidden(!touchable)
    }
    .font(.yo(Metrics.rowFont))
    .foregroundStyle(.white)
    .animation(nil, value: title)  // other words stand in their place at once; only "!"s, numbers and room animate
  }

  private static let ring = 30.0

  /// Something on its way out: a ring at the bar's left closing like an upload; a tap on it undoes the send.
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
/// The camera's zoom, as the Camera app has it: its choices in a glass capsule ("0.5  1×  2  3"), the chosen one with
/// its "×"; a tap goes to another. Not there with one choice only (the front camera).
struct CameraZoom: View {
  let choices: [Double]
  @Binding var chosen: Double

  var body: some View {
    if choices.count > 1 {
      HStack(spacing: 2) {
        ForEach(choices, id: \.self) { choice in
          let on = choice == chosen
          Button {
            Haptic.tap(.light)
            withAnimation(.snappy(duration: 0.2)) { chosen = choice }
          } label: {
            Text(Self.label(choice) + (on ? "×" : ""))
              .font(.system(size: on ? 13 : 11, weight: .bold).monospacedDigit())
              .foregroundStyle(on ? Color.yellow : .white)
              .frame(width: 36, height: 36)
              .background(.black.opacity(on ? 0.35 : 0.2), in: .circle)
              .contentShape(.circle)
          }
          .buttonStyle(.plain)
          .accessibilityLabel("Zoom \(Self.label(choice))×")
          .accessibilityAddTraits(on ? .isSelected : [])
        }
      }
      .padding(4)
      .glassEffect(.regular.interactive(), in: .capsule)
      .transition(.opacity)
    }
  }

  /// "0.5", "1", "2", "2.5": no ".0".
  static func label(_ choice: Double) -> String {
    choice == choice.rounded() ? String(Int(choice)) : String(format: "%.1f", choice)
  }
}

struct Bangs: View {
  let count: Int

  var body: some View {
    let (bangs, more) = ShareBridge.bangs(count)  // as a notification says them (ShareBridge.yo)
    HStack(spacing: 0) {
      ForEach(0..<bangs, id: \.self) { _ in
        Text("!").transition(.opacity.animation(.easeOut(duration: 0.15)))
      }
      if more > 0 {
        Text(" +\(more)")
          .monospacedDigit()
          .contentTransition(.numericText(value: Double(more)))
          .transition(.opacity)
      }
    }
  }
}

/// A bar's line, for sending and receiving alike: the words in the middle, their "!"s right after them, a Yo that came
/// at the bar's end. The words stay in the middle until they would run into the end (or the edge): then they move left
/// as far as needed, never past `start` (the margin, or after the undo ring); a name too long even then gets smaller
/// (to half), then is cut with "…". Nothing overlaps.
struct BarLine: Layout {
  static let margin = 20.0
  static let endPadding = 30.0
  static let gap = 14.0
  /// How far from the bar's left edge the words may go at most.
  var start = margin

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
    let room = max(0, limit - bounds.minX - start - afterSize.width)
    let wordsWidth = min(words.sizeThatFits(.unspecified).width, room)
    var x = bounds.midX - wordsWidth / 2
    x -= max(0, x + wordsWidth + afterSize.width - limit)
    x = max(bounds.minX + start, x)
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

/// A bar that slides left under a horizontal swipe and shows Block and Remove underneath (as Mail's and Signal's
/// lists do); it snaps open or closed when let go. A vertical move is the list's scroll, a tap or a hold the bar's.
struct SwipeActions<Content: View>: View {
  @Binding var open: Bool
  let enabled: Bool
  /// A swipe on it began.
  let began: () -> Void
  let block: () -> Void
  let remove: () -> Void
  @ViewBuilder let content: Content
  /// How far the finger moved the bar, while it does.
  @State private var dragged: CGFloat?

  private static var button: CGFloat { 88 }
  private static var width: CGFloat { 2 * button }

  var body: some View {
    let resting = open ? -Self.width : 0
    let moved = resting + (dragged ?? 0)
    // Past the buttons it follows with resistance; never to the right.
    let offset = moved < -Self.width ? -Self.width + (moved + Self.width) / 3 : min(0, moved)
    content
      .overlay {
        // Open: a tap on the bar closes it (the bar does nothing else then).
        if open { Color.clear.contentShape(.rect).onTapGesture { close() } }
      }
      .offset(x: offset)
      .background(alignment: .trailing) {
        if offset < 0 {
          HStack(spacing: 0) {
            action("Block", "hand.raised.fill", Color(.systemGray)) { close(); block() }  // not a bar's own colour
            action("Remove", "trash.fill", .alizarin) { remove() }
          }
          .frame(width: max(Self.width, -offset))
          .transition(.identity)
        }
      }
      .gesture(
        HorizontalPan(enabled: enabled) { x in
          if dragged == nil { began() }
          dragged = x
        } ended: { x, velocity in
          let end = resting + x
          withAnimation(.snappy(duration: 0.3)) {
            open = velocity < -400 || (velocity < 400 && end < -Self.width / 2)
            dragged = nil
          }
        }
      )
      .onChange(of: enabled) { _, on in
        if !on { close() }
      }
      .accessibilityAction(named: "Block") { block() }
      .accessibilityAction(named: "Remove") { remove() }
  }

  private func close() {
    withAnimation(.snappy(duration: 0.3)) { open = false }
  }

  private func action(_ title: String, _ icon: String, _ color: Color, _ run: @escaping () -> Void) -> some View {
    Button(action: run) {
      VStack(spacing: 4) {
        Image(systemName: icon).font(.system(size: 20, weight: .semibold))
        Text(title).font(.system(size: 13, weight: .semibold))
      }
      .foregroundStyle(.white)
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(color)
    }
    .buttonStyle(.plain)
  }
}

/// A pan that begins only when it starts sideways: the scroll view keeps vertical moves, and a tap stays a tap.
struct HorizontalPan: UIGestureRecognizerRepresentable {
  var enabled = true
  let changed: (CGFloat) -> Void
  let ended: (_ translation: CGFloat, _ velocity: CGFloat) -> Void

  func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
    let pan = UIPanGestureRecognizer()
    pan.delegate = context.coordinator
    return pan
  }

  func updateUIGestureRecognizer(_ pan: UIPanGestureRecognizer, context: Context) {
    pan.isEnabled = enabled
  }

  func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
    let x = pan.translation(in: pan.view).x
    switch pan.state {
    case .began, .changed: changed(x)
    case .ended, .cancelled, .failed: ended(x, pan.velocity(in: pan.view).x)
    default: break
    }
  }

  func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

  final class Coordinator: NSObject, UIGestureRecognizerDelegate {
    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
      guard let pan = gesture as? UIPanGestureRecognizer else { return false }
      let velocity = pan.velocity(in: pan.view)
      return abs(velocity.x) > abs(velocity.y) * 1.5
    }
  }
}

/// A video recording: from the bar's left, darker as its time runs to the longest a video may be, breathing once a
/// second.
struct RecordingFill: View {
  let seconds: Double
  @State private var start = Date.now

  var body: some View {
    TimelineView(.animation) { context in
      let elapsed = context.date.timeIntervalSince(start)
      let breath = 0.5 + 0.5 * cos(elapsed * 2 * .pi)
      GeometryReader { size in
        Rectangle()
          .fill(.black.opacity(0.16 + 0.08 * breath))
          .frame(width: size.size.width * min(1, elapsed / seconds))
      }
    }
    .allowsHitTesting(false)
    .accessibilityHidden(true)
  }
}

/// The bar under the finger: a quick light-up.
struct RowPress: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .overlay(.white.opacity(configuration.isPressed ? 0.18 : 0))
      .scaleEffect(configuration.isPressed ? 0.985 : 1)
      .animation(.smooth(duration: 0.15), value: configuration.isPressed)
      .onChange(of: configuration.isPressed) { _, pressed in
        if pressed { Haptic.prepare() }
      }
  }
}

/// Photos and videos, as Stories: the whole screen, one after the other (oldest first), a line for each at the top
/// filling up; a photo at most `photoSeconds`, a video as long as it runs. A finger held anywhere pauses it (the line
/// and the video), letting go goes on; a tap on the left third goes back, elsewhere to the next. Back reaches all
/// shown while the viewer is open (they are gone once it closes). Past the last, or swiped down, it closes. Each is
/// reported when it shows. Opened by holding the thumbnail, letting go of it closes sooner (FriendList).
struct HeldMedia: View {
  let queue: [Arrival]
  let photoSeconds: Double
  let shows: (Arrival) -> Void
  let finished: () -> Void
  @State private var index = 0
  /// Counts each start (also of the same one again): what the showing one's tasks go by.
  @State private var round = 0
  /// How long the one showing runs; nil while it is made ready.
  @State private var seconds: Double?
  /// How much of it ran before the current stretch, and when that stretch began (nil while paused): its line fills
  /// from the clock, so pausing and skipping never leave a line half way or running on.
  @State private var ran = 0.0
  @State private var since: Date?
  /// A finger on the screen: paused until it lets go.
  @GestureState private var held = false  // also let go when the system takes the touch
  /// Where and when the finger on the screen came down: a new place is a new touch (one the system took leaves
  /// nothing behind).
  @State private var touched: (at: CGPoint, time: Date)?
  /// How far a swipe down has pulled the picture.
  @State private var pulled = 0.0
  @State private var width = 393.0
  @State private var player: AVPlayer?
  /// The showing photo in full, read from its file when it shows (the list keeps only a small thumbnail).
  @State private var full: UIImage?

  /// The showing one's time runs while it is ready and no finger holds it.
  private struct Run: Equatable {
    let round: Int
    let held: Bool
    let ready: Bool
  }

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
          } else if let photo = full ?? current.photo {
            Image(uiImage: photo).resizable().scaledToFit().frame(maxWidth: .infinity, maxHeight: .infinity)
          }
        }
        .ignoresSafeArea()
        // Who it is from at the top left, how long ago beside it, the lines under them (as stories show it).
        VStack(alignment: .leading, spacing: 8) {
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
          }
          .foregroundStyle(.white)
          .shadow(color: .black.opacity(0.35), radius: 3)
          .padding(.horizontal, 8)
          .accessibilityElement(children: .combine)
          .accessibilityLabel("Yo from \(current.from)")
          TimelineView(.animation(paused: since == nil)) { timeline in
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
        }
        .padding(.horizontal, 4)
        .padding(.top, 4)
        .frame(maxHeight: .infinity, alignment: .top)
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
    .task(id: round) { await load() }
    .task(id: Run(round: round, held: held, ready: seconds != nil)) { await run() }
    .onDisappear { player?.pause() }
    .accessibilityAction(named: "Previous") { go(to: index - 1) }
    .accessibilityAction(named: "Next") { go(to: index + 1) }
    .accessibilityAction(.escape) { finished() }
  }

  /// Down: paused at once. Up: going on; a quick tap steps (left third back, else on), a swipe down closes.
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
        if moved.height > 90 && moved.height > abs(moved.width) { return finished() }
        withAnimation(.snappy(duration: 0.25)) { pulled = 0 }
        guard quick, abs(moved.width) < 20, abs(moved.height) < 20 else { return }
        go(to: drag.startLocation.x < width / 3 ? index - 1 : index + 1)
      }
  }

  /// How much of the showing one has run, 0 to 1.
  private func progress(at date: Date) -> Double {
    guard let seconds, seconds > 0 else { return 0 }
    return min(1, (ran + (since.map { date.timeIntervalSince($0) } ?? 0)) / seconds)
  }

  /// Another one (back before the first: the first again), from its start.
  private func go(to next: Int) {
    index = max(0, next)
    seconds = nil
    ran = 0
    since = nil
    round += 1
  }

  /// The one to show made ready: reported, a video loaded and its length known. Past the last: closed.
  private func load() async {
    guard index < queue.count else { return finished() }
    let current = queue[index]
    shows(current)
    player?.pause()
    guard let video = current.video else {
      player = nil
      full = nil
      if let file = current.file {
        full = await Task.detached(priority: .userInitiated) { UIImage(contentsOfFile: file.path) }.value
      }
      guard !Task.isCancelled else { return }
      seconds = photoSeconds
      return
    }
    let player = AVPlayer(url: video)
    self.player = player
    let length = (try? await AVURLAsset(url: video).load(.duration).seconds) ?? 0
    guard !Task.isCancelled else { return }
    seconds = length > 0 ? length : photoSeconds
  }

  /// The showing one's time: running (the video playing) until it is over, then the next; paused while held.
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
