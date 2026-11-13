// The app's side of sharing (ShareBridge): it tells the extensions who is on the list, and sends what the share
// extension could not send itself as soon as it opens, through the media outbox as a photo or video taken here.
import Foundation

@MainActor
enum ShareInbox {
  private static var taking = false
  /// Bumped at sign-out: a take under way stops.
  private static var generation = 0

  /// The list the extension shows: the friends on it (the blocked are not), nil when signed out. A few kilobytes,
  /// written in order on the main actor: an older list never lands after a newer one or after a sign-out.
  static func publish(_ list: ShareBridge.List?) {
    guard let file = ShareBridge.friendsFile else { return }
    if let list, let data = try? JSONEncoder().encode(list) {
      try? data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    } else {
      try? FileManager.default.removeItem(at: file)
    }
  }

  /// Signed out: nothing shared waits to go, nobody is shown.
  static func clear() {
    generation += 1
    guard let container = ShareBridge.container else { return }
    for name in ["friends.json", "Outbox"] {
      try? FileManager.default.removeItem(at: container.appending(path: name))
    }
  }

  /// What waits in the extension's outbox goes into the media outbox, oldest first; each leaves the group once the
  /// outbox has it. With no core yet (or one that will not take it now) it stays for the next opening.
  static func take(outbox: MediaOutbox, accounts: AccountStore) {
    guard !taking, accounts.client != nil, let dir = ShareBridge.outbox else { return }
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
        guard generation == self.generation, accounts.client != nil, let owner = accounts.shareOwner else { return }
        if let data, share.owner == owner, Date.now.timeIntervalSince(share.created) < MediaOutbox.givenUpAfter {
          let file = MediaFile(
            kind: share.kind, data: data, width: share.width, height: share.height, durationMs: share.durationMs)
          guard await outbox.send(file, to: share.to, accounts: accounts) else { continue }
          Stats.sentYo(to: share.name)
          Task.detached(priority: .utility) { file.keepIfWanted() }
        }
        try? FileManager.default.removeItem(at: note)
        try? FileManager.default.removeItem(at: media)
      }
    }
  }

  /// The shares whose note is written, oldest first. Media an hour old without a note (the extension ended between
  /// the two) and notes that do not read go.
  nonisolated private static func waiting(in dir: URL) -> [ShareBridge.Share] {
    let manager = FileManager.default
    let files = (try? manager.contentsOfDirectory(at: dir, includingPropertiesForKeys: [.creationDateKey])) ?? []
    var shares: [ShareBridge.Share] = []
    for file in files where file.pathExtension == "json" {
      if let data = try? Data(contentsOf: file), let share = try? JSONDecoder().decode(ShareBridge.Share.self, from: data) {
        shares.append(share)
      } else {
        try? manager.removeItem(at: file)
      }
    }
    let noted = Set(shares.map(\.id.uuidString))
    for file in files where file.pathExtension != "json" && !noted.contains(file.deletingPathExtension().lastPathComponent) {
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
  var shared: ShareBridge.Friend { ShareBridge.Friend(name: name, color: color, to: user ?? name) }
}
