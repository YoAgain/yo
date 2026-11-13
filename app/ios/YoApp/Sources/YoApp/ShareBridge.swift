// What the app and its share extension (YoShare) hand each other, in their App Group (`group`): the
// friends on the list for the extension to show, and the photos and videos picked there for the app to send. The
// extension runs no core and no network: two processes must never write the account's state. So a share waits in the
// group's outbox, made as it goes (MediaFile), and the app sends it as soon as it opens. One writer per file: the app
// writes the friends, the extension adds shares (the media first, then their note, which makes them whole), the app
// takes them out. Compiled into both targets (Sources/YoShare links to this file).
import Foundation

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

  /// The App Group of Yo from the App Store and TestFlight and its extension. Yo Dev (dev/push.sh) has no extension
  /// and no group: its container is nil and nothing is shared.
  static let group = "group.app.justyo.ios"

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
}
