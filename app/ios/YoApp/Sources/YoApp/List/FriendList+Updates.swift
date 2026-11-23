import PhotosUI
import SwiftUI
import UIKit
import YoCore

/// The list's reactions to what changes, and the core's snapshots rendered into the bars.
extension FriendList {
  func reacting(_ content: some View) -> some View {
    changes(content)
      .onChange(of: cheers) { Haptic.arrival() }
      .sensoryFeedback(.start, trigger: recordingFor) { (_: Friend?, new: Friend?) in new != nil }
      .sensoryFeedback(.selection, trigger: viewing) { (old: [Arrival]?, new: [Arrival]?) in old == nil && new != nil }
      .preferredColorScheme(.dark)  // white status bar and bar items on purple
  }

  func changes(_ content: some View) -> some View {
    content
      .onChange(of: inbox.arrived, initial: true) { arrived() }
      .onAppear {
        Perf.timed("list.appear.publish") { publish(listModel.foreground(now: listNow, active: scenePhase == .active)) }
        #if YO_DEV
        HitchMonitor.shared.start()
        HitchMonitor.shared.scene = "list"
        #endif
        PerfProbe.install()
        Haptic.prepareArrival()
        Perf.mark("launch.appear.end")
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
      .task { await inbox.listen(accounts: accounts, friends: friends) }
      .onChange(of: listFriends, initial: true) { syncListFriends() }
      .onChange(of: inbox.memberNames) { syncSenderNames() }
      .onChange(of: inbox.mediaChanges) { syncListMedia() }
      .onChange(of: outbox.unsentTo, initial: true) { syncUnsent() }
      .onChange(of: inbox.unsentBars) { syncUnsent() }
      .onAppear {
        // A photo or video that did not go at first and went now: its bar tells it.
        outbox.wentLater = { to, kind in
          guard let friend = friends.all.first(where: { $0.target == to }) else { return }
          showSent(to: friend, kind == .video ? BarSendText.sentVideo : BarSendText.sentPhoto, thenOnTop: true)
        }
      }
      .onChange(of: scenePhase, initial: true) { publish(listModel.foreground(now: listNow, active: scenePhase == .active)) }
      // A Yo sent from outside the list (Action button, Control Center, Siri, a notification's Yo Back): on top.
      .task { IntentSent.listen(); takeIntentSent() }
      .onChange(of: scenePhase) { _, phase in
        guard phase == .active else { return }
        takeIntentSent()
      }
      .onReceive(NotificationCenter.default.publisher(for: IntentSent.changed)) { _ in takeIntentSent() }
      // The top bar ("Offline", a deletion's undo) comes or goes above the list (YoApp), the list's top inset changing
      // with it: a list at its top stays at its top, its first bar whole under the bar.
      .onChange(of: DeleteNotice.shared.banner != nil || DeleteNotice.shared.deletion != nil) {
        guard !scrolled else { return }
        var instant = Transaction(); instant.disablesAnimations = true
        withTransaction(instant) { position.scrollTo(edge: .top) }
        // Again once the new inset is laid out (the first may still see the old one).
        DispatchQueue.main.async { withTransaction(instant) { position.scrollTo(edge: .top) } }
      }
      // An extension wrote the shared state: what it saved comes to the list at once.
      .task { StateChanged.listen() }
      .onReceive(NotificationCenter.default.publisher(for: StateChanged.changed)) { _ in
        guard scenePhase == .active else { return }
        Task { await inbox.takeSaved(accounts: accounts, friends: friends) }
      }
      // "Yo Camera" (Shortcuts, Siri, the Action button): open straight in camera mode, once no sheet is up.
      .onChange(of: AppRoute.shared.camera && !sheetUp, initial: true) { _, on in
        if on { AppRoute.shared.camera = false; if !cameraOn { cameraSwitched(true) } }
      }
      // A friend opened from Spotlight: their bar in view.
      .onChange(of: AppRoute.shared.friend, initial: true) { _, target in
        guard let target else { return }
        AppRoute.shared.friend = nil
        if cameraOn { cameraSwitched(false) }
        guard let friend = friends.shown.first(where: { $0.target == target }) else { return }
        withAnimation(.snappy) { position.scrollTo(id: friend.key.uuidString, anchor: .center) }
      }
      // A sheet coming up over the camera (none should): the list under it, not the camera.
      .onChange(of: sheetUp) { _, up in if up && cameraOn { cameraSwitched(false) } }
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
        publish(listModel.rowsVisible(now: listNow, ids: []))
        publish(listModel.foreground(now: listNow, active: false))
        clocks.stop()
      }
      // An invite link's name, once the hub can be asked (Invite.swift); kept while it does not answer, asked again
      // when the connection is back or the app comes to the front.
      .task(id: "\(Invite.shared.pending ?? "")/\(accounts.client != nil)/\(Availability.shared.offline)/\(scenePhase)") {
        guard let name = Invite.shared.pending, accounts.client != nil, !Availability.shared.offline,
          scenePhase == .active
        else { return }
        if await invited(name), Invite.shared.pending == name { Invite.shared.pending = nil }
      }
      .task { for await names in camera.codes() { codesSighted(names) } }
  }

