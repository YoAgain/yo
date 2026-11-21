// The list, as in 2014: one coloured bar per friend from edge to edge, the name large in the middle; a tap sends a
// Yo. Whoever you last sent a Yo to or got one from is on top; each friend keeps their colour as they move. Camera
// mode (an edge swipe or the button) turns the bars see-through over the live picture: a tap sends a photo, a hold
// records a video, both waiting `sendDelay` under an undo ring.
import PhotosUI
import SwiftUI
import UIKit
import UserNotifications
import YoCore
import _PhotosUI_SwiftUI
import os

struct FriendList: View {
  static let photoSeconds = 3.0  // Snapchat's first default
  static let listTiming = listTimings()
  static let sendDelay = Double(listTiming.undoMs) / 1000
  static let yoSeconds = Double(listTiming.yoSolidMs + listTiming.yoFadeMs) / 1000
  static let yoSolidSeconds = Double(listTiming.yoSolidMs) / 1000

  @Environment(\.scenePhase) var scenePhase
  @Environment(Inbox.self) var inbox
  @Environment(AccountStore.self) var accounts
  @Environment(Friends.self) var friends
  @Environment(MediaOutbox.self) var outbox

  final class MediaInputCache {
    var each: [String: [Bool]] = [:]
    /// The bars last told "Not Sent · Tap to Retry" (`syncUnsent`).
    var unsent: Set<String> = []
  }
  @State var listMedia = MediaInputCache()
  @State var listModel = DeleteNotice.shared.model
  @State var listOrder: [String] = []
  @State var listDeadline: UInt64?
  @State var lockedRow: String?
  var listNow: UInt64 { DeleteNotice.now }
  /// Caches render data only; the core makes every ordering decision.
  final class RenderedRows {
    var source: [Friend] = []
    var order: [String] = []
    var result: [Friend] = []
    func resolve(_ all: [Friend], ids: [String]) -> [Friend] {
      if order != ids || !source.elementsEqual(all, by: { $0.contentMatches($1) }) {
        source = all; order = ids
        let values = Dictionary(all.map { ($0.key.uuidString, $0) }, uniquingKeysWith: { a, _ in a })
        result = ids.compactMap { values[$0] }
      }
      return result
    }
  }
  @State var renderedRows = RenderedRows()
  var orderedFriends: [Friend] { renderedRows.resolve(friends.all, ids: listOrder) }
  var listFriends: [ListFriend] {
    friends.shown.map { ListFriend(id: $0.key.uuidString, name: $0.name, request: $0.request, group: $0.group != nil) }
  }

  @State var adding = false
  @State var newGroup = false
  @State var newName = ""
  /// Shown in the empty field for a moment instead of "Name": the lookup, "No Such User", "That’s You".
  @State var hint: String?
  /// Only the newest submitted name's answer and hint touch the field.
  @State var submitted = 0
  @FocusState var typing: Bool
  @State var settings = false
  @State var settingsAction: SettingsView.ListAction?
  @State var position = ScrollPosition(idType: String.self)
  static let addBlockBottomID = "add-block-bottom"
  @State var keyboardDuration = 0.25
  @State var scrolled = false
  /// The bar swiped open (Block and Remove showing): one at a time.
  @State var swiped: UUID?

  /// A reference, so capture and upload tasks update the live row after another tap.
  @State var bars = Bars()

  @State var blocking: (friend: Friend, reported: Bool)?
  @State var leaving: Friend?
  @State var emptyGroup: Friend?
  @State var hintShown = false
  /// Contacts whose new key waits for a confirmation (verified before): only they get "Confirm New Key".
  @State var unconfirmed: Set<String> = []
  @State var hintEpoch = 0
  /// A photo or video went out since the camera opened: its hint is not needed again until the next opening.
  @State var hintNeeded = true
  @State var renaming: Friend?
  @State var newGroupName = ""
  @State var membersOf: Friend?
  @State var problem: (title: String, message: String)?
  /// A scanned or tapped invite, verified by the hub, waiting for "Add & Yo" (`invited`).
  @State var inviting: Invitation?
  @State var firstYo: Notifications?
  enum Notifications { case allowed, notAsked, denied }
  static let firstYoKey = "firstYo.shown"

