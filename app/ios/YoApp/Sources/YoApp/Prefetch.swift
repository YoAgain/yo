// Photos and videos fetched by the notification service as their push comes, for the app to take from the App Group
// when it opens: the Yo's photo is there at once, not a fetch later. Compiled into the app and YoNotify (Sources/YoNotify
// links to this file).
//
// App and service claim each blob before fetching, then acknowledge only after storing it. The app takes the
// service's file, or waits while its claim is fresh. A shared journal retries failed acknowledgements.
// The file is written as the device may be locked: complete unless open (made while locked, read only unlocked).
// The app takes it into its own media (complete protection) and deletes it here; a sign-out deletes them all.
import Foundation
import YoCore

enum Prefetch {
  /// What the service fetches: the 24 MB it has hold the bytes and their opened copy.
  static let largest: UInt32 = 6 << 20
  /// A fetch marked longer ago is over (its process was ended): the other fetches itself. Longer than a request may
  /// take (`Hub.timeout`, 70 s), so a slow fetch of the app is not taken over while it runs.
  static let underWaySeconds: TimeInterval = 90

  private static var directory: URL? { ShareBridge.container?.appending(path: "Prefetched") }

  /// The folder, never in the backup: what is in it is opened, a photo meant to be seen once (as MediaStorage's).
  private static func make(_ directory: URL) {
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    var directory = directory
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? directory.setResourceValues(values)
  }

  private static func name(_ media: YoMedia) -> String { media.id.map { String(format: "%02x", $0) }.joined() }
  private static func file(_ media: YoMedia) -> URL? { directory?.appending(path: name(media) + ".media") }
  private static func mark(_ media: YoMedia) -> URL? { directory?.appending(path: name(media) + ".fetching") }
  /// Left once the app has a photo or video for good: no process of this device fetches it again.
  /// Swept with the rest after 31 days, past the blob's life.
  private static func taken(_ media: YoMedia) -> URL? { directory?.appending(path: name(media) + ".taken") }

  private struct Ack: Codable {
    let id: Data
    let file: String
  }

  private static var acknowledgements: URL {
    let base = ShareBridge.container ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    return base.appending(path: "BlobAcks")
  }

  /// Persist the intent before writing the atomic media file. Only an existing file may be acknowledged.
  @discardableResult
  static func prepareAck(_ media: YoMedia, file: URL) -> Bool {
    let directory = acknowledgements
    make(directory)
    return ackLocked(directory) {
      let path = directory.appending(path: name(media) + ".json")
      let done = directory.appending(path: name(media) + ".acked")
      if FileManager.default.fileExists(atPath: done.path) { return true }
      guard let data = try? JSONEncoder().encode(Ack(id: media.id, file: file.path)) else { return false }
      return (try? data.write(to: path, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil
    } ?? false
  }

  static func stored(_ media: YoMedia) {
    let directory = acknowledgements
    _ = try? Data().write(to: directory.appending(path: name(media) + ".stored"),
      options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
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
        guard (try? Data().write(to: url.deletingPathExtension().appendingPathExtension("stored"),
          options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil else { return }
        guard (try? client.ackMedia(id: ack.id)) != nil else { return }
        // Mark before removing the intent: the other process sees the successful acknowledgement.
        guard (try? Data().write(to: done, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil else { return }
        try? FileManager.default.removeItem(at: url)
      }
    }
    return files.contains { $0.pathExtension == "json" && FileManager.default.fileExists(atPath: $0.path) }
  }

  /// Short journal writes and per-blob ack requests use locks independent of account-state turns.
  @discardableResult
  private static func ackLocked<T>(_ directory: URL, name: String = ".acks.lock", _ work: () -> T) -> T? {
    let fd = open(directory.appending(path: name).path, O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
    guard fd >= 0 else { return nil }
    defer { close(fd) }
    guard flock(fd, LOCK_EX | LOCK_NB) == 0 else { return nil }
    defer { flock(fd, LOCK_UN) }
    return work()
  }

  // MARK: The notification service

  /// Fetches the photos and videos of `events` one after the other (small enough ones only), each to its file.
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
      // Room to store it, with plenty to spare; otherwise leave the GET budget for the app.
      guard Date.now < deadline, room(for: one.size, in: directory) else { return }
      guard let file = file(one), !FileManager.default.fileExists(atPath: file.path) else {
        continue
      }
      // The claim first: the app fetching it now, this one does not (and the app does not give the photo up).
      guard let token = claim(one) else { continue }
      defer { unclaim(one, token: token) }
      guard prepareAck(one, file: file) else { continue }
      guard let data = try? client.fetchMedia(media: one) else { continue }
      // Still this account's (the app publishes its friends with its own id; signed out meanwhile, it does not, or
      // another account's): an old account's photo is not left here.
      guard ShareBridge.readList()?.owner == owner else { return }
      // Write atomically before acknowledging. A failed write leaves the hub copy available.
      if (try? data.write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])) == nil {
        try? data.write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])
      }
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
  /// The claim's token, which only its holder can give back (`unclaim`): nil when it is another's.
  static func claim(_ media: YoMedia, again: Bool = false) -> String? {
    let token = UUID().uuidString
    guard let directory, let mark = mark(media) else { return token }
    make(directory)
    // Under one lock for both processes: two finding the same stale claim cannot both take it over.
    return locked(directory) { () -> String? in
      // The app has it already: the notification service fetches it no more. The app may (`again`): the same blob in
      // another conversation is another message, with fetches of its own at the hub.
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

  /// The claim given back (fetched, or not), if it is still this holder's: one taken over after `underWaySeconds`
  /// (this process suspended that long) is the other's now, and stays.
  static func unclaim(_ media: YoMedia, token: String) {
    guard let directory, let mark = mark(media) else { return }
    locked(directory) {
      guard (try? String(contentsOf: mark, encoding: .utf8)) == token else { return }
      try? FileManager.default.removeItem(at: mark)
    }
  }

  /// `work` with the claims' lock held (flock on a file beside them, across the app and its extensions).
  @discardableResult
  private static func locked<T>(_ directory: URL, _ work: () -> T) -> T? {
    // Media fetches claim from the UI actor too: contention retries later, never waits on the main thread.
    guard let access = SharedAccess.take(
      directory.appendingPathComponent(".claims.lock"), within: Thread.isMainThread ? 0 : .infinity)
    else { return nil }
    defer { access.finish() }
    return try? access.perform(work)
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
    guard let directory, let file = file(media) else { return }
    make(directory)
    if let taken = taken(media) { FileManager.default.createFile(atPath: taken.path, contents: nil) }
    try? FileManager.default.removeItem(at: file)
    if let mark = mark(media) { try? FileManager.default.removeItem(at: mark) }  // left by a service that was ended
  }

  /// Signed out: none of them stays.
  static func clear() {
    try? FileManager.default.removeItem(at: acknowledgements)
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