  /// New Yos since the last batch (`Inbox.batchSeconds`): one haptic, each sender's bar on top once.
  func arrived() {
    Perf.timed("list.arrived") {
      let batch = inbox.takeArrivals()
      guard !batch.isEmpty else { return }
      syncListFriends()
      cheers += 1
      withAnimation(inbox.busy ? nil : .snappy(duration: 0.2)) {
        for arrival in batch {
          guard let friend = friends.shownBar(for: arrival) else { continue }
          publish(listModel.received(now: listNow, id: friend.key.uuidString,
            kind: arrival.video != nil ? .video : (arrival.photo != nil ? .photo : .yo), token: arrival.id,
            sender: groupSender(arrival, friend: friend)))
        }
      }
      syncListMedia()
    }
  }

  func groupSender(_ arrival: Arrival, friend: Friend) -> String? {
    guard friend.group != nil else { return nil }
    // Plain group Yo ids are group/user/time; media ids start with group+user.
    let user = Inbox.sender(of: arrival) ?? arrival.id.split(separator: "/").dropFirst().first.map(String.init)
    if let user, let name = inbox.memberNames[user] ?? friends.all.first(where: { $0.user == user })?.name {
      publish(listModel.senderNamed(now: listNow, user: user, name: name))
    }
    return user
  }

  func syncSenderNames() {
    var names = Dictionary(friends.all.compactMap { friend in friend.user.map { ($0, friend.name) } },
      uniquingKeysWith: { first, _ in first })
    names.merge(inbox.memberNames) { _, known in known }
    for (user, name) in names where name != "?" {
      publish(listModel.senderNamed(now: listNow, user: user, name: name))
    }
  }

  func syncListFriends() {
    Perf.timed("list.syncFriends") {
      syncSenderNames()
      publish(listModel.friends(now: listNow, rows: listFriends))
      syncListMedia()
    }
  }

  func syncListMedia() {
    for friend in friends.shown {
      let id = friend.key.uuidString
      let waiting = inbox.media(for: friend) ?? []
      let videos = waiting.map(\.isVideo)
      guard (listMedia.each[id] ?? []) != videos else { continue }
      listMedia.each[id] = videos
      publish(listModel.mediaChanged(now: listNow, id: id, kinds: videos.map { $0 ? .video : .photo }))
      // Kept from before a restart, no Yo told the model who sent it: the newest one's sender, from its id.
      if friend.group != nil, let user = waiting.last.flatMap(Inbox.sender(of:)) {
        publish(listModel.mediaSender(now: listNow, id: id, user: user))
      }
    }
  }

