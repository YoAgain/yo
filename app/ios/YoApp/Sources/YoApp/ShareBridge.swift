// What the app and its extensions (YoShare, YoNotify) hand each other, in their App Group (`group`) beside the
// account's state (SharedState.swift): the friends on the list for the extensions to show and name, and the photos and
// videos the share extension could not send itself (no network, the hub's "not now", no state to send with) for the
// app to send as soon as it opens. One writer per file: the app writes the friends, the extension adds shares (the
// media first, then their note, which makes them whole), the app takes them out. And a Yo notification as both the
// notification service and the app write it. Compiled into all three targets (Sources/YoShare and Sources/YoNotify
// link to this file).
import Foundation
import UserNotifications
import YoCore

enum ShareBridge {
  /// A friend as the extension shows them: the name, the colour, and whom the Yo goes to.
  struct Friend: Codable, Equatable, Sendable {
    var name: String
    var color: Int
    /// The user id at the hub, else the name (before the first Yo), as the app's own sends.
    var to: String
  }

  /// What the app shows the extension: whose list it is (the account) and the friends on it.
  struct List: Codable, Equatable, Sendable {
    let owner: String
    let friends: [Friend]
  }

  /// One share waiting for the app: a photo or video for a friend. Its media lies beside it, `<id>.jpg` or `<id>.mp4`.
  struct Share: Codable, Sendable {
    let id: UUID
    /// The account whose list it was picked from: only that one sends it.
    let owner: String
    let to: String
    let name: String
    let kind: MediaFile.Kind
    let width: Int
    let height: Int
    let durationMs: UInt32
    let created: Date
  }

  /// The App Group of the app and its extensions; Yo Dev's own (its accounts are the dev hub's). Without it in the
  /// signature (Yo Dev from dev/push.sh: no extensions, no group) the container is nil and nothing is shared.
  static let group = Hub.isDevApp ? "group.app.justyo.ios.dev" : "group.app.justyo.ios"

  /// The group's folder; nil while the signature carries no App Group.
  static var container: URL? {
    FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
  }

  static var friendsFile: URL? { container?.appending(path: "friends.json") }

  static var outbox: URL? {
    guard var dir = container?.appending(path: "Outbox", directoryHint: .isDirectory) else { return nil }
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    // Never in the backup, as the app's own outbox (MediaStorage).
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? dir.setResourceValues(values)
    return dir
  }

  static func media(_ share: Share, in dir: URL) -> URL {
    dir.appending(path: share.id.uuidString + (share.kind == .photo ? ".jpg" : ".mp4"))
  }

  static func note(_ id: UUID, in dir: URL) -> URL { dir.appending(path: id.uuidString + ".json") }

  /// The list the extension shows (nil: signed out, it shows none).
  static func readList() -> List? {
    guard let file = friendsFile, let data = try? Data(contentsOf: file) else { return nil }
    return try? JSONDecoder().decode(List.self, from: data)
  }

