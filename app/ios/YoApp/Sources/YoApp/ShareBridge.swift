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
    Diagnostics.attempt({ try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true) })
    // Never in the backup, as the app's own outbox (MediaStorage).
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    Diagnostics.attempt({ try dir.setResourceValues(values) })
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
      Diagnostics.record(error)
      try? FileManager.default.removeItem(at: media(share, in: dir))
      return false
    }
  }

  /// A Yo notification's category: "Yo Back" right on it, also on the Lock Screen without unlocking (no
  /// authentication, not opening the app: NotificationDelegate sends it as a guest, as the Yo Back intent does). The
  /// hub's push names it too, so the action is there even when the notification service runs out of time.
  static let yoCategory = "yo"
  static let yoBackAction = "yo.back"

  /// The Yo category, set by the app and by the notification service alike: after a restart the service may show a
  /// Yo before the app has run.
  static func registerCategories() {
    let yoBack = UNNotificationAction(identifier: yoBackAction, title: "Yo Back", options: [])
    UNUserNotificationCenter.current().setNotificationCategories([
      UNNotificationCategory(identifier: yoCategory, actions: [yoBack], intentIdentifiers: [])
    ])
  }

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
    /// A group's Yos (`user` is "group:" and its id): the name of its bar in the list ("FAMILY", "Group").
    var bar: String? = nil
    /// From someone not yet a contact, or a group a stranger added this account to (spec 4.10): "Yo Request".
    var request = false

    /// The original's words, the name as its owner writes it: "Yo from LEO", "📷 from LEO", "🎥 from LEO"; more read
    /// as on the bar, with the photos and videos among them: "Yo!!!!! +4 from LEO · 📷 2".
    var line: String {
      let who = bar.map { $0.isEmpty ? name : "\(name) @ \($0)" } ?? name
      return request ? "Yo Request from \(who)" : "\(what) from \(who)"
    }

    /// What came, without who: "Yo", "📷", "🎥", "Yo!!!!! +4 · 📷 2".
    var what: String {
      if count == 1 {
        switch kind {
        case .photo: return "📷"
        case .video: return "🎥"
        case nil: return "Yo"
        }
      }
      var what = ShareBridge.yo(count)
      if photos > 0 { what += photos > 1 ? " · 📷 \(photos)" : " · 📷" }
      if videos > 0 { what += videos > 1 ? " · 🎥 \(videos)" : " · 🎥" }
      return what
    }

    /// Stable across a sender’s run; message ids remain in userInfo for deduplication.
    var notificationIdentifier: String { "yo:" + user }

    /// When the newest was sent, from its id ("user/sent at").
    var sentAtMs: UInt64 { UInt64(id.split(separator: "/").last ?? "") ?? 0 }

    /// As a notification keeps it (`userInfo["yos"]`), for the next one to add to.
    var info: [String: Any] {
      ["sender": name, "name": name, "user": user, "count": count, "photos": photos, "videos": videos, "id": id,
       "kind": kind?.rawValue ?? "yo", "bar": bar ?? "", "request": request]
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
      bar = (info["bar"] as? String).flatMap { $0.isEmpty ? nil : $0 }
      request = info["request"] as? Bool ?? false
    }
  }

  /// `new` (the newest first) with the Yos the Yo notifications still on the screen name, per sender: one
  /// notification per sender keeps its accumulated count ("Yo!!!!! +4 from LEO · 📷 2").
  private static func merged(_ new: [Yos], with delivered: [UNNotification]) -> (yos: [Yos], replaced: [String]) {
    // One notification per sender (or group): a sender's earlier notification is taken into its new one; the others
    // stay as they are.
    var all = new
    var replaced: [String] = []
    for note in delivered.sorted(by: { $0.date > $1.date })
    where note.request.content.categoryIdentifier == yoCategory {
      guard let kept = note.request.content.userInfo["yos"] as? [[String: Any]] else { continue }
      let earlier = kept.compactMap(Yos.init(info:))
      guard earlier.contains(where: { e in all.contains { $0.user == e.user } }) else { continue }
      replaced.append(note.request.identifier)
      for e in earlier {
        if let at = all.firstIndex(where: { $0.user == e.user }) {
          all[at].count += e.count
          all[at].photos += e.photos
          all[at].videos += e.videos
        }
      }
    }
    return (all, replaced)
  }

  /// The Yos among `events`, one per sender, the newest sender first, as a notification names them; `name` names a
  /// sender by user id. What Yo does not show yet (a text, a link, a place) comes as a Yo, as the app shows it.
  /// A group's Yos are one entry per group, named "LEO in FAMILY" by its newest sender (`group` names a group by id).
  /// `requests`: the senders (user ids) and groups ("group:" and the id) not accepted yet (`requests(of:)`).
  static func yos(
    in events: [YoEvent], name: (String) -> String, group: (String) -> String = { _ in "" },
    requests: Set<String> = []
  ) -> [Yos] {
    var senders: [String: Yos] = [:]
    for event in events {
      // The id as the app takes the Yo in (Inbox.take), so one notified and one received count once.
      let (key, yoID, sentAtMs, message, from): (String, String, UInt64, MessageKind, String)
      var bar: String?
      var groupName = ""
      switch event {
      case .message(let user, let at, let kind):
        (key, yoID, sentAtMs, message, from) = (user, "\(user)/\(at)", at, kind, user)
      case .groupMessage(let id, let user, let at, let kind):
        groupName = group(id)
        bar = groupName.isEmpty ? "Group" : groupName  // as the list names the group's bar (Friends.takeGroups)
        (key, yoID, sentAtMs, message, from) = ("group:" + id, "\(id)/\(user)/\(at)", at, kind, user)
      default: continue
      }
      let kind = message.media?.kind
      let before = senders[key]
      let newer = before.map { sentAtMs >= $0.sentAtMs } ?? true
      var yos = Yos(
        name: newer ? (groupName.isEmpty ? name(from) : "\(name(from)) in \(groupName)") : before!.name, user: key, count: (before?.count ?? 0) + 1,
        photos: (before?.photos ?? 0) + (kind == .photo ? 1 : 0),
        videos: (before?.videos ?? 0) + (kind == .video ? 1 : 0),
        kind: newer ? kind : before!.kind, id: newer ? yoID : before!.id)
      yos.bar = bar
      yos.request = requests.contains(key)
      senders[key] = yos
    }
    return senders.values.sorted { $0.sentAtMs > $1.sentAtMs }
  }

  /// Who is a request (spec 4.10), as `yos` takes it: contacts and groups not accepted yet.
  static func requests(of client: YoClient) -> Set<String> {
    Set(client.contacts().filter { !$0.accepted }.map(\.user))
      .union(((Diagnostics.attempt({ try client.groups() })) ?? []).filter { !$0.accepted && !$0.left }.map { "group:" + $0.id })
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
    // As the original's (2014): "Yo" on top, then "From LEO" ("Yo!!" / "From LEO @ FAMILY"; a photo "📷"). Several:
    // a line each.
    if senders.count == 1 {
      let who = newest.bar.map { $0.isEmpty ? newest.name : "\(newest.name) @ \($0)" } ?? newest.name
      // A request says who, not what.
      content.title = newest.request ? "Yo Request" : newest.what
      content.body = "From \(who)"
    } else {
      content.title = ""
      content.body = senders.map(\.line).joined(separator: "\n")
    }
    content.sound = UNNotificationSound(named: UNNotificationSoundName("yo.caf"))
    content.categoryIdentifier = yoCategory
    content.threadIdentifier = newest.user
    content.interruptionLevel = .active
    // `user` for Yo Back, also to someone not on the list yet; `id` so the app shows the Yo once.
    // A group's: from its bar, where the app shows it.
    content.userInfo = [
      "from": newest.bar ?? newest.name, "sender": newest.name, "user": newest.user, "id": newest.id, "yos": senders.map(\.info),
    ]
  }

  /// The Yo notification for `new` Yos (none: a push that brought nothing new), added to those the Yo notifications on
  /// the screen name, written into `content`; the identifiers of the notifications it takes the place of.
  static func write(_ content: UNMutableNotificationContent, _ new: [Yos], over delivered: [UNNotification]) -> [String] {
    let (all, replaced) = merged(new, with: delivered)
    // The app's red badge: as many as there are Yo notifications (one per sender or group, as merged here), until
    // the app is opened, which clears both (Inbox.collectDelivered). None left: no badge.
    let badge = NSNumber(value: all.count)
    content.badge = badge
    // The newest sender in this notification; every other sender gets a notification of its own.
    guard let newest = all.first else { return replaced }
    yo(content, [newest])
    quietUpdate(content, for: newest, over: delivered)
    for other in all.dropFirst() {
      let own = UNMutableNotificationContent()
      yo(own, [other])
      own.badge = badge
      quietUpdate(own, for: other, over: delivered)
      UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: other.notificationIdentifier, content: own, trigger: nil))
    }
    return replaced
  }

  /// Replacing an existing sender’s notification should not play the Yo sound again.
  private static func quietUpdate(_ content: UNMutableNotificationContent, for sender: Yos, over delivered: [UNNotification]) {
    if delivered.contains(where: { $0.request.identifier == sender.notificationIdentifier }) {
      content.sound = nil
      content.interruptionLevel = .passive
    }
  }

  /// A Yo notification from the app or the share extension (Yos they took themselves, which the notification service
  /// then finds read): one with those on the screen. Not for a Yo a notification already names.
  static func post(_ yos: [Yos]) async {
    let center = UNUserNotificationCenter.current()
    let delivered = await center.deliveredNotifications()
    // Every Yo a notification already names, by each sender's newest id in it: none is counted twice.
    let written = Set(
      delivered.flatMap { note in
        ((note.request.content.userInfo["yos"] as? [[String: Any]]) ?? []).compactMap { $0["id"] as? String }
          + [note.request.content.userInfo["id"] as? String].compactMap { $0 }
      })
    let new = yos.filter { !written.contains($0.id) }.sorted { $0.sentAtMs > $1.sentAtMs }
    guard let newest = new.first else { return }
    let content = UNMutableNotificationContent()
    _ = write(content, new, over: delivered)
    try? await center.add(UNNotificationRequest(identifier: newest.notificationIdentifier, content: content, trigger: nil))
  }
}

