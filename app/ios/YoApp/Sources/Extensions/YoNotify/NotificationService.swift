// Turns the hub's bare "Yo" push into one notification per new Yo ("Yo from Mia in #Hey 📷"), threaded per sender,
// as a guest on the app's state (SharedState). Media is prefetched before delivery: afterwards iOS may end the
// process mid-fetch, wasting the hub's fetch. A push with nothing new re-delivers the newest quietly in place of the
// shown ones; one that cannot be read says "You may have received a Yo" (NotifyFallback) and records why.
import Foundation
import Security
import UserNotifications
import WidgetKit
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
    // Asked at once, beside the fetch, for the sound (the first unread of a conversation sounds); waited for at most
    // half a second after the fetch, so no Yo waits on a slow answer. The badge is the core's count (YoClient.unread).
    let listing = Listing()
    UNUserNotificationCenter.current().getDeliveredNotifications { listing.set($0) }
    DispatchQueue.global(qos: .userInitiated).async {
      pending.lifetime.run {
        switch Self.newest() {
        case .failure(let reason):
          pending.fallback(reason)
        case .success(let (client, events)):
          // Before delivery (see header); new fetches start only within 5 s of the push.
          Prefetch.fetch(events, client: client, startingBefore: start + 5)
          // Read just before writing: what the app wrote meanwhile is in it.
          let senders = ShareBridge.yos(
          in: events, name: { ShareBridge.name(of: $0, client: client) },
          group: { id in (try? client.groups())?.first { $0.id == id }?.name ?? "" }, requests: ShareBridge.requests(of: client))
          ShareBridge.noteLast(senders, client: client)
          Self.rise(senders, client: client)
          // iOS shows the push whatever its content, so it carries the newest Yo; the others are added beside it.
          let replaced = pending.update {
            ShareBridge.write(
              $0, senders, over: listing.notifications(waiting: 0.5) ?? [], unread: client.unread())
          } ?? []
          pending.deliver(replacing: replaced)
        }
        Perf.mark("nse.delivered")
      }
    }
  }

  override func serviceExtensionTimeWillExpire() {
    pending.fallback(.timeout)
  }

  /// iOS's list of delivered notifications, once it answered.
  private final class Listing: @unchecked Sendable {
    private let lock = NSLock()
    private let answered = DispatchSemaphore(value: 0)
    private var answer: [UNNotification]?

    func set(_ notifications: [UNNotification]) {
      lock.withLock { answer = notifications }
      answered.signal()
    }

    /// Nil if iOS has not answered within `seconds` from now.
    func notifications(waiting seconds: Double) -> [UNNotification]? {
      if let answer = lock.withLock({ answer }) { return answer }
      guard answered.wait(timeout: .now() + seconds) == .success else { return nil }
      answered.signal()
      return lock.withLock { answer }
    }
  }

  /// The bars of `yos` (newest first) on top of the list the widgets show, as the app's list lifts them on taking
  /// them: the app may not run before the widgets are next looked at. The app writes its own list when it does.
  static func rise(_ yos: [ShareBridge.Yos], client: YoClient) {
    guard let file = ShareBridge.friendsFile, let list = ShareBridge.readList(),
      list.owner == Hub.bytes(client.userId())?.base64EncodedString()
    else { return }
    var rest = list.friends
    var risen: [ShareBridge.Friend] = []
    for yo in yos {
      if let at = rest.firstIndex(where: { $0.to == yo.user }) { risen.append(rest.remove(at: at)) }
    }
    let friends = risen + rest
    guard friends != list.friends,
      let data = try? JSONEncoder().encode(ShareBridge.List(owner: list.owner, friends: friends))
    else { return }
    Diagnostics.attempt({ try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]) })
    WidgetCenter.shared.reloadAllTimelines()
  }

  /// How long the app may hold the state before the push goes out unread.
  static let lockWait = 10.0

  private static func newest() -> Result<(YoClient, [YoEvent]), NotifyFallback.Reason> {
    let client: YoClient
    switch Perf.timed("nse.open", { Guest.opened(within: lockWait, timeout: 10) }) {
    case .success(let opened): client = opened
    case .failure(.keychain(errSecInteractionNotAllowed)): return .failure(.lockedBeforeFirstUnlock)
    case .failure(.noGroup), .failure(.noAccount): return .failure(.noAccount)
    case .failure(.restore): return .failure(.decrypt)
    case .failure(.busy): return .failure(.busy)
    case .failure(.keychain): return .failure(.other)
    }
    do {
      let events = try Perf.timed("nse.sync", { try client.sync(waitS: 0) })
      return .success((client, events))
    } catch YoError.Network(let message) {
      // A step's turn refused (SharedAccess.unavailable): the app held the state, not the network.
      return .failure(message == SharedAccess.unavailableMessage ? .busy : .network)
    } catch YoError.Refused(let status) {
      return .failure(.hubStatus(status))
    } catch YoError.Protocol, YoError.State {
      return .failure(.decrypt)
    } catch {
      return .failure(.other)
    }
  }

  /// Delivered once, by the work or by the time running out, whichever first.
  private final class Pending: @unchecked Sendable {
    let lifetime = SharedAccess.Lifetime()
    private let lock = NSLock()
    private var content: UNMutableNotificationContent?
    private var handler: ((UNNotificationContent) -> Void)?

    func set(content: UNMutableNotificationContent, handler: @escaping (UNNotificationContent) -> Void) {
      lock.withLock { (self.content, self.handler) = (content, handler) }
    }

    /// Written by `update`: the time running out hands it over as it is.
    private var read = false

    func update<T>(_ change: (UNMutableNotificationContent) -> T) -> T? {
      lock.withLock {
        guard let content else { return nil }
        read = true
        return change(content)
      }
    }

    func fallback(_ reason: NotifyFallback.Reason) {
      let unread = lock.withLock {
        guard let content, !read else { return false }
        content.body = NotifyFallback.text
        return true
      }
      if unread { NotifyFallback.record(reason) }
      deliver()
    }

    /// `replaced` is removed before handing over: afterwards iOS may freeze the process.
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