  @Environment(\.accessibilityReduceMotion) var reduceMotion
  @State var edgeActive = false
  @State var edgeSettling = false
  @State var edgeOffset: CGFloat = 0
  @State var cameraOn = false
  @State var leftAt: Date?
  @State var cameraPrepared = false
  @State var listScrolling = false
  /// The bars that just changed place: a tap on them waits a moment.
  @State var movedLock: (ids: Set<String>, until: Date)?
  /// While bars are tapped the order stays: the newest waits here until `tapQuiet` passed and no finger is down.
  @State var tapQuiet = Date.distantPast
  @State var fingersDown: Set<UUID> = []
  @State var heldOrder: [String]?
  @State var heldSince: Date?
  @State var heldFlush: Task<Void, Never>?
  static let tapQuietSeconds = 1.5
  static let reorderTapLock = 0.5
  static let heldOrderCap = 8.0
  @AppStorage(SettingKey.leftHanded) var leftHanded = false
  @State var camera = Camera()
  @State var zooms: [Double] = []
  @State var zoom = 1.0
  @State var pinchStart: Double?
  /// The most the camera zooms, as the choices count.
  @State var zoomMost = 1.0
  /// While filming: the zoom it started at; the finger drawn down zooms from there (`recordZoom`).
  @State var recordZoomStart: Double?
  @State var recordZoomTicked = false
  @State var recordingFor: Friend?
  /// Each hold's own mark: the 10 s limit sends only the video it was started for.
  @State var recordingRound: UUID?
  @State var libraryItems: [_PhotosUI_SwiftUI.PhotosPickerItem] = []
  @State var picked: [Picked] = []
  /// The trimmer, open for a picked video longer than a Yo's.
  @State var videoTrim: VideoTrim?

  @State var cheers = 0
  @State var landings = Landings()

  @Observable final class LandingSlot { var value: Landing? }
  final class Landings {
    var slots: [String: LandingSlot] = [:]
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
  @State var clocks = Clocks()
  var keyChanged: Friend? {
    friends.keyAlerts.lazy.compactMap { key in friends.all.first { $0.key == key } }.first
  }

  @State var viewing: [Arrival]?
  /// The finger still holding the sign that opened `viewing`: read by the viewer alone, so the list is not redrawn.
  @State var heldFinger = HeldFinger()
  @State var shown: [Arrival] = []

  /// Wakes the core at `listDeadline`; every deadline and lifetime belongs to the core.
  @MainActor final class Clocks: NSObject {
    var displayLink: CADisplayLink?
    var update: (() -> Void)?
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
    @objc func tick() { update?() }
  }

  /// A bar's run of Yos: each new one adds a "!" and restarts its time; after `yoSolidSeconds` on screen the last
  /// fades out.
  struct Landing: Equatable {
    var first: String
    /// The "!"s shown now: all of the run's Yos, fewer while its time counts them down.
    var count: Int
    /// All Yos of this run: a Yo coming while the "!"s go down shows them all again, and one more.
    var total: Int
    var fading: Date?
    var paused = false
    var fadeElapsed = 0.0
  }