/// Who sent the newest Yo, for "Yo Back" from the Action button, Shortcuts or Siri (YoIntents.swift): written wherever
/// Yos are taken (the app, the notification service, the share extension, an intent's own send), the newest wins.
/// A file in the group, readable after the first unlock as the state is: Yo Back works while the iPhone is locked.
extension ShareBridge {
  struct LastYo: Codable, Equatable, Sendable {
    /// The account it came to (as `List.owner`): another account never sends it back.
    let owner: String
    /// Whom the Yo goes back to: a user id, or "group:" and the group's id.
    let to: String
    /// As the notification names it: the sender, or the group's bar.
    let name: String
    let sentAtMs: UInt64
  }

  static var lastYoFile: URL? { container?.appending(path: "last-yo.json") }

  static func readLastYo() -> LastYo? {
    guard let file = lastYoFile, let data = try? Data(contentsOf: file) else { return nil }
    return try? JSONDecoder().decode(LastYo.self, from: data)
  }

  /// The newest of `yos` (as `yos(in:)` makes them), if newer than the one noted.
  static func noteLast(_ yos: [Yos], client: YoClient) {
    guard let newest = yos.filter({ !$0.request }).max(by: { $0.sentAtMs < $1.sentAtMs }), let file = lastYoFile,
      let owner = Hub.bytes(client.userId())?.base64EncodedString()
    else { return }
    if let noted = readLastYo(), noted.owner == owner, noted.sentAtMs >= newest.sentAtMs { return }
    let name = newest.bar.map { $0.isEmpty ? newest.name : $0 } ?? newest.name
    let last = LastYo(owner: owner, to: newest.user, name: name, sentAtMs: newest.sentAtMs)
    guard let data = try? JSONEncoder().encode(last) else { return }
    Diagnostics.attempt({ try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]) })
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

/// Photos and videos to a friend (a user id or a name) or to a group ("group:" and its id, `Friend.target`).
extension YoClient {
  func mediaChat(toTarget to: String) throws -> MediaChat {
    to.hasPrefix("group:") ? try groupMediaChat(group: String(to.dropFirst(6))) : try mediaChat(to: to)
  }

  func sendPhoto(toTarget to: String, media: YoMedia) throws -> [YoEvent] {
    to.hasPrefix("group:")
      ? try sendGroupPhoto(group: String(to.dropFirst(6)), media: media) : try sendPhoto(to: to, media: media)
  }

  func sendVideo(toTarget to: String, media: YoMedia, durationMs: UInt32) throws -> [YoEvent] {
    to.hasPrefix("group:")
      ? try sendGroupVideo(group: String(to.dropFirst(6)), media: media, durationMs: durationMs)
      : try sendVideo(to: to, media: media, durationMs: durationMs)
  }
}
