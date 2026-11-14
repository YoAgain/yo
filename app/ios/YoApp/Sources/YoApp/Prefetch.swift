// Photos and videos fetched by the notification service as their push comes, for the app to take from the App Group
// when it opens: the Yo's photo is there at once, not a fetch later. Compiled into the app and YoNotify (Sources/YoNotify
// links to this file).
//
// A blob counts its fetches at the hub (one per device of the chat), so the two never both fetch it: the service
// marks a fetch under way (`.fetching`) before it starts, and the app, whose own fetch the hub refuses meanwhile
// ("no longer there"), takes the service's file, or waits for it while the mark is fresh, instead of giving the photo
// up. The file is written as the device may be locked: complete unless open (made while locked, read only unlocked).
// The app takes it into its own media (complete protection) and deletes it here; a sign-out deletes them all.
import Foundation
import YoCore

enum Prefetch {
  /// What the service fetches: the 24 MB it has hold the bytes and their opened copy.
  static let largest: UInt32 = 6 << 20
  /// A fetch marked longer ago is over (the service was ended): the app fetches itself.
  static let underWaySeconds: TimeInterval = 30

  private static var directory: URL? { ShareBridge.container?.appending(path: "Prefetched") }

  private static func name(_ media: YoMedia) -> String { media.id.map { String(format: "%02x", $0) }.joined() }
  private static func file(_ media: YoMedia) -> URL? { directory?.appending(path: name(media) + ".media") }
  private static func mark(_ media: YoMedia) -> URL? { directory?.appending(path: name(media) + ".fetching") }

  // MARK: The notification service

  /// Fetches the photos and videos of `events` one after the other (small enough ones only), each to its file.
  /// Best effort: what is not here when the app opens, the app fetches itself.
  static func fetch(_ events: [YoEvent], client: YoClient, startingBefore deadline: Date = .distantFuture) {
    guard let directory else { return }
    let media: [YoMedia] = events.compactMap { event in
      guard case .message(_, _, let kind) = event else { return nil }
      return kind.media?.media
    }
    guard !media.isEmpty, let owner = Hub.bytes(client.userId())?.base64EncodedString() else { return }
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    for one in media where one.size <= largest {
      // Room to write it, with plenty to spare: a fetch whose file then cannot be written has used this device's
      // fetch at the hub, and the photo would be lost. Without, the app fetches it itself (and keeps it in memory
      // until it can write).
      guard Date.now < deadline, room(for: one.size, in: directory) else { return }
      guard let file = file(one), let mark = mark(one), !FileManager.default.fileExists(atPath: file.path) else {
        continue
      }
      // The claim first: the app fetching it now, this one does not (and the app does not give the photo up).
      guard claim(one) else { continue }
      defer { try? FileManager.default.removeItem(at: mark) }
      guard let data = try? client.fetchMedia(media: one) else { continue }
      // Still this account's (the app publishes its friends with its own id; signed out meanwhile, it does not, or
      // another account's): an old account's photo is not left here.
      guard ShareBridge.readList()?.owner == owner else { return }
      try? data.write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])
    }
  }

  /// Whether the disk has room for `size` bytes and 100 MB more.
  private static func room(for size: UInt32, in directory: URL) -> Bool {
    let free = (try? directory.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]))?
      .volumeAvailableCapacityForImportantUsage
    return (free ?? 0) > Int64(size) + 100 << 20
  }

  // MARK: The app

  /// The photo or video as the service fetched it; nil when it did not (or it does not read yet).
  static func take(_ media: YoMedia) -> Data? {
    guard let file = file(media) else { return nil }
    return try? Data(contentsOf: file)
  }

  /// Whether the service's file is there, read or not (locked: it opens only unlocked).
  static func has(_ media: YoMedia) -> Bool {
    guard let file = file(media) else { return false }
    return FileManager.default.fileExists(atPath: file.path)
  }

  /// Claims this device's fetch of `media`, for the service or the app, atomically (a file only one can make): false
  /// while the other has it. A claim older than `underWaySeconds` was left by a process that ended: taken over. No
  /// App Group (Yo Dev): nothing to share it with, true.
  static func claim(_ media: YoMedia) -> Bool {
    guard let directory, let mark = mark(media) else { return true }
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    if FileManager.default.fileExists(atPath: mark.path) && !underWay(media) {
      try? FileManager.default.removeItem(at: mark)
    }
    let fd = open(mark.path, O_CREAT | O_EXCL | O_WRONLY | O_CLOEXEC, 0o600)
    guard fd >= 0 else { return false }
    close(fd)
    return true
  }

  /// The claim given back (fetched, or not).
  static func unclaim(_ media: YoMedia) {
    guard let mark = mark(media) else { return }
    try? FileManager.default.removeItem(at: mark)
  }

  /// Whether the service is fetching it now: the app waits for its file rather than giving the photo up.
  static func underWay(_ media: YoMedia) -> Bool {
    guard let mark = mark(media),
      let at = try? FileManager.default.attributesOfItem(atPath: mark.path)[.creationDate] as? Date
    else { return false }
    return Date.now.timeIntervalSince(at) < underWaySeconds
  }

  /// Kept by the app (or gone for good): its file goes.
  static func done(_ media: YoMedia) {
    guard let file = file(media) else { return }
    try? FileManager.default.removeItem(at: file)
    if let mark = mark(media) { try? FileManager.default.removeItem(at: mark) }  // left by a service that was ended
  }

  /// Signed out: none of them stays.
  static func clear() {
    guard let directory else { return }
    try? FileManager.default.removeItem(at: directory)
  }

  /// What was never taken (its Yo seen elsewhere, or not this account's): gone after the hub's 30 days.
  static func sweep() {
    guard let directory,
      let files = try? FileManager.default.contentsOfDirectory(
        at: directory, includingPropertiesForKeys: [.creationDateKey])
    else { return }
    let old = Date.now.addingTimeInterval(-31 * 24 * 3600)
    for url in files {
      let made = (try? url.resourceValues(forKeys: [.creationDateKey]))?.creationDate ?? .distantPast
      if made < old { try? FileManager.default.removeItem(at: url) }
    }
  }
}
