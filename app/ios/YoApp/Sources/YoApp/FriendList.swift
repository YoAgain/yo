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
import AudioToolbox
import ImageIO
import PhotosUI
import UniformTypeIdentifiers
import _PhotosUI_SwiftUI

/// Incoming Yos glow white, as bright as the screen allows (HDR); `pulse` (0…1) only ever adds to it: brighter, never
/// dimmer, never another hue. What you sent stays plain white.
private struct YoIntensity: ViewModifier {
  let incoming: Bool
  let pulse: Double

  func body(content: Content) -> some View {
    content
      .foregroundStyle(incoming ? Color.white.exposureAdjust(3 + max(0, pulse)) : .white)
      .allowedDynamicRange(incoming ? .high : .standard)
  }
}

private enum ListMotion {
  static let expansion = Animation.spring(duration: 0.35, bounce: 0)
  static let flight = Animation.spring(duration: 0.35, bounce: 0.16)
}

private extension EnvironmentValues {
  @Entry var yoFlood = false
}

import SwiftUI
import os
import UserNotifications
import YoCore

struct FriendList: View {
  /// How long a photo stays when its thumbnail is held.
  static let photoSeconds = 3.0  // Snapchat's first default
  /// How long a photo or video waits before it goes, time to undo it.
  private static let listTiming = listTimings()
  static let sendDelay = Double(listTiming.undoMs) / 1000
  /// How long a plain Yo shows on its bar.
  static let yoSeconds = Double(listTiming.yoSolidMs + listTiming.yoFadeMs) / 1000
  /// How long of that it stands fully before it fades.
  static let yoSolidSeconds = Double(listTiming.yoSolidMs) / 1000

  @Environment(\.scenePhase) private var scenePhase
  @Environment(Inbox.self) private var inbox
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @Environment(MediaOutbox.self) private var outbox

  private final class MediaInputCache { var each: [String: [Bool]] = [:] }
  @State private var listMedia = MediaInputCache()
  @State private var listModel = DeleteNotice.shared.model
  @State private var listOrder: [String] = []
  @State private var listDeadline: UInt64?
  @State private var lockedRow: String?
  private var listNow: UInt64 { UInt64(ProcessInfo.processInfo.systemUptime * 1000) }
  /// Cache render data only; the core supplies every ordering decision.
  private final class RenderedRows {
    private var source: [Friend] = []
    private var order: [String] = []
    private var result: [Friend] = []
    func resolve(_ all: [Friend], ids: [String]) -> [Friend] {
      if order != ids || !source.elementsEqual(all, by: { $0.contentMatches($1) }) {
        source = all; order = ids
        let values = Dictionary(all.map { ($0.key.uuidString, $0) }, uniquingKeysWith: { a, _ in a })
        result = ids.compactMap { values[$0] }
      }
      return result
    }
  }
  @State private var renderedRows = RenderedRows()
  private var orderedFriends: [Friend] { renderedRows.resolve(friends.all, ids: listOrder) }
  private var listFriends: [ListFriend] {
    friends.shown.map { ListFriend(id: $0.key.uuidString, name: $0.name, request: $0.request, group: $0.group != nil) }
  }

  // The list
  @State private var adding = false
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
  @State private var settingsAction: SettingsView.ListAction?
  @State private var position = ScrollPosition(idType: String.self)
  private static let addBlockBottomID = "add-block-bottom"
  @State private var keyboardDuration = 0.25
  /// Rows are behind the title: it wears its glass pill.
  @State private var scrolled = false
  /// The bar swiped open (its Block and Remove showing), by `Friend.key`: one at a time.
  @State private var swiped: UUID?

  // Sending
  /// Shared row render slots and captured bytes. Reference storage lets capture/upload tasks
  /// update the live row after a library selection or another tap.
  @State private var bars = Bars()