  /// Split from `body`: one chain of modifiers less to type-check.
  var asking: some View {
    navigation
      .alert(
      "Leave \(leaving?.name ?? "")?",
      isPresented: $leaving.isPresent, presenting: leaving
    ) { bar in
      Button("Leave", role: .destructive) { leave(bar) }
      Button("Cancel", role: .cancel) {}
    } message: { _ in
      Text("You won’t get its Yos any more. A member can add you again.")
    }
    .alert(
      inviting?.name ?? "", isPresented: $inviting.isPresent, presenting: inviting
    ) { invitation in
      if invitation.problem != nil {
        Button("OK", role: .cancel) {}
      } else {
        Button(productText(text: invitation.known ? .yo : .addAndYo, lang: .english)) { acceptInvite(invitation) }
          .keyboardShortcut(.defaultAction)
        Button(productText(text: .cancel, lang: .english), role: .cancel) {}
      }
    } message: { invitation in
      Text(invitation.problem ?? productText(text: invitation.known ? .inviteYo : .inviteAddYo, lang: .english))
    }
    .alert(
      productText(text: .groupEmpty, lang: .english),
      isPresented: $emptyGroup.isPresent, presenting: emptyGroup
    ) { bar in
      Button("Delete Group", role: .destructive) { deleteEmpty(bar) }
      Button("Cancel", role: .cancel) {}
    } message: { bar in
      Text("Nobody else is in \(bar.name) any more. Delete it?")
    }
  }

  func chooseRequest(_ friend: Friend, yes: Bool) {
    Haptic.tap()
    answer(friend, yes: yes)
  }

