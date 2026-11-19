// What the app and its extensions hand each other in the App Group: the friends list, shares the extension could
// not send itself, and Yo notifications. One writer per file.
import Foundation
import UserNotifications
import YoCore

enum ShareBridge {
  struct Friend: Codable, Equatable, Sendable {
    var name: String
    var color: Int
    /// The user id at the hub, else the name (before the first Yo).
    var to: String
  }

  struct List: Codable, Equatable, Sendable {
    let owner: String
    let friends: [Friend]
  }

  /// A share waiting for the app; its media lies beside it, `<id>.jpg` or `<id>.mp4`.
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
    /// Not made small yet (the extension ran out of memory): the original's file extension; the app converts it.
    var original: String? = nil
  }

  /// Without the group in the signature (Yo Dev from dev/push.sh) the container is nil and nothing is shared.
  static let group = Hub.isDevApp ? "group.app.justyo.ios.dev" : "group.app.justyo.ios"

  static var container: URL? {
    FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
  }

  static var friendsFile: URL? { container?.appending(path: "friends.json") }

  static var outbox: URL? {
    guard var dir = container?.appending(path: "Outbox", directoryHint: .isDirectory) else { return nil }
    Diagnostics.attempt({ try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true) })
    // Never in the backup, as MediaStorage.
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    Diagnostics.attempt({ try dir.setResourceValues(values) })
    return dir
  }

  static func media(_ share: Share, in dir: URL) -> URL {
    dir.appending(path: share.id.uuidString + (share.kind == .photo ? ".jpg" : ".mp4"))
  }

  static func note(_ id: UUID, in dir: URL) -> URL { dir.appending(path: id.uuidString + ".json") }

  /// Nil: signed out.
  static func readList() -> List? {
    guard let file = friendsFile, let data = try? Data(contentsOf: file) else { return nil }
    return try? JSONDecoder().decode(List.self, from: data)
  }

  /// The media first, the note last: a share without its note is not there yet.
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

  /// An original the extension had no memory to convert, for the app to convert and send. Order as `put`.
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

  /// "Yo Back" works on the Lock Screen without unlocking (sent as a guest). The hub's push names the category too, so
  /// the action is there even when the notification service runs out of time.
  static let yoCategory = "yo"
  static let yoBackAction = "yo.back"

  /// Set by the app and the service alike: after a restart the service may show a Yo before the app has run.
  static func registerCategories() {
    let yoBack = UNNotificationAction(identifier: yoBackAction, title: "Yo Back", options: [])
    UNUserNotificationCenter.current().setNotificationCategories([
      UNNotificationCategory(identifier: yoCategory, actions: [yoBack], intentIdentifiers: [])
    ])
  }

  /// One Yo, one notification of its own (iOS stacks them by thread).
  struct Yos: Sendable {
    /// "Mia"; a group's "Mia in #Hey".
    let name: String
    /// The sender's user id, or "group:" and the group's id: the notification's thread.
    let user: String
    let kind: MediaFile.Kind?
    /// As Inbox.take calls it ("user/sent at"), so one notified and one received count once.
    let id: String
    /// A group's bar name in the list.
    var bar: String? = nil
    var request = false

    var line: String {
      productNotificationLine(name: name, kind: kind == .video ? .video : kind == .photo ? .photo : .yo,
        request: request, lang: .english)
    }

    var notificationIdentifier: String { productNotificationId(eventId: id) }

    var sentAtMs: UInt64 { UInt64(id.split(separator: "/").last ?? "") ?? 0 }

    init(name: String, user: String, kind: MediaFile.Kind?, id: String) {
      (self.name, self.user, self.kind, self.id) = (name, user, kind, id)
    }
  }

  /// The Yos among `events`, newest first. `requests`: senders and groups not accepted yet (`requests(of:)`).
  static func yos(
    in events: [YoEvent], name: (String) -> String, group: (String) -> String = { _ in "" },
    requests: Set<String> = []
  ) -> [Yos] {
    var all: [Yos] = []
    for event in events {
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

  /// Contacts and groups not accepted yet (spec 4.10).
  static func requests(of client: YoClient) -> Set<String> {
    Set(client.contacts().filter { !$0.accepted }.map(\.user))
      .union(((Diagnostics.attempt({ try client.groups() })) ?? []).filter { !$0.accepted && !$0.left }.map { "group:" + $0.id })
  }

  /// The name as the app's list shows it, else as the core knows them.
  static func name(of user: String, client: YoClient) -> String {
    let owner = Hub.bytes(client.userId())?.base64EncodedString()
    if let list = readList(), list.owner == owner, let friend = list.friends.first(where: { $0.to == user }) {
      return friend.name
    }
    let contact = client.contacts().first { $0.user == user }
    if let name = contact?.handles.first ?? contact?.name, !name.isEmpty, name != "?" { return name }
    UnknownSender.record(place: "notification", user: user, contact: contact.map(UnknownSender.facts), group: nil,
      friend: readList()?.friends.contains { $0.to == user } ?? false)
    return productText(text: .someone, lang: .english)
  }

  /// One Yo notification, threaded by sender or group (iOS ignores a summary argument since iOS 15).
  static func yo(_ content: UNMutableNotificationContent, _ yo: Yos, sounds: Bool) {
    (content.title, content.subtitle, content.body) = ("", "", yo.line)
    content.sound = sounds ? UNNotificationSound(named: UNNotificationSoundName("yo.caf")) : nil
    content.interruptionLevel = sounds ? .active : .passive
    content.categoryIdentifier = yoCategory
    content.threadIdentifier = yo.user
    // `user` for Yo Back, also to someone not on the list yet; `id` so the app shows the Yo once.
    content.userInfo = ["from": yo.bar ?? yo.name, "sender": yo.name, "user": yo.user, "id": yo.id]
  }

  /// The newest of `new` goes into `content`, each other one is added as its own. Returns the identifiers of the
  /// notifications `content` replaces.
  static func write(_ content: UNMutableNotificationContent, _ new: [Yos], over delivered: [UNNotification]) -> [String] {
    // Nothing new (a push whose Yos an earlier push or the app took): never one more bare "Yo" (an empty one is not
    // hidden either, without the filtering entitlement), but the newest Yo notification again, quietly, in its place.
    guard let newest = new.first else { return again(content, over: delivered) }
    let shown = delivered.filter { $0.request.content.categoryIdentifier == yoCategory }
    // As many as there are Yo notifications; opening the app clears both (Inbox.collectDelivered).
    let badge = NSNumber(value: shown.count + new.count)
    // Only the first unread of a thread sounds; the next come quietly while it is on the screen.
    let sounds = productNotificationSounds(delivered: shown.map(\.request.content.threadIdentifier),
      new: new.reversed().map(\.user))
    // Oldest first, so this push's own lands on top.
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

  /// The newest Yo notification on screen repeated quietly into `content`; returns its identifier to remove. None on
  /// screen: a bare silent "Yo" (an empty one would show "You may have received a Yo").
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

  /// Yos the app or share extension took themselves (the service then finds them read); skips ones already shown.
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

/// Who sent the newest Yo, for the Yo Back intent: written wherever Yos are taken, newest wins. Readable after first
/// unlock, so Yo Back works while locked.
extension ShareBridge {
  struct LastYo: Codable, Equatable, Sendable {
    /// As `List.owner`: another account never sends it back.
    let owner: String
    let to: String
    let name: String
    let sentAtMs: UInt64
  }

  static var lastYoFile: URL? { container?.appending(path: "last-yo.json") }

  static func readLastYo() -> LastYo? {
    guard let file = lastYoFile, let data = try? Data(contentsOf: file) else { return nil }
    return try? JSONDecoder().decode(LastYo.self, from: data)
  }

  /// The newest of `yos`, if newer than the one noted.
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
  var media: (media: YoMedia, kind: MediaFile.Kind)? {
    switch self {
    case .photo(let photo): (photo, .photo)
    case .video(let video, _): (video, .video)
    default: nil
    }
  }
}

/// Yos sent from outside the list, for the list to move the bar on top with "Sent Yo" (FriendList). A file in the App
/// Group (the send may run in YoControls or YoShare) plus a Darwin notification for a list on screen.
struct IntentSent: Codable, Sendable {
  let to: String
  var back = false
  let at: Date

  static let changed = Notification.Name("yo.intentSent")
  private static var darwinName: String { ShareBridge.group + ".intentSent" }
  private static var file: URL? { ShareBridge.container?.appending(path: "intent-sent.json") }
  /// A Yo sent a day ago is no news on opening.
  static let kept: TimeInterval = 24 * 3600

  static func note(to: String, back: Bool) {
    guard let file else { return }
    let notes = (read(file) + [IntentSent(to: to, back: back, at: .now)]).suffix(20)
    guard let data = try? JSONEncoder().encode(Array(notes)) else { return }
    try? data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    CFNotificationCenterPostNotification(
      CFNotificationCenterGetDarwinNotifyCenter(), CFNotificationName(darwinName as CFString), nil, nil, true)
  }

  /// Oldest first; the file goes.
  static func take() -> [IntentSent] {
    guard let file else { return [] }
    let notes = read(file)
    try? FileManager.default.removeItem(at: file)
    return notes.filter { $0.at > .now - kept }
  }

  private static func read(_ file: URL) -> [IntentSent] {
    guard let data = try? Data(contentsOf: file) else { return [] }
    return (try? JSONDecoder().decode([IntentSent].self, from: data)) ?? []
  }

  /// Posts `changed` on the main queue for a note from any process.
  @MainActor static func listen() {
    guard !listening else { return }
    listening = true
    CFNotificationCenterAddObserver(
      CFNotificationCenterGetDarwinNotifyCenter(), nil,
      { _, _, _, _, _ in DispatchQueue.main.async { NotificationCenter.default.post(name: IntentSent.changed, object: nil) } },
      darwinName as CFString, nil, .deliverImmediately)
  }
  @MainActor private static var listening = false
}