  // Blocking and reporting
  /// The friend the Block dialog asks about, and whether it follows a report.
  @State private var blocking: (friend: Friend, reported: Bool)?
  /// A group to leave, asked first: its Yos stop for good (nobody can bring one back but a member adding again).
  @State private var leaving: Friend?
  /// A group with nobody else left, tapped: "Group Is Empty", delete it?
  @State private var emptyGroup: Friend?
  /// The camera's hint is showing; counted on by each photo or video taken, which hides it for 3 s more.
  @State private var hintShown = false
  /// Contacts whose new key waits for a confirmation (verified before): only they get "Confirm New Key".
  @State private var unconfirmed: Set<String> = []
  @State private var hintEpoch = 0
  /// A group's bar being renamed (the new name typed), or whose members are shown.
  @State private var renaming: Friend?
  @State private var newGroupName = ""
  /// A group whose members are shown (and added, removed): GroupMembersSheet.
  @State private var membersOf: Friend?
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
      var hasNotice = false
      var modelTitle = ""
      var modelCount = 1
      var modelMarkCount = 0
      var modelMediaSign = ""
      var modelPaused = false
      var modelSenders: [RenderedSender] = []
      /// Photos and videos on their way out, each with when it started; any number at once (a tap on the bar sends
      /// another, the ring at its left undoes them).
      var outgoing: [Outgoing] = []
      /// Nil while captures join; the full undo interval starts after their flights settle.
      var countdownStarted: Date?
      /// Keep the already-counted batch visible after undo ends, until its sends finish.
      var sendingBatch: (id: UUID, count: Int, items: [Outgoing])?
      /// How long the stack on its way out waits (`run`): `sendDelay`, or a friend's "Undo Before Sending" time.
      var seconds = FriendList.sendDelay
    }

    /// Each friend's bar state, by `Friend.key`, made when first asked for.
    @ObservationIgnored private var each: [UUID: Bar] = [:]
    func of(_ friend: Friend) -> Bar {
      if let bar = each[friend.key] { return bar }
      let bar = Bar()
      each[friend.key] = bar
      return bar
    }
    /// Each photo or video of a stack being made ready (taken, or the video ended), by its id.
    @ObservationIgnored var preparing: [UUID: Task<Prepared?, Never>] = [:]
    /// A stack's undo interval: each new photo starts them again; when they are over, the whole stack goes.
    @ObservationIgnored var stackRounds: [UUID: UUID] = [:]
    /// Per friend, the last stack letting its Yos go: the next waits for it, so they go in the order taken.
    @ObservationIgnored var releasing: [UUID: Task<Void, Never>] = [:]
    @ObservationIgnored var flying: Set<UUID> = []
    @ObservationIgnored var awaitingThumbnail: Set<UUID> = []
  }

  struct Outgoing: Equatable {
    let id = UUID()
    let started = Date.now
    /// What the bar says meanwhile ("Sent 📷!"), the undo ring at the bar's left.
    let text: String
    var thumbnail: UIImage?
    let origin: CGPoint?
  }

  // Camera mode
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var edgeActive = false
  @State private var edgeSettling = false
  @State private var edgeOffset: CGFloat = 0
  @State private var cameraOn = false
  @State private var leftAt: Date?
  @State private var cameraPrepared = false
  @State private var listScrolling = false
  /// A group's bar that shrank back, and until when the bars under it (which moved up) take no taps (`send`).
  @State private var tapLock: (bar: UUID, until: Date)?
  @AppStorage(SettingKey.leftHanded) private var leftHanded = false
  @State private var camera = Camera()
  /// The camera's zooms (CameraZoom) and the one chosen.
  @State private var zooms: [Double] = []
  @State private var zoom = 1.0
  @State private var pinchStart: Double?
  @State private var recordingFor: Friend?
  @State private var libraryItems: [_PhotosUI_SwiftUI.PhotosPickerItem] = []
  /// Ordered library picks kept for repeated sends until closed.
  @State private var picked: [Picked] = []

  /// A photo from the library: what is sent, and its pictures for the screen, made once (a 48-megapixel photo decoded
  /// at each redraw kept the list from answering).
  struct Picked: Equatable {
    let data: Data
    var video: MediaFile? = nil
    let image: UIImage
    let thumbnail: UIImage?

    static func == (lhs: Self, rhs: Self) -> Bool {
      lhs.data == rhs.data && lhs.video?.data == rhs.video?.data
        && lhs.image === rhs.image && lhs.thumbnail === rhs.thumbnail
    }

    /// Off the main actor: decoded and scaled down for the screen and the library button.
    static func make(_ data: Data) async -> Picked? {
      await Task.detached(priority: .userInitiated) {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let options: [CFString: Any] = [
          kCGImageSourceCreateThumbnailFromImageAlways: true,
          kCGImageSourceCreateThumbnailWithTransform: true,
          kCGImageSourceThumbnailMaxPixelSize: 2868,
          kCGImageSourceShouldCacheImmediately: true,
        ]
        guard let pixels = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        return Picked(data: data, image: UIImage(cgImage: pixels),
                      thumbnail: FriendList.decodeOutgoingThumbnail(data))
      }.value
    }
  }

  // Arriving
  /// Counts Yos arriving, for the haptic.
  @State private var cheers = 0
  /// The plain Yos on bars now, by sender.
  @State private var landings = Landings()

  @Observable final class LandingSlot { var value: Landing? }
  final class Landings {
    private var slots: [String: LandingSlot] = [:]
    func slot(_ name: String) -> LandingSlot {
      if let slot = slots[name] { return slot }
      let slot = LandingSlot()
      slots[name] = slot
      return slot
    }
    subscript(_ name: String) -> Landing? {
      get { slot(name).value }
      set { slot(name).value = newValue }
    }
  }
  /// The renderer wakes the model only at its returned deadline.
  @State private var clocks = Clocks()
  /// The first friend whose new key is still to be said (Friends.keyAlerts).
  private var keyChanged: Friend? {
    friends.keyAlerts.lazy.compactMap { key in friends.all.first { $0.key == key } }.first
  }

  /// The photos and videos playing while a thumbnail is held, and those already shown.
  @State private var viewing: [Arrival]?
  @State private var shown: [Arrival] = []

  /// Display wakeup driver; every deadline and lifetime belongs to the core.
  @MainActor final class Clocks: NSObject {
    private var displayLink: CADisplayLink?
    private var update: (() -> Void)?
    func start(_ update: @escaping () -> Void) {
      guard displayLink == nil else { return }
      self.update = update
      let link = CADisplayLink(target: self, selector: #selector(tick))
      link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
      link.add(to: .main, forMode: .common)
      displayLink = link
    }
    func stop() {
      displayLink?.invalidate()
      displayLink = nil
      update = nil
    }
    @objc private func tick() { update?() }
  }

  /// The Yos on a bar now, a stack: each new one adds a "!" and starts the stack's time again. Only when none came for
  /// `yoSolidSeconds` (counted while the bar is on screen) do the "!"s go, one quickly after the other, and the last
  /// Yo fades out slowly.
  struct Landing: Equatable {
    /// The first Yo of this run: it knocks; the ones after only add a "!".
    var first: String
    /// The "!"s shown now: all of the run's Yos, fewer while its time counts them down.
    var count: Int
    /// All Yos of this run: a Yo coming while the "!"s go down shows them all again, and one more.
    var total: Int
    /// The last one is fading out since then.
    var fading: Date?
    var paused = false
    var fadeElapsed = 0.0
  }

  /// The list, and the question before leaving a group (on its own: one chain of modifiers less to type-check).
  private var asking: some View {
    navigation
      .overlay { BangPhysicsLayer() }  // dev prototype: falling "!"s (BangPhysics.swift)
      .alert(
      "Leave \(leaving?.name ?? "")?",
      isPresented: Binding(get: { leaving != nil }, set: { if !$0 { leaving = nil } }), presenting: leaving
    ) { bar in
      Button("Leave", role: .destructive) { leave(bar) }
      Button("Cancel", role: .cancel) {}
    } message: { _ in
      Text("You won’t get its Yos any more. A member can add you again.")
    }
    .alert(
      "Group Is Empty",
      isPresented: Binding(get: { emptyGroup != nil }, set: { if !$0 { emptyGroup = nil } }), presenting: emptyGroup
    ) { bar in
      Button("Delete Group", role: .destructive) { deleteEmpty(bar) }
      Button("Cancel", role: .cancel) {}
    } message: { bar in
      Text("Nobody else is in \(bar.name) any more. Delete it?")
    }
  }

  private func chooseRequest(_ friend: Friend, yes: Bool) {
    Haptic.tap()
    answer(friend, yes: yes)
  }

  /// A request answered: yes makes a contact (or joins the group); no takes the bar off the list, a block into Blocked.
  private func answer(_ bar: Friend, yes: Bool, block: Bool = false) {
    guard let client = accounts.client else { return }
    Task {
      if yes {
        let events: [YoEvent]?
        do {
          events = try await Hub.run {
            if let group = bar.group { return try client.acceptGroup(group: group) }
            return try client.accept(who: bar.target)
          }
        } catch {
          events = nil
          Logger(subsystem: "app.justyo", category: "requests").error(
            "accept \(bar.group == nil ? "contact" : "group", privacy: .public): \(String(describing: error), privacy: .public)")
        }
        guard let events else { showSent(to: bar, "Not Accepted"); return }
        await inbox.absorb(events, client: client, friends: friends)
        if let contacts = try? await Hub.run({ client.contacts() }) { friends.markRequests(contacts) }
        if let groups = try? await Hub.run({ try client.groups() }) { friends.takeGroups(groups) }
        guard client === accounts.client else { return }
        showSent(to: bar, "Sent Yo", thenOnTop: true)
        if let problem = await inbox.send(to: bar, accounts: accounts, friends: friends) {
          if problem == Inbox.groupEmpty { emptyGroup = bar }
          showSent(to: bar, problem)
        } else {
          await sentYo()
        }
      } else if let group = bar.group {
        if let events = try? await Hub.run({ try client.leaveGroup(group: group) }) {
          await inbox.absorb(events, client: client, friends: friends)
        }
        if let groups = try? await Hub.run({ try client.groups() }) { withAnimation(.snappy) { friends.takeGroups(groups) } }
      } else {
        if block {
          // Off the list and into Settings › Blocked, as any block (unblocked there, it is a request again).
          try? await Hub.run { try client.setBlocked(who: bar.target, blocked: true) }
          withAnimation(.snappy) { friends.setBlocked(bar, true) }
        } else {
          try? await Hub.run { try client.decline(who: bar.target) }
          withAnimation(.snappy) { friends.remove(bar) }
        }
      }
    }
  }

  var body: some View {
    reacting(
      asking
        // The list is always dark (white on purple); the profile follows the iPhone's own appearance.
        .sheet(isPresented: $settings, onDismiss: {
          let action = settingsAction
          settingsAction = nil
          switch action {
          case .addContact:
            beginAdding()
          case nil:
            break
          }
        }) {
          SettingsView { action in
            settingsAction = action
            settings = false
          }
          .preferredColorScheme(SystemAppearance.scheme)
        }
        .sheet(isPresented: $newGroup) { NewGroupSheet().preferredColorScheme(SystemAppearance.scheme) }
        .confirmCard(
          $blocking,
          title: { $0.reported ? "Thanks for Reporting" : "Block \($0.friend.name)?" },
          message: {
            $0.reported
              ? "Block \($0.friend.name) too? You won’t get their Yos."
              : "You won’t get their Yos. Unblock them under Yo ID › Blocked."
          },
          action: { "Block \($0.friend.name)" },
          cancel: blocking?.reported == true ? "Not Now" : "Cancel",
          run: { block($0.friend) })
        .alert(
          "Rename \(renaming?.name ?? "")",
          isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } }), presenting: renaming
        ) { bar in
          TextField("Group Name", text: $newGroupName)
          Button("Rename") { renameGroup(bar, to: newGroupName) }
          Button("Cancel", role: .cancel) {}
        }
        .sheet(item: $membersOf) { bar in GroupMembersSheet(bar: bar).preferredColorScheme(SystemAppearance.scheme) }
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
        // Whether the contact whose key changed waits for a confirmation (was verified): asked as the alert comes.
        .task(id: keyChanged?.key) {
          guard let client = accounts.client, keyChanged != nil,
            let contacts = try? await Hub.run({ client.contacts() })
          else { return }
          unconfirmed = Set(contacts.filter(\.unconfirmed).map(\.user))
        }
        // A contact's account key changed (spec 3.2): said once; Contacts shows it until their details are opened.
        .alert(
          "\(keyChanged?.name ?? "") Has a New Key",
          isPresented: Binding(
            get: { keyChanged != nil }, set: { if !$0, let friend = keyChanged { friends.keyAlertShown(friend) } }),
          presenting: keyChanged
        ) { friend in
          Button("OK", role: .cancel) {}
          // Only for a contact that was verified: its Yos wait for this (spec 3.4).
          if let user = friend.user, unconfirmed.contains(user) {
            Button("Confirm New Key") {
              Task { try? await Verification.confirm(user, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox) }
            }
          }
        } message: { friend in
          Text(
            "\(friend.name) may have a new iPhone. If you didn’t expect this, compare safety numbers in Profile > "
              + "Contacts.")
        }
        .overlay {
          ZStack {
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
          .animation(.easeOut(duration: 0.15), value: viewing != nil)
        }
    )
  }

  private var cameraLook: Double {
    let progress = Double(edgeOffset)
    return cameraOn ? 1 - progress : progress
  }

  private var navigation: some View {
    GeometryReader { geometry in
      ZStack {
        Color.amethyst.ignoresSafeArea()
        listNavigation
          .environment(\.sharedListColors, true)
          .allowsHitTesting(!edgeActive && !edgeSettling)
      }
      .gesture(CameraEdgePan(
        left: cameraOn != leftHanded,
        enabled: !edgeSettling && viewing == nil && recordingFor == nil,
        touched: {
          if !cameraOn && Camera.authorized { camera.start() }
        },
        released: {
          if !cameraOn && !edgeActive { camera.stop() }
        },
        changed: { x in
          if !edgeActive {
            #if YO_DEV
            HitchMonitor.shared.scene = "edge wipe"
            #endif
            edgeActive = true
            swiped = nil
          }
          let direction: CGFloat = cameraOn != leftHanded ? 1 : -1
          var transaction = Transaction()
          transaction.disablesAnimations = true
          withTransaction(transaction) {
            edgeOffset = min(1, max(0, x * direction / geometry.size.width))
          }
        }, ended: { x, velocity, cancelled in
          guard edgeActive else { return }
          let direction: CGFloat = cameraOn != leftHanded ? 1 : -1
          let complete = !cancelled && (x * direction > geometry.size.width * 0.4
            || velocity * direction >= 500)
          if complete { Haptic.tap() }
          edgeSettling = true
          withAnimation(reduceMotion ? nil : .easeOut(duration: 0.25), completionCriteria: .removed) {
            edgeOffset = complete ? 1 : 0
          } completion: {
            var transaction = Transaction()
            transaction.disablesAnimations = true
            withTransaction(transaction) {
              if complete { cameraSwitched(!cameraOn) }
              edgeOffset = 0
              edgeActive = false
              edgeSettling = false
            }
            HitchScene.event("edge wipe", then: cameraOn ? "camera" : "list")
            if !cameraOn { camera.stop() }
          }
        }))
    }
    .ignoresSafeArea(.container)
  }

  private var listNavigation: some View {
    NavigationStack {
      ScrollView {
        LazyVStack(spacing: 0) {
          // Reordering preserves each bar's gestures, undo clock and thumbnail flight.
          ForEach(orderedFriends, id: \.key) { friend in
            row(friend)
              .modifier(FlightRowLayer(bar: bars.of(friend)))
              .id(friend.key.uuidString)
              .environment(\.yoFlood, inbox.busy)
              .transaction {
                if inbox.busy {
                  $0.animation = nil
                  $0.disablesAnimations = true
                }
              }
              .transition(.opacity)
          }
          addRow.id(Self.addBlockBottomID)
        }
        .scrollTargetLayout()
      }
      // In camera mode the last bar scrolls up out from under the controls (their row and its margin).
      .contentMargins(.bottom, cameraOn && hintShown ? 54 : 0, for: .scrollContent)
      // Pinch to zoom in camera mode, as the Camera app: from the zoom at the start of the pinch, within the camera's
      // range (its widest choice up to 10x).
      .simultaneousGesture(
        MagnifyGesture()
          .onChanged { value in
            guard cameraOn, let lowest = zooms.min() else { return }
            let start = pinchStart ?? zoom
            pinchStart = start
            zoom = min(max(start * value.magnification, lowest), 10)
          }
          .onEnded { _ in pinchStart = nil },
        isEnabled: cameraOn)
      .scrollIndicators(.hidden)
      .scrollBounceBehavior(.basedOnSize)
      .scrollDismissesKeyboard(.interactively)
      .scrollPosition($position)
      .defaultScrollAnchor(.top, for: .sizeChanges)
      .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { note in
        keyboardDuration = (note.userInfo?[UIResponder.keyboardAnimationDurationUserInfoKey] as? Double) ?? 0.25
      }
      .onScrollGeometryChange(for: CGFloat.self) { $0.containerSize.height } action: { old, height in
        // Keyboard avoidance has laid out the new viewport: align the entire block's bottom, including its actions.
        if typing && adding && height < old { revealAddBlock() }
      }
      .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardDidChangeFrameNotification)) { _ in
        if typing && adding { revealAddBlock() }
      }
      .onScrollTargetVisibilityChange(idType: String.self, threshold: 0.01) { ids in
        onScreen = Set(ids)
        publish(listModel.rowsVisible(now: listNow, ids: ids))
      }
      // No bar behind the controls: they float as glass over the list, and at the top the background is the brand's
      // Amethyst exactly. The status bar keeps the system's soft edge, so the clock stays readable over the bars.
      .toolbarBackgroundVisibility(.hidden, for: .navigationBar)
      .scrollEdgeEffectStyle(.soft, for: .top)
      .onScrollGeometryChange(for: Bool.self) { $0.contentOffset.y + $0.contentInsets.top > 4 } action: { _, now in
        withAnimation(.smooth(duration: 0.3)) { scrolled = now }
      }
      .onScrollPhaseChange { _, phase in
        listScrolling = phase != .idle
        if phase != .idle && swiped != nil { withAnimation(.snappy) { swiped = nil } }  // scrolling closes it
      }
      // Amethyst behind everything; in camera mode the live picture or the picked photo.
      .backgroundPreferenceValue(ListColorKey.self) { colors in
        GeometryReader { geometry in
          ZStack(alignment: .topLeading) {
            Color.amethyst
            ForEach(colors, id: \.id) { entry in
              let bounds = entry.clips.reduce(geometry[entry.bounds]) { $0.intersection(geometry[$1]) }
              entry.color.opacity(entry.opacity)
                .animation(.easeOut(duration: 0.06), value: entry.opacity)
                .frame(width: max(0, bounds.width), height: max(0, bounds.height))
                .offset(x: bounds.isNull ? 0 : bounds.minX, y: bounds.isNull ? 0 : bounds.minY)
                .transition(.opacity)
            }
            cameraBackdrop
              .frame(width: geometry.size.width, height: geometry.size.height)
              .clipped()
              .opacity(0.55)
              .clipShape(CameraWipeShape(progress: cameraLook, left: leftHanded))
          }
          .allowsHitTesting(false)
        }
        .ignoresSafeArea()
      }
      // The camera's controls in one row at the bottom, its button in the corner opening and closing it.
      // In camera mode what to do, in glass just over the controls (at the top it would cover a bar's name).
      .overlay(alignment: .bottom) {
        VStack(spacing: 10) {
          if cameraOn && hintShown { title }
        }
        // Said only after a while in camera mode without a photo or video: 3 s idle, again after each one taken.
        .task(id: cameraOn ? hintEpoch : -1) {
          withAnimation(.smooth(duration: 0.2)) { hintShown = false }
          guard cameraOn else { return }
          try? await Task.sleep(for: .seconds(3))
          guard !Task.isCancelled else { return }
          withAnimation(.smooth(duration: 0.4)) { hintShown = true }
        }
      }
      .toolbar {
        ToolbarItemGroup(placement: .bottomBar) {
          if leftHanded {
            cameraButton
            if cameraOn || cameraPrepared { flipButton.opacity(cameraOn ? 1 : 0).allowsHitTesting(cameraOn).accessibilityHidden(!cameraOn) }
          } else { leadingControl }
        }
        ToolbarSpacer(.flexible, placement: .bottomBar)
        // The zoom as its own glass circle (a text item in the bar's shared glass turns into a capsule).
        ToolbarItem(placement: .bottomBar) {
          if (cameraOn || cameraPrepared) && picked.isEmpty {
            CameraZoom(choices: zooms, chosen: $zoom, pinching: pinchStart != nil)
              .opacity(cameraOn ? 1 : 0).allowsHitTesting(cameraOn).accessibilityHidden(!cameraOn)
              .onChange(of: zoom) { camera.zoom(to: zoom, immediate: pinchStart != nil) }
          }
        }
        .sharedBackgroundVisibility(.hidden)
        ToolbarSpacer(.flexible, placement: .bottomBar)
        ToolbarItemGroup(placement: .bottomBar) {
          if !leftHanded && (cameraOn || cameraPrepared) { flipButton.opacity(cameraOn ? 1 : 0).allowsHitTesting(cameraOn).accessibilityHidden(!cameraOn) }
          if leftHanded { leadingControl } else { cameraButton }
        }
      }
      .navigationBarTitleDisplayMode(.inline)
      .toolbarVisibility(.hidden, for: .navigationBar)  // the list from the top, no heading
    }
  }

  /// No heading over the list (the bars are the app); in camera mode what to do, in a glass capsule over the controls.
  private var title: some View {
    Text(picked.isEmpty ? "Tap or hold a name" : "Tap a name to send selected media")
      .font(.system(size: 15, weight: .semibold))
      .foregroundStyle(.white)
      .lineLimit(1)
      .fixedSize()
      .padding(.horizontal, 18)
      .frame(height: Metrics.titleHeight)
      .glassEffect(.regular, in: .capsule)
      .allowsHitTesting(false)  // the bars under it still take photos
      .transition(.opacity.combined(with: .scale(scale: 0.9)))
  }

  // MARK: Bars

  /// A bar's colour; the same colour right under itself gets a step darker each time (a run of three: colour,
  /// darker, darker still), so equal bars never melt into one block. Read top down: the first keeps its colour.
  private func shade(_ friend: Friend) -> Color {
    let list = orderedFriends
    guard let at = list.firstIndex(where: { $0.key == friend.key }) else { return .row(friend.color) }
    let own = friend.color % Color.rows.count
    var depth = 0
    var above = at - 1
    while above >= 0, list[above].color % Color.rows.count == own { depth += 1; above -= 1 }
    return depth == 0 ? .row(friend.color) : Color.row(friend.color).mix(with: .black, by: min(0.12 * Double(depth), 0.36))
  }

  @ViewBuilder private func row(_ friend: Friend) -> some View {
    let waiting = inbox.media(for: friend) ?? []
    let bar = LiveBar(
      friend: friend,
      bar: bars.of(friend),
      color: shade(friend),  // see-through over the camera
      cameraOn: cameraOn,
      recording: recordingFor == friend,
      stop: { cancelSending(friend) },
      media: waiting,
      landing: landings.slot(friend.name),
      hint: cameraOn ? "Takes and sends a photo. Use Record Video to record a video." : nil,
      hold: cameraOn && picked.isEmpty && !friend.request ? { hold(friend, $0, origin: $1) } : nil,
      cancelHold: {
        guard recordingFor == friend else { return }
        recordingChanged(nil)
        Task {
          if let file = await camera.stopVideo() { Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: file) } }
        }
      },
      press: { pressed(friend.key, $0) },
      flightChanged: { id, active in
        if active { bars.flying.insert(id) } else { bars.flying.remove(id) }
        flushRise()
      },
      flyBegan: { id in
        HitchScene.event("photo stack fly", then: cameraOn ? "camera" : "list")
        bars.awaitingThumbnail.remove(id)
        flushRise()
      },
      view: { see(friend, $0) }
    ) { origin in
      send(to: friend, origin: origin)
    }
    // Swiped left (not in camera mode, where the bars take photos): Block and Remove underneath, as in Mail.
    SwipeActions(
      open: Binding(
        get: { swiped == friend.key },
        set: { open in swiped = open ? friend.key : swiped == friend.key ? nil : swiped }),
      enabled: !cameraOn,
      request: friend.request ? { yes in chooseRequest(friend, yes: yes) } : nil,
      actionsEnabled: friend.group == nil,
      mirrored: leftHanded,
      activity: lastActivity(for: friend),
      began: { if swiped != friend.key { withAnimation(.snappy) { swiped = nil } } },  // another one closes
      block: { blocking = (friend, false) },
      remove: { remove(friend) }
    ) {
      // One view whatever the mode, so the camera only makes the bars see-through and rebuilds nothing: no menu while
      // holding records a video or shows the photos.
      VStack(spacing: 0) {
        // The bar itself lifts on a long press: square and nearly full width (a separate preview got the menu's
        // rounded, inset platter). It carries its own colour under it, so the lifted bar is bright, not the list's
        // shared colour layer left behind; in camera mode the bars stay see-through.
        bar.equatable()
          .background(cameraOn ? Color.clear : shade(friend))
          .contentShape(.contextMenuPreview, Rectangle())
          .contextMenu {
            if !cameraOn && !friend.request { menu(for: friend) }
          }
        if friend.request {
          requestChoices(friend)
            .frame(height: Metrics.rowHeight)
        }
      }
        .accessibilityActions {
          if friend.request {
            Button("Accept and Yo") { chooseRequest(friend, yes: true) }
            Button("Decline") { chooseRequest(friend, yes: false) }
          }
        }  // also with photos waiting: a tap opens them
    }
  }

  private func requestChoices(_ friend: Friend) -> some View {
    GeometryReader { geometry in
      HStack(spacing: 0) {
        if leftHanded { requestChoice(friend, yes: true, width: geometry.size.width * 2 / 3) }
        requestChoice(friend, yes: false, width: geometry.size.width / 3)
        if !leftHanded { requestChoice(friend, yes: true, width: geometry.size.width * 2 / 3) }
      }
    }
  }

  private func requestChoice(_ friend: Friend, yes: Bool, width: CGFloat) -> some View {
    Button { chooseRequest(friend, yes: yes) } label: {
      Text(yes ? "Yo" : "No")
        .font(.yo(Metrics.rowFont))
        .foregroundStyle(.white)
        .frame(width: width)
        .frame(maxHeight: .infinity)
        .background { addBackground(Color(hex: yes ? 0x2ECC71 : 0xE74C3C)) }
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(yes ? "Accept and Yo" : "Decline")
  }

  private func lastActivity(for friend: Friend) -> BarActivity {
    let out = Stats.lastSent(to: friend.name), back = Stats.lastReceived(from: friend.name)
    let mine = (out ?? .distantPast) >= (back ?? .distantPast)
    let date = mine ? out : back
    let text = date.map { let age = Arrival.age($0); return age == "now" ? "Yo now" : "Yo \(age) ago" } ?? "No Yo yet"
    return BarActivity(text: text, icon: date == nil ? "clock" : mine ? "arrow.up" : "arrow.down",
      accessibility: date == nil ? text : "Last Yo \(mine ? "sent" : "received"), \(text)")
  }

  /// A bar's menu: a friend's Report, Block and Remove; a group's Members, Rename and Leave.
  @ViewBuilder private func menu(for friend: Friend) -> some View {
    // The last Yo (a group's too), whichever way it went, in one greyed row at the menu's top: "↑ Yo 7m ago" sent by me,
    // "↓ Yo 1d ago" from them (up and down as upload and download).
    let activity = lastActivity(for: friend)
    let run = bars.of(friend)
    // The photo stack's exact number (its thumbnail shows only a stack).
    let stacked = (run.outgoing.isEmpty ? run.sendingBatch?.items ?? [] : run.outgoing)
      .filter { $0.text.hasSuffix("📷") || $0.text.hasSuffix("🎥") }.count
    if stacked > 0 {
      Button(stacked == 1 ? "1 photo waiting to send" : "\(stacked) photos waiting to send", systemImage: "photo.stack") {}
        .disabled(true)
    }
    if run.modelTitle == "Sent Yo", run.modelCount > 0 {
      Button("Sent Yo \(run.modelCount) times") {}.disabled(true)
    }
    let received = landings[friend.name]?.total ?? run.modelMarkCount
    if received > 0 {
      Button("Yo \(received) times") {}.disabled(true)
    }
    let lastRow = Button(activity.text, systemImage: activity.icon) {}.disabled(true)
      .accessibilityLabel(activity.accessibility)
    if friend.group != nil {
      Group {
        AnyView(Group {
          lastRow
          Button("Members", systemImage: "person.3") { showMembers(friend) }
        })
        AnyView(Button("Rename", systemImage: "pencil") {
          newGroupName = friend.name
          renaming = friend
        })
        AnyView(Button("Leave \(friend.name)", systemImage: "rectangle.portrait.and.arrow.right", role: .destructive) {
          leaving = friend
        })
      }
    } else {
    Group {
      AnyView(lastRow)
      AnyView(Menu("Block & Report", systemImage: "hand.raised") {
        Button("Block", systemImage: "hand.raised") { blocking = (friend, false) }
        Section("Report") {
          Button("Spam") { report(friend, .spam) }
          Button("Abuse") { report(friend, .abuse) }
          Button("Something Else") { report(friend, .other) }
        }
      })
      AnyView(Button("Remove", systemImage: "trash", role: .destructive) { remove(friend) })
    }
  }
  }

  // MARK: Groups

  /// Who is in a group, to add and remove (GroupMembersSheet).
  private func showMembers(_ bar: Friend) { membersOf = bar }

  private func renameGroup(_ bar: Friend, to name: String) {
    let name = name.trimmingCharacters(in: .whitespaces)
    guard let id = bar.group, let client = accounts.client, !name.isEmpty else { return }
    Task {
      guard let events = try? await Hub.run({ try client.renameGroup(group: id, name: Self.groupName(name)) }) else { return }
      await inbox.absorb(events, client: client, friends: friends)
      if let groups = try? await Hub.run({ try client.groups() }) { friends.takeGroups(groups) }
      let renamed = friends.all.first { $0.group == id } ?? bar
      await GroupNotice.send(
        [.user(GroupNotice.me(accounts)), " renamed the group to ", .group(renamed.name)], to: renamed, accounts: accounts, outbox: outbox)
    }
  }

  /// A group's name as the core takes it: at most 32 bytes of UTF-8, cut at a whole character.
  nonisolated static func groupName(_ name: String) -> String {
    var cut = ""
    for character in name where cut.utf8.count + String(character).utf8.count <= 32 { cut.append(character) }
    return cut
  }

  private func leave(_ bar: Friend) {
    guard let id = bar.group, let client = accounts.client else { return }
    Task {
      // "LEO left" first, and out only once it went (at most half a minute): a member who left sends nothing.
      if let notice = await GroupNotice.send([.user(GroupNotice.me(accounts)), " left ", .group(bar.name)], to: bar, accounts: accounts, outbox: outbox) {
        for _ in 0..<60 where outbox.isWaiting(notice) { try? await Task.sleep(for: .milliseconds(500)) }
      }
      if let events = try? await Hub.run({ try client.leaveGroup(group: id) }) {
        await inbox.absorb(events, client: client, friends: friends)
      }
      if let groups = try? await Hub.run({ try client.groups() }) { withAnimation(.snappy) { friends.takeGroups(groups) } }
    }
  }

  /// A group of this account alone, deleted: left without a picture (nobody would see it), its bar gone.
  private func deleteEmpty(_ bar: Friend) {
    guard let id = bar.group, let client = accounts.client else { return }
    Task {
      if let events = try? await Hub.run({ try client.leaveGroup(group: id) }) {
        await inbox.absorb(events, client: client, friends: friends)
      }
      if let groups = try? await Hub.run({ try client.groups() }) { withAnimation(.snappy) { friends.takeGroups(groups) } }
    }
  }

  private func remove(_ friend: Friend) {
    withAnimation(.snappy) {
      swiped = nil
      publish(listModel.delete(now: listNow, id: friend.key.uuidString))
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

  private func revealAddBlock() {
    withAnimation(.easeOut(duration: keyboardDuration)) {
      position.scrollTo(id: Self.addBlockBottomID, anchor: .bottom)
    }
  }


  private func beginAdding() {
    submitted += 1
    hint = nil
    newName = ""
    swiped = nil
    adding = true
    typing = true
    revealAddBlock()
  }

  private var addRow: some View {
    VStack(spacing: 0) {
      if adding {
        TextField(hint ?? "Name", text: $newName)
          .font(.yo(Metrics.rowFont))
          .foregroundStyle(.white)
          .multilineTextAlignment(.center)
          .textInputAutocapitalization(.words)
          .autocorrectionDisabled()
          .submitLabel(.done)
          .focused($typing)
          .onSubmit(add)
          .onChange(of: newName) { _, value in
            let clean = Username.clean(value)
            if clean != value { newName = clean }
          }
          .frame(height: Metrics.rowHeight)
          .background { addBackground(Color.row(friends.nextColor)) }
          .onAppear { typing = true }
        HStack(spacing: 0) {
          Button {
            typing = false
            adding = false
            newGroup = true
          } label: {
            addAction("Group", icon: "person.2.badge.plus", color: Color(hex: 0x3498DB))
          }
          if let account = accounts.account {
            InviteShareLink(name: account.username) {
              addAction("Invite", icon: "square.and.arrow.up", color: .amethyst)
            }
          }
        }
        .buttonStyle(.plain)
      } else {
        // Until the first friend is on the list (bots don't count) the row says what it does; nothing pops over it.
        RowButton(
          friends.all.contains(where: { !AddFriendHint.bots.contains($0.name.lowercased()) }) ? "+" : "+ Add a Friend",
          color: Color.row(friends.nextColor), cameraOn: cameraOn
        ) { _ in
          beginAdding()
        }
        .accessibilityLabel("Add Friend")
      }
    }
    .transaction { $0.animation = nil }
    .animation(reduceMotion ? nil : ListMotion.expansion) { content in
      content.frame(height: Metrics.rowHeight * (adding ? 2 : 1), alignment: .top).clipped()
        .modifier(ListColorClip())
    }
    .onChange(of: typing) { _, active in
      HitchScene.event("add block", then: "list")
      if !active { adding = false }
    }
  }

  private func addAction(_ title: String, icon: String, color: Color) -> some View {
    Label(title, systemImage: icon)
      .font(.yo(22))
      .foregroundStyle(.white)
      .frame(maxWidth: .infinity)
      .frame(height: Metrics.rowHeight)
      .background { addBackground(color) }
      .contentShape(Rectangle())
  }

  @ViewBuilder private func addBackground(_ color: Color) -> some View {
    Color.clear.modifier(ListColor(color: color))
  }

  private var settingsButton: some View {
    Button { settings = true } label: {
      // Y and o drawn apart, the o tucked under the Y's right arm, so the glass button stays round.
      HStack(alignment: .lastTextBaseline, spacing: -5) {
        Text("Y").font(.yo(29))
        Text("o").font(.yo(23))
      }
      .fixedSize()
      .frame(width: 32, height: 32)
    }
      .overlay(alignment: .topTrailing) { SupportBadge().offset(x: 10, y: -8) }
      .accessibilityLabel("Settings")
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
      if newName.isEmpty { adding = false }
    }
  }

  /// The name of an invite link: added the same way; what is wrong shows in Add Friend's field.
  private func invited(_ name: String) async {
    guard let problem = await addFriend(name) else { return }
    withAnimation(.snappy) { adding = true }
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
        position.scrollTo(id: known.key.uuidString, anchor: .center)
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

  @ViewBuilder private var cameraBackdrop: some View {
    if cameraOn, picked.count > 1 {
      GeometryReader { geometry in
        let columns = picked.count <= 4 ? 2 : 3
        let side = (geometry.size.width - CGFloat(columns - 1)) / CGFloat(columns)
        let rows = (picked.count + columns - 1) / columns
        VStack(spacing: 1) {
          ForEach(0..<rows, id: \.self) { row in
            HStack(spacing: 1) {
              ForEach(0..<columns, id: \.self) { column in
                let index = row * columns + column
                if index < picked.count {
                  Image(uiImage: picked[index].image)
                    .resizable().scaledToFill()
                    .frame(width: side, height: side)
                    .clipped()
                } else {
                  Color.black.frame(width: side, height: side)
                }
              }
            }
          }
        }
        .frame(width: geometry.size.width, height: geometry.size.height)
      }
      .background(.black)
    } else if cameraOn, let item = picked.first {
      Image(uiImage: item.image).resizable().scaledToFill()
    } else {
      CameraPreview(camera: camera)
    }
  }

  @ViewBuilder private var leadingControl: some View {
    ZStack {
      settingsButton.opacity(cameraOn ? 0 : 1).allowsHitTesting(!cameraOn).accessibilityHidden(cameraOn)
      if cameraOn || cameraPrepared {
        PhotosPicker(selection: $libraryItems, maxSelectionCount: 10, selectionBehavior: .ordered,
                   matching: .any(of: [.images, .videos])) {
          Label("Photo Library", systemImage: "photo.on.rectangle")
        }
        .opacity(cameraOn ? 1 : 0).allowsHitTesting(cameraOn).accessibilityHidden(!cameraOn)
      }
    }
  }

  private var flipButton: some View {
    Button {
      Haptic.tap(.light)
      camera.flip()
      Task { await readZooms() }
    } label: { Label("Switch Camera", systemImage: "arrow.triangle.2.circlepath") }
  }

  private var cameraButton: some View {
    Button {
      withAnimation(reduceMotion ? nil : .easeOut(duration: 0.25)) {
        if cameraOn && !libraryItems.isEmpty {
          picked = []
          libraryItems = []
        } else { cameraSwitched(!cameraOn) }
      }
    } label: {
      Label(cameraOn ? (libraryItems.isEmpty ? "Close Camera" : "Clear Selection") : "Camera",
            systemImage: cameraOn ? "xmark" : "camera.fill")
    }
    .font(.system(size: 30))
    .background {
      if cameraPrepared { Image(systemName: "xmark").font(.system(size: 30)).hidden() }
    }
  }

  private func switchCamera(_ on: Bool) async {
    guard on else { return camera.stop() }
    _ = Self.outgoingThumbnailQueue  // enqueue decode warm-up before camera startup/capture
    // Asked first: closed meanwhile, it stays closed.
    if Camera.authorized { if cameraOn { camera.start() } }
    else if await Camera.allowed() { if cameraOn { camera.start() } } else { cameraSwitched(false) }
    await readZooms()
  }

  /// The zooms of the camera now in use, at "1" (as it starts, and after a flip).
  private func readZooms() async {
    zooms = await camera.zoomChoices()
    zoom = 1
  }

  // MARK: Sending

  private func send(to friend: Friend, origin: CGPoint?) {
    // A group's bar just shrank back (its senders' lines went): the bars under it moved up under the finger, so a tap
    // on one of them in the first half second goes nowhere rather than to whoever slid there. The group's own bar
    // and those above it stay where they were: they take taps (to fire on).
    if let lock = tapLock, Date.now < lock.until, let group = friends.shown.firstIndex(where: { $0.key == lock.bar }),
      let tapped = friends.shown.firstIndex(where: { $0.key == friend.key }), tapped > group
    {
      return
    }
    // A request asks first (spec 4.10): nothing goes to them before a yes.
    if friend.request { return }

    // Firing back into a group whose senders' lines stand: they stay a while longer (its time starts again), so the
    // bar does not shrink under the finger.
    if friend.group != nil { publish(listModel.keepAlive(now: listNow, id: friend.key.uuidString)) }
    hintEpoch += 1  // the camera's hint goes, and waits again
    Perf.mark("tap.action")
    Perf.mark("thumb.tap")
    defer { PerfProbe.frame("tap") }
    // A tap while a bar is swiped open closes it, as in Mail; it sends nothing. So does one while "+" is open.
    guard swiped == nil else { return withAnimation(.snappy) { swiped = nil } }
    // Here, as the finger lifts: not after the list has drawn the change. A photo: the shutter's click.
    if cameraOn && picked.isEmpty { Haptic.shutter() } else { Haptic.tap() }
    if !cameraOn { Haptic.sendClick() }
    let library = cameraOn ? picked : []
    // "Undo Before Sending" (Per Contact): the Yo waits under the ring as a photo does; a tap on it keeps it here.
    if !cameraOn && library.isEmpty, let own = friends.all.first(where: { $0.key == friend.key }), own.undoFirst {
      let (inbox, accounts, friends) = (inbox, accounts, friends)
      run(friend, "Sent Yo", seconds: own.undoSeconds) { _ in
        Prepared(
          send: {
            if let problem = await inbox.send(to: friend, accounts: accounts, friends: friends) {
              if problem == Inbox.groupEmpty { emptyGroup = friend }
              showSent(to: friend, problem)
              return false
            }
            return true
          },
          undo: {})
      }
      return
    }
    guard cameraOn || !library.isEmpty else {
      showSent(to: friend, "Sent Yo", thenOnTop: true)
      Task {
        if let problem = await inbox.send(to: friend, accounts: accounts, friends: friends) {
          if problem == Inbox.groupEmpty { emptyGroup = friend }
          showSent(to: friend, problem)
        } else {
          await sentYo()
        }
      }
      return
    }
    let client = accounts.client  // the account tapped in: a photo waiting its 1.5 seconds never goes from the next one
    let at = Date.now
    if !library.isEmpty {
      for item in library {
        run(friend, item.video == nil ? "Sent 📷" : "Sent 🎥", origin: origin) { thumbnail in
          if let image = item.thumbnail { thumbnail(image) }
          let file: MediaFile?
          if let video = item.video {
            file = video
          } else {
            let data = item.data
            file = await Task.detached { MediaFile.photo(data) }.value
          }
          guard !Task.isCancelled, let file else { return nil }
          return await stage(file, to: friend, from: client, at: at)
        }
      }
    } else {
      run(friend, "Sent 📷", origin: origin, placeholder: camera.previewThumbnail()) { thumbnail in
        guard let taken = await camera.photo(), !Task.isCancelled else { return nil }
        Perf.mark("thumb.capture.done")
        if let image = await Self.outgoingThumbnail(taken) { thumbnail(image) }
        guard !Task.isCancelled, let file = await Task.detached(operation: { MediaFile.photo(taken) }).value,
          !Task.isCancelled else { return nil }
        return await stage(file, to: friend, from: client, at: at)
      }
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

  /// Keep only the prepared bytes during undo. Sealing, disk storage and upload begin after the delay.
  private func stage(
    _ file: MediaFile, to friend: Friend, from client: YoClient?, at: Date
  ) async -> Prepared? {
    guard !Task.isCancelled, let client, client === accounts.client else { return nil }
    let outbox = outbox
    return Prepared(
      send: {
        guard client === accounts.client else { return false }
        let sent = await outbox.sendOnce(file, to: friend.target, from: client, accounts: accounts, at: at)
        if sent { Stats.sentYo(to: friend.name) }
        return sent
      },
      undo: {})
  }

  private func recordingChanged(_ friend: Friend?) {
    recordingFor = friend
    publish(listModel.recording(now: listNow, id: friend?.key.uuidString))
  }

  /// Holding a name in camera mode: a video from the moment it is held until it is let go.
  private func hold(_ friend: Friend, _ holding: Bool, origin: CGPoint?) {
    hintEpoch += 1
    if holding {
      guard recordingFor == nil else { return }  // one video at a time: a second finger records nothing
      recordingChanged(friend)
      camera.startVideo()
    } else if recordingFor == friend {
      recordingChanged(nil)
      Haptic.tap()
      let client = accounts.client
      let at = Date.now
      run(friend, "Sent 🎥", origin: origin, placeholder: camera.previewThumbnail()) { thumbnail in
        guard let recording = await camera.stopVideo() else { return nil }
        Perf.mark("thumb.capture.done")
        defer { Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: recording) } }
        guard !Task.isCancelled else { return nil }
        return await withTaskCancellationHandler {
          if let image = await Self.outgoingVideoThumbnail(recording) { thumbnail(image) }
          guard !Task.isCancelled, let file = await MediaFile.video(recording), !Task.isCancelled else { return nil }
          return await stage(file, to: friend, from: client, at: at)
        } onCancel: {
          try? FileManager.default.removeItem(at: recording)
        }
      }
    }
  }

  /// A photo or video on its way out joins the friend's stack: every one in it waits until `sendDelay` has passed
  /// since the newest (each new one starts the time again, the ring after the words showing it); then the stack goes,
  /// in order. Meanwhile `prepare` takes the photo or ends the video. A tap on the ring keeps the whole stack here.
  private func run(
    _ friend: Friend, _ text: String, seconds: Double = FriendList.sendDelay, origin: CGPoint? = nil, placeholder: UIImage? = nil,
    prepare: @escaping @MainActor (@MainActor (UIImage) -> Void) async -> Prepared?
  ) {
    let bars = bars  // the object itself into the task, not this view's copy of the state
    let one = Outgoing(text: text, thumbnail: placeholder, origin: origin)
    let bar = bars.of(friend)
    bar.seconds = seconds
    bar.outgoing.append(one)
    bar.countdownStarted = nil
    bar.sendingBatch = (bar.outgoing.first!.id, bar.outgoing.count, bar.outgoing)
    let snapshot = listModel.sendStarted(now: listNow, id: friend.key.uuidString,
      kind: text.hasSuffix("🎥") ? .video : (text.hasSuffix("📷") ? .photo : .yo),
      undoMs: UInt64(seconds * 1000))
    publish(snapshot)
    bars.awaitingThumbnail.insert(one.id)
    flushRise()
    bars.preparing[one.id] = Task {
      defer {
        if !bar.outgoing.contains(where: { $0.id == one.id && $0.thumbnail != nil }) {
          bars.awaitingThumbnail.remove(one.id)
          Task { await Task.yield(); flushRise() }
        }
      }
      return await prepare { image in
        Perf.mark("thumb.thumbnail.ready")
        // Capture may finish after the undo queue has moved into sending.
        if let index = bar.sendingBatch?.items.firstIndex(where: { $0.id == one.id }) {
          bar.sendingBatch?.items[index].thumbnail = image
        }
        guard let index = bar.outgoing.firstIndex(where: { $0.id == one.id }) else { return }
        // Superseded thumbnails no longer have a view to report their fly-in.
        for previous in bar.outgoing.prefix(index) {
          bars.awaitingThumbnail.remove(previous.id)
        }
        bar.outgoing[index].thumbnail = image
        if bar.outgoing.dropFirst(index + 1).contains(where: { $0.thumbnail != nil }) {
          bars.awaitingThumbnail.remove(one.id)
          Task { await Task.yield(); flushRise() }
        }
      }
    }
    let key = friend.key
    let round = UUID()
    bars.stackRounds[key] = round
    Task {
      // Capture and fly-in belong to this same pending batch, even if the core's
      // presentation deadline expires while they are in progress.
      while bar.outgoing.contains(where: { bars.awaitingThumbnail.contains($0.id) || bars.flying.contains($0.id) }) {
        try? await Task.sleep(for: .milliseconds(20))
        guard bars.stackRounds[key] == round else { return }
      }
      guard bars.stackRounds[key] == round else { return }
      bar.countdownStarted = .now
      try? await Task.sleep(for: .seconds(seconds))
      guard bars.stackRounds[key] == round, !bar.outgoing.isEmpty else { return }  // another came, or undone
      let stack = bar.outgoing
      let batchID = bar.sendingBatch?.id
      bars.stackRounds[key] = nil
      bar.outgoing = []
      bar.countdownStarted = nil
      publish(listModel.tick(now: listNow))
      for one in stack { bars.awaitingThumbnail.remove(one.id) }
      flushRise()
      // After the stack before (a video still being finished): the friend gets them in the order taken.
      let before = bars.releasing[key]
      let releasing = Task {
        await before?.value
        var failed = false
        for one in stack {
          guard let prepared = await bars.preparing.removeValue(forKey: one.id)?.value else {
            failed = true
            continue
          }
          if !(await prepared.send()) { failed = true }
        }
        // Starts already counted every item. Finish the batch once, without replaying those counts.
        if bar.sendingBatch?.id == batchID { bar.sendingBatch = nil }
        showSent(to: friend, failed ? "Not Sent" : (stack.last?.text ?? "Not Sent"), thenOnTop: !failed)
      }
      bars.releasing[key] = releasing
      await releasing.value
      if bars.releasing[key] == releasing { bars.releasing[key] = nil }
    }
  }

  /// One persistent worker: its first job exercises the exact JPEG/ImageIO path before capture.
  nonisolated private static let outgoingThumbnailQueue: DispatchQueue = {
    let queue = DispatchQueue(label: "yo.outgoing-thumbnail", qos: .userInitiated)
    queue.async {
      // Keep all Core Graphics/UIKit objects local to the worker's Sendable closure.
      guard let context = CGContext(data: nil, width: 2, height: 2, bitsPerComponent: 8,
        bytesPerRow: 8, space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue),
        let pixels = context.makeImage(), let data = UIImage(cgImage: pixels).jpegData(compressionQuality: 0.8)
      else { return }
      _ = decodeOutgoingThumbnail(data)
    }
    return queue
  }()

  /// Capture bytes go straight to ImageIO, before any media file is written.
  nonisolated private static func outgoingThumbnail(_ data: Data) async -> UIImage? {
    await withCheckedContinuation { continuation in
      outgoingThumbnailQueue.async {
        continuation.resume(returning: decodeOutgoingThumbnail(data))
      }
    }
  }

  nonisolated private static func decodeOutgoingThumbnail(_ data: Data) -> UIImage? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceThumbnailMaxPixelSize: 360,
      kCGImageSourceShouldCacheImmediately: true,
    ]
    return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
  }

  /// Read the first frame before export, while the undo time is running. Export owns deletion of the recording.
  nonisolated private static func outgoingVideoThumbnail(_ url: URL) async -> UIImage? {
    await Task.detached(priority: .userInitiated) { () async -> UIImage? in
      do {
        let generator = AVAssetImageGenerator(asset: AVURLAsset(url: url))
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width: 360, height: 360)
        generator.requestedTimeToleranceBefore = .zero
        generator.requestedTimeToleranceAfter = .zero
        let frame = try await generator.image(at: .zero)
        return UIImage(cgImage: frame.image)
      } catch {
        return nil
      }
    }.value
  }

  /// Prepared bytes, without an outbox entry or media key; `send` starts the actual attempt.
  struct Prepared {
    let send: @MainActor () async -> Bool
    let undo: @MainActor () -> Void
  }

  /// The ring tapped: the whole stack stays here.
  private func cancelSending(_ friend: Friend) {
    let bar = bars.of(friend)
    for one in bar.outgoing {
      // Cancel capture/export work; no outbox entry or media key exists during undo.
      bars.awaitingThumbnail.remove(one.id)
      if let preparing = bars.preparing.removeValue(forKey: one.id) {
        preparing.cancel()
        Task { await preparing.value?.undo() }
      }
    }
    bars.stackRounds[friend.key] = nil
    bar.sendingBatch = nil
    bar.outgoing = []
    bar.countdownStarted = nil
    showSent(to: friend, "Canceled")
  }

  /// Network/capture handlers report their result; text, counts and lifetime are core-owned.
  private func showSent(to friend: Friend, _ text: String, thenOnTop: Bool = false) {
    if text == "Not Sent", cameraOn { SendFailure.shared.show(name: friend.name, retry: { outbox.retryNow() }) }
    let id = friend.key.uuidString
    let snapshot: ListSnapshot
    switch text {
    case "Canceled": snapshot = listModel.sendUndone(now: listNow, id: id)
    case "Not Sent": snapshot = listModel.sendFailed(now: listNow, id: id, text: text)
    case "Sent Yo" where thenOnTop: snapshot = listModel.sendFinished(now: listNow, id: id, kind: .yo)
    case "Sent 📷" where thenOnTop: snapshot = listModel.sendFinished(now: listNow, id: id, kind: .photo)
    case "Sent 🎥" where thenOnTop: snapshot = listModel.sendFinished(now: listNow, id: id, kind: .video)
    default: snapshot = listModel.notice(now: listNow, id: id, text: text, rise: thenOnTop)
    }
    withAnimation(.snappy(duration: 0.2)) { publish(snapshot) }
  }

  // MARK: Seeing

  /// A finger on a bar's thumbnail plays its photos and videos, oldest first. Held, letting go closes them; a short
  /// tap lets them run to the end (held: paused; a tap on the left third: back; elsewhere: on). What was shown is gone.
  private func see(_ friend: Friend, _ touch: ThumbnailTouch) {
    switch touch {
    case .down:
      shown = []
      // Shown under the bar's name (a group's photos are kept by its id).
      // A group's as "LEO in FAMILY": who sent it, and where.
      viewing = inbox.media(for: friend)?.map {
        let user = Inbox.sender(of: $0)
        let name = user.flatMap { u in inbox.memberNames[u] ?? friends.all.first { $0.user == u }?.name }
        let from = friend.group == nil ? friend.name : "\(name ?? "Someone") in \(friend.name)"
        return Arrival(id: $0.id, from: from, photo: $0.photo, video: $0.video, file: $0.file)
      }
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
    syncListMedia()
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
      .onAppear {
        publish(listModel.foreground(now: listNow, active: scenePhase == .active))
        #if YO_DEV
        HitchMonitor.shared.start()
        HitchMonitor.shared.scene = "list"
        #endif
        PerfProbe.install()
        Haptic.prepareArrival()
      }
      .task {
        try? await Task.sleep(for: .milliseconds(700))
        while listScrolling && !Task.isCancelled {
          try? await Task.sleep(for: .milliseconds(250))
        }
        guard !Task.isCancelled, !cameraPrepared, !edgeActive, !cameraOn,
              scenePhase == .active, Camera.authorized else { return }
        HitchScene.event("camera prepare", then: "list")
        await camera.prepare()
        guard !Task.isCancelled, !edgeActive, !cameraOn, scenePhase == .active else { return }
        await readZooms()
        cameraPrepared = true
      }
      // While the list is on screen: Yos come in from the hub.
      .task { await inbox.listen(accounts: accounts, friends: friends) }
      .onChange(of: listFriends, initial: true) { syncListFriends() }
      .onChange(of: inbox.memberNames) { syncSenderNames() }
      .onChange(of: scenePhase, initial: true) { publish(listModel.foreground(now: listNow, active: scenePhase == .active)) }
      // Back after more than 30 s away: the list again, not the camera (a quick app switch keeps the camera).
      .onChange(of: scenePhase) { _, phase in
        if phase == .background { leftAt = .now }
        if phase == .active, let away = leftAt {
          leftAt = nil
          if cameraOn, Date.now.timeIntervalSince(away) > 30 { cameraSwitched(false) }
        }
      }
      .onChange(of: libraryItems) { _, items in load(items) }
      .onDisappear {
        cameraSwitched(false)
        onScreen = []
        publish(listModel.rowsVisible(now: listNow, ids: []))
        publish(listModel.foreground(now: listNow, active: false))
        clocks.stop()
      }
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
      .onChange(of: cheers) { Haptic.arrival() }
      .sensoryFeedback(.start, trigger: recordingFor) { (_: Friend?, new: Friend?) in new != nil }
      .sensoryFeedback(.selection, trigger: viewing) { (old: [Arrival]?, new: [Arrival]?) in old == nil && new != nil }
  }

  private func animations(_ content: some View) -> some View {
    content
      .preferredColorScheme(.dark)  // white status bar and bar items on purple
  }

  /// New Yos, as many as came since the last batch (`Inbox.batchSeconds`): one step for all of them, whatever their
  /// number. One haptic; each sender's bar goes on top once and counts all its Yos at once (one "!" for each); then
  /// the clocks start once.
  private func arrived() {
    Perf.timed("list.arrived") { applyArrivals() }
  }

  private func applyArrivals() {
    let batch = inbox.takeArrivals()
    guard !batch.isEmpty else { return }
    syncListFriends()
    cheers += 1
    withAnimation(inbox.busy ? nil : .snappy(duration: 0.2)) {
      for arrival in batch {
        guard let friend = friends.shownFriend(named: arrival.from) else { continue }
        publish(listModel.received(now: listNow, id: friend.key.uuidString,
          kind: arrival.video != nil ? .video : (arrival.photo != nil ? .photo : .yo), token: arrival.id,
          sender: groupSender(arrival, friend: friend)))
      }
    }
    syncListMedia()
  }

  private func groupSender(_ arrival: Arrival, friend: Friend) -> String? {
    guard friend.group != nil else { return nil }
    // Plain group Yo ids are group/user/time; media ids start with group+user.
    let user = Inbox.sender(of: arrival) ?? arrival.id.split(separator: "/").dropFirst().first.map(String.init)
    if let user, let name = inbox.memberNames[user] ?? friends.all.first(where: { $0.user == user })?.name {
      publish(listModel.senderNamed(now: listNow, user: user, name: name))
    }
    return user
  }

  private func syncSenderNames() {
    var names = Dictionary(friends.all.compactMap { friend in friend.user.map { ($0, friend.name) } },
      uniquingKeysWith: { first, _ in first })
    names.merge(inbox.memberNames) { _, known in known }
    for (user, name) in names where name != "?" {
      publish(listModel.senderNamed(now: listNow, user: user, name: name))
    }
  }

  private func syncListFriends() {
    syncSenderNames()
    publish(listModel.friends(now: listNow, rows: listFriends))
    syncListMedia()
  }

  private func syncListMedia() {
    for friend in friends.shown {
      let id = friend.key.uuidString
      let videos = (inbox.media(for: friend) ?? []).map { $0.video != nil }
      guard (listMedia.each[id] ?? []) != videos else { continue }
      listMedia.each[id] = videos
      publish(listModel.mediaChanged(now: listNow, id: id, kinds: videos.map { $0 ? .video : .photo }))
    }
  }

  /// Project core state into per-row observable render slots; gestures and artwork stay here.
  private func publish(_ snapshot: ListSnapshot) {
    DeleteNotice.shared.receive = { publish($0) }
    DeleteNotice.shared.banner = snapshot.banner
    DeleteNotice.shared.deletion = snapshot.deletion
    DeleteNotice.shared.undo = snapshot.deletion == nil ? nil : {
      withAnimation(.snappy) { publish(listModel.undoDelete(now: listNow)) }
    }
    for id in snapshot.committedDeletes {
      if let friend = friends.all.first(where: { $0.key.uuidString == id }) { friends.remove(friend) }
    }
    let now = Date.now
    let order = snapshot.rows.map(\.id)
    let anchor = snapshot.lockedRow ?? lockedRow
    if order != listOrder {
      let hadOrder = !listOrder.isEmpty
      withAnimation(inbox.busy || reduceMotion ? nil : ListMotion.expansion) {
        // With no alignment anchor, an already visible target does not jump. SwiftUI
        // preserves that identity through the reorder, including content inserted above it.
        if let anchor { position.scrollTo(id: anchor) }
        else if !scrolled { position.scrollTo(edge: .top) }
        listOrder = order
        if hadOrder && snapshot.deletion == nil && snapshot.committedDeletes.isEmpty { friends.promoteBatch(order.reversed().compactMap(UUID.init(uuidString:))) }
      }
    }
    if snapshot.lockedRow != lockedRow, let id = snapshot.lockedRow {
      var transaction = Transaction(); transaction.disablesAnimations = true
      withTransaction(transaction) { position.scrollTo(id: id) }
    }
    lockedRow = snapshot.lockedRow
    let friendsByID = Dictionary(friends.shown.map { ($0.key.uuidString, $0) }, uniquingKeysWith: { a, _ in a })
    for row in snapshot.rows {
      guard let friend = friendsByID[row.id] else { continue }
      let bar = bars.of(friend)
      let sending = !bar.outgoing.isEmpty || !row.sending ? bar.sendingBatch : nil
      let title = sending == nil ? row.title : "Sent Yo"
      let count = sending?.count ?? Int(row.count)
      if bar.modelTitle != title { bar.modelTitle = title }
      if bar.modelCount != count { bar.modelCount = count }
      if bar.modelMarkCount != Int(row.markCount) { bar.modelMarkCount = Int(row.markCount) }
      if bar.modelMediaSign != row.mediaSign { bar.modelMediaSign = row.mediaSign }
      if bar.hasNotice != (row.noticeDeadline != nil) { bar.hasNotice = row.noticeDeadline != nil }
      let paused = !row.clockRunning
      let pauseChanged = bar.modelPaused != paused
      if pauseChanged { bar.modelPaused = paused }
      let hadSenders = !bar.modelSenders.isEmpty
      let senders = row.senders.map { sender in
        RenderedSender(user: sender.user, name: sender.name, count: Int(sender.count),
          elapsed: sender.elapsed, at: now.addingTimeInterval(-Double(sender.elapsed) / 1000))
      }
      if pauseChanged || bar.modelSenders.map(\.elapsed) != senders.map(\.elapsed)
        || bar.modelSenders.map(\.user) != senders.map(\.user)
        || bar.modelSenders.map(\.name) != senders.map(\.name)
        || bar.modelSenders.map(\.count) != senders.map(\.count) { bar.modelSenders = senders }
      let previous = landings[friend.name]
      if row.yoCount > 0 {
        var landing = Landing(first: row.yoToken, count: Int(row.yoCount), total: Int(row.yoTotal))
        landing.paused = paused
        landing.fadeElapsed = Double(row.fadeElapsed ?? 0) / 1000
        if let elapsed = row.fadeElapsed {
          let date = now.addingTimeInterval(-Double(elapsed) / 1000)
          landing.fading = previous?.fading.flatMap { abs($0.timeIntervalSince(date)) < 0.05 ? $0 : nil } ?? date
        }
        if landing != previous { landings[friend.name] = landing }
      } else if previous != nil {
        landings[friend.name] = nil
        if friend.group != nil, hadSenders { tapLock = (friend.key, now + 0.5) }
      }
    }
    listDeadline = snapshot.nextTick
    if snapshot.nextTick != nil { clocks.start { tickClocks() } } else { clocks.stop() }
  }

  private func tickClocks() {
    guard let deadline = listDeadline, listNow >= deadline else { return }
    withAnimation(.easeOut(duration: 0.15)) { publish(listModel.tick(now: listNow)) }
  }

  private func pressed(_ key: UUID, _ down: Bool) {
    publish(listModel.press(now: listNow, id: key.uuidString, down: down))
  }

  private func flushRise() {
    publish(listModel.flight(now: listNow, active: !bars.awaitingThumbnail.isEmpty || !bars.flying.isEmpty))
  }

  /// Load in selection order. A selection superseded by another pick or closed cannot land.
  private func load(_ items: [PhotosPickerItem]) {
    picked = []
    guard !items.isEmpty else { return }
    Task {
      var made: [Picked] = []
      for item in items {
        guard libraryItems == items else { return }
        guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
        if item.supportedContentTypes.contains(where: { $0.conforms(to: .movie) }) {
          let url = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mov")
          do {
            try await Task.detached { try data.write(to: url) }.value
            let image = await Self.outgoingVideoThumbnail(url)
            // Export applies the same limits as camera video and removes the temporary source.
            let video = await MediaFile.video(url)
            if let video, let image {
              made.append(Picked(data: Data(), video: video, image: image, thumbnail: image))
            }
          } catch {
            Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: url) }
          }
        } else if let photo = await Picked.make(data) {
          made.append(photo)
        }
      }
      if libraryItems == items { picked = made }
    }
  }

  private func cameraSwitched(_ on: Bool) {
    cameraOn = on
    HitchScene.event("edge wipe", then: on ? "camera" : "list")
    publish(listModel.camera(now: listNow, on: on))
    if !on {
      // Closing the camera changes only its session. Selection and the bars' pending
      // batches keep their lifetimes; only Clear Selection or Undo discards them.
      if recordingFor != nil {  // a hold the camera closed under: no video waits, the next hold records again
        recordingChanged(nil)
        Task {
          if let file = await camera.stopVideo() { Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: file) } }
          flushRise()
        }
      }
    }
    Task { await switchCamera(on) }
  }
}

