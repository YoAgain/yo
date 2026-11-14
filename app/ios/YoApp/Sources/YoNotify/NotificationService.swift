// The notification service extension: a push says only "Yo" (the hub knows nothing more, spec 6.3); this one makes
// it "Yo from YoGPT" ("📷 from …", "🎥 from …"; more read as on the bar, "Yo!!!!! +4 from YoGPT · 📷 2"), one
// notification with the Yos those still on the screen name (ShareBridge.write). It opens the app's state as a guest
// (SharedState.swift: in turns with the app, under the lock in the App Group), receives once without waiting, and
// names who the new Yos are from (a line per sender, the newest first). What it receives stays in the state for the
// app (never acknowledged here), which shows it as ever. The photos and videos those Yos carry are fetched to the App
// Group (Prefetch) before the notification goes (after it, iOS may end the process mid-fetch), within the extension's
// limits (about 24 MB, 30 s): the app shows them as it opens, not a fetch later. Once the notification is out, the
// hub is told it arrived (`afterSync`).
//
// The hub pushes once per envelope, and not every envelope is a new Yo: the Welcome before a friend's first Yo, the
// rest of a burst a push before already took, a commit, a blocked sender's Yo (dropped unseen, spec 4.8), a Yo the app
// took itself (it was open, or just left: then it writes the notification itself). Such a push, read and found with
// nothing new, goes out quietly (no sound, no banner) and takes the place of the Yo notifications on the screen,
// saying what they said, rather than lying on top of them as one more "Yo". It goes out as it came ("Yo", with the sound) when nothing could be read:
// the app holds the lock longer than `lockWait` (it is in front then and shows Yos itself), there is no state, the hub
// does not answer in time, or the time is up.
import Foundation
import UserNotifications
import YoCore

@objc(YoNotificationService)
final class NotificationService: UNNotificationServiceExtension {
  private let pending = Pending()

  override func didReceive(
    _ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
  ) {
    Perf.mark("nse.start")
    let content = (request.content.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
    content.title = ""  // "Yo" once, not as title and text
    pending.set(content: content, handler: contentHandler)
    let pending = pending
    let start = Date.now
    DispatchQueue.global(qos: .userInitiated).async {
      // Read: the new Yos with those on the screen, in one notification; none new, the same quietly. Else, unread,
      // the push goes as it came.
      let read = Self.newest()
      var replaced: [String] = []
      if let (client, events) = read {
        // The photos and videos before the notification goes: once it is delivered, iOS may end or freeze this
        // process at any moment, and a fetch cut off there has used this device's fetch at the hub with nothing
        // written (the app then finds the blob gone and gives the photo up). A new fetch starts only within 5 seconds
        // of the push, so a slow network delays the notification little.
        Prefetch.fetch(events, client: client, startingBefore: start + 5)
        // Then what is on the screen, read just before writing: what the app wrote meanwhile is in it.
        let senders = ShareBridge.yos(in: events) { ShareBridge.name(of: $0, client: client) }
        let delivered = Self.delivered()
        replaced =
          pending.update { content in
            let replaced = ShareBridge.write(content, senders, over: delivered)
            if senders.isEmpty {
              (content.sound, content.interruptionLevel) = (nil, .passive)
              // Nothing on the screen to say again (a Welcome, a commit, after the app was opened): no text, which
              // iOS does not show, rather than a bare "Yo" left in Notification Center.
              if replaced.isEmpty { content.body = "" }
            }
            return replaced
          } ?? []
      }
      pending.deliver()
      Perf.mark("nse.delivered")
      // Those it takes the place of only now that its own is out: never a moment with none on the screen, in which the
      // app (taking the saved Yos) would write one more.
      UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: replaced)
      // Not before: a lost acknowledgement costs only a repeat, which the app skips as seen.
      _ = try? read?.0.afterSync()
    }
  }

  /// Time is up: the push as it came.
  override func serviceExtensionTimeWillExpire() {
    pending.deliver()
  }

  /// How long the app may hold the state before the push goes out as it came.
  static let lockWait = 10.0

  /// The notifications on the screen (none if the system does not say within two seconds).
  private static func delivered() -> [UNNotification] {
    final class Found: @unchecked Sendable { var notifications: [UNNotification] = [] }
    let found = Found()
    let done = DispatchSemaphore(value: 0)
    UNUserNotificationCenter.current().getDeliveredNotifications { delivered in
      found.notifications = delivered
      done.signal()
    }
    return done.wait(timeout: .now() + 2) == .success ? found.notifications : []
  }

  /// What one receive without waiting brings, as a guest on the app's state; nil when nothing could be read.
  private static func newest() -> (YoClient, [YoEvent])? {
    guard let client = Perf.timed("nse.open", { Guest.open(within: lockWait, timeout: 10) }),
      let events = Perf.timed("nse.sync", { try? client.sync(waitS: 0) })
    else { return nil }
    return (client, events)
  }

  /// The content and its handler, delivered once (by the work or by the time running out, whichever first).
  private final class Pending: @unchecked Sendable {
    private let lock = NSLock()
    private var content: UNMutableNotificationContent?
    private var handler: ((UNNotificationContent) -> Void)?

    func set(content: UNMutableNotificationContent, handler: @escaping (UNNotificationContent) -> Void) {
      lock.withLock { (self.content, self.handler) = (content, handler) }
    }

    /// Changes the content while it is not delivered yet (nil once it is).
    func update<T>(_ change: (UNMutableNotificationContent) -> T) -> T? {
      lock.withLock { content.map(change) }
    }

    func deliver() {
      let (content, handler) = lock.withLock {
        defer { (self.content, self.handler) = (nil, nil) }
        return (self.content, self.handler)
      }
      if let content, let handler { handler(content) }
    }
  }
}