  /// Projects core state into per-row observable render slots; gestures and artwork stay here.
  func publish(_ snapshot: ListSnapshot) {
    DeleteNotice.shared.receive = { publish($0) }
    DeleteNotice.shared.yo = { bar in Task { await yoNow(bar) } }
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
      // Only a reshuffle of the same bars waits for the taps to end; a bar added or gone goes at once.
      let deleting = snapshot.deletion != nil || !snapshot.committedDeletes.isEmpty
      if !listOrder.isEmpty && !deleting && Set(order) == Set(listOrder) && orderHeld(now) {
        heldOrder = order
        if heldSince == nil { heldSince = now }
        scheduleHeldFlush()
      } else {
        applyOrder(order, anchor: anchor, promote: !listOrder.isEmpty && !deleting)
      }
    } else {
      heldOrder = nil
      heldSince = nil
    }
    // A press alone never scrolls (a swipe up to the App Switcher starts as one on the lowest bar): only a reorder
    // does, and keeps the touched bar where it stands (`applyOrder`).
    lockedRow = snapshot.lockedRow
    let friendsByID = Dictionary(friends.shown.map { ($0.key.uuidString, $0) }, uniquingKeysWith: { a, _ in a })
    for row in snapshot.rows {
      guard let friend = friendsByID[row.id] else { continue }
      let bar = bars.of(friend)
      let sending = !bar.outgoing.isEmpty || !row.sending ? bar.sendingBatch : nil
      let title = sending == nil ? row.title : BarSendText.sentYo
      // The sent photos go with their "Sent Yo", in the same update.
      if !bar.sentStack.isEmpty, title != BarSendText.sentYo || !bar.outgoing.isEmpty || bar.sendingBatch != nil {
        bar.sentStack = []
      }
      let count = sending?.count ?? Int(row.count)
      if bar.modelTitle != title { bar.modelTitle = title }
      if bar.modelCount != count { bar.modelCount = count }
      if bar.modelMarkCount != Int(row.markCount) { bar.modelMarkCount = Int(row.markCount) }
      if bar.modelMediaSign != row.mediaSign { bar.modelMediaSign = row.mediaSign }
      if bar.hasNotice != (row.noticeDeadline != nil) { bar.hasNotice = row.noticeDeadline != nil }
      let paused = !row.clockRunning
      let pauseChanged = bar.modelPaused != paused
      if pauseChanged { bar.modelPaused = paused }
      if bar.modelUnsent != row.unsent { bar.modelUnsent = row.unsent }
      if bar.modelLine != row.line { bar.modelLine = row.line }
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
      }
    }
    listDeadline = snapshot.nextTick
    if snapshot.nextTick != nil { clocks.start { tickClocks() } } else { clocks.stop() }
  }

  func applyOrder(_ order: [String], anchor: String?, promote: Bool) {
    heldOrder = nil
    heldSince = nil
    let old = listOrder
    let moved = !old.isEmpty
    // In one frame, no sliding: the bars stand in their new places at once.
    var instant = Transaction(); instant.disablesAnimations = true
    withTransaction(instant) {
      if let anchor, let place = anchorPlace(anchor) { position.scrollTo(id: anchor, anchor: place) }
      else if !scrolled { position.scrollTo(edge: .top) }
      listOrder = order
      if promote { friends.promoteBatch(order.reversed().compactMap(UUID.init(uuidString:))) }
    }
    // Rows slid: only the bars that changed place take no tap for a moment.
    if moved, let first = order.indices.first(where: { $0 >= old.count || old[$0] != order[$0] }) {
      movedLock = (Set(order[first...]), Date.now + Self.reorderTapLock)
    }
  }

  /// The touched bar's place in the viewport as a scroll anchor: the same point of bar and viewport meet, so the bar
  /// stays exactly where it is. None when it is not on screen (then nothing scrolls for it).
  func anchorPlace(_ id: String) -> UnitPoint? {
    guard let key = UUID(uuidString: id), let place = bars.each[key]?.place else { return nil }
    let room = place.viewport - place.frame.height
    guard room > 0, place.frame.maxY > 0, place.frame.minY < place.viewport else { return nil }
    return UnitPoint(x: 0.5, y: min(1, max(0, place.frame.minY / room)))
  }

  /// Any touch on a bar: the order stays until the taps have been quiet a moment.
  func tapped(_ key: UUID) {
    tapQuiet = Date.now + Self.tapQuietSeconds
  }

  /// A tap ignored: the bar just changed place.
  func tapLocked(_ friend: Friend) -> Bool {
    guard let moved = movedLock, Date.now < moved.until else { return false }
    return moved.ids.contains(friend.key.uuidString)
  }

  func orderHeld(_ now: Date) -> Bool {
    // Taps going on hold it however long they last; the cap only frees a finger that seems stuck.
    if now < tapQuiet { return true }
    if let since = heldSince, now.timeIntervalSince(since) >= Self.heldOrderCap { return false }
    return !fingersDown.isEmpty
  }

  /// The held order always goes in the end: once the taps are quiet (or the cap is reached).
  func scheduleHeldFlush() {
    guard heldFlush == nil else { return }
    heldFlush = Task { @MainActor in
      defer { heldFlush = nil }
      while heldOrder != nil, !Task.isCancelled {
        let wait = max(0.05, min(tapQuiet.timeIntervalSinceNow, Self.heldOrderCap))
        try? await Task.sleep(for: .seconds(wait))
        if orderHeld(.now) { continue }
        releaseHold()
      }
    }
  }

  /// The taps are over: the bars take the held order in one move.
  func releaseHold() {
    let order = heldOrder
    var instant = Transaction(); instant.disablesAnimations = true
    withTransaction(instant) {
      if let order, Set(order) == Set(listOrder) { applyOrder(order, anchor: lockedRow, promote: true) }
    }
    heldOrder = nil
    heldSince = nil
  }

  func tickClocks() {
    guard let deadline = listDeadline, listNow >= deadline else { return }
    withAnimation(.easeOut(duration: 0.15)) { publish(listModel.tick(now: listNow)) }
  }

  func pressed(_ key: UUID, _ down: Bool) {
    if down { fingersDown.insert(key) } else { fingersDown.remove(key) }
    tapped(key)
    publish(listModel.press(now: listNow, id: key.uuidString, down: down))
  }

  func flushRise() {
    publish(listModel.flight(now: listNow, active: !bars.awaitingThumbnail.isEmpty))
  }
}