/// A finger on a bar's photos: down (they show at once), then a short tap (they run on) or let go after holding.
enum ThumbnailTouch {
  case down, tapped, released
}

/// A friend's bar as it is now: what it says after a send and what is on its way out (`FriendList.Bars.Bar`) are
/// read here, so a tap redraws this bar alone and not the whole list.
struct LiveBar: View, @MainActor Equatable {
  // Closures access live State locations. Observable row objects still invalidate their own body;
  // unrelated parent haptics/order updates can reuse unchanged bars.
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
  }

  let friend: Friend
  let bar: FriendList.Bars.Bar
  let color: Color
  let cameraOn: Bool
  let recording: Bool
  let stop: () -> Void
  let media: [Arrival]
  let landing: FriendList.LandingSlot
  private var yo: FriendList.Landing? { landing.value }
  let hint: String?
  let hold: ((Bool, CGPoint?) -> Void)?
  let cancelHold: () -> Void
  let press: (Bool) -> Void
  let flightChanged: (UUID, Bool) -> Void
  let flyBegan: (UUID) -> Void
  let view: (ThumbnailTouch) -> Void
  let action: (CGPoint?) -> Void

  var body: some View {
    RowButton(
      // A request shows one "Yo Request from X": no count of its Yos ("!!!"), no photo, not even its 📷 (what spam
      // aims at); all of it only after a yes.
      title, count: friend.request ? 0 : count, color: color, cameraOn: cameraOn, recording: recording,
      outgoing: bar.outgoing.isEmpty ? bar.sendingBatch?.items.last?.started : (bar.countdownStarted ?? .distantFuture),
      outgoingItems: bar.outgoing.isEmpty ? (bar.sendingBatch?.items ?? []) : bar.outgoing,
      undoAvailable: !bar.outgoing.isEmpty, sendSeconds: bar.seconds, stop: stop,
      media: friend.request ? [] : media, mediaMarkCount: bar.modelMarkCount, mediaSign: bar.modelMediaSign, yo: friend.request ? nil : yo, hint: hint, hold: hold, cancelHold: cancelHold, press: press, flightChanged: flightChanged, flyBegan: flyBegan, view: view, faded: friend.gone && !bar.hasNotice,
      icon: friend.group == nil ? nil : "person.3.fill", senders: bar.modelSenders, flies: true, barID: friend.name,
      action: action)
      // Gone: still their name (whose bar it is), faded; a tap says "Not on Yo".
      .accessibilityValue(friend.gone ? "Not on Yo" : "")
  }

  /// The bar's words: what is happening, else the name.
  private var title: String { bar.modelTitle.isEmpty ? friend.name : bar.modelTitle }
  private var count: Int { bar.modelCount }

}