  /// The extension: a photo or video for a friend into the outbox. The media first, the note last: a share without
  /// its note is not there yet.
  static func put(_ file: MediaFile, for friend: Friend, of owner: String) -> Bool {
    guard let dir = outbox else { return false }
    let share = Share(
      id: UUID(), owner: owner, to: friend.to, name: friend.name, kind: file.kind, width: file.width, height: file.height,
      durationMs: file.durationMs, created: .now)
    do {
      try file.data.write(to: media(share, in: dir), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
      try JSONEncoder().encode(share).write(
        to: note(share.id, in: dir), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
      return true
    } catch {
      try? FileManager.default.removeItem(at: media(share, in: dir))
      return false
    }
  }

  /// A Yo notification's category: "Yo Back" right on it (`Inbox.askForNotifications`).
  static let yoCategory = "yo"

  /// How a run of Yos reads, on the app's bar and in a notification alike: "Yo", one "!" for each Yo after the
  /// first, at most `mostBangs`, then " +n" for the rest ("Yo!!!!! +4" for ten).
  static let mostBangs = 5

  /// The "!"s of `count` Yos and how many more there are than "!"s.
  static func bangs(_ count: Int) -> (bangs: Int, more: Int) {
    let wanted = max(0, count - 1)
    let bangs = min(wanted, mostBangs)
    return (bangs, wanted - bangs)
  }

  /// "Yo", "Yo!!", "Yo!!!!! +4".
  static func yo(_ count: Int) -> String {
    let (bangs, more) = bangs(count)
    return "Yo" + String(repeating: "!", count: bangs) + (more > 0 ? " +\(more)" : "")
  }

  /// Yos from one sender, for a notification.
  struct Yos: Sendable {
    let name: String
    let user: String
    var count: Int
    /// Of them, photos and videos: always named, never lost among the Yos.
    var photos = 0
    var videos = 0
    /// The newest one's: a photo, a video, or nil for a Yo.
    let kind: MediaFile.Kind?
    /// The newest one as the app's inbox calls it ("user/sent at"): shown once, from the notification or the core.
    let id: String

    /// The original's words, the name as its owner writes it: "Yo from LEO", "📷 from LEO", "🎥 from LEO"; more read
    /// as on the bar, with the photos and videos among them: "Yo!!!!! +4 from LEO · 📷 2".
    var line: String {
      if count == 1 {
        switch kind {
        case .photo: return "📷 from \(name)"
        case .video: return "🎥 from \(name)"
        case nil: return "Yo from \(name)"
        }
      }
      var line = "\(ShareBridge.yo(count)) from \(name)"
      if photos > 0 { line += photos > 1 ? " · 📷 \(photos)" : " · 📷" }
      if videos > 0 { line += videos > 1 ? " · 🎥 \(videos)" : " · 🎥" }
      return line
    }

    /// When the newest was sent, from its id ("user/sent at").
    var sentAtMs: UInt64 { UInt64(id.split(separator: "/").last ?? "") ?? 0 }

    /// As a notification keeps it (`userInfo["yos"]`), for the next one to add to.
    var info: [String: Any] {
      ["name": name, "user": user, "count": count, "photos": photos, "videos": videos, "id": id,
       "kind": kind?.rawValue ?? "yo"]
    }

    init(
      name: String, user: String, count: Int, photos: Int = 0, videos: Int = 0, kind: MediaFile.Kind?, id: String
    ) {
      (self.name, self.user, self.count, self.photos, self.videos, self.kind, self.id) =
        (name, user, count, photos, videos, kind, id)
    }

    init?(info: [String: Any]) {
      guard let name = info["name"] as? String, let user = info["user"] as? String, let count = info["count"] as? Int,
        let id = info["id"] as? String
      else { return nil }
      let kind = (info["kind"] as? String).flatMap(MediaFile.Kind.init(rawValue:))
      self.init(
        name: name, user: user, count: count, photos: info["photos"] as? Int ?? 0, videos: info["videos"] as? Int ?? 0,
        kind: kind, id: id)
    }
  }

  /// `new` (the newest first) with the Yos the Yo notifications still on the screen name, per sender: one
  /// notification says them all ("Yo!!!!! +4 from LEO · 📷 2"), and those before it go (`replaced`).
  private static func merged(_ new: [Yos], with delivered: [UNNotification]) -> (yos: [Yos], replaced: [String]) {
    var all = new
    var replaced: [String] = []
    for note in delivered.sorted(by: { $0.date > $1.date })
    where note.request.content.categoryIdentifier == yoCategory {
      guard let kept = note.request.content.userInfo["yos"] as? [[String: Any]] else { continue }
      replaced.append(note.request.identifier)
      for earlier in kept.compactMap(Yos.init(info:)) {
        if let at = all.firstIndex(where: { $0.user == earlier.user }) {
          all[at].count += earlier.count
          all[at].photos += earlier.photos
          all[at].videos += earlier.videos
        } else {
          all.append(earlier)
        }
      }
    }
    return (all, replaced)
  }

  /// The Yos among `events`, one per sender, the newest sender first, as a notification names them; `name` names a
  /// sender by user id. What Yo does not show yet (a text, a link, a place) comes as a Yo, as the app shows it.
  static func yos(in events: [YoEvent], name: (String) -> String) -> [Yos] {
    var senders: [String: Yos] = [:]
    for event in events {
      guard case .message(let user, let sentAtMs, let message) = event else { continue }
      let kind = message.media?.kind
      let before = senders[user]
      let newer = before.map { sentAtMs >= $0.sentAtMs } ?? true
      senders[user] = Yos(
        name: before?.name ?? name(user), user: user, count: (before?.count ?? 0) + 1,
        photos: (before?.photos ?? 0) + (kind == .photo ? 1 : 0),
        videos: (before?.videos ?? 0) + (kind == .video ? 1 : 0),
        kind: newer ? kind : before!.kind, id: newer ? "\(user)/\(sentAtMs)" : before!.id)
    }
    return senders.values.sorted { $0.sentAtMs > $1.sentAtMs }
  }

  /// The name as the app's list shows it (the friends it published), else as the core knows them.
  static func name(of user: String, client: YoClient) -> String {
    let owner = Hub.bytes(client.userId())?.base64EncodedString()
    if let list = readList(), list.owner == owner, let friend = list.friends.first(where: { $0.to == user }) {
      return friend.name
    }
    let contact = client.contacts().first { $0.user == user }
    return contact?.handles.first ?? contact?.name ?? "Someone"
  }

  /// A Yo notification, written by the notification service (a push) or by the app (Yos it took while in the
  /// background): a line per sender, the newest first, whose thread and "Yo Back" it is; the Yo sound.
  static func yo(_ content: UNMutableNotificationContent, _ senders: [Yos]) {
    guard let newest = senders.first else { return }
    content.title = ""
    content.body = senders.map(\.line).joined(separator: "\n")
    content.sound = UNNotificationSound(named: UNNotificationSoundName("yo.caf"))
    content.categoryIdentifier = yoCategory
    content.threadIdentifier = newest.user
    content.interruptionLevel = .active
    // `user` for Yo Back, also to someone not on the list yet; `id` so the app shows the Yo once.
    content.userInfo = ["from": newest.name, "user": newest.user, "id": newest.id, "yos": senders.map(\.info)]
  }

  /// The Yo notification for `new` Yos (none: a push that brought nothing new), added to those the Yo notifications on
  /// the screen name, written into `content`; the identifiers of the notifications it takes the place of.
  static func write(_ content: UNMutableNotificationContent, _ new: [Yos], over delivered: [UNNotification]) -> [String] {
    let (all, replaced) = merged(new, with: delivered)
    yo(content, all)
    return replaced
  }

  /// A Yo notification from the app or the share extension (Yos they took themselves, which the notification service
  /// then finds read): one with those on the screen. Not for a Yo a notification already names.
  static func post(_ yos: [Yos]) async {
    let center = UNUserNotificationCenter.current()
    let delivered = await center.deliveredNotifications()
    let written = Set(delivered.compactMap { $0.request.content.userInfo["id"] as? String })
    let new = yos.filter { !written.contains($0.id) }.sorted { $0.sentAtMs > $1.sentAtMs }
    guard let newest = new.first else { return }
    let content = UNMutableNotificationContent()
    center.removeDeliveredNotifications(withIdentifiers: write(content, new, over: delivered))
    try? await center.add(UNNotificationRequest(identifier: newest.id, content: content, trigger: nil))
  }
}

extension MessageKind {
  /// The photo or video a Yo carries, and which it is; nil for a Yo (or what Yo does not show yet).
  var media: (media: YoMedia, kind: MediaFile.Kind)? {
    switch self {
    case .photo(let photo): (photo, .photo)
    case .video(let video, _): (video, .video)
    default: nil
    }
  }
}
