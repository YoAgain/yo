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
    /// Not made small yet (the extension had no memory for it): the original's file extension ("mov", "heic"); the
    /// app makes it (MediaFile) before it goes. Nil: ready as it lies.
    var original: String? = nil
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

  /// The extension: a photo or video it could not make small itself (too little memory), copied into the outbox as it
  /// was shared, for the app to make and send. The media first, the note last, as `put`.
  static func putOriginal(_ url: URL, kind: MediaFile.Kind, for friend: Friend, of owner: String) -> Bool {
    guard let dir = outbox else { return false }
    let share = Share(
      id: UUID(), owner: owner, to: friend.to, name: friend.name, kind: kind, width: 0, height: 0, durationMs: 0,
      created: .now, original: url.pathExtension.isEmpty ? (kind == .photo ? "jpg" : "mov") : url.pathExtension)
    do {
      try FileManager.default.copyItem(at: url, to: media(share, in: dir))
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

  /// One Yo, for a notification of its own (as Signal: every message its own, iOS stacks them by sender).
  struct Yos: Sendable {
    /// Who sent it: "Mia"; a group's "Mia in #Hey".
    let name: String
    /// The sender's user id, or "group:" and the group's id: the notification's thread.
    let user: String
    /// A photo, a video, or nil for a Yo.
    let kind: MediaFile.Kind?
    /// As the app's inbox calls it ("user/sent at"): shown once, from the notification or the core.
    let id: String
    /// A group's Yo (`user` is "group:" and its id): the name of its bar in the list ("FAMILY", "Group").
    var bar: String? = nil
    /// From someone not yet a contact, or a group a stranger added this account to (spec 4.10): "Yo Request".
    var request = false

    /// "Yo from Mia", "Yo from Mia 📷", "Yo from Mia in #Hey 🎥", "Yo Request from Mia".
    var line: String {
      productNotificationLine(name: name, kind: kind == .video ? .video : kind == .photo ? .photo : .yo,
        request: request, lang: .english)
    }

    /// One per Yo: none takes another's place.
    var notificationIdentifier: String { productNotificationId(eventId: id) }

    /// When it was sent, from its id ("user/sent at").
    var sentAtMs: UInt64 { UInt64(id.split(separator: "/").last ?? "") ?? 0 }

    init(name: String, user: String, kind: MediaFile.Kind?, id: String) {
      (self.name, self.user, self.kind, self.id) = (name, user, kind, id)
    }
  }

  /// The Yos among `events`, one per Yo, the newest first, as notifications name them; `name` names a sender by user
  /// id. What Yo does not show yet (a text, a link, a place) comes as a Yo, as the app shows it. A group's Yo is named
  /// "LEO in #FAMILY" (`group` names a group by id). `requests`: the senders (user ids) and groups ("group:" and the
  /// id) not accepted yet (`requests(of:)`).
  static func yos(
    in events: [YoEvent], name: (String) -> String, group: (String) -> String = { _ in "" },
    requests: Set<String> = []
  ) -> [Yos] {
    var all: [Yos] = []
    for event in events {
      // The id as the app takes the Yo in (Inbox.take), so one notified and one received count once.
      var yos: Yos
      switch event {
      case .message(let user, let at, let kind):
        yos = Yos(name: name(user), user: productNotificationThread(user: user, group: nil), kind: kind.media?.kind, id: "\(user)/\(at)")
      case .groupMessage(let id, let user, let at, let kind):
        let groupName = group(id)
        let from = name(user)
        yos = Yos(
          name: productNotificationGroupName(name: from, group: groupName, lang: .english),
          user: productNotificationThread(user: user, group: id), kind: kind.media?.kind,
          id: "\(id)/\(user)/\(at)")
        yos.bar = groupName.isEmpty ? productText(text: .group, lang: .english) : groupName  // as the list names the group's bar (Friends.takeGroups)
      default: continue
      }
      yos.request = requests.contains(yos.user)
      all.append(yos)
    }
    return all.sorted { $0.sentAtMs > $1.sentAtMs }
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
    if let name = contact?.handles.first ?? contact?.name, !name.isEmpty, name != "?" { return name }
    UnknownSender.record(place: "notification", user: user, contact: contact.map(UnknownSender.facts), group: nil,
      friend: readList()?.friends.contains { $0.to == user } ?? false)
    return "Someone"
  }

  /// A Yo notification, written by the notification service (a push) or by the app (Yos it took while in the
  /// background): "Yo from Mia", in the thread of its sender (or group), which iOS stacks (its own "+n" wording: a
  /// summary argument is ignored since iOS 15), with "Yo Back". The Yo sound only when `sounds` (the first of its thread on the screen), else quietly.
  static func yo(_ content: UNMutableNotificationContent, _ yo: Yos, sounds: Bool) {
    (content.title, content.subtitle, content.body) = ("", "", yo.line)
    content.sound = sounds ? UNNotificationSound(named: UNNotificationSoundName("yo.caf")) : nil
    content.interruptionLevel = sounds ? .active : .passive
    content.categoryIdentifier = yoCategory
    content.threadIdentifier = yo.user
    // `user` for Yo Back, also to someone not on the list yet; `id` so the app shows the Yo once.
    // A group's: from its bar, where the app shows it.
    content.userInfo = ["from": yo.bar ?? yo.name, "sender": yo.name, "user": yo.user, "id": yo.id]
  }

  /// The notifications for `new` Yos (the newest first; none: a push that brought nothing new): the newest written into
  /// `content`, each other one added as its own. The identifiers of the notifications `content` takes the place of.
  static func write(_ content: UNMutableNotificationContent, _ new: [Yos], over delivered: [UNNotification]) -> [String] {
    // Nothing new (a push whose Yos an earlier push or the app took): never one more bare "Yo" (an empty one is not
    // hidden either, without the filtering entitlement), but the newest Yo notification again, quietly, in its place.
    guard let newest = new.first else { return again(content, over: delivered) }
    let shown = delivered.filter { $0.request.content.categoryIdentifier == yoCategory }
    // The app's red badge: as many as there are Yo notifications, until the app is opened, which clears both
    // (Inbox.collectDelivered).
    let badge = NSNumber(value: shown.count + new.count)
    // The sound for the first unread of a sender (or group) only: while one of its thread is on the screen, the next
    // come quietly (no sound, no banner); once read or cleared, the next one sounds again.
    let sounds = productNotificationSounds(delivered: shown.map(\.request.content.threadIdentifier),
      new: new.reversed().map(\.user))
    // The oldest first, the newest last (this push's own, on top).
    for (other, sound) in zip(new.dropFirst().reversed(), sounds) {
      let own = UNMutableNotificationContent()
      yo(own, other, sounds: sound)
      own.badge = badge
      UNUserNotificationCenter.current().add(
        UNNotificationRequest(identifier: other.notificationIdentifier, content: own, trigger: nil))
    }
    yo(content, newest, sounds: sounds.last ?? false)
    content.badge = badge
    return []
  }

  /// The newest Yo notification on the screen said again into `content`, quietly, with the badge as it is; its
  /// identifier, to go. None on the screen: a bare "Yo" with no sound (a receipt after the app was opened; read, so
  /// not the push's "You may have received a Yo", and an empty one would show that).
  private static func again(_ content: UNMutableNotificationContent, over delivered: [UNNotification]) -> [String] {
    (content.sound, content.interruptionLevel) = (nil, .passive)
    let yos = delivered.filter {
      $0.request.content.categoryIdentifier == yoCategory && $0.request.content.userInfo["id"] != nil
    }
    guard let newest = yos.max(by: { $0.date < $1.date }) else {
      (content.title, content.subtitle, content.body) = ("", "", productText(text: .yo, lang: .english))
      return []
    }
    let shown = newest.request.content
    (content.title, content.subtitle, content.body) = (shown.title, shown.subtitle, shown.body)
    (content.categoryIdentifier, content.threadIdentifier, content.userInfo) =
      (shown.categoryIdentifier, shown.threadIdentifier, shown.userInfo)
    content.badge = NSNumber(value: yos.count)
    return [newest.request.identifier]
  }

  /// Yo notifications from the app or the share extension (Yos they took themselves, which the notification service
  /// then finds read). Not for a Yo a notification already names.
  static func post(_ yos: [Yos]) async {
    let center = UNUserNotificationCenter.current()
    let delivered = await center.deliveredNotifications()
    let written = Set(delivered.compactMap { $0.request.content.userInfo["id"] as? String })
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