/// A tap's haptic, played in the tap's own handler from a generator kept ready: `.sensoryFeedback` plays only in
/// SwiftUI's next update, once the list has been rebuilt for the change it watches (8 to 10 ms after the tap in the
/// Simulator, later when the list is busy), and a generator not prepared starts late.
@MainActor enum Haptic {
  private static let heavy = Perf.timed("inbox.haptic.create") { UIImpactFeedbackGenerator(style: .heavy) }

  static func prepareArrival() {
    Perf.timed("inbox.haptic.prepare") { heavy.prepare() }
  }

  private static var arrivalPlayed = false

  static func arrival() {
    guard !arrivalPlayed else { return }
    arrivalPlayed = true
    Perf.timed("inbox.haptic.play") { heavy.impactOccurred() }
    PerfProbe.nextFrame {
      arrivalPlayed = false
      prepareArrival()
    }
  }

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

  /// A Yo sent: the keyboard's quiet click with the haptic. A system sound: silent with the ring switch off, mixed
  /// with any music (never interrupts it), one for each Yo however fast.
  static func sendClick() {
    AudioServicesPlaySystemSound(1104)
  }

  /// A finger is down: the next `tap` plays without the engine's start.
  static func prepare() {
    medium.prepare()
    rigid.prepare()
  }
}

