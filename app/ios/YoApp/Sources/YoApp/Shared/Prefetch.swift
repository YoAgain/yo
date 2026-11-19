// Media the notification service fetches as the push comes, for the app to take from the App Group (app + YoNotify).
// Both processes claim a blob before fetching and acknowledge only after storing it; a shared journal retries failed
// acks. Files are "complete unless open": written while locked, read only unlocked.
import Foundation
import YoCore

enum Prefetch {
  /// The service's 24 MB must hold the bytes and their opened copy.
  static let largest: UInt32 = 6 << 20
  /// Longer than a request may take (`Hub.timeout`, 70 s): an older claim's process was ended.
  static let underWaySeconds: TimeInterval = 90

  private static var directory: URL? { ShareBridge.container?.appending(path: "Prefetched") }

  /// Never in the backup: a photo meant to be seen once.
  private static func make(_ directory: URL) {
    Diagnostics.attempt({ try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true) })
    var directory = directory
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    Diagnostics.attempt({ try directory.setResourceValues(values) })
  }

  private static func name(_ media: YoMedia) -> String { media.id.map { String(format: "%02x", $0) }.joined() }
  private static func file(_ media: YoMedia) -> URL? { directory?.appending(path: name(media) + ".media") }
  private static func mark(_ media: YoMedia) -> URL? { directory?.appending(path: name(media) + ".fetching") }
  /// The app has it for good: never fetched again here. Swept after 31 days, past the blob's life.
  private static func taken(_ media: YoMedia) -> URL? { directory?.appending(path: name(media) + ".taken") }

  private struct Ack: Codable {
    let id: Data
    let file: String
  }

  private static var acknowledgements: URL {
    let base = ShareBridge.container ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    return base.appending(path: "BlobAcks")
  }

  /// The intent, persisted before the media file is written: only an existing file may be acknowledged.
  @discardableResult
  static func prepareAck(_ media: YoMedia, file: URL) -> Bool {
    let directory = acknowledgements
    make(directory)
    return ackLocked(directory) {
      let path = directory.appending(path: name(media) + ".json")
      let done = directory.appending(path: name(media) + ".acked")
      if FileManager.default.fileExists(atPath: done.path) { return true }
      guard let data = try? JSONEncoder().encode(Ack(id: media.id, file: file.path)) else { return false }
      return put(data, at: path)
    } ?? false
  }

  static func stored(_ media: YoMedia) {
    put(Data(), at: acknowledgements.appending(path: name(media) + ".stored"))
  }

  @discardableResult
  private static func put(_ data: Data, at url: URL) -> Bool {
    Diagnostics.attempt({ try data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]) }) != nil
  }

  /// App and service serialize retries. A lost HTTP answer may consume another fetch (anonymous by design).
  @discardableResult
  static func retryAcks(client: YoClient, startingBefore deadline: Date = .distantFuture) -> Bool {
    let directory = acknowledgements
    make(directory)
    let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
    let old = Date.now.addingTimeInterval(-31 * 24 * 3600)
    for url in files where url.lastPathComponent != ".acks.lock" {
      if let date = try? url.resourceValues(forKeys: [.creationDateKey]).creationDate, date < old {
        try? FileManager.default.removeItem(at: url)
      }
    }
    for url in files where url.pathExtension == "json" {
      guard Date.now < deadline else { break }
      // Only this blob is held across the request. A failed ack must not delay another photo's storage.
      ackLocked(directory, name: url.deletingPathExtension().lastPathComponent + ".acklock") {
        let done = url.deletingPathExtension().appendingPathExtension("acked")
        if FileManager.default.fileExists(atPath: done.path) {
          try? FileManager.default.removeItem(at: url)
          return
        }
        guard let data = try? Data(contentsOf: url), let ack = try? JSONDecoder().decode(Ack.self, from: data),
          (FileManager.default.fileExists(atPath: ack.file)
            || FileManager.default.fileExists(atPath: url.deletingPathExtension().appendingPathExtension("stored").path)) else { return }
        guard put(Data(), at: url.deletingPathExtension().appendingPathExtension("stored")) else { return }
        guard (Diagnostics.attempt({ try client.ackMedia(id: ack.id) })) != nil else { return }
        // Mark before removing the intent: the other process sees the successful acknowledgement.
        guard put(Data(), at: done) else { return }
        try? FileManager.default.removeItem(at: url)
      }
    }
    return files.contains { $0.pathExtension == "json" && FileManager.default.fileExists(atPath: $0.path) }
  }

  /// Short journal writes and per-blob ack requests use locks independent of account-state turns. Revocable
  /// (SharedAccess): an ack request may outlast the process's time, and expiry gives the lock back rather than keeping
  /// it into suspension (0xdead10cc); at worst both processes then ack the same blob.
  @discardableResult
  private static func ackLocked<T>(_ directory: URL, name: String = ".acks.lock", _ work: () -> T) -> T? {
    guard let access = SharedAccess.take(directory.appending(path: name), within: 0) else { return nil }
    defer { access.finish() }
    return work()
  }

  // MARK: The notification service

  /// Best effort: what is not here when the app opens, the app fetches itself.
  static func fetch(_ events: [YoEvent], client: YoClient, startingBefore deadline: Date = .distantFuture) {
    retryAcks(client: client, startingBefore: deadline)
    guard let directory else { return }
    let media: [YoMedia] = events.compactMap { event in
      switch event {
      case .message(_, _, let kind), .groupMessage(_, _, _, let kind): return kind.media?.media
      default: return nil
      }
    }
    guard !media.isEmpty, let owner = Hub.bytes(client.userId())?.base64EncodedString() else { return }
    make(directory)
    for one in media where one.size <= largest {
      // Without room, leave the GET budget for the app.
      guard Date.now < deadline, room(for: one.size, in: directory) else { return }
      guard let file = file(one), !FileManager.default.fileExists(atPath: file.path) else {
        continue
      }
      guard let token = claim(one) else { continue }
      defer { unclaim(one, token: token) }
      guard prepareAck(one, file: file) else { continue }
      guard let data = Diagnostics.attempt({ try client.fetchMedia(media: one) }).flatMap({ $0 }) else { continue }
      // Signed out or switched meanwhile: an old account's photo is not left here.
      guard ShareBridge.readList()?.owner == owner else { return }
      // Written before acknowledging: a failed write leaves the hub's copy.
      Diagnostics.attempt({ try data.write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen]) })
      if FileManager.default.fileExists(atPath: file.path) { stored(one) }
      retryAcks(client: client, startingBefore: deadline)
    }
  }

  /// Whether the disk has room for `size` bytes and 100 MB more.
  private static func room(for size: UInt32, in directory: URL) -> Bool {
    let free = (try? directory.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]))?
      .volumeAvailableCapacityForImportantUsage
    return (free ?? 0) > Int64(size) + 100 << 20
  }

  // MARK: The app

  /// Nil when the service did not fetch it, or it does not read yet (locked).
  static func take(_ media: YoMedia) -> Data? {
    guard let file = file(media) else { return nil }
    return try? Data(contentsOf: file)
  }

  /// The service's file is there, readable or not.
  static func has(_ media: YoMedia) -> Bool {
    guard let file = file(media) else { return false }
    return FileManager.default.fileExists(atPath: file.path)
  }

  /// A token only its holder can give back (`unclaim`); nil while the other process has it. A claim older than
  /// `underWaySeconds` is taken over. No App Group (Yo Dev): always a token.
  static func claim(_ media: YoMedia, again: Bool = false) -> String? {
    let token = UUID().uuidString
    guard let directory, let mark = mark(media) else { return token }
    make(directory)
    // Under one lock for both processes: two finding the same stale claim cannot both take it over.
    return locked(directory) { () -> String? in
      // `again`: the same blob in another conversation is another message, with fetches of its own at the hub.
      if !again, let taken = taken(media), FileManager.default.fileExists(atPath: taken.path) { return nil }
      if FileManager.default.fileExists(atPath: mark.path) && !underWay(media) {
        try? FileManager.default.removeItem(at: mark)
      }
      let fd = open(mark.path, O_CREAT | O_EXCL | O_WRONLY | O_CLOEXEC, 0o600)
      guard fd >= 0 else { return nil }
      _ = Array(token.utf8).withUnsafeBytes { write(fd, $0.baseAddress, $0.count) }
      close(fd)
      return token
    } ?? nil
  }

  /// Only if still this holder's: one taken over after `underWaySeconds` stays the other's.
  static func unclaim(_ media: YoMedia, token: String) {
    guard let directory, let mark = mark(media) else { return }
    locked(directory) {
      guard (try? String(contentsOf: mark, encoding: .utf8)) == token else { return }
      try? FileManager.default.removeItem(at: mark)
    }
  }

  @discardableResult
  private static func locked<T>(_ directory: URL, _ work: () -> T) -> T? {
    // Media fetches claim from the UI actor too: contention retries later, never waits on the main thread.
    guard let access = SharedAccess.take(
      directory.appendingPathComponent(".claims.lock"), within: Thread.isMainThread ? 0 : .infinity)
    else { return nil }
    defer { access.finish() }
    return try? access.perform(work)
  }

  static func underWay(_ media: YoMedia) -> Bool {
    guard let mark = mark(media),
      let at = try? FileManager.default.attributesOfItem(atPath: mark.path)[.creationDate] as? Date
    else { return false }
    return Date.now.timeIntervalSince(at) < underWaySeconds
  }

  static func done(_ media: YoMedia) {
    guard let directory, let file = file(media) else { return }
    make(directory)
    if let taken = taken(media) { FileManager.default.createFile(atPath: taken.path, contents: nil) }
    try? FileManager.default.removeItem(at: file)
    if let mark = mark(media) { try? FileManager.default.removeItem(at: mark) }  // left by a service that was ended
  }

  static func clear() {
    try? FileManager.default.removeItem(at: acknowledgements)
    guard let directory else { return }
    try? FileManager.default.removeItem(at: directory)
  }

  /// Never taken (seen elsewhere, or another account's): gone after the hub's 30 days.
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