  /// A request answered: yes makes a contact (or joins the group); no takes the bar off the list.
  func answer(_ bar: Friend, yes: Bool) {
    guard let client = accounts.client else { return Problems.shared.report(.failed) }
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
          Problems.shared.report(error)
          Logger(subsystem: "app.justyo", category: "requests").error(
            "accept \(bar.group == nil ? "contact" : "group", privacy: .public): \(String(describing: error), privacy: .public)")
        }
        guard let events else { showSent(to: bar, "Not Accepted"); return }
        await inbox.absorb(events, client: client, friends: friends)
        if let contacts = try? await Hub.run({ client.contacts() }) { friends.markRequests(contacts) }
        if let groups = try? await Hub.run({ try client.groups() }) { friends.takeGroups(groups) }
        guard client === accounts.client else { return }
        await yoNow(bar)  // accepting sends a Yo
      } else if let group = bar.group {
        await leaveGroup(group, client: client)
      } else {
        do {
          try await Hub.run { try client.decline(who: bar.target) }
        } catch {
          Problems.shared.report(error)
          return  // still asking: the request stays, to answer again
        }
        withAnimation(.snappy) { friends.remove(bar) }
      }
    }
  }

  var body: some View {
    let _ = Perf.mark("list.body")
    return reacting(
      asking
        // The list is always dark; the sheets follow the iPhone's own appearance.
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
          .appObjects()
        }
        .sheet(isPresented: $newGroup) { NewGroupSheet().preferredColorScheme(SystemAppearance.scheme).appObjects() }
        .videoTrimming($videoTrim)
        .confirmDestructive(
          $blocking,
          title: { $0.reported ? "Thanks for Reporting" : "Block \($0.friend.name)?" },
          message: {
            $0.reported
              ? "Block \($0.friend.name) too? You won’t get their Yos."
              : "You won’t get their Yos. Unblock them under Yodentity › \(accounts.account?.username ?? "your name") › Blocked."
          },
          action: { "Block \($0.friend.name)" },
          cancel: blocking?.reported == true ? "Not Now" : "Cancel",
          run: { block($0.friend) })
        .alert(
          "Rename \(renaming?.name ?? "")",
          isPresented: $renaming.isPresent, presenting: renaming
        ) { bar in
          TextField("Group Name", text: $newGroupName)
          Button("Rename") { renameGroup(bar, to: newGroupName) }
            .disabled(newGroupName.trimmingCharacters(in: .whitespaces).isEmpty)  // an empty name was dropped unsaid
          Button("Cancel", role: .cancel) {}
        }
        .sheet(item: $membersOf) { bar in
          GroupMembersSheet(bar: bar).preferredColorScheme(SystemAppearance.scheme).appObjects()
        }
        .alert(
          problem?.title ?? "", isPresented: $problem.isPresent
        ) {
          Button("OK", role: .cancel) {}
        } message: {
          Text(problem?.message ?? "")
        }
        .alert(
          "First Yo Sent!", isPresented: $firstYo.isPresent,
          presenting: firstYo
        ) { notifications in
          if notifications == .allowed {
            Button("Awesome", role: .cancel) {}
          } else {
            Button(productText(text: .turnOnNotifications, lang: .english)) { Task { await turnOnNotifications(notifications) } }
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
            HeldMedia(queue: viewing, photoSeconds: Self.photoSeconds, finger: heldFinger) { arrival in
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

  var listNavigation: some View {
    NavigationStack {
      ScrollView {
        LazyVStack(spacing: 0) {
          // Always in the stack, empty in camera mode: taking the stack's first child out on the switch lost the
          // scroll view's anchor, and the first bar slid up out of sight.
          NotificationsBar(hidden: cameraOn)
          // The shade in the data: `Friend`'s == is its key alone, so after a reorder the ForEach kept old shades.
          ForEach(shadedFriends, id: \.friend.key) { shaded in
            let friend = shaded.friend
            row(friend, shade: shaded.shade)
              .modifier(FlightRowLayer(bar: bars.of(friend)))
              .onGeometryChange(for: Bars.Place?.self) { proxy in
                proxy.bounds(of: .scrollView).map { Bars.Place(frame: proxy.frame(in: .scrollView), viewport: $0.height) }
              } action: { bars.of(friend).place = $0 }
              .id(friend.key.uuidString)
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
      // Pinch to zoom in camera mode, as the Camera app: from its widest choice up to 10x.
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
      // A finger recording a video, or holding photos open, may wander: the list stays put until it lifts.
      .scrollDisabled(recordingFor != nil || viewing != nil)
      .scrollIndicators(.hidden)
      .scrollBounceBehavior(.basedOnSize)
      .scrollDismissesKeyboard(.interactively)
      .scrollPosition($position)
      .defaultScrollAnchor(.top, for: .sizeChanges)
      .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { note in
        keyboardDuration = (note.userInfo?[UIResponder.keyboardAnimationDurationUserInfoKey] as? Double) ?? 0.25
      }
      .onScrollGeometryChange(for: CGFloat.self) { $0.containerSize.height } action: { old, height in
        // Keyboard avoidance laid out the new viewport: align the whole add block's bottom.
        if typing && adding && height < old { revealAddBlock() }
      }
      .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardDidChangeFrameNotification)) { _ in
        if typing && adding { revealAddBlock() }
      }
      .onScrollTargetVisibilityChange(idType: String.self, threshold: 0.01) { ids in
        publish(listModel.rowsVisible(now: listNow, ids: ids))
      }
      // No bar behind the controls: Amethyst exactly at the top; the status bar keeps the system's soft edge.
      .toolbarBackgroundVisibility(.hidden, for: .navigationBar)
      .scrollEdgeEffectStyle(.soft, for: .top)
      // The glass controls float over the bars: no dimmed band behind them at the bottom.
      .scrollEdgeEffectHidden(true, for: .bottom)
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
                .animation(nil, value: entry.opacity)  // at once, in one frame
                .frame(width: max(0, bounds.width), height: max(0, bounds.height))
                .offset(x: bounds.isNull ? 0 : bounds.minX, y: bounds.isNull ? 0 : bounds.minY)
                .transition(.opacity)
            }
            cameraBackdrop
              .frame(width: geometry.size.width, height: geometry.size.height)
              .clipped()
              // See-through bars over the colours; a pressed bar clearer still (its flash), never coloured.
              .mask {
                ZStack(alignment: .topLeading) {
                  Color.black.opacity(0.55)
                  ForEach(colors, id: \.id) { entry in
                    let bounds = entry.clips.reduce(geometry[entry.bounds]) { $0.intersection(geometry[$1]) }
                    Color.black.opacity(entry.pressed ? 0.85 : 0)
                      .animation(nil, value: entry.pressed)  // see-through at once and back at once, no fade
                      .frame(width: max(0, bounds.width), height: max(0, bounds.height))
                      .offset(x: bounds.isNull ? 0 : bounds.minX, y: bounds.isNull ? 0 : bounds.minY)
                  }
                }
              }
              .clipShape(CameraWipeShape(progress: cameraLook, left: leftHanded))
          }
          .allowsHitTesting(false)
        }
        .ignoresSafeArea()
      }
      // In camera mode what to do, in glass just over the controls (at the top it would cover a bar's name).
      .overlay(alignment: .bottom) {
        VStack(spacing: 10) {
          if cameraOn && hintShown { title }
        }
        // Only after 3 s idle in camera mode, and not again once a photo or video went out.
        .task(id: cameraOn ? hintEpoch : -1) {
          withAnimation(.smooth(duration: 0.2)) { hintShown = false }
          guard cameraOn, hintNeeded else { return }
          try? await Task.sleep(for: .seconds(3))
          guard !Task.isCancelled else { return }
          withAnimation(.smooth(duration: 0.4)) { hintShown = true }
        }
      }
      .toolbar {
        ToolbarItemGroup(placement: .bottomBar) {
          if leftHanded {
            cameraButton
            // Only while the camera is open: the bar draws its glass even for a see-through item.
            if cameraOn && !selecting { flipButton }
          } else { leadingControl }
        }
        ToolbarSpacer(.flexible, placement: .bottomBar)
        // The zoom as its own glass circle (a text item in the bar's shared glass turns into a capsule).
        ToolbarItem(placement: .bottomBar) {
          if (cameraOn || cameraPrepared) && picked.isEmpty {
            CameraZoom(choices: zooms, chosen: $zoom, pinching: pinchStart != nil || recordZoomStart != nil)
              .opacity(cameraOn ? 1 : 0).allowsHitTesting(cameraOn).accessibilityHidden(!cameraOn)
              .onChange(of: zoom) {
                camera.zoom(to: zoom, immediate: pinchStart != nil || recordZoomStart != nil || reduceMotion)
              }
          }
        }
        .sharedBackgroundVisibility(.hidden)
        ToolbarSpacer(.flexible, placement: .bottomBar)
        ToolbarItemGroup(placement: .bottomBar) {
          if !leftHanded && cameraOn && !selecting { flipButton }
          if leftHanded { leadingControl } else { cameraButton }
        }
      }
      .navigationBarTitleDisplayMode(.inline)
      .toolbarVisibility(.hidden, for: .navigationBar)  // the list from the top, no heading
    }
  }

  struct ShadedFriend: Equatable {
    let friend: Friend
    let depth: Int
    var shade: Color {
      depth == 0 ? .row(friend.color) : Color.row(friend.color).mix(with: .black, by: min(0.12 * Double(depth), 0.36))
    }
  }

  /// The same colour right under itself gets a step darker each time, so equal bars never melt into one block.
  var shadedFriends: [ShadedFriend] {
    var depth = 0
    var previous: Int?
    return orderedFriends.map { friend in
      let own = friend.color % Color.rows.count
      depth = previous == own ? depth + 1 : 0
      previous = own
      return ShadedFriend(friend: friend, depth: depth)
    }
  }

  @ViewBuilder func row(_ friend: Friend, shade: Color) -> some View {
    let waiting = inbox.media(for: friend) ?? []
    let bar = LiveBar(
      friend: friend,
      bar: bars.of(friend),
      color: shade,
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
      holdDrag: { recordZoom(friend, y: $0, bar: $1, height: $2) },
      press: { pressed(friend.key, $0) },
      flyBegan: { id in
        HitchScene.event("photo stack fly", then: cameraOn ? "camera" : "list")
        bars.awaitingThumbnail.remove(id)
        flushRise()
      },
      view: { see(friend, $0) }
    ) { origin in
      send(to: friend, origin: origin)
    }
    SwipeActions(
      open: Binding(
        get: { swiped == friend.key },
        set: { open in swiped = open ? friend.key : swiped == friend.key ? nil : swiped }),
      enabled: !cameraOn,
      request: friend.request ? { yes in chooseRequest(friend, yes: yes) } : nil,
      actionsEnabled: friend.group == nil,
      mirrored: leftHanded,
      activity: lastActivity(for: friend),
      began: { if swiped != friend.key { withAnimation(.snappy) { swiped = nil } } },
      block: { blocking = (friend, false) },
      remove: { remove(friend) }
    ) {
      VStack(spacing: 0) {
        // The bar itself lifts on a long press (a separate preview got the menu's inset platter), with its own colour
        // under it; in camera mode and during the edge wipe it stays see-through.
        bar.equatable()
          .background(cameraOn || edgeActive || edgeSettling ? Color.clear : shade)
          .contentShape(.contextMenuPreview, Rectangle())
          .contextMenu {
            if !cameraOn && !friend.request {
              menu(for: friend)
            }
          }
        if friend.request {
          requestChoices(friend, shade: shade)
            .frame(height: Metrics.rowHeight)
        }
      }
        .accessibilityActions {
          if friend.request {
            Button("Accept and Yo") { chooseRequest(friend, yes: true) }
            Button("Decline") { chooseRequest(friend, yes: false) }
          }
        }
    }
  }

  func requestChoices(_ friend: Friend, shade: Color) -> some View {
    GeometryReader { geometry in
      HStack(spacing: 0) {
        if leftHanded { requestChoice(friend, shade: shade, yes: true, width: geometry.size.width * 2 / 3) }
        requestChoice(friend, shade: shade, yes: false, width: geometry.size.width / 3)
        if !leftHanded { requestChoice(friend, shade: shade, yes: true, width: geometry.size.width * 2 / 3) }
      }
    }
  }

  func requestChoice(_ friend: Friend, shade: Color, yes: Bool, width: CGFloat) -> some View {
    Button { chooseRequest(friend, yes: yes) } label: {
      Text(yes ? "Yo" : "No")
        .font(.yo(Metrics.rowFont))
        .foregroundStyle(.white)
        .frame(width: width)
        .frame(maxHeight: .infinity)
        .background { addBackground(yes ? shade : Color(hex: 0xE74C3C)) }
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(yes ? "Accept and Yo" : "Decline")
  }

  func lastActivity(for friend: Friend) -> BarActivity {
    let out = Stats.lastSent(to: friend.name), back = Stats.lastReceived(from: friend.name)
    let mine = (out ?? .distantPast) >= (back ?? .distantPast)
    let date = mine ? out : back
    let text = date.map { let age = Arrival.age($0); return age == "now" ? "Yo now" : "Yo \(age) ago" } ?? "No Yo yet"
    return BarActivity(text: text, icon: date == nil ? "clock" : mine ? "arrow.up" : "arrow.down",
      accessibility: date == nil ? text : "Last Yo \(mine ? "sent" : "received"), \(text)")
  }

  /// A bar's menu: a friend's Report, Block and Remove; a group's Members, Rename and Leave.
  @ViewBuilder func menu(for friend: Friend) -> some View {
    // The last Yo, either way, greyed at the menu's top: "↑ Yo 7m ago" sent, "↓ Yo 1d ago" received.
    let activity = lastActivity(for: friend)
    let run = bars.of(friend)
    let stacked = (run.outgoing.isEmpty ? run.sendingBatch?.items ?? [] : run.outgoing)
      .filter { productMediaKind(label: $0.text) != .yo }.count
    if stacked > 0 {
      Button(stacked == 1 ? "1 photo waiting to send" : "\(stacked) photos waiting to send", systemImage: "photo.stack") {}
        .disabled(true)
    }
    Button(activity.text, systemImage: activity.icon) {}.disabled(true)
      .accessibilityLabel(activity.accessibility)
    if friend.group != nil {
      Button("Members", systemImage: "person.3") { membersOf = friend }
      Button("Rename", systemImage: "pencil") {
        newGroupName = friend.name
        renaming = friend
      }
      Button("Leave \(friend.name)", systemImage: "rectangle.portrait.and.arrow.right", role: .destructive) {
        leaving = friend
      }
    } else {
      Menu("Block", systemImage: "hand.raised") {
        Button("Block", systemImage: "hand.raised") { blocking = (friend, false) }
        Section("Report") {
          Button("Spam") { report(friend, .spam) }
          Button("Abuse") { report(friend, .abuse) }
          Button("Something Else") { report(friend, .other) }
        }
      }
      Button("Remove", systemImage: "trash", role: .destructive) { remove(friend) }
    }
  }

  func renameGroup(_ bar: Friend, to name: String) {
    let name = name.trimmingCharacters(in: .whitespaces)
    guard let id = bar.group, let client = accounts.client, !name.isEmpty else { return }
    Task {
      let events: [YoEvent]
      do {
        events = try await Hub.run { try client.renameGroup(group: id, name: Self.groupName(name)) }
      } catch {
        Problems.shared.report(error)
        return
      }
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

  func leave(_ bar: Friend) {
    guard let id = bar.group, let client = accounts.client else { return }
    Task {
      // "LEO left" first, and out only once it went (at most half a minute): a member who left sends nothing.
      if let notice = await GroupNotice.send([.user(GroupNotice.me(accounts)), " left ", .group(bar.name)], to: bar, accounts: accounts, outbox: outbox) {
        for _ in 0..<60 where outbox.isWaiting(notice) { try? await Task.sleep(for: .milliseconds(500)) }
      }
      await leaveGroup(id, client: client)
    }
  }

  /// Out of a group at the hub, then the list as the core has it; a failure is said, the group stays.
  func leaveGroup(_ id: String, client: YoClient) async {
    do {
      let events = try await Hub.run { try client.leaveGroup(group: id) }
      await inbox.absorb(events, client: client, friends: friends)
    } catch {
      Problems.shared.report(error)
    }
    if let groups = try? await Hub.run({ try client.groups() }) { withAnimation(.snappy) { friends.takeGroups(groups) } }
  }

  /// A group of this account alone, deleted: left without a picture (nobody would see it), its bar gone.
  func deleteEmpty(_ bar: Friend) {
    guard let id = bar.group, let client = accounts.client else { return }
    Task { await leaveGroup(id, client: client) }
  }

  func remove(_ friend: Friend) {
    withAnimation(.snappy) {
      swiped = nil
      publish(listModel.delete(now: listNow, id: friend.key.uuidString))
    }
  }

  func report(_ friend: Friend, _ reason: ReportReason) {
    Task {
      do {
        try await accounts.report(friend.user ?? friend.name, reason: reason)
        blocking = (friend, true)
      } catch {
        Problems.shared.report(error)
      }
    }
  }

  func block(_ friend: Friend) {
    Task {
      do {
        try await accounts.setBlocked(friend.user ?? friend.name, true)
        withAnimation(.snappy) { friends.setBlocked(friend, true) }
      } catch {
        Problems.shared.report(error)
      }
    }
  }

  var settingsButton: some View {
    Button { settings = true } label: {
      // Y and o drawn apart, the o tucked under the Y's right arm, so the glass button stays round.
      HStack(alignment: .lastTextBaseline, spacing: -5) {
        Text("Y").font(.yo(29))
        Text("o").font(.yo(23))
      }
      .fixedSize()
      .frame(width: 32, height: 32)
    }
      .overlay(alignment: .topTrailing) { SupportBadge().offset(x: 4, y: -2) }
      .accessibilityLabel("Settings")
  }
}

#Preview {
  FriendList()
    .environment(Inbox())
    .environment(Friends())
}

/// An invite's friend, as the hub spells it: `known` when already on the list.
struct Invitation {
  let name: String
  let user: String?
  let known: Bool
  /// What is wrong instead ("No Such User", "That’s You"): the alert says so, with only OK.
  var problem: String? = nil
}