/// The slot keeps its identity even while empty. The resting stack survives each new flight. No number: a small real
/// stack, the top photo with one or two photos' edges behind it (2 photos: one edge, 3 or more: two; never more). The
/// exact number is in the bar's menu.
private struct OutgoingThumbnailSlot: View {
  let item: FriendList.Outgoing?
  let more: Int
  var video = false
  let destination: CGPoint
  let fallbackOrigin: CGPoint
  var coordinateOrigin: CGPoint = .zero
  var side: CGFloat = 28
  let began: (UUID) -> Void
  @State private var restingImage: UIImage?
  @State private var restingVideo = false

  /// Photos' edges shown behind the top one.
  private var edges: Int { min(max(0, more), 2) }
  /// The top photo leaves room for the edges behind it.
  private var topScale: CGFloat { edges == 0 ? 1 : 0.8 }
  private var topShift: CGFloat { -side * 0.045 * CGFloat(edges) }

  var body: some View {
    let top = CGPoint(x: destination.x + topShift, y: destination.y + topShift)
    ZStack {
      ForEach(Array((1...2).reversed()), id: \.self) { layer in
        if layer <= edges, let image = restingImage ?? item?.thumbnail {
          Image(uiImage: image).resizable().scaledToFill()
            .frame(width: side * topScale, height: side * topScale).clipped()
            .brightness(-0.12 * Double(layer))
            .overlay { Rectangle().stroke(.black.opacity(0.35), lineWidth: 1) }
            .rotationEffect(.degrees(layer == 1 ? 6 : -5))
            .position(x: top.x + side * 0.09 * CGFloat(layer), y: top.y + side * 0.09 * CGFloat(layer))
            .transition(.opacity)
        }
      }
      Group {
        if let restingImage { Image(uiImage: restingImage).resizable().scaledToFill() }
        else { Color.clear }
      }
        .frame(width: side, height: side).clipped()
        .overlay { VideoPlayGlyph(visible: restingVideo, side: side) }
        .scaleEffect(topScale)
        .position(top)
        .opacity(item == nil || restingImage == nil ? 0 : 1)
      if let item, let image = item.thumbnail {
        OutgoingThumbnail(image: image, video: item.text.contains("🎥"), destination: top, scale: topScale,
          origin: item.origin.map { CGPoint(x: $0.x - coordinateOrigin.x, y: $0.y - coordinateOrigin.y) } ?? fallbackOrigin, side: side, began: {
            Perf.mark("thumb.fly.start")
            began(item.id)
          })
          .id(item.id)
          .transition(.identity)
      }
    }
    .animation(.spring(duration: 0.3, bounce: 0.2), value: edges)
    .onChange(of: item, initial: true) { old, new in
      if old?.id != new?.id {
        if new == nil { restingImage = nil }
        else if let thumbnail = old?.thumbnail { restingImage = thumbnail; restingVideo = old?.text.contains("🎥") == true }
      }
    }
  }
}

/// A captured item flies once, then rests on top of the stack: it lands with a short plop. SwiftUI interpolates
/// cached pixels only.
private struct OutgoingThumbnail: View {
  let image: UIImage
  var video = false
  let destination: CGPoint
  var scale: CGFloat = 1
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
      .scaleEffect(resting ? scale : Metrics.rowHeight * 0.6 / side)
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
  /// How many the words count: a "!" for each after the first (Bangs).
  var count = 1
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
  /// A Yo on this bar now: its words knock and it says "Yo" at its end ("Yo!!" for three), fading out within
  /// `yoSeconds`.
  var yo: FriendList.Landing?
  var yoSeconds = FriendList.yoSeconds
  private static let solid = FriendList.yoSolidSeconds
  var hint: String?
  var hold: ((Bool, CGPoint?) -> Void)?
  var cancelHold: () -> Void = {}
  var press: (Bool) -> Void = { _ in }
  var flightChanged: (UUID, Bool) -> Void = { _, _ in }
  var flyBegan: (UUID) -> Void = { _ in }
  var view: (ThumbnailTouch) -> Void = { _ in }
  /// The words faded (a friend no longer on Yo).
  var faded = false
  /// A group's bar: its sign before the name (SF Symbol), and who sent the Yos on it, a line each under the name
  /// (`SenderBlocks`).
  var icon: String?
  var senders: [RenderedSender] = []
  let action: (CGPoint?) -> Void
  /// A hold started a video: letting go stops it, and the bar's tap does not take a photo as well.
  @Environment(\.dynamicTypeSize) private var measureType
  @Environment(\.locale) private var measureLocale
  @Environment(\.legibilityWeight) private var measureWeight
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
  /// A friend's bar: a Yo sent from it flies out of it, up and away.
  var flies = false
  /// The bar's own name, whatever its words say ("Sent Yo"): where its physics "!"s start (BangPhysics.swift).
  var barID: String?
  private var physicsBar: String { barID ?? title }
  /// The Yos on their way out of the bar now, each flying up and fading (FlyingYo).
  @State private var flights: [UUID] = []
  @State private var sentSuffixWidth: CGFloat = 0

  init(
    _ title: String, count: Int = 1, color: Color, cameraOn: Bool = false, recording: Bool = false, outgoing: Date? = nil,
    outgoingItems: [FriendList.Outgoing] = [], undoAvailable: Bool = true,
    sendSeconds: Double = FriendList.sendDelay, stop: @escaping () -> Void = {},
    media: [Arrival] = [], mediaMarkCount: Int = 0, mediaSign: String = "", yo: FriendList.Landing? = nil, hint: String? = nil, hold: ((Bool, CGPoint?) -> Void)? = nil, cancelHold: @escaping () -> Void = {}, press: @escaping (Bool) -> Void = { _ in }, flightChanged: @escaping (UUID, Bool) -> Void = { _, _ in }, flyBegan: @escaping (UUID) -> Void = { _ in },
    view: @escaping (ThumbnailTouch) -> Void = { _ in }, faded: Bool = false, icon: String? = nil, senders: [RenderedSender] = [],
    flies: Bool = false, barID: String? = nil, action: @escaping (CGPoint?) -> Void
  ) {
    self.barID = barID
    self.flies = flies
    self.icon = icon
    self.senders = senders
    self.title = title
    self.count = count
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
    self.flightChanged = flightChanged
    self.flyBegan = flyBegan
    self.view = view
    self.faded = faded
    self.action = action
  }

  var body: some View {
    ZStack(alignment: leftHanded ? .trailing : .leading) {
      // One button and one label subtree across camera changes: stacks and flights keep their state.
      Button {
        guard stationaryTouch.isStationary, !cameraOn || !holding else { return }
        if !cameraOn && flies {
          let id = UUID()
          flights.append(id)
          Task {
            try? await Task.sleep(for: .seconds(2))
            flights.removeAll { $0 == id }
            flightChanged(id, false)
          }
        }
        action(touchOrigin)
        touchOrigin = nil
      } label: { label }
      .buttonStyle(BarPress(cameraOn: cameraOn, touchDown: shutterDown, recording: recording, press: press))
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
          if stationaryTouch.isStationary { hold(false, holdOrigin) }
          else { cancelHold() }
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
      // What takes its own touches (the undo ring, the photos) lies over the bar, in the same line (Buttons in a Button
      // do not work). The ring stands at the bar's very left, away from where the bar was tapped: a tap more is
      // another photo, never an undo by mistake.
      .overlay {
        line(touchable: true)
          // Received media stays visible; camera taps and holds reach the shutter underneath.
          .allowsHitTesting(!cameraOn)
          .accessibilityHidden(cameraOn)
      }
      // At the bar's start (its end for the left hand), away from where it is tapped: the undo ring.
      // A sibling of the camera/list button: changing its gestures and style cannot
      // replace the stack's thumbnail flight or countdown.
      if let outgoing {
        undoRing(since: outgoing)
          .padding(leftHanded ? .trailing : .leading, BarLine.margin)
      }
    }
  }

  /// A bar of the friend's colour from edge to edge, the name large in the middle (the original's), red while
  /// recording.
  private var label: some View {
    line(touchable: false)
      .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
      .background {
        ZStack(alignment: .leading) {
          if recording { Color.alizarin.opacity(0.85) }
          else {
            Color.clear.modifier(ListColor(color: color,
              opacity: cameraOn && shutterDown ? 0 : 1))
          }
          if recording { RecordingFill(seconds: Camera.longestVideo) }
          if !cameraOn { RowFlash(total: yo?.total ?? 0) }
        }
        // Animate only opacity: press feedback must not retarget the camera wipe.
        .animation(cameraOn && (shutterDown || recording)
          ? .easeOut(duration: 0.06) : ListMotion.expansion) { content in
          content.opacity(cameraOn && shutterDown && !recording ? 0 : 1)
        }
      }
      .contentShape(.rect)
      .bangPhysicsBar(physicsBar)  // dev prototype (BangPhysics.swift)
  }

