// The app's side of sharing (ShareBridge): publishes the list to the extensions and sends what the share
// extension could not send itself through the media outbox.
import Foundation
import WidgetKit

@MainActor
enum ShareInbox {
  private static var taking = false
  /// Bumped at sign-out: a take under way stops.
  private static var generation = 0

  /// Nil when signed out. Written in order on one queue: an older list never lands after a newer one or a sign-out.
  static func publish(_ list: ShareBridge.List?) {
    WatchLink.shared.publish(list)
    guard let file = ShareBridge.friendsFile else { return }
    let data = list.flatMap { try? JSONEncoder().encode($0) }
    writing.async {
      if let data {
        Diagnostics.attempt({ try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]) })
      } else {
        try? FileManager.default.removeItem(at: file)
      }
      // Siri and Shortcuts know friends by name ("Yo Mia").
      YoShortcuts.updateAppShortcutParameters()
      SiriMessages.teachNames(list)
    }
    reloadWidgets()
    FriendSpotlight.update(list)
  }

  /// Coalesced: a run of Yos reorders the list with each.
  private static func reloadWidgets() {
    guard widgetReload == nil else { return }
    widgetReload = Task {
      try? await Task.sleep(for: .seconds(2))
      widgetReload = nil
      WidgetCenter.shared.reloadAllTimelines()
      // A renamed friend: the Yo Back control names them as the list does.
      ControlCenter.shared.reloadControls(ofKind: ShareBridge.yoBackControlKind)
    }
  }

  private static var widgetReload: Task<Void, Never>?

  private static let writing = DispatchQueue(label: "app.justyo.share-list", qos: .utility)

  static func clear() {
    WatchLink.shared.publish(nil)
    generation += 1
    FriendSpotlight.wipe()
    guard let container = ShareBridge.container else { return }
    try? FileManager.default.removeItem(at: container.appending(path: "Outbox"))
    let list = container.appending(path: "friends.json")
    writing.async {  // after any list still being written
      try? FileManager.default.removeItem(at: list)
      SiriMessages.teachNames(nil)
    }
  }

  /// The extension's outbox into the media outbox, oldest first; each leaves the group once the outbox has it.
  static func take(outbox: MediaOutbox, accounts: AccountStore) {
    guard !taking, let client = accounts.client, let dir = ShareBridge.outbox else { return }
    taking = true
    let generation = generation
    Task {
      defer { taking = false }
      let shares = await Task.detached(priority: .utility) { waiting(in: dir) }.value
      for share in shares {
        let media = ShareBridge.media(share, in: dir)
        let note = ShareBridge.note(share.id, in: dir)
        // An original the extension could not make small is made here.
        let made: Made
        if let ext = share.original {
          made = await original(share, at: media, ext: ext)
        } else {
          made = .data(await Task.detached(priority: .utility) { try? Data(contentsOf: media) }.value)
        }
        // Signed out or switched meanwhile: nothing goes under the new account.
        guard generation == self.generation, client === accounts.client, let owner = accounts.shareOwner else { return }
        // There but not readable now (locked): tried again later.
        if case .unreadable = made { continue }
        if case .data(nil) = made, FileManager.default.fileExists(atPath: media.path) { continue }
        let file: MediaFile? = switch made {
        case .data(let data?):
          MediaFile(kind: share.kind, data: data, width: share.width, height: share.height, durationMs: share.durationMs)
        case .file(let file): file
        default: nil
        }
        if let file, share.owner == owner {
          // The share's id: taken in once even when the app ended before the note was removed.
          guard
            await outbox.send(
              file, to: share.to, from: client, accounts: accounts, id: share.id, at: share.created)
          else { continue }
          Stats.sentYo(to: share.name)
        }
        try? FileManager.default.removeItem(at: note)
        try? FileManager.default.removeItem(at: media)
      }
    }
  }

  private enum Made: Sendable { case data(Data?), file(MediaFile?), unreadable }

  /// From a copy: MediaFile.video deletes what it reads.
  nonisolated private static func original(_ share: ShareBridge.Share, at media: URL, ext: String) async -> Made {
    let copy = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).\(ext)")
    guard (try? FileManager.default.copyItem(at: media, to: copy)) != nil else {
      return FileManager.default.fileExists(atPath: media.path) ? .unreadable : .file(nil)
    }
    switch share.kind {
    case .video:
      return .file(await MediaFile.video(copy))
    case .photo:
      defer { try? FileManager.default.removeItem(at: copy) }
      return .file(await Task.detached(priority: .utility) {
        (try? Data(contentsOf: copy, options: .alwaysMapped)).flatMap(MediaFile.photo)
      }.value)
    }
  }

  /// Noted shares, oldest first. Media an hour old without a note go; a note that does not decode is set aside.
  nonisolated private static func waiting(in dir: URL) -> [ShareBridge.Share] {
    let manager = FileManager.default
    let files = (try? manager.contentsOfDirectory(at: dir, includingPropertiesForKeys: [.creationDateKey])) ?? []
    var shares: [ShareBridge.Share] = []
    for file in files where file.pathExtension == "json" {
      guard let data = try? Data(contentsOf: file) else { continue }
      if let share = try? JSONDecoder().decode(ShareBridge.Share.self, from: data) {
        shares.append(share)
      } else {
        try? manager.moveItem(at: file, to: file.appendingPathExtension("unreadable"))
      }
    }
    // Any note, read or not, protects its media by id prefix ("<id>.json", "<id>.json.unreadable", "<id>.jpg").
    let id = { (file: URL) in String(file.lastPathComponent.prefix { $0 != "." }) }
    let notes = files.filter { $0.lastPathComponent.contains(".json") }
    let noted = Set(notes.map(id))
    for file in files where !file.lastPathComponent.contains(".json") && !noted.contains(id(file)) {
      let created = (try? file.resourceValues(forKeys: [.creationDateKey]))?.creationDate ?? .distantPast
      if Date.now.timeIntervalSince(created) > 3600 { try? manager.removeItem(at: file) }
    }
    return shares.sorted { $0.created < $1.created }
  }
}

extension AccountStore {
  /// Shares picked from this account's list go only from it.
  var shareOwner: String? { account?.userID.base64EncodedString() }

  func shareList(_ friends: Friends) -> ShareBridge.List? {
    shareOwner.map { ShareBridge.List(owner: $0, friends: friends.shown.map(\.shared)) }
  }
}

extension Friend {
  var shared: ShareBridge.Friend { ShareBridge.Friend(name: name, color: color, to: target) }
}
