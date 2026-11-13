// The notification service extension: a push says only "Yo" (the hub knows nothing more, spec 6.3); this one makes
// it "Yo from YoGPT" ("📷 from …", "🎥 from …", "3 Yos from …"). It opens the app's state as a guest (SharedState.swift:
// in turns with the app, under the lock in the App Group), receives once without waiting, and names the sender of
// the newest Yo. What it receives stays in the state for the app (never acknowledged here), which shows it as ever.
// Within the extension's limits (about 24 MB, 30 s): no photo or video is fetched here, and the push goes out as it
// came ("Yo") when the app holds the lock (it is in front then and shows Yos itself), there is no state, the hub
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
    let content = (request.content.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
    pending.set(content: content, handler: contentHandler)
    let pending = pending
    DispatchQueue.global(qos: .userInitiated).async {
      if let line = Self.line() {
        pending.update { content in
          content.title = ""
          content.body = line.text
          content.threadIdentifier = line.user
          content.userInfo["from"] = line.name
          content.userInfo["id"] = line.id
        }
      }
      pending.deliver()
    }
  }

  /// Time is up: the push as it came.
  override func serviceExtensionTimeWillExpire() {
    pending.deliver()
  }

  struct Line: Sendable {
    let text: String
    let name: String
    let user: String
    /// The Yo as the app's inbox calls it ("user/sent at"): shown once, from here or from the core.
    let id: String
  }

  /// What the newest Yo says, nil when nothing new came (or nothing could be read).
  private static func line() -> Line? {
    guard let client = Guest.open(within: 3, timeout: 10) else { return nil }
    guard let events = try? client.sync(waitS: 0) else { return nil }
    var newest: (user: String, sentAtMs: UInt64, kind: MessageKind)?
    var count: [String: Int] = [:]
    for event in events {
      guard case .message(let user, let sentAtMs, let kind) = event else { continue }
      switch kind {
      case .yo, .photo, .video: break
      default: continue  // nothing the app shows
      }
      count[user, default: 0] += 1
      if newest == nil || sentAtMs >= newest!.sentAtMs { newest = (user, sentAtMs, kind) }
    }
    guard let newest else { return nil }
    let name = name(of: newest.user, client: client)
    let several = count[newest.user] ?? 1
    let text: String =
      if several > 1 {
        "\(several) Yos from \(name)"
      } else {
        switch newest.kind {
        case .photo: "📷 from \(name)"
        case .video: "🎥 from \(name)"
        default: "Yo from \(name)"
        }
      }
    return Line(text: text, name: name, user: newest.user, id: "\(newest.user)/\(newest.sentAtMs)")
  }

  /// The name as the app's list shows it (ShareBridge: the friends it published), else as the core knows them.
  private static func name(of user: String, client: YoClient) -> String {
    let owner = Hub.bytes(client.userId())?.base64EncodedString()
    if let list = ShareBridge.readList(), list.owner == owner, let friend = list.friends.first(where: { $0.to == user }) {
      return friend.name
    }
    let contact = client.contacts().first { $0.user == user }
    return contact?.handles.first ?? contact?.name ?? "Someone"
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