  /// The bar's line, sending and receiving alike (BarLine): the words and their "!"s; a Yo that came at the end; room
  /// at the start for the undo ring while something is on its way out. Laid out twice the same: in the bar the words
  /// and their "!"s, over it what takes its own touches (the Yo at the end), so each sits exactly where the other
  /// leaves it room.
  private func line(touchable: Bool) -> some View {
    BarLine(
      start: (outgoing == nil ? BarLine.margin : BarLine.margin + Self.ring + BarLine.gap) + (icon == nil ? 0 : Self.iconRoom),
      mirrored: leftHanded,
      centeredSuffixWidth: 0,
      below: !senders.isEmpty && (yo != nil || !media.isEmpty),  // a group's senders: under the name, "Kai: Yo!!"
      lineEnd: senders.isEmpty || media.isEmpty ? 0 : Self.sign  // its photo sign on the name's line
    ) {
        // Exclamation marks only behind a Yo ("Sent Yo!!"), never behind a name or "+".
        Bangs(count: title.hasSuffix("Yo") ? count : 0, prefix: title,
          flightOverlay: title == "Sent Yo" && !touchable ? AnyView(
            ForEach(flights, id: \.self) { id in
              FlyingYo { flights.removeAll { $0 == id }; flightChanged(id, false) }
                .onAppear { flightChanged(id, true) }
            }
          ) : nil, physics: touchable ? nil : physicsBar)
          .bangPhysicsLabel(physicsBar, incoming: false, on: !touchable && title == "Sent Yo")
          // Measured by Bangs' own key (what it shows now, not the count it is still counting up to).
          .contentTransition(.identity)  // other words come at once, never two drawn over each other
        // A group: its sign before its name, outside it: the name alone stands in the middle (`iconRoom` keeps the
        // sign on the bar when a long name moves to the start).
        .overlay(alignment: .leading) {
          if let icon {
            Image(systemName: icon)
              .font(.system(size: Metrics.rowFont * 0.62, weight: .bold))
              .frame(width: Self.iconRoom - 8, alignment: .trailing)
              .offset(x: -Self.iconRoom)  // wholly before the name, 8 apart
              .accessibilityLabel("Group")
          }
        }
        .modifier(Knock(trigger: yo?.first))
        .opacity(touchable ? 0 : faded ? 0.55 : 1)
        .accessibilityHidden(touchable)
      Color.clear.frame(width: 0, height: 0)
      ZStack(alignment: leftHanded ? .leading : .trailing) { mark(touchable: touchable) }
        .frame(maxWidth: .infinity, alignment: leftHanded ? .leading : .trailing)
        .clipped()
        // Outside the clip: each further incoming Yo's "!" comes in from the screen's edge.
        .overlay {
          if touchable && sweeps {
            IncomingSweeps(total: yo?.total ?? 0, landing: 3 + (media.isEmpty || mediaSign.isEmpty ? 0 : Self.sign + 4))
          }
        }
        // Replacing media with a plain Yo (or back) must not retain the old label
        // alongside the new one while their different widths are laid out.
        .animation(nil, value: media.isEmpty)
        .animation(nil, value: senders.isEmpty)
        .layoutValue(key: BarMeasureKey.self, value: media.isEmpty && yo == nil ? measureKey("empty") : nil)
        // Always a part, so a missing "!" never takes its place
        .modifier(Knock(trigger: yo?.first))
        .opacity(touchable ? 1 : 0)
        .accessibilityHidden(!touchable)
      // Each measurement/candidate owns a stable slot even when its content is empty.
      ZStack {
        if title == "Sent Yo" || title == "Yo" {
          Bangs(count: count, maximumBangs: 0, prefix: title)
        }
      }.hidden().accessibilityHidden(true)
      ZStack { mark(touchable: false, compact: true) }
        .hidden().accessibilityHidden(true)

    }
    .font(.yo(Metrics.rowFont))
    .foregroundStyle(.white)
    .animation(nil, value: title)  // other words stand in their place at once; only "!"s, numbers and room animate
  }

  /// The "!" sweep runs on a one-line mark at the bar's right end (a group's senders and the left hand pulse only).
  private var sweeps: Bool { senders.isEmpty && !leftHanded }

  private func measureKey(_ text: String) -> String {
    "\(measureType):\(measureLocale.identifier):\(String(describing: measureWeight)):\(leftHanded):" + text
  }

  private static let ring = 56.0
  /// Room before a group's name for its sign (`icon`, at most 28 wide) and the gap of 8 after it.
  private static let iconRoom = 36.0
  private static let sign = 34.0

