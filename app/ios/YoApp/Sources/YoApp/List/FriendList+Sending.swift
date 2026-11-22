import AVFoundation
import ImageIO
import SwiftUI
import UIKit
import UserNotifications
import YoCore

extension FriendList {
  /// Each bar's render slot and what it is sending.
  @MainActor @Observable final class Bars {
    /// Read only by its own bar (`LiveBar`), so a tap redraws that bar alone.
    @MainActor @Observable final class Bar {
      var hasNotice = false
      var modelTitle = ""
      var modelCount = 1
      var modelMarkCount = 0
      var modelMediaSign = ""
      var modelPaused = false
      /// Something to it did not go: a tap retries it (the core's `ListRow.unsent`).
      var modelUnsent = false
      /// What the bar's one line tells (the core's): the title, or the Yos that came after it.
      var modelLine: ListLine = .title
      /// Photos and videos on their way out; the ring at the bar's left undoes them.
      var outgoing: [Outgoing] = []
      /// Nil while captures join; the full undo interval starts after their flights settle.
      var countdownStarted: Date?
      /// The already-counted batch, visible after undo ends until its sends finish.
      var sendingBatch: (id: UUID, count: Int, items: [Outgoing])?
      /// The photos a "Sent Yo" confirms: they stay beside it and go when its words go.
      var sentStack: [Outgoing] = []
      /// The undo time: `sendDelay`, or the friend's "Undo Before Sending".
      var seconds = FriendList.sendDelay
      /// A video being held on this bar: the stack's countdown waits for it, so the video joins the same stack.
      var recording = false
      /// Where the bar stands in the list's viewport (its frame there, and the viewport's height): a reorder keeps a
      /// touched bar at this place. Not observed: it changes with every scrolled frame.
      @ObservationIgnored var place: Place?
    }

    struct Place: Equatable {
      let frame: CGRect
      let viewport: CGFloat
    }

    @ObservationIgnored var each: [UUID: Bar] = [:]
    func of(_ friend: Friend) -> Bar {
      if let bar = each[friend.key] { return bar }
      let bar = Bar()
      each[friend.key] = bar
      return bar
    }
    @ObservationIgnored var preparing: [UUID: Task<Prepared?, Never>] = [:]
    /// Each new photo starts a new round; the stack goes when the newest round's time is over.
    @ObservationIgnored var stackRounds: [UUID: UUID] = [:]
    /// Per friend, the last stack going out: the next waits for it, so they go in the order taken.
    @ObservationIgnored var releasing: [UUID: Task<Void, Never>] = [:]
    @ObservationIgnored var awaitingThumbnail: Set<UUID> = []
  }

  struct Outgoing: Equatable {
    let id = UUID()
    let started = Date.now
    /// What the bar says meanwhile ("Sent 📷!").
    let text: String
    var thumbnail: UIImage?
    let origin: CGPoint?
  }

