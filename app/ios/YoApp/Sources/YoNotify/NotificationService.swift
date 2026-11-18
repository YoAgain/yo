// The notification service extension: a push says only "Yo" (the hub knows nothing more, spec 6.3); this one makes
// it "Yo from YoGPT" ("📷 from …", "🎥 from …"; more read as on the bar, "Yo!!!!! +4 from YoGPT · 📷 2"), one
// notification with the Yos those still on the screen name (ShareBridge.write). It opens the app's state as a guest
// (SharedState.swift: in turns with the app, under the lock in the App Group), receives once without waiting, and
// names who the new Yos are from (a line per sender, the newest first). What it receives stays in the state for the
// app (never acknowledged here), which shows it as ever. The photos and videos those Yos carry are fetched to the App
// Group (Prefetch) before the notification goes (after it, iOS may end the process mid-fetch), within the extension's
// limits (about 24 MB, 30 s): the app shows them as it opens, not a fetch later. The app acknowledges them on its
// next sync; the service takes no more state turns after delivering.
//
// The hub pushes once per envelope, and not every envelope is a new Yo: a receipt or a retry, the rest of a burst a
// push before already took, a blocked sender's Yo (dropped unseen, spec 4.8), a Yo the app
// took itself (it was open, or just left: then it writes the notification itself). Such a push, read and found with
// nothing new, goes out quietly (no sound, no banner) and takes the place of the Yo notifications on the screen,
// saying what they said, rather than lying on top of them as one more "Yo". When nothing could be read it goes out as
// Signal's does, "You may have received a Yo", with the sound (NotifyFallback): locked since a restart (no key before
// the first unlock), no account or App Group, the app holds the lock longer than `lockWait` (it is in front then and
// shows Yos itself), the state does not open, no network, the hub refuses, or the time is up. Each of those is
// recorded with its reason, for the app's next diagnostics upload.
import Foundation
import Security
import UserNotifications
import YoCore

@objc(YoNotificationService)
final class NotificationService: UNNotificationServiceExtension {
  private var pending = Pending()

  override func didReceive(
    _ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
  ) {
    Perf.mark("nse.start")
    ShareBridge.registerCategories()  // "Yo Back" on it, also before the app has run since a restart
    let content = (request.content.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
    content.title = ""  // "Yo" once, not as title and text
    let pending = Pending()
    self.pending = pending
    pending.set(content: content, handler: contentHandler)
    let start = Date.now
    DispatchQueue.global(qos: .userInitiated).async {
      pending.lifetime.run {
        // Read: the new Yos with those on the screen, in one notification; none new, the same quietly. Else, unread,
        // "You may have received a Yo", and why.
        switch Self.newest() {
        case .failure(let reason):
          pending.fallback(reason)
        case .success(let (client, events)):
          // The photos and videos before the notification goes: once it is delivered, iOS may end or freeze this
          // process at any moment, and a fetch cut off there has used this device's fetch at the hub with nothing
          // written (the app then finds the blob gone and gives the photo up). A new fetch starts only within 5 seconds
          // of the push, so a slow network delays the notification little.
          Prefetch.fetch(events, client: client, startingBefore: start + 5)
          // Then what is on the screen, read just before writing: what the app wrote meanwhile is in it.
          let senders = ShareBridge.yos(
          in: events, name: { ShareBridge.name(of: $0, client: client) },
          group: { id in (try? client.groups())?.first { $0.id == id }?.name ?? "" }, requests: ShareBridge.requests(of: client))
          ShareBridge.noteLast(senders, client: client)
          let delivered = Self.delivered()
          // This push's own notification is the sender's one (iOS shows a push whatever its content: an emptied
          // one comes out as the bare "Yo" it came as), with the Yos of the one it replaces; nothing new, the newest
          // on the screen again, quietly. The ones it replaces go as it is handed over.
          let replaced = pending.update { ShareBridge.write($0, senders, over: delivered) } ?? []
          pending.deliver(replacing: replaced)
        }
        Perf.mark("nse.delivered")
      }
    }
  }

  /// Time is up: "You may have received a Yo".
  override func serviceExtensionTimeWillExpire() {
    pending.fallback(.timeout)
  }

  /// How long the app may hold the state before the push goes out unread.
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

  /// What one receive without waiting brings, as a guest on the app's state; else why nothing could be read.
  private static func newest() -> Result<(YoClient, [YoEvent]), NotifyFallback.Reason> {
    let client: YoClient
    switch Perf.timed("nse.open", { Guest.opened(within: lockWait, timeout: 10) }) {
    case .success(let opened): client = opened
    case .failure(.keychain(errSecInteractionNotAllowed)): return .failure(.lockedBeforeFirstUnlock)
    case .failure(.noGroup), .failure(.noAccount): return .failure(.noAccount)
    case .failure(.restore): return .failure(.decrypt)
    case .failure(.keychain), .failure(.busy): return .failure(.other)
    }
    do {
      let events = try Perf.timed("nse.sync", { try client.sync(waitS: 0) })
      return .success((client, events))
    } catch YoError.Network {
      return .failure(.network)
    } catch YoError.Refused(let status) {
      return .failure(.hubStatus(status))
    } catch YoError.Protocol, YoError.State {
      return .failure(.decrypt)
    } catch {
      return .failure(.other)
    }
  }

  /// The content and its handler, delivered once (by the work or by the time running out, whichever first).
  private final class Pending: @unchecked Sendable {
    let lifetime = SharedAccess.Lifetime()
    private let lock = NSLock()
    private var content: UNMutableNotificationContent?
    private var handler: ((UNNotificationContent) -> Void)?

    func set(content: UNMutableNotificationContent, handler: @escaping (UNNotificationContent) -> Void) {
      lock.withLock { (self.content, self.handler) = (content, handler) }
    }

    /// The content is what was read (written by `update`): the time running out hands it over as it is.
    private var read = false

    /// Changes the content while it is not delivered yet (nil once it is).
    func update<T>(_ change: (UNMutableNotificationContent) -> T) -> T? {
      lock.withLock {
        guard let content else { return nil }
        read = true
        return change(content)
      }
    }

    /// Unread: "You may have received a Yo" (also under a hub of before, whose push said "Yo"), recorded with its
    /// reason; nothing when it was delivered already, and as it is when it was read just before the time ran out.
    func fallback(_ reason: NotifyFallback.Reason) {
      let unread = lock.withLock {
        guard let content, !read else { return false }
        content.body = NotifyFallback.text
        return true
      }
      if unread { NotifyFallback.record(reason) }
      deliver()
    }

    /// Hands the content over; `replacing`: the notifications it takes the place of, removed just before (once it is
    /// handed over, iOS may freeze the process before a removal after it goes out).
    func deliver(replacing replaced: [String] = []) {
      let (content, handler) = lock.withLock {
        defer { (self.content, self.handler) = (nil, nil) }
        return (self.content, self.handler)
      }
      if let content, let handler {
        lifetime.stop()
        if !replaced.isEmpty { UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: replaced) }
        handler(content)
      }
    }
  }
}