  /// "📷" or "🎥" while photos and videos wait unseen: a finger on it shows them at once; a short tap lets them run,
  /// holding closes on letting go. Only the sign takes the touch: the rest of the bar sends Yos as ever.
  @ViewBuilder private var sign: some View {
    if !mediaSign.isEmpty {
      Text(mediaSign)
        .font(.system(size: 24))
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

  /// Something on its way out: a ring at the bar's left closing like an upload; a tap on it undoes the send.
  private func undoRing(since start: Date) -> some View {
    Button(action: stop) {
      TimelineView(.animation(minimumInterval: 1.0 / 120)) { timeline in
        let done = min(1, max(0, timeline.date.timeIntervalSince(start) / max(0.001, sendSeconds)))
        ZStack {
          if outgoingItems.contains(where: { $0.text.hasSuffix("📷") || $0.text.hasSuffix("🎥") }) {
            GeometryReader { geometry in
              OutgoingThumbnailSlot(
                item: outgoingItems.last(where: { $0.thumbnail != nil }),
                more: outgoingItems.count - 1,
                destination: CGPoint(x: Self.ring / 2, y: Self.ring / 2),
                fallbackOrigin: CGPoint(x: Self.ring / 2, y: Self.ring / 2),
                coordinateOrigin: geometry.frame(in: .named(touchSpace)).origin,
                side: Self.ring, began: flyBegan)
            }
            .clipShape(Rectangle())  // square, like the bars: no rounded corners in the list
            Rectangle().stroke(.white.opacity(0.35), lineWidth: 3)
            Rectangle()
              .trim(from: 0, to: 1 - done)
              .stroke(.white, style: StrokeStyle(lineWidth: 3, lineCap: .butt))
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
    .accessibilityLabel(undoAvailable ? "Undo Send" : "Sending")
    .transition(.scale.combined(with: .opacity))
  }

  /// At the bar's end (its start for the left hand), in the name's own type: "Yo" and a "!" for each Yo after the
  /// first, then the photo sign while photos or videos wait ("Yo!! 📷"; "📷 Yo!!" for the left hand). The "!"s count
  /// every Yo of the run, with or without a photo; with only unseen photos left, one for each.
  @ViewBuilder private func mark(touchable: Bool, compact: Bool = false) -> some View {
    if !media.isEmpty, !senders.isEmpty {
      // A group's: the photo sign on the name's line at the bar's end, as on any bar; the senders' lines under it.
      VStack(spacing: 0) {
        HStack(spacing: 0) {
          if leftHanded { sign }
          Spacer(minLength: 0)
          if !leftHanded { sign }
        }
        .frame(height: Metrics.rowHeight)
        SenderBlocks(senders: senders, mirrored: leftHanded, paused: yo?.paused ?? cameraOn)
      }
      .allowsHitTesting(touchable)  // the copy in the bar under it is only for the layout
      .transition(.opacity)
    } else if !media.isEmpty {
      HStack(spacing: 4) {
        if leftHanded { sign }
        Bangs(count: mediaMarkCount, maximumBangs: compact ? 0 : 5, prefix: "Yo", incoming: true,
              intensityCount: yo?.total ?? mediaMarkCount, sweeps: sweeps && yo != nil,
              physics: touchable && !compact ? physicsBar : nil)
        .bangPhysicsLabel(physicsBar, incoming: true, on: touchable && !compact)
        .allowsHitTesting(false)  // a tap there sends a Yo
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(media.count == 1 ? "Yo with a photo or video" : "\(media.count) Yos with photos or videos")
        if !leftHanded { sign }
      }
      .lineLimit(1)
      .allowsHitTesting(touchable)  // the copy in the bar under it is only for the layout
      .transition(.scale.combined(with: .opacity))
    } else if let yo {
      // "Yo" and a "!" for each Yo after the first: a "!" goes quickly when its Yo is over, the last Yo fades slowly.
      Group {
        if !senders.isEmpty {
          VStack(spacing: 0) {
            Color.clear.frame(height: Metrics.rowHeight)  // the name's line, as with a photo sign
            SenderBlocks(senders: senders, mirrored: leftHanded, paused: yo.paused)
          }
        } else {
          Bangs(count: yo.count, maximumBangs: compact ? 0 : 5, prefix: "Yo", incoming: true, intensityCount: yo.total,
                sweeps: sweeps, physics: touchable && !compact ? physicsBar : nil)
          .lineLimit(1)
          .bangPhysicsLabel(physicsBar, incoming: true, on: touchable && !compact)
        }
      }
      .modifier(ClockFade(since: yo.fading, seconds: yoSeconds - Self.solid, paused: yo.paused, elapsed: yo.fadeElapsed))
      .accessibilityElement(children: .ignore)
      .accessibilityLabel(
        senders.isEmpty
          ? (yo.count == 1 ? "Yo" : "\(yo.count) Yos")
          : senders.map { "\($0.count == 1 ? "Yo" : "\($0.count) Yos") from \($0.name)" }.joined(separator: ", "))
      // It comes in from the bar's outer edge, sliding to its place; it goes by fading.
      .transition(.asymmetric(
        insertion: .move(edge: leftHanded ? .leading : .trailing).combined(with: .opacity),
        removal: .opacity))
    }
  }
}

/// The camera's zoom: one small circle with the factor ("1×"); each tap goes to the next of its choices (0.5, 1, 2, 3,
/// then round again), ramped by the camera. Not there with one choice only (the front camera).
struct CameraZoom: View {
  let choices: [Double]
  @Binding var chosen: Double
  /// Under a pinch the number follows the fingers at once (no rolling digits).
  var pinching = false
  /// The number shown: under a pinch at most twenty times a second, so a fast pinch skips numbers as the Camera app's
  /// does instead of drawing every step; the last one always lands.
  @State private var shown: Double?
  @State private var shownAt = Date.distantPast

  var body: some View {
    if choices.count > 1 {
      // One small circle with the factor, as Signal has it: each tap the next choice, round again.
      Button {
        Haptic.tap(.light)
        // The next choice up (after a pinch between two: the one above it), round to the first.
        let next = choices.first { $0 > chosen + 0.001 } ?? choices[0]
        withAnimation(.snappy(duration: 0.2)) { chosen = next }
      } label: {
        Text(Self.label(pinching ? (shown ?? chosen) : chosen) + "×")
          .font(.system(size: 13, weight: .bold).monospacedDigit())
          .foregroundStyle(.white)
          .contentTransition(pinching ? .identity : .numericText())
          .transaction { if pinching { $0.animation = nil } }
          // A fixed square, so the glass stays a circle whatever the number ("1.7×" shrinks to fit).
          .lineLimit(1)
          .minimumScaleFactor(0.8)
          .padding(6)
          .frame(width: 44, height: 44)
          .glassEffect(.regular.interactive(), in: .circle)
          .contentShape(.circle)

      }
      .onChange(of: chosen) {
        guard pinching else { shown = nil; return }
        if Date.now.timeIntervalSince(shownAt) >= 0.05 { shown = chosen; shownAt = .now }
      }
      .onChange(of: pinching) { if !pinching { shown = nil } }
      .accessibilityLabel("Zoom \(Self.label(chosen))×")
      .accessibilityHint("Changes the zoom")
      .buttonStyle(.plain)
      .transition(.opacity)
    }
  }

  /// "0.5", "1", "2", "2.5": no ".0".
  static func label(_ choice: Double) -> String {
    choice == choice.rounded() ? String(Int(choice)) : String(format: "%.1f", choice)
  }
}

/// A group's Yos, by whom: a line each, "Kai: Yo!!" in the middle as a bar's name, as tall as a bar, in the order they
/// came, all under the bar's name (`BarLine.below`), which stays as on any bar. One sender, one line: the bar grows by
/// a bar's height with each.
struct RenderedSender {
  let user: String
  let name: String
  let count: Int
  let elapsed: UInt64
  let at: Date
}

struct SenderBlocks: View {
  let senders: [RenderedSender]
  var mirrored = false
  var paused = false

  var body: some View {
    // Each line fades on its own once its sender stops, as a bar's Yo: solid `yoSolidSeconds`, then gone by
    // `yoSeconds`. Its place stays (empty) until the whole bar's Yos are over and it shrinks.
    VStack(spacing: 0) {
      ForEach(senders, id: \.user) { sender in
        Group {
          Bangs(count: sender.count, prefix: "\(sender.name): Yo", incoming: true)
          .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
        }
        .modifier(ClockFade(since: sender.at.addingTimeInterval(FriendList.yoSolidSeconds),
          seconds: FriendList.yoSeconds - FriendList.yoSolidSeconds, paused: paused,
          elapsed: Double(sender.elapsed) / 1000 - FriendList.yoSolidSeconds))
        .transition(.opacity)
      }
    }
    .font(.yo(Metrics.rowFont))  // as a bar's name
    .allowsHitTesting(false)  // a tap there sends a Yo
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(senders.map { "\($0.count == 1 ? "Yo" : "\($0.count) Yos") from \($0.name)" }.joined(separator: ", "))
  }
}

private struct ClockFade: ViewModifier {
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

/// Keep Yo at its full type size; spare width buys up to five exclamation marks.
/// Run intensity and each arrival's pulse belong to the glyphs, not a layout copy.
struct Bangs: View {
  @Environment(\.yoFlood) private var busy
  @Environment(\.dynamicTypeSize) private var measureType
  @Environment(\.locale) private var measureLocale
  @Environment(\.legibilityWeight) private var measureWeight
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var pulses = 0
  let count: Int
  var intensityCount: Int?
  var flightOverlay: AnyView?
  var maximumBangs: Int
  var prefix: String
  @State private var displayed: Int
  @State private var nextTurn = ContinuousClock.now
  /// The Yos not shown yet, one "!" each, strictly one after the other (`gap` apart).
  @State private var arrivals: [ContinuousClock.Instant] = []
  @State private var lastShow = ContinuousClock.now
  /// The bar whose physics "!"s carry on beyond `maximumBangs` (BangPhysics.swift, dev only).
  var physics: String?
  static let gap = Duration.milliseconds(100)
  /// More waiting than this (a flood): no more one by one, the count shows at once.
  private static let queueLimit = 10

  var incoming = false
  /// Each new "!" is swept in from the screen's edge first (IncomingSweeps): it shows here when that lands.
  var sweeps = false

  init(count: Int, maximumBangs: Int = 5, prefix: String = "", incoming: Bool = false, intensityCount: Int? = nil,
       sweeps: Bool = false, flightOverlay: AnyView? = nil, physics: String? = nil) {
    self.physics = physics
    self.sweeps = sweeps
    self.flightOverlay = flightOverlay
    self.incoming = incoming
    self.intensityCount = intensityCount
    self.prefix = prefix
    self.maximumBangs = maximumBangs
    self.count = count
    _displayed = State(initialValue: count)
  }

  var body: some View {
    let bangs = min(max(0, busy ? count : displayed), maximumBangs)
    let motionReduced = reduceMotion
    BangsFit {
      // Drop marks before compromising the full-size Yo. Candidates go by rank (0 = most marks): the one showing stays
      // the same view when a mark comes, so the words are never drawn twice (no crossfade) and a flying "Yo" in it
      // stays on its glyph.
      ForEach(0...bangs, id: \.self) { rank in
        run(bangs: bangs - rank).fixedSize(horizontal: true, vertical: false).transition(.identity)
      }
      run(bangs: 0).fixedSize(horizontal: true, vertical: false)
    }
    .lineLimit(1)
    // One pulse for all candidates, on the glyphs themselves: only ever brighter (and a touch larger).
    .keyframeAnimator(initialValue: 0.0, trigger: pulses) { text, pulse in
      text
        .modifier(YoIntensity(incoming: incoming, pulse: isYo ? pulse : 0))
        .scaleEffect(motionReduced || !isYo ? 1 : 1 + max(0, pulse) * 0.025)
    } keyframes: { _ in
      KeyframeTrack {
        LinearKeyframe(1, duration: 0.08)
        LinearKeyframe(0, duration: 0.17)
      }
    }
    .layoutValue(key: BarMeasureKey.self, value:
      "bangs:\(prefix):\(maximumBangs):\(busy ? count : displayed):\(measureType):\(measureLocale.identifier):\(String(describing: measureWeight))")
    .animation(busy ? nil : .easeOut(duration: 0.16), value: displayed)
    .onChange(of: intensityCount ?? count) { old, new in
      if new > old && isYo { pulses += 1 }
    }
    .onChange(of: isYo) { _, active in
      if active && count > 0 { pulses += 1 }
    }
    .onAppear { if isYo && count > 0 { pulses += 1 } }
    .onChange(of: count) { _, new in
      let waiting = new - displayed
      if waiting > Self.queueLimit {
        arrivals.removeAll()
        displayed = new
        lastShow = .now
      } else if waiting > arrivals.count {
        arrivals.append(contentsOf: repeatElement(ContinuousClock.now, count: waiting - arrivals.count))
      } else if waiting < arrivals.count {
        arrivals.removeAll()
      }
    }
    // Up: one "!" for each Yo, `gap` apart; one swept in from the edge (IncomingSweeps) shows once it lands there.
    .task(id: arrivals.isEmpty) {
      while let arrival = arrivals.first {
        let next = displayed + 1
        let swept = sweeps && !busy && !motionReduced && (next <= maximumBangs || !BangPhysics.on)
        let at = max(lastShow.advanced(by: Self.gap), swept ? arrival.advanced(by: IncomingSweeps.flight) : arrival)
        do { try await ContinuousClock().sleep(until: at) } catch { return }
        guard !arrivals.isEmpty else { return }
        arrivals.removeFirst()
        displayed = next
        lastShow = at
      }
    }
    // Down: the "!"s go as before.
    .task(id: count) {
      guard count < displayed, arrivals.isEmpty else { return }
      do { try await ContinuousClock().sleep(until: nextTurn) } catch { return }
      guard !Task.isCancelled, count < displayed else { return }
      displayed = count
      nextTurn = .now.advanced(by: .milliseconds(200))
    }
    // Beyond `maximumBangs`, each "!" is a physics one (dev only), one for one with what the label counts.
    .onChange(of: displayed) { old, new in
      BangPhysics.sync(physics, incoming: incoming, target: physicsTarget(new), up: new > old)
    }
    .onAppear { BangPhysics.sync(physics, incoming: incoming, target: physicsTarget(displayed), up: true) }
    .onDisappear { BangPhysics.vanish(physics, incoming: incoming) }
  }

  private func physicsTarget(_ shown: Int) -> Int {
    physics != nil && (incoming || prefix == "Sent Yo") ? max(0, shown - maximumBangs) : 0
  }

  private var isYo: Bool { incoming || prefix == "Yo" || prefix == "Sent Yo" }

  private func run(bangs: Int) -> some View {
    Group {
      if prefix == "Sent Yo" {
        HStack(spacing: 0) {
          Text("Sent ")
          Text("Yo").overlay { flightOverlay }
          Text(String(repeating: "!", count: bangs))
        }
      } else {
        Text(prefix + String(repeating: "!", count: bangs))
      }
    }
      .lineLimit(1)
      .contentTransition(.identity)  // a "!" more comes at once, never two texts over each other
      // Reserve the pulse's growth so adjacent text and the thumbnail stay clear.
      .padding(.horizontal, isYo ? 3 : 0)
      .padding(.vertical, isYo ? 1 : 0)
  }

}

/// Each incoming Yo after the first (which comes in with the whole mark): a small "Yo" sweeps in from the screen's edge
/// into the label's "Yo" and merges with it; then Bangs shows one "!" more (up to five; beyond, in dev builds, physics
/// "Yo"s take over, BangPhysics.swift). Several fly one after the other, `Bangs.gap` apart, as Bangs shows them; never
/// a queue longer than Bangs keeps. Not with Reduce Motion (the glow pulses only).
struct IncomingSweeps: View {
  static let flightSeconds = 0.26
  static let flight = Duration.seconds(flightSeconds)
  /// All Yos of the run.
  let total: Int
  /// From the overlay's trailing edge to where the last "!" ends.
  let landing: CGFloat
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.yoFlood) private var busy
  /// Each flight and the "!"s the label shows while it flies (the "Yo" lands just before them).
  @State private var flying: [Flight] = []
  /// When the last sweep set off: the next one `Bangs.gap` after it, as Bangs shows them.
  @State private var lastLaunch = ContinuousClock.now

  struct Flight: Hashable {
    let id = UUID()
    let bangs: Int
  }

  var body: some View {
    ZStack(alignment: .trailing) {
      ForEach(flying, id: \.self) { flight in
        SweepingYo(bangs: flight.bangs) { flying.removeAll { $0 == flight } }
      }
    }
    .padding(.trailing, landing)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .trailing)
    .allowsHitTesting(false)
    .accessibilityHidden(true)
    .onChange(of: total) { old, new in
      // A flood (more than Bangs queues) shows at once, without sweeps.
      guard old > 0, new > old, new - old <= 10, !reduceMotion, !busy else { return }
      // One for each Yo, one after the other; beyond five they are physics "Yo"s (dev) instead.
      for k in (old + 1)...new where k <= 5 || !BangPhysics.on {
        let at = max(lastLaunch.advanced(by: Bangs.gap), .now)
        if at > .now.advanced(by: Bangs.gap * 10) { break }
        lastLaunch = at
        let flight = Flight(bangs: min(5, max(0, k - 1)))
        Task {
          try? await ContinuousClock().sleep(until: at)
          flying.append(flight)
        }
      }
    }
  }
}

/// A small "Yo" from beyond the screen's edge into the label's "Yo" (as many "!"s before the end as it shows), growing
/// to its size as it lands, then gone into it.
private struct SweepingYo: View {
  let bangs: Int
  let done: () -> Void
  @State private var landed = false
  @State private var gone = false

  var body: some View {
    HStack(spacing: 0) {
      Text("Yo")
        .modifier(YoIntensity(incoming: true, pulse: 0))
        .scaleEffect(landed ? 1 : 0.55, anchor: .trailing)
        .offset(x: landed ? 0 : BarLine.endPadding + 40 + Double(bangs) * 9)  // from beyond the screen's edge
        .opacity(gone ? 0 : 1)
      Text(String(repeating: "!", count: bangs)).hidden()
    }
    .lineLimit(1)
    .fixedSize()
    .onAppear {
      withAnimation(.easeOut(duration: IncomingSweeps.flightSeconds)) {
        landed = true
      } completion: {
        withAnimation(.easeOut(duration: 0.12)) { gone = true } completion: { done() }
      }
    }
  }
}

/// The bar flashes once for each incoming Yo, on every screen (the HDR glow comes on top where the screen has it).
private struct RowFlash: View {
  let total: Int
  @State private var flashes = 0

  var body: some View {
    Color.white
      .keyframeAnimator(initialValue: 0.0, trigger: flashes) { white, value in
        white.opacity(value)
      } keyframes: { _ in
        KeyframeTrack {
          LinearKeyframe(0.18, duration: 0.06)
          LinearKeyframe(0, duration: 0.2)
        }
      }
      .allowsHitTesting(false)
      .accessibilityHidden(true)
      .onChange(of: total) { old, new in if new > old { flashes += 1 } }
  }
}

/// Choose from intrinsic candidates in this pass, without retaining ViewThatFits' last selection.
private struct BangsFit: Layout {
  private func selection(_ proposal: ProposedViewSize, _ subviews: Subviews) -> Int? {
    subviews.indices.first { subviews[$0].sizeThatFits(.unspecified).width <= (proposal.width ?? .infinity) }
      ?? subviews.indices.last
  }

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    guard let index = selection(proposal, subviews) else { return .zero }
    return subviews[index].sizeThatFits(proposal)
  }

  func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
    let selected = selection(ProposedViewSize(width: bounds.width, height: bounds.height), subviews)
    for index in subviews.indices {
      // Unselected fixed-size candidates stay outside the clipped suffix bounds.
      subviews[index].place(at: CGPoint(x: index == selected ? bounds.minX : bounds.minX - 100_000,
        y: bounds.midY), anchor: .leading,
        proposal: ProposedViewSize(width: bounds.width, height: bounds.height))
    }
  }
}

/// A bar's line, for sending and receiving alike: the words in the middle, their "!"s right after them, a Yo that came
/// at the bar's end. The words stay in the middle until they would run into the end (or the edge): then they move left
/// as far as needed, never past `start` (the margin, or after the undo ring); a name too long even then gets smaller
/// (to half), then is cut with "…". Nothing overlaps.
private struct BarMeasureKey: LayoutValueKey {
  static let defaultValue: String? = nil
}

struct BarLine: Layout, Equatable {
  static let margin = 20.0
  static let endPadding = 30.0
  static let gap = 14.0
  /// How far from the bar's start (left; right when mirrored) the words may go at most: room for the ring and sign.
  var start = margin
  /// For the left hand: the end part ("Yo!!") at the left, the room kept at the right.
  var mirrored = false
  // Interpolate the measured suffix width so adding a bang eases the entire phrase to its new centre.
  var centeredSuffixWidth: CGFloat = 0
  var animatableData: CGFloat {
    get { centeredSuffixWidth }
    set { centeredSuffixWidth = newValue }
  }
  /// The end part from the bar's top, as wide as the bar (a group's senders, `SenderBlocks`): its first line is the
  /// words' (a photo sign at its end), the senders' lines under it; the words stand in the middle of the bar's first
  /// `rowHeight`, as on any bar; the bar grows with the end part.
  var below = false
  /// With `below`: room kept at the end of the first line for what stands there (a group's photo sign), so the words
  /// and their "!"s move aside from it as from any end part.
  var lineEnd = 0.0

  struct Cache {
    var sizes: [CGSize] = []
    var keys: [String?] = []
  }

  func makeCache(subviews: Subviews) -> Cache {
    Perf.timed("media.sign.cache") {
      var cache = Cache()
      updateCache(&cache, subviews: subviews)
      return cache
    }
  }

  func updateCache(_ cache: inout Cache, subviews: Subviews) {
    // SwiftUI invalidates this when the subviews change (including text, counts and environment).
    for (index, view) in subviews.enumerated() {
      let key = view[BarMeasureKey.self]
      if index >= cache.sizes.count {
        cache.sizes.append(view.sizeThatFits(.unspecified))
        cache.keys.append(key)
      } else if index != 0 || key == nil || cache.keys[index] != key {
        cache.sizes[index] = view.sizeThatFits(.unspecified)
        cache.keys[index] = key
      }
    }
    if cache.sizes.count > subviews.count {
      cache.sizes.removeLast(cache.sizes.count - subviews.count)
      cache.keys.removeLast(cache.keys.count - subviews.count)
    }
  }

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) -> CGSize {
    return Perf.timed("media.sign.measure") {
      if below, cache.sizes.count > 2 {
        return CGSize(width: proposal.width ?? 393, height: max(Metrics.rowHeight, cache.sizes[2].height))
      }
      return CGSize(width: proposal.width ?? 393, height: cache.sizes.reduce(0) { max($0, $1.height) })
    }
  }

  func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) {
    Perf.timed("media.sign.place") {
      // An empty part (no "!"s, no Yo at the end) is no subview at all: take what is there.
      guard let words = subviews.first else { return }
      let after = subviews.count > 1 ? subviews[1] : nil
      let last: LayoutSubview? = subviews.count > 2 ? subviews[2] : nil
      let afterSize = cache.sizes.count > 1 ? cache.sizes[1] : .zero
      // Below: the end part under the line, at the bar's end; the line then as on a bar with nothing at its end.
      let end: LayoutSubview? = below ? nil : last
      if below, let last {
        // As wide as the bar less its margins, from its top.
        last.place(
          at: CGPoint(x: bounds.minX + Self.endPadding, y: bounds.minY), anchor: .topLeading,
          proposal: ProposedViewSize(width: max(0, bounds.width - 2 * Self.endPadding), height: nil))
      }
      let endSize: CGSize = !below && cache.sizes.count > 2 ? cache.sizes[2] : .zero
      // The words between `low` and `high`, the end part outside them.
      // Below, the end part's width on the first line is `lineEnd` (its sign), not its own.
      let total = max(0, bounds.width - start - Self.endPadding - (endSize.width > 0 ? Self.gap : 0))
      let minimumAfter = cache.sizes.count > 3 ? cache.sizes[3].width : 0
      let minimumEnd = cache.sizes.count > 4 ? cache.sizes[4].width : 0
      // Spend spare room on marks only after reserving the full-size words and Yo.
      let endWidth = below ? lineEnd : min(endSize.width,
        max(minimumEnd, total - max(cache.sizes[0].width, minimumAfter)))
      let endX = mirrored ? bounds.minX + Self.endPadding : bounds.maxX - Self.endPadding - endWidth
      let low = mirrored
        ? (endWidth > 0 ? endX + endWidth + Self.gap : bounds.minX + Self.margin) : bounds.minX + start
      let high = mirrored ? bounds.maxX - start : (endWidth > 0 ? endX - Self.gap : bounds.maxX - Self.margin)
      let available = max(0, high - low)
      let afterWidth = min(afterSize.width, max(0, available - max(cache.sizes[0].width, minimumAfter)))
      let room = max(0, available - afterWidth)
      let wordsWidth = max(minimumAfter, min(cache.sizes[0].width, room))
      var x = bounds.midX - (wordsWidth + min(centeredSuffixWidth, afterWidth)) / 2
      x -= max(0, x + wordsWidth + afterWidth - high)
      x = max(low, x)
      // The line in the middle of the bar's first `rowHeight`, where it is on every bar (the end part under it, if so).
      let lineY = below ? bounds.minY + Metrics.rowHeight / 2 : bounds.midY
      words.place(
        at: CGPoint(x: x, y: lineY), anchor: .leading, proposal: ProposedViewSize(width: wordsWidth, height: nil))
      after?.place(at: CGPoint(x: x + wordsWidth, y: lineY), anchor: .leading, proposal: ProposedViewSize(width: afterWidth, height: nil))
      end?.place(at: CGPoint(x: endX, y: lineY), anchor: .leading,
        proposal: ProposedViewSize(width: endWidth, height: nil))
    }
  }
}

/// A Yo knocks: the words on a bar nudge left and right once, as a poke; the bar itself stays (moving it would show
/// what is behind it).
struct Knock: ViewModifier {
  @Environment(\.yoFlood) private var busy
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
      if new != nil && !busy { knocks += 1 }
    }
    // A bar that comes in on top already carrying its Yo knocks as it arrives.
    .onAppear {
      if trigger != nil && !busy { knocks += 1 }
    }
  }
}

/// A bar that slides left under a horizontal swipe and shows Block and Remove underneath (as Mail's and Signal's
/// lists do); it snaps open or closed when let go. A vertical move is the list's scroll, a tap or a hold the bar's.
struct BarActivity {
  let text: String
  let icon: String
  let accessibility: String
}

struct SwipeActions<Content: View>: View {
  @Binding var open: Bool
  let enabled: Bool
  var request: ((Bool) -> Void)? = nil
  let actionsEnabled: Bool
  let mirrored: Bool
  let activity: BarActivity
  /// A swipe on it began.
  let began: () -> Void
  let block: () -> Void
  let remove: () -> Void
  @ViewBuilder let content: Content
  /// How far the finger moved the bar, while it does.
  @State private var dragged: CGFloat?

  private static var button: CGFloat { 128 }  // room for the row-sized words with air on both sides
  private static var width: CGFloat { 2 * button }