  func send(to friend: Friend, origin: CGPoint?) {
    // A group's bar just shrank back: a tap in the next half second on a bar that slid under the finger goes nowhere.
    guard !tapLocked(friend) else { return }
    tapped(friend.key)
    // A request asks first (spec 4.10): a tap on its bar is the yes.
    if friend.request { chooseRequest(friend, yes: true); return }
    // "Not Sent · Tap to Retry": the tap sends what waits for it again, not one more.
    if bars.of(friend).modelUnsent, !cameraOn || picked.isEmpty, swiped == nil {
      retryUnsent(friend)
      return
    }

    // Firing back into a group: its senders' lines stay longer, so the bar does not shrink under the finger.
    if friend.group != nil { publish(listModel.keepAlive(now: listNow, id: friend.key.uuidString)) }
    hintEpoch += 1
    if cameraOn { hintNeeded = false }
    Perf.mark("tap.action")
    Perf.mark("thumb.tap")
    defer { PerfProbe.frame("tap") }
    // A tap while a bar is swiped open only closes it, as in Mail.
    guard swiped == nil else { return withAnimation(.snappy) { swiped = nil } }
    // The library's pick still loading: it goes with the next tap, never a photo from the camera instead.
    if cameraOn && libraryLoading { return }
    // Here, as the finger lifts, not after the list has drawn the change.
    if cameraOn && picked.isEmpty { Haptic.shutter() } else { Haptic.tap() }
    if !cameraOn { Haptic.sendClick() }
    let library = cameraOn ? picked : []
    // "Undo Before Sending" (Per Contact): the Yo waits under the ring as a photo does; a tap on it keeps it here.
    if !cameraOn && library.isEmpty, let own = friends.all.first(where: { $0.key == friend.key }), own.undoFirst {
      let (inbox, accounts, friends) = (inbox, accounts, friends)
      run(friend, BarSendText.sentYo, seconds: own.undoSeconds) { _ in
        Prepared(
          send: {
            // A request past its bucket is no failure: it waits by itself and goes with their yes.
            if let problem = await inbox.send(to: friend, accounts: accounts, friends: friends),
              problem != Inbox.requestWaiting {
              if problem == Inbox.groupEmpty { emptyGroup = friend }
              showSent(to: friend, problem)
              return false
            }
            return true
          })
      }
      return
    }
    guard cameraOn || !library.isEmpty else {
      showSent(to: friend, BarSendText.sentYo, thenOnTop: true)
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
    let client = accounts.client  // a photo waiting its undo time never goes from the next account
    let at = Date.now
    if !library.isEmpty {
      for item in library {
        run(friend, item.video == nil ? BarSendText.sentPhoto : BarSendText.sentVideo, origin: origin) { thumbnail in
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
      run(friend, BarSendText.sentPhoto, origin: origin, placeholder: camera.previewThumbnail()) { thumbnail in
        guard let taken = await camera.photo(), !Task.isCancelled else { return nil }
        Perf.mark("thumb.capture.done")
        if let image = await Self.outgoingThumbnail(taken) { thumbnail(image) }
        guard !Task.isCancelled, let file = await Task.detached(operation: { MediaFile.photo(taken) }).value,
          !Task.isCancelled else { return nil }
        return await stage(file, to: friend, from: client, at: at)
      }
    }
  }

  /// What did not go to `friend` goes again now: photos and videos in line, and Yos kept in the inbox.
  func retryUnsent(_ friend: Friend) {
    Haptic.tap()
    outbox.retryNow(to: friend.target)
    if inbox.unsentBars.contains(friend.key) { inbox.resume(accounts: accounts, friends: friends) }
    Problems.shared.resolve(.notSent)
  }

  /// The bars of what waits after a failed try (photos, videos, Yos) say so until it went; one that went later
  /// tells it.
  func syncUnsent() {
    var now: Set<String> = []
    for friend in friends.all {
      if outbox.unsentTo.contains(friend.target) || inbox.unsentBars.contains(friend.key) {
        now.insert(friend.key.uuidString)
      }
    }
    let before = listMedia.unsent
    guard now != before else { return }
    listMedia.unsent = now
    for id in now.subtracting(before) { publish(listModel.unsent(now: listNow, id: id, on: true)) }
    for id in before.subtracting(now) { publish(listModel.unsent(now: listNow, id: id, on: false)) }
    if now.isEmpty { Problems.shared.resolve(.notSent) }
  }

  /// The Yo a person's add sends (a contact typed in, a request accepted, a group made): "Sent Yo" where the bar
  /// stands, then it rises (`yo_product::list`); if it does not go, the bar says why instead and stays.
  func yoNow(_ bar: Friend) async {
    syncListFriends()  // the list model has the bar before its Yo is told
    showSent(to: bar, BarSendText.sentYo, thenOnTop: true)
    if let problem = await inbox.send(to: bar, accounts: accounts, friends: friends) {
      if problem == Inbox.groupEmpty { emptyGroup = bar }
      showSent(to: bar, problem)
    } else {
      await sentYo()
    }
  }

  /// The first Yo ever sent from this phone: what the friend gets, and how to get a Yo back with notifications off.
  func sentYo() async {
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

  func turnOnNotifications(_ notifications: Notifications) async {
    if notifications == .notAsked {
      await inbox.askForNotifications()
    } else if let url = URL(string: UIApplication.openNotificationSettingsURLString) {
      await UIApplication.shared.open(url)
    }
  }

  /// Only the prepared bytes during undo; sealing, disk storage and upload begin after it.
  func stage(
    _ file: MediaFile, to friend: Friend, from client: YoClient?, at: Date
  ) async -> Prepared? {
    guard !Task.isCancelled else { return nil }
    // Taken right after a cold start: it waits a moment for the core of the same account, it is not lost.
    var client = client
    if client == nil, let account = accounts.account?.userID {
      let soon = await accounts.clientSoon(seconds: 5) { accounts.account?.userID == account }
      if accounts.account?.userID == account { client = soon }
    }
    guard let client, client === accounts.client else { return nil }
    let outbox = outbox
    return Prepared(
      send: {
        guard client === accounts.client else { return false }
        let sent = await outbox.sendOnce(file, to: friend.target, from: client, accounts: accounts, at: at)
        if sent { Stats.sentYo(to: friend.name) }
        return sent
      })
  }

  func recordingChanged(_ friend: Friend?) {
    if let previous = recordingFor { bars.of(previous).recording = false }
    // A new video zooms from where the camera is; the end of one goes back there.
    if let start = recordZoomStart {
      recordZoomStart = nil
      if zoom != start { zoom = start }
    }
    recordZoomStart = friend == nil ? nil : zoom
    recordZoomTicked = false
    if let friend {
      let bar = bars.of(friend)
      bar.recording = true
      if !bar.outgoing.isEmpty { bar.countdownStarted = nil }  // the ring waits with the stack
    }
    recordingFor = friend
    publish(listModel.recording(now: listNow, id: friend?.key.uuidString))
  }

  /// The filming finger at `y`, drawn out of its `bar` (down or up): the camera zooms in, as far as `productRecordZoom`
  /// says; back on the bar, out again. A light tick as it starts.
  func recordZoom(_ friend: Friend, y: CGFloat, bar: CGRect, height: CGFloat) {
    guard recordingFor?.key == friend.key, let start = recordZoomStart else { return }
    let next = productRecordZoom(
      fingerY: Double(y), barTop: Double(bar.minY), barBottom: Double(bar.maxY), height: Double(height), start: start,
      cameraMost: zoomMost)
    if next > start + 0.01, !recordZoomTicked {
      recordZoomTicked = true
      Haptic.tap(.light)
    }
    if abs(next - zoom) > 0.001 { zoom = next }
  }

  func hold(_ friend: Friend, _ holding: Bool, origin: CGPoint?) {
    if holding { guard !tapLocked(friend) else { return } }
    tapped(friend.key)
    hintEpoch += 1
    if cameraOn { hintNeeded = false }
    if holding {
      guard recordingFor == nil else { return }  // one video at a time: a second finger records nothing
      recordingChanged(friend)
      camera.startVideo()
      Task { zoomMost = await camera.zoomMost() }  // the camera's own, as it is now
      // At the longest length the video goes as if the finger had lifted; the later lift finds no recording.
      let round = UUID()
      recordingRound = round
      Task {
        try? await Task.sleep(for: .seconds(MediaFile.longestVideo))
        guard recordingRound == round, let current = recordingFor, current.key == friend.key else { return }
        hold(current, false, origin: nil)
      }
    } else if recordingFor == friend {
      recordingChanged(nil)
      Haptic.tap()
      let client = accounts.client
      let at = Date.now
      run(friend, BarSendText.sentVideo, origin: origin, placeholder: camera.previewThumbnail()) { thumbnail in
        guard let recording = await camera.stopVideo() else { return nil }
        Perf.mark("thumb.capture.done")
        defer { Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: recording) } }
        guard !Task.isCancelled else { return nil }
        return await withTaskCancellationHandler {
          if let image = await Self.outgoingVideoThumbnail(recording) { thumbnail(image) }
          guard !Task.isCancelled, let file = await MediaFile.video(recording),
            !Task.isCancelled else { return nil }
          return await stage(file, to: friend, from: client, at: at)
        } onCancel: {
          try? FileManager.default.removeItem(at: recording)
        }
      }
    }
  }

  /// A photo or video joins the friend's stack, which goes `seconds` after the newest settled (each one restarts the
  /// time), in order; `prepare` takes the photo or ends the video meanwhile. A tap on the ring keeps the whole stack.
  func run(
    _ friend: Friend, _ text: String, seconds: Double = FriendList.sendDelay, origin: CGPoint? = nil, placeholder: UIImage? = nil,
    prepare: @escaping @MainActor (@MainActor (UIImage) -> Void) async -> Prepared?
  ) {
    let bars = bars  // the object itself into the task, not this view's copy
    let one = Outgoing(text: text, thumbnail: placeholder, origin: origin)
    let bar = bars.of(friend)
    bar.seconds = seconds
    bar.outgoing.append(one)
    bar.countdownStarted = nil
    bar.sendingBatch = (bar.outgoing.first!.id, bar.outgoing.count, bar.outgoing)
    let snapshot = listModel.sendStarted(now: listNow, id: friend.key.uuidString,
      kind: productMediaKind(label: text),
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
      // Capture and fly-in belong to this batch, even if the core's deadline expires meanwhile.
      while bar.outgoing.contains(where: { bars.awaitingThumbnail.contains($0.id) }) {
        try? await Task.sleep(for: .milliseconds(20))
        guard bars.stackRounds[key] == round else { return }
      }
      guard bars.stackRounds[key] == round else { return }
      // A video held on this bar pauses the countdown and joins this stack; a canceled hold gives the full time back.
      while true {
        if bar.recording {
          if bar.countdownStarted != nil { bar.countdownStarted = nil }
        } else if let started = bar.countdownStarted {
          if Date.now.timeIntervalSince(started) >= seconds { break }
        } else {
          bar.countdownStarted = .now
        }
        try? await Task.sleep(for: .milliseconds(50))
        guard bars.stackRounds[key] == round, !bar.outgoing.isEmpty else { return }  // another came, or undone
      }
      guard bars.stackRounds[key] == round, !bar.outgoing.isEmpty else { return }  // another came, or undone
      let stack = bar.outgoing
      let batchID = bar.sendingBatch?.id
      bars.stackRounds[key] = nil
      bar.outgoing = []
      bar.countdownStarted = nil
      publish(listModel.tick(now: listNow))
      for one in stack { bars.awaitingThumbnail.remove(one.id) }
      flushRise()
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
        // Starts already counted every item: finish the batch once, without replaying those counts.
        if bar.sendingBatch?.id == batchID {
          bar.sentStack = failed ? [] : bar.sendingBatch?.items ?? []
          bar.sendingBatch = nil
        }
        showSent(to: friend, failed ? BarSendText.notSent : (stack.last?.text ?? BarSendText.notSent), thenOnTop: !failed)
      }
      bars.releasing[key] = releasing
      await releasing.value
      if bars.releasing[key] == releasing { bars.releasing[key] = nil }
    }
  }

  /// One persistent worker: its first job exercises the exact JPEG/ImageIO path before capture.
  nonisolated static let outgoingThumbnailQueue: DispatchQueue = {
    let queue = DispatchQueue(label: "yo.outgoing-thumbnail", qos: .userInitiated)
    queue.async {
      guard let context = CGContext(data: nil, width: 2, height: 2, bitsPerComponent: 8,
        bytesPerRow: 8, space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue),
        let pixels = context.makeImage(), let data = UIImage(cgImage: pixels).jpegData(compressionQuality: 0.8)
      else { return }
      _ = decodeOutgoingThumbnail(data)
    }
    return queue
  }()

  nonisolated static func outgoingThumbnail(_ data: Data) async -> UIImage? {
    await withCheckedContinuation { continuation in
      outgoingThumbnailQueue.async {
        continuation.resume(returning: decodeOutgoingThumbnail(data))
      }
    }
  }

  nonisolated static func decodeOutgoingThumbnail(_ data: Data) -> UIImage? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceThumbnailMaxPixelSize: 360,
      kCGImageSourceShouldCacheImmediately: true,
    ]
    return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
  }

  /// The first frame, read while the undo time runs, upright as `MediaFile.video` stands it.
  nonisolated static func outgoingVideoThumbnail(_ url: URL) async -> UIImage? {
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

  /// Prepared bytes, without an outbox entry or media key yet; `send` makes the attempt.
  struct Prepared {
    let send: @MainActor () async -> Bool
  }

  func cancelSending(_ friend: Friend) {
    let bar = bars.of(friend)
    for one in bar.outgoing {
      // Only capture and export to cancel: no outbox entry or media key exists during undo.
      bars.awaitingThumbnail.remove(one.id)
      bars.preparing.removeValue(forKey: one.id)?.cancel()
    }
    bars.stackRounds[friend.key] = nil
    bar.sendingBatch = nil
    bar.outgoing = []
    bar.countdownStarted = nil
    showSent(to: friend, BarSendText.canceled)
  }

  /// Yos sent from outside the list (IntentSent), oldest first: the newest ends on top.
  func takeIntentSent() {
    guard scenePhase == .active else { return }
    for sent in IntentSent.take() {
      let skeleton = Username.skeleton(sent.to)
      let bar = friends.all.first { $0.target == sent.to }
        ?? friends.all.first { $0.group == nil && $0.user == nil && Username.skeleton($0.name) == skeleton }
      guard let bar, !bar.blocked else { continue }
      showSent(to: bar, BarSendText.sentYo, thenOnTop: true)
    }
  }

  func showSent(to friend: Friend, _ text: String, thenOnTop: Bool = false) {
    // Never silent: the banner says why (a problem told already, such as "No Connection", stays).
    if text == BarSendText.notSent, Problems.shared.text == nil {
      Problems.shared.report(.notSent, retry: { retryUnsent(friend) })
    }
    let id = friend.key.uuidString
    let snapshot: ListSnapshot
    switch text {
    case BarSendText.canceled: snapshot = listModel.sendUndone(now: listNow, id: id)
    case BarSendText.notSent: snapshot = listModel.sendFailed(now: listNow, id: id, text: text)
    case BarSendText.sentYo where thenOnTop: snapshot = listModel.sendFinished(now: listNow, id: id, kind: .yo)
    case BarSendText.sentPhoto where thenOnTop: snapshot = listModel.sendFinished(now: listNow, id: id, kind: .photo)
    case BarSendText.sentVideo where thenOnTop: snapshot = listModel.sendFinished(now: listNow, id: id, kind: .video)
    default: snapshot = listModel.notice(now: listNow, id: id, text: text, rise: thenOnTop)
    }
    withAnimation(.snappy(duration: 0.2)) { publish(snapshot) }
  }

  /// A finger on a bar's media sign plays them, oldest first: held, letting go closes them; a short tap lets them run.
  func see(_ friend: Friend, _ touch: ThumbnailTouch) {
    if case let .moved(point, barBottom) = touch {  // the viewer alone follows it: not a tap on the list
      heldFinger.barBottom = barBottom
      heldFinger.point = point
      return
    }
    tapped(friend.key)
    switch touch {
    case .down:
      shown = []
      heldFinger.point = nil
      // Only what is downloaded, written whole and checked (ReceivedMedia): one still loading stays on its bar, and
      // the viewer never waits for a file or takes one in while open. A group's as "LEO in FAMILY".
      let ready = (inbox.media(for: friend) ?? []).filter { $0.pending == nil }
      guard !ready.isEmpty else { return }
      viewing = ready.map {
        let user = Inbox.sender(of: $0)
        let name = user.flatMap { u in inbox.memberNames[u] ?? friends.all.first { $0.user == u }?.name }
        let from = friend.group == nil ? friend.name : "\(name ?? productText(text: .someone, lang: .english)) in \(friend.name)"
        return Arrival(id: $0.id, from: from, photo: $0.photo, video: $0.video, file: $0.file)
      }
    case .tapped:
      heldFinger.point = nil
    case .released:
      closeViewer()
    case .moved:
      break
    }
  }

  func closeViewer() {
    heldFinger.point = nil
    viewing = nil
    inbox.seen(shown)
    shown = []
    syncListMedia()
  }
}
