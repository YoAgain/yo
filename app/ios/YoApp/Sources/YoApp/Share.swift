// The app's side of sharing (ShareBridge): it tells the extensions who is on the list, and sends what the share
// extension could not send itself as soon as it opens, through the media outbox as a photo or video taken here.
import Foundation
import WidgetKit

@MainActor
enum ShareInbox {
  private static var taking = false
  /// Bumped at sign-out: a take under way stops.
  private static var generation = 0

  /// The list the extension shows: the friends on it (the blocked are not), nil when signed out. A few kilobytes,
  /// encoded here and written off the main thread (a file write is no work for a tap), in order on one queue: an older
  /// list never lands after a newer one or after a sign-out.
  static func publish(_ list: ShareBridge.List?) {
    guard let file = ShareBridge.friendsFile else { return }
    let data = list.flatMap { try? JSONEncoder().encode($0) }
    writing.async {
      if let data {
        Diagnostics.attempt({ try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]) })
      } else {
        try? FileManager.default.removeItem(at: file)
      }
      // Siri and Shortcuts know the friends by name ("Yo Mia"): told once the list is written.
      YoShortcuts.updateAppShortcutParameters()
    }
    reloadWidgets()
  }

  /// The widgets show the list as it is (YoWidgets.swift): told a moment after it changes, at most once in that
  /// moment however often it changes meanwhile (a run of Yos reorders it with each).
  private static func reloadWidgets() {
    guard widgetReload == nil else { return }
    widgetReload = Task {
      try? await Task.sleep(for: .seconds(2))
      widgetReload = nil
      WidgetCenter.shared.reloadAllTimelines()
    }
  }

  private static var widgetReload: Task<Void, Never>?

  private static let writing = DispatchQueue(label: "app.justyo.share-list", qos: .utility)

  /// Signed out: nothing shared waits to go, nobody is shown.
  static func clear() {
    generation += 1
    guard let container = ShareBridge.container else { return }
    try? FileManager.default.removeItem(at: container.appending(path: "Outbox"))
    let list = container.appending(path: "friends.json")
    writing.async { try? FileManager.default.removeItem(at: list) }  // after any list still being written
  }

  /// What waits in the extension's outbox goes into the media outbox, oldest first; each leaves the group once the
  /// outbox has it. With no core yet (or one that will not take it now) it stays for the next opening.
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
        let data = await Task.detached(priority: .utility) { try? Data(contentsOf: media) }.value
        // Checked after the read: signed out (or into another account) meanwhile, nothing goes under the new one.
        guard generation == self.generation, client === accounts.client, let owner = accounts.shareOwner else { return }
        // There and not readable now (protected until the iPhone is unlocked, a passing error): tried again later, not
        // thrown away.
        if data == nil && FileManager.default.fileExists(atPath: media.path) { continue }
        if let data, share.owner == owner {
          let file = MediaFile(
            kind: share.kind, data: data, width: share.width, height: share.height, durationMs: share.durationMs)
          // By the share's own id: taken in once, also when the app ended before the note below went.
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

  /// The shares whose note is written, oldest first. Media an hour old without a note (the extension ended between
  /// the two) go; a note that reads but is no note is set aside, one not readable now waits.
  nonisolated private static func waiting(in dir: URL) -> [ShareBridge.Share] {
    let manager = FileManager.default
    let files = (try? manager.contentsOfDirectory(at: dir, includingPropertiesForKeys: [.creationDateKey])) ?? []
    var shares: [ShareBridge.Share] = []
    for file in files where file.pathExtension == "json" {
      // Not readable now: left for later. Read and not a note: set aside (`.unreadable`), never deleted unseen.
      guard let data = try? Data(contentsOf: file) else { continue }
      if let share = try? JSONDecoder().decode(ShareBridge.Share.self, from: data) {
        shares.append(share)
      } else {
        try? manager.moveItem(at: file, to: file.appendingPathExtension("unreadable"))
      }
    }
    // Every note there protects its media, read or not (one not readable now, one set aside): by the id its name
    // starts with ("<id>.json", "<id>.json.unreadable", "<id>.jpg").
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
  /// The account as the share extension knows it: shares picked from its list go only from it.
  var shareOwner: String? { account?.userID.base64EncodedString() }

  /// What the share extension shows: this account's friends on the list, nil when signed out.
  func shareList(_ friends: Friends) -> ShareBridge.List? {
    shareOwner.map { ShareBridge.List(owner: $0, friends: friends.shown.map(\.shared)) }
  }
}

extension Friend {
  /// As the share extension shows them.
  var shared: ShareBridge.Friend { ShareBridge.Friend(name: name, color: color, to: target) }
}