  var body: some View {
    let resting: CGFloat = enabled && open ? -Self.width : 0
    let moved = enabled ? resting + (dragged ?? 0) : 0
    let direction: CGFloat = mirrored ? -1 : 1
    let revealing = moved > 0
    let offset: CGFloat = request != nil || revealing ? moved : actionsEnabled
      ? (moved < -Self.width ? -Self.width + (moved + Self.width) / 3 : moved) : 0
    content
      .overlay {
        // Open: a tap on the bar closes it (the bar does nothing else then).
        if open { Color.clear.contentShape(.rect).onTapGesture { close() } }
      }
      .offset(x: direction * offset)
      .background(alignment: mirrored ? .leading : .trailing) {
        if request != nil, offset != 0 {
          Text(offset > 0 ? "No" : "Yo")
            .font(.yo(Metrics.rowFont))
            .foregroundStyle(.white)
            .frame(width: abs(offset), height: Metrics.rowHeight)
            .background(Color(hex: offset > 0 ? 0xE74C3C : 0x2ECC71))
            .frame(maxWidth: .infinity, alignment: (offset > 0) != mirrored ? .leading : .trailing)
        } else if offset > 0 {
          Label(activity.text, systemImage: activity.icon)
            .font(.yo(Metrics.rowFont))
            .foregroundStyle(.white)
            .lineLimit(1)
            .minimumScaleFactor(0.5)
            .padding(.horizontal, 24)
            .frame(width: max(280, offset), height: Metrics.rowHeight)
            .background(Color.amethyst)
            .frame(maxWidth: .infinity, maxHeight: .infinity,
              alignment: mirrored ? .trailing : .leading)
            .accessibilityLabel(activity.accessibility)
            .allowsHitTesting(false)
            .transition(.identity)
        } else if offset < 0 {
          HStack(spacing: 0) {
            if mirrored { action("Trash", .alizarin) { remove() } }
            action("Block", Color(.systemGray)) { close(); block() }  // not a bar's own colour
            if !mirrored { action("Trash", .alizarin) { remove() } }
          }
          .frame(width: max(Self.width, -offset))
          .transition(.identity)
        }
      }
      .gesture(
        HorizontalPan(enabled: enabled) { x in
          if dragged == nil { began() }
          dragged = x * direction
        } ended: { x, velocity in
          let end = resting + x * direction
          let speed = velocity * direction
          if let request, abs(end) > 100 || (abs(end) > 20 && abs(speed) > 700 && end * speed > 0) {
            request(end < 0)
          }
          withAnimation(.spring(duration: 0.35, bounce: 0.2)) {
            open = request == nil && actionsEnabled && end < 0 && (speed < -400 || (speed < 400 && end < -Self.width / 2))
            dragged = nil
          }
        }
      )
      .onChange(of: enabled) { _, on in
        if !on { close() }
      }
      .accessibilityActions {
        if enabled && request == nil {
          Button("Block") { block() }
          Button("Trash") { remove() }
        }
      }
  }

  private func close() {
    withAnimation(.snappy(duration: 0.3)) { open = false }
  }

  private func action(_ title: String, _ color: Color, _ run: @escaping () -> Void) -> some View {
    Button(action: run) {
      Text(title).font(.yo(Metrics.rowFont))
      .lineLimit(1).minimumScaleFactor(0.6)
      .foregroundStyle(.white)
      .padding(.horizontal, 16)
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(color)
    }
    .buttonStyle(.plain)
  }
}

/// Observes the entire touch without competing with scroll, row or edge pans. Once moved,
/// a touch stays ineligible even if it returns to its origin before the Button fires.
@MainActor
final class BarTouchGate {
  var blocked = false
  weak var touch: UITouch?
  var origin = CGPoint.zero

  var isStationary: Bool {
    !blocked && !(touch?.gestureRecognizers ?? []).contains {
      $0 is UIPanGestureRecognizer && ($0.state == .began || $0.state == .changed || $0.state == .ended)
    }
  }
}

struct BarTouchObserver: UIGestureRecognizerRepresentable {
  let gate: BarTouchGate
  var held: (Bool) -> Void = { _ in }
  var observesHold = false
  var pressed: (Bool) -> Void = { _ in }
  let cancelled: () -> Void

  func makeUIGestureRecognizer(context: Context) -> Observer {
    let observer = Observer()
    observer.gate = gate
    observer.cancelled = cancelled
    observer.pressed = pressed
    observer.held = held
    observer.observesHold = observesHold
    observer.cancelsTouchesInView = false
    observer.delaysTouchesBegan = false
    observer.delaysTouchesEnded = false
    return observer
  }

  func updateUIGestureRecognizer(_ observer: Observer, context: Context) {
    observer.cancelled = cancelled
    observer.pressed = pressed
    observer.held = held
    observer.observesHold = observesHold
  }

  func handleUIGestureRecognizerAction(_ observer: Observer, context: Context) {}

  final class Observer: UIGestureRecognizer {
    var gate: BarTouchGate?
    var cancelled: () -> Void = {}
    var held: (Bool) -> Void = { _ in }
    var observesHold = false
    var pressed: (Bool) -> Void = { _ in }
    private var holdTask: Task<Void, Never>?
    private var holding = false

    override func reset() {
      holdTask?.cancel()
      pressed(false)
      holding = false
      super.reset()
    }

    override func canPrevent(_ other: UIGestureRecognizer) -> Bool { false }
    override func canBePrevented(by other: UIGestureRecognizer) -> Bool { false }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
      guard let gate, let touch = touches.first else { return }
      gate.blocked = touches.count != 1
      gate.touch = touch
      gate.origin = touch.location(in: nil)
      pressed(!gate.blocked)
      if observesHold {
        holdTask = Task { @MainActor [weak self] in
          do { try await Task.sleep(for: .milliseconds(300)) } catch { return }
          guard let self, self.gate?.isStationary == true else { return }
          self.holding = true
          self.held(true)
        }
      }
    }

    private func checkMovement() {
      guard let gate, let touch = gate.touch, !gate.blocked else { return }
      let point = touch.location(in: nil)
      if hypot(point.x - gate.origin.x, point.y - gate.origin.y) > 10 || !gate.isStationary {
        gate.blocked = true
        holdTask?.cancel()
        pressed(false)
        cancelled()
      }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) { checkMovement() }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
      checkMovement()
      holdTask?.cancel()
      if holding { held(false) }
      pressed(false)
      state = .failed
    }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
      gate?.blocked = true
      holdTask?.cancel()
      pressed(false)
      cancelled()
      state = .failed
    }
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
    pan.maximumNumberOfTouches = 1
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
    func gestureRecognizer(_ gesture: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
      guard let window = gesture.view?.window else { return false }
      let x = touch.location(in: window).x
      return x > 24 && x < window.bounds.width - 24
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer,
                           shouldRequireFailureOf other: UIGestureRecognizer) -> Bool {
      other is UIScreenEdgePanGestureRecognizer
    }

    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
      guard let pan = gesture as? UIPanGestureRecognizer else { return false }
      let velocity = pan.velocity(in: pan.view)
      return abs(velocity.x) > abs(velocity.y) * 1.5
    }
  }
}

/// A horizontal pan beginning within the outer 8% of either edge; vertical scrolls keep priority.
struct CameraEdgePan: UIGestureRecognizerRepresentable {
  let left: Bool
  let enabled: Bool
  let touched: () -> Void
  let released: () -> Void
  let changed: (CGFloat) -> Void
  let ended: (CGFloat, CGFloat, Bool) -> Void

  func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
    let pan = EdgeRecognizer()
    pan.released = released
    pan.maximumNumberOfTouches = 1
    context.coordinator.left = left
    context.coordinator.touched = touched
    pan.delegate = context.coordinator
    return pan
  }

  func updateUIGestureRecognizer(_ pan: UIPanGestureRecognizer, context: Context) {
    // Store the next edge even while the previous pan is ending. UIKit resets to
    // .possible later, without necessarily triggering another SwiftUI update.
    context.coordinator.left = left
    context.coordinator.touched = touched
    (pan as? EdgeRecognizer)?.released = released
    pan.isEnabled = enabled
  }

  func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
    let x = pan.translation(in: pan.view).x
    switch pan.state {
    case .began, .changed: changed(x)
    case .ended: ended(x, pan.velocity(in: pan.view).x, false)
    case .cancelled, .failed: ended(x, 0, true)
    default: break
    }
  }

  func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

  /// UIKit may reject a vertical edge touch without ever beginning the pan.
  /// Release its speculative capture start too.
  final class EdgeRecognizer: UIPanGestureRecognizer {
    var released: () -> Void = {}
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
      super.touchesEnded(touches, with: event)
      released()
    }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
      super.touchesCancelled(touches, with: event)
      released()
    }
  }

  final class Coordinator: NSObject, UIGestureRecognizerDelegate {
    var left = true
    var touched: () -> Void = {}
    private var touchLeft = true

    func gestureRecognizer(_ gesture: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
      guard let view = gesture.view else { return false }
      touchLeft = left
      let x = touch.location(in: view).x
      let zone = view.bounds.width * (touchLeft ? 0.25 : 0.08)
      let accepted = touchLeft ? x <= zone : x >= view.bounds.width - zone
      if accepted { touched() }
      return accepted
    }

    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
      guard let pan = gesture as? UIPanGestureRecognizer else { return false }
      let delta = pan.translation(in: pan.view)
      let inward = touchLeft ? delta.x : -delta.x
      let accepted = inward > abs(delta.y) * 1.5
      return accepted
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer,
                           shouldRequireFailureOf other: UIGestureRecognizer) -> Bool {
      false
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer,
                           shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
      // Vertical scrolling never waits for the camera edge decision.
      // Buttons still wait so an inward swipe cannot also fire the shutter.
      !(other.view is UIScrollView) && other !== gesture
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
      other.view is UIScrollView && other is UIPanGestureRecognizer
    }
  }
}

/// A video recording: from the bar's left, darker as its time runs to the longest a video may be, breathing once a
/// second.
struct RecordingFill: View {
  let seconds: Double
  @State private var start = Date.now

  var body: some View {
    TimelineView(.animation(minimumInterval: 1.0 / 120)) { context in
      let elapsed = context.date.timeIntervalSince(start)
      let breath = 0.5 + 0.5 * cos(elapsed * 2 * .pi)
      Canvas { context, size in
        context.fill(Path(CGRect(x: 0, y: 0, width: size.width * min(1, elapsed / seconds), height: size.height)), with: .color(.black.opacity(0.16 + 0.08 * breath)))
      }
    }
    .allowsHitTesting(false)
    .accessibilityHidden(true)
  }
}

/// A single style keeps the label alive across camera changes. Animate feedback modifiers only.
private struct BarPress: ButtonStyle {
  let cameraOn: Bool
  let touchDown: Bool
  let recording: Bool
  let press: (Bool) -> Void

  func makeBody(configuration: Configuration) -> some View {
    let pressed = cameraOn ? touchDown : configuration.isPressed
    let down = cameraOn ? touchDown || recording : configuration.isPressed
    configuration.label
      .animation(.easeOut(duration: cameraOn ? 0.06 : 0.15)) { content in
        content
          .brightness(cameraOn && down ? -0.12 : 0)
          // In the list only a quick flash, as the original Yo: no shrinking.
          .overlay(.white.opacity(!cameraOn && down ? 0.18 : 0))
      }
      .onChange(of: pressed) { _, pressed in
        if pressed { Haptic.prepare() }
        press(pressed)
      }
  }
}

/// Photos and videos, as Stories: the whole screen, one after the other (oldest first), a line for each at the top
/// filling up; a photo at most `photoSeconds`, a video as long as it runs. A finger held anywhere pauses it (the line
/// and the video), letting go goes on; a tap on the left third goes back, elsewhere to the next. Back reaches all
/// shown while the viewer is open (they are gone once it closes). Past the last it closes; swiped down it closes and
/// the unseen rest goes with it, as if viewed. Each is
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

  // As stories show it: a band at the top under the Dynamic Island with the lines and who it is from, the picture
  // fitted below it, never under them; the status bar and the home indicator out of the way.
  var body: some View {
    ZStack {
      Color.black.ignoresSafeArea()
      if index < queue.count {
        let current = queue[index]
        VStack(spacing: 10) {
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
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 4)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Yo from \(current.from)")
          }
          .padding(.horizontal, 8)
          .padding(.top, 4)
          Group {
            if current.video != nil, let player {
              PlayerLayer(player: player)
            } else if let photo = full ?? current.photo {
              Image(uiImage: photo).resizable().scaledToFit().clipShape(.rect(cornerRadius: 14))
            }
          }
          .clipShape(.rect(cornerRadius: 14))
          .frame(maxWidth: .infinity, maxHeight: .infinity)
          .padding(.horizontal, 4)
          .padding(.bottom, 4)  // above the home indicator, clear of the screen's round corners
        }
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
        if moved.height > 90 && moved.height > abs(moved.width) { return swipedAway() }
        withAnimation(.snappy(duration: 0.25)) { pulled = 0 }
        guard quick, abs(moved.width) < 20, abs(moved.height) < 20 else { return }
        go(to: drag.startLocation.x < width / 3 ? index - 1 : index + 1)
      }
  }

  /// Swiped away: whatever is still unseen goes too, at once, as if each had been viewed (view once: deleted, its
  /// keys with it). No question; a light tap says they are gone.
  private func swipedAway() {
    player?.pause()
    let rest = queue.indices.contains(index) ? Array(queue[index...]) : []
    if rest.count > 1 || (rest.count == 1 && seconds == nil) { Haptic.tap(.light) }
    for arrival in rest { shows(arrival) }
    finished()
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

  /// The one to show made ready: a video loaded and its length known, then reported (seen only once it shows: one
  /// closed while loading stays). Past the last: closed.
  private func load() async {
    guard index < queue.count else { return finished() }
    let current = queue[index]
    player?.pause()
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
    self.player = player
    let length = (try? await AVURLAsset(url: video).load(.duration).seconds) ?? 0
    guard !Task.isCancelled else { return }
    shows(current)
    // A video is 10 s at most (spec 5): a longer one, from a sender that ignores it, is cut there.
    seconds = length > 0 ? min(length, Self.longestVideo) : photoSeconds
  }

  static let longestVideo = 10.5

  /// A received photo decoded at most 2048 px on its longer side, whatever size its sender gave it (spec 5: the
  /// receiver checks the bytes, not the sender's word): a huge one costs no memory beyond that.
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

/// A Yo sent from a bar: "Yo" in the bar's type rises out of it and fades, as if thrown up and away.
struct FlyingYo: View {
  let done: () -> Void
  @State private var gone = false
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  var body: some View {
    Text("Yo")
      .foregroundStyle(.white)
      .offset(y: gone && !reduceMotion ? -90 : 0)
      .scaleEffect(gone ? 1.15 : 1)
      .opacity(gone ? 0 : 1)
      .onAppear {
        withAnimation(.easeOut(duration: 0.55)) { gone = true } completion: { done() }
      }
      .accessibilityHidden(true)
  }
}

/// Bounds come from the actual row subtree: scrolling, expansion and reordering have one layout.
private struct ListColorEntry {
  let id: UUID
  let bounds: Anchor<CGRect>
  let color: Color
  let opacity: Double
  var clips: [Anchor<CGRect>] = []
}

private struct ListColorKey: PreferenceKey {
  static var defaultValue: [ListColorEntry] { [] }
  static func reduce(value: inout [ListColorEntry], nextValue: () -> [ListColorEntry]) {
    value.append(contentsOf: nextValue())
  }
}

/// Carry local clipping into the shared colour plane during expanding action rows.
private struct ListColorClip: ViewModifier {
  func body(content: Content) -> some View {
    content.transformAnchorPreference(key: ListColorKey.self, value: .bounds) { entries, bounds in
      for index in entries.indices { entries[index].clips.append(bounds) }
    }
  }
}

private struct SharedListColorsKey: EnvironmentKey {
  static let defaultValue = false
}

private extension EnvironmentValues {
  var sharedListColors: Bool {
    get { self[SharedListColorsKey.self] }
    set { self[SharedListColorsKey.self] = newValue }
  }
}

private struct ListColor: ViewModifier {
  @Environment(\.sharedListColors) private var shared
  @State private var id = UUID()
  let color: Color
  var opacity: Double = 1

  func body(content: Content) -> some View {
    if shared {
      content.anchorPreference(key: ListColorKey.self, value: .bounds) {
        [ListColorEntry(id: id, bounds: $0, color: color, opacity: opacity)]
      }
    } else {
      content.background(color.opacity(opacity))
    }
  }
}

/// Only the shared preview is clipped; the row colours and foreground never have wipe masks.
private struct CameraWipeShape: Shape {
  var progress: Double
  var left: Bool
  var animatableData: Double {
    get { progress }
    set { progress = newValue }
  }
  func path(in rect: CGRect) -> Path {
    let width = rect.width * progress
    return Path(CGRect(x: left ? 0 : rect.width - width, y: 0,
                       width: max(0, width), height: rect.height))
  }
}

/// Row-local observation keeps flight layering from invalidating the whole list on every send.
private struct FlightRowLayer: ViewModifier {
  let bar: FriendList.Bars.Bar
  func body(content: Content) -> some View {
    content.zIndex(bar.hasNotice || !bar.outgoing.isEmpty ? 10 : 0)
  }
}
