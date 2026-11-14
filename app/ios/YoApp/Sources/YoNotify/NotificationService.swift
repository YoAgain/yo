// The notification service extension: a push says only "Yo" (the hub knows nothing more, spec 6.3); this one makes
// it "Yo from YoGPT" ("📷 from …", "🎥 from …", ShareBridge.yo). More Yos read as on the bar and go into one
// notification with those still on the screen ("Yo!!!!! +4 from YoGPT · 📷 2", ShareBridge.merged). It opens the app's state as a guest
// (SharedState.swift: in turns with the app, under the lock in the App Group), receives once without waiting, and
// names who the new Yos are from (a line per sender, the newest first). What it receives stays in the state for the
// app (never acknowledged here), which shows it as ever; once the notification is out, the hub is told it arrived
// (`afterSync`). Then the photos and videos those Yos carry are fetched to the App Group (Prefetch), within the
// extension's limits (about 24 MB, 30 s): the app shows them as it opens, not a fetch later.
//
// The hub pushes once per envelope, and not every envelope is a new Yo: the Welcome before a friend's first Yo, the
// rest of a burst a push before already took, a commit, a blocked sender's Yo (dropped unseen, spec 4.8), a Yo the app
// took itself (it was open, or just left: then it writes the notification itself). Such a push, read and found with
// nothing new, goes out quietly (no sound, no banner) and takes the place of the Yo notification shown last, rather
// than lying on top of it as one more "Yo". It goes out as it came ("Yo", with the sound) when nothing could be read:
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
    DispatchQueue.global(qos: .userInitiated).async {
      let (found, client, events) = Self.newest()
      switch found {
      case .yos(let senders):
        // With the Yos the notifications on the screen already name: one notification for all of them.
        let (all, replaced) = ShareBridge.merged(senders, with: Self.delivered())
        pending.update { ShareBridge.yo($0, all) }
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: replaced)
      case .nothingNew:
        let last = Self.lastYo()
        pending.update { content in
          if let last {
            content.body = last.content.body
            content.threadIdentifier = last.content.threadIdentifier
            content.categoryIdentifier = last.content.categoryIdentifier
            content.userInfo = last.content.userInfo
          }
          content.sound = nil
          content.interruptionLevel = .passive
        }
        if let last { UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [last.identifier]) }
      case .unread:
        break
      }
      pending.deliver()
      Perf.mark("nse.delivered")
      // Not before: a lost acknowledgement costs only a repeat, which the app skips as seen.
      _ = try? client?.afterSync()
      if let client { Prefetch.fetch(events, client: client) }
    }
  }

  /// Time is up: the push as it came.
  override func serviceExtensionTimeWillExpire() {
    pending.deliver()
  }

  enum Found: Sendable {
    /// New Yos, by sender, the newest first.
    case yos([ShareBridge.Yos])
    /// Read: nothing new to show.
    case nothingNew
    /// Nothing could be read.
    case unread
  }

  /// How long the app may hold the state before the push goes out as it came.
  static let lockWait = 10.0

  /// The Yo notification shown last and not seen yet (the app takes them all in when it opens).
  private static func lastYo() -> UNNotificationRequest? {
    delivered().filter { $0.request.content.categoryIdentifier == ShareBridge.yoCategory }.max { $0.date < $1.date }?
      .request
  }

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

  private static func newest() -> (Found, YoClient?, [YoEvent]) {
    guard let client = Perf.timed("nse.open", { Guest.open(within: lockWait, timeout: 10) }) else {
      return (.unread, nil, [])
    }
    guard let events = Perf.timed("nse.sync", { try? client.sync(waitS: 0) }) else { return (.unread, client, []) }
    let yos = ShareBridge.yos(in: events, client: client)
    return (yos.isEmpty ? .nothingNew : .yos(yos), client, events)
  }

  /// The content and its handler, delivered once (by the work or by the time running out, whichever first).
  private final class Pending: @unchecked Sendable {
    private let lock = NSLock()
    private var content: UNMutableNotificationContent?
    private var handler: ((UNNotificationContent) -> Void)?

    func set(content: UNMutableNotificationContent, handler: @escaping (UNNotificationContent) -> Void) {
      lock.withLock { (self.content, self.handler) = (content, handler) }
    }

    /// Changes the content while it is not delivered yet.
    func update(_ change: (UNMutableNotificationContent) -> Void) {
      lock.withLock { if let content { change(content) } }
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
