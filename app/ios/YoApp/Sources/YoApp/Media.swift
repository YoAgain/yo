// Photos and videos (spec 5, 6.8): made small and plain before they go (a JPEG of at most 1600 pixels, a video of
// at most 10 seconds and 8 MB, neither with a place or a device in it), sealed by the core under a key of their own,
// and kept in an outbox until the hub has them; only then their Yo goes. A hub that says "not now" (the day's quota,
// no room) keeps them waiting here, and they go later by themselves. What arrives is fetched at once, kept on this
// iPhone until it has been seen once, then deleted.
import AVFoundation
import Foundation
import ImageIO
import Observation
import UIKit
import UniformTypeIdentifiers
import YoCore

extension MediaFile {
  /// To the camera roll, when Keep Sent Photos is on.
  func keepIfWanted() {
    switch kind {
    case .photo:
      Library.saveIfWanted(photo: data)
    case .video:
      let file = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mp4")
      guard (try? data.write(to: file)) != nil else { return }
      Library.saveIfWanted(video: file)
      try? FileManager.default.removeItem(at: file)
    }
  }
}

/// Where photos and videos wait on this iPhone: never in its backup (a photo seen once must not live on there), and
/// gone with the account when it signs out.
enum MediaStorage {
  static func directory(_ name: String) -> URL {
    var dir = URL.applicationSupportDirectory.appending(path: name, directoryHint: .isDirectory)
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? dir.setResourceValues(values)
    return dir
  }

  /// Writes off the main actor.
  static func write(_ data: Data, to url: URL, _ protection: Data.WritingOptions) async -> Bool {
    await Task.detached(priority: .utility) { (try? data.write(to: url, options: [.atomic, protection])) != nil }.value
  }

  /// Signing out: nothing of the account's photos and videos stays, sent or received.
  @MainActor static func clearAll() {
    for name in ["Outbox", "Received"] {
      try? FileManager.default.removeItem(at: URL.applicationSupportDirectory.appending(path: name))
    }
    ReceivedMedia.shared.forgetAll()
  }
}

/// What the Yo of a photo or video says, as it waits on disk.
struct StoredMedia: Codable {
  let id: Data
  let key: Data
  let sha256: Data
  let size: UInt32
  let width: UInt16
  let height: UInt16

  init(_ media: YoMedia) {
    (id, key, sha256, size, width, height) = (media.id, media.key, media.sha256, media.size, media.width, media.height)
  }

  var media: YoMedia { YoMedia(id: id, key: key, sha256: sha256, size: size, width: width, height: height) }
}

/// Photos and videos on their way out, kept on disk (sealed) until the hub has them and their Yo is sent: an app
/// that quits, a hub that says "not now", no network, none of it loses one, and none is given up. A few uploads side by
/// side, while Yos go on beside them (an upload does not hold the core); the Yos to each friend in the order taken,
/// each once its upload and those before it to that friend are done (one the hub holds back holds back only that
/// friend's after it). A Yo sent again after a crash (before its item left the queue) goes once: the core knows its
/// blob's id.
@MainActor @Observable
final class MediaOutbox {
  private struct Item: Codable {
    let id: UUID
    /// The account that sent it (user id): only that one sends it.
    let owner: String
    /// The friend's user id, or name before the first Yo.
    let to: String
    let kind: MediaFile.Kind
    let media: StoredMedia
    let durationMs: UInt32
    let created: Date
    var tries = 0
    var next = Date.distantPast
    /// The hub has the blob: only its Yo is left (nil in items saved before this was kept).
    var uploaded: Bool?
  }

  /// How many wait.
  private(set) var waiting = 0
  /// Where what the core received while sending goes (the inbox): Yos that came meanwhile are not lost.
  /// Awaited before the next core call; with the core's contacts, read with them, and the client they came from.
  @ObservationIgnored var received: (@MainActor ([YoEvent], [YoContact], YoClient) async -> Void)?
  /// Counts sign-outs: what began before one ends without a trace.
  @ObservationIgnored private var generation = 0
  @ObservationIgnored private var items: [Item] = []
  @ObservationIgnored private var running = false
  @ObservationIgnored private var wake: Task<Void, Never>?
  @ObservationIgnored private weak var accounts: AccountStore?

  private static var directory: URL { MediaStorage.directory("Outbox") }
  private static func blob(_ id: UUID) -> URL { directory.appending(path: "\(id.uuidString).blob") }
  private static var index: URL { directory.appending(path: "outbox.json") }

  init() {
    if let data = try? Data(contentsOf: Self.index), let saved = try? JSONDecoder().decode([Item].self, from: data) {
      items = saved
    }
    waiting = items.count
  }

  /// The queue to disk; false when it did not write (it stays in memory, written with the next change).
  @discardableResult
  private func save() -> Bool {
    waiting = items.count
    guard let data = try? JSONEncoder().encode(items) else { return false }
    return (try? data.write(to: Self.index, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]))
      != nil
  }

  /// Seals a photo or video for a friend and puts it in line, from `client`, the account the gesture began in (never
  /// from one signed in since); it goes as soon as the hub takes it. True once it is on disk, its blob and the queue.
  func send(_ file: MediaFile, to friend: String, from client: YoClient?, accounts: AccountStore) async -> Bool {
    guard let client, client === accounts.client else { return false }
    let width = UInt16(clamping: file.width)
    let height = UInt16(clamping: file.height)
    guard
      let (sealed, owner) = try? await Hub.run({
        (try sealMedia(plain: file.data, width: width, height: height), client.userId())
      })
    else { return false }
    let item = Item(
      id: UUID(), owner: owner, to: friend, kind: file.kind, media: StoredMedia(sealed.media),
      durationMs: file.durationMs, created: .now)
    let blob = Self.blob(item.id)
    guard await MediaStorage.write(sealed.ciphertext, to: blob, .completeFileProtectionUntilFirstUserAuthentication)
    else { return false }
    guard client === accounts.client else {  // signed out meanwhile
      try? FileManager.default.removeItem(at: blob)
      return false
    }
    items.append(item)
    guard save() else {
      items.removeAll { $0.id == item.id }
      waiting = items.count
      try? FileManager.default.removeItem(at: blob)
      return false
    }
    flush(accounts: accounts)
    return true
  }

  /// Signed out: nothing waits any more, nothing under way goes on (the files went with MediaStorage.clearAll).
  func clear() {
    generation += 1
    items = []
    waiting = 0
    wake?.cancel()
  }

  /// Sends what is due: up to `together` uploads side by side (the hub takes a few of one user at once), then the Yos
  /// each friend's queue lets go (`releasable`). A burst of photos then goes about as fast as it was taken, not one
  /// upload after the other. What the hub refuses for now waits its turn again (30 seconds, doubling to half an hour).
  /// Also when the app comes to the foreground. A build the hub calls too old (426) sends nothing more.
  func flush(accounts: AccountStore) {
    self.accounts = accounts
    guard !running else { return }
    running = true
    let generation = generation
    Task {
      defer {
        running = false
        // Signed out and in meanwhile: this round was the old account's; the new one's photos go now, not at the
        // next opening.
        if generation != self.generation && !items.isEmpty { flush(accounts: accounts) }
      }
      while let client = accounts.client, !HubTransport.unsupported {
        let uploads = Array(items.filter { $0.uploaded != true && $0.next <= .now }.prefix(Self.together))
        guard !uploads.isEmpty || !releasable().isEmpty else { break }
        let tasks = uploads.map { item in Task { await self.upload(item, client: client, generation: generation) } }
        for (item, task) in zip(uploads, tasks) {
          let outcome = await task.value
          guard generation == self.generation, settle(item, outcome, client: client, accounts: accounts) else {
            return
          }
        }
        var held: Set<String> = []  // a friend whose Yo the hub did not take: their next ones wait behind it
        for item in releasable() where !held.contains(item.to) {
          let outcome = await send(item, client: client, generation: generation)
          guard generation == self.generation, settle(item, outcome, client: client, accounts: accounts) else {
            return
          }
          if outcome != .sent && outcome != .dropped { held.insert(item.to) }
        }
      }
      scheduleWake()
    }
  }

  /// Uploads under way at once, at most (the hub takes 4 of one user).
  private static let together = 3

  /// The Yos that may go now: per friend in the order taken, those whose uploads are done, up to the first that is
  /// not (or that waits its turn).
  private func releasable() -> [Item] {
    var blocked: Set<String> = []
    return items.filter { item in
      guard !blocked.contains(item.to) else { return false }
      guard item.uploaded == true, item.next <= .now else {
        blocked.insert(item.to)
        return false
      }
      return true
    }
  }

  /// What an outcome leaves of an item: gone (sent, or refused for good), uploaded, or waiting its turn again. False:
  /// stop (the hub says this phone was replaced, or this build is too old).
  private func settle(_ item: Item, _ outcome: Outcome, client: YoClient, accounts: AccountStore) -> Bool {
    guard let at = items.firstIndex(where: { $0.id == item.id }) else { return true }
    switch outcome {
    case .sent, .dropped:
      items.remove(at: at)
      let blob = Self.blob(item.id)
      Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: blob) }
    case .uploaded:
      items[at].uploaded = true
      items[at].tries = 0
      items[at].next = .distantPast
    case .later:
      items[at].tries += 1
      items[at].next = .now + min(1800, 30 * pow(2, Double(min(items[at].tries - 1, 6))))
    case .replaced:
      accounts.replaced(client)  // clears this outbox too
      return false
    case .stopped:
      return false
    }
    save()
    return true
  }

  /// Wakes for the next item whose turn comes (one held behind another goes with that one).
  private func scheduleWake() {
    wake?.cancel()
    guard let next = items.map(\.next).filter({ $0 > .now }).min() else { return }
    wake = Task { [weak self] in
      try? await Task.sleep(for: .seconds(max(1, next.timeIntervalSinceNow)))
      guard !Task.isCancelled, let self, let accounts = self.accounts else { return }
      self.flush(accounts: accounts)
    }
  }

  private enum Outcome { case uploaded, sent, later, dropped, replaced, stopped }

  /// The chat first (a new friend's opens now), then the upload outside the client's lock: Yos go on meanwhile, and
  /// so do the other uploads.
  private func upload(_ item: Item, client: YoClient, generation: Int) async -> Outcome {
    let media = item.media.media
    let blob = Self.blob(item.id)
    return await outcome {
      // Another account's (signed out meanwhile): never sent under this one.
      guard try await Hub.run({ client.userId() }) == item.owner else { return .dropped }
      let (chat, contacts) = try await Hub.run { (try client.mediaChat(to: item.to), client.contacts()) }
      await received?(chat.events, contacts, client)
      guard generation == self.generation else { return .dropped }
      try await Hub.run {
        try client.uploadMedia(media: media, ciphertext: try Data(contentsOf: blob), fetches: chat.fetches)
      }
      return .uploaded
    }
  }

  /// The Yo, only after the hub has the blob.
  private func send(_ item: Item, client: YoClient, generation: Int) async -> Outcome {
    guard generation == self.generation else { return .dropped }
    let media = item.media.media
    return await outcome {
      guard try await Hub.run({ client.userId() }) == item.owner else { return .dropped }
      let (events, after) = try await Hub.run {
        switch item.kind {
        case .photo: (try client.sendPhoto(to: item.to, media: media), client.contacts())
        case .video: (try client.sendVideo(to: item.to, media: media, durationMs: item.durationMs), client.contacts())
        }
      }
      await received?(events, after, client)
      return .sent
    }
  }

  private func outcome(_ work: () async throws -> Outcome) async -> Outcome {
    do {
      return try await work()
    } catch YoError.Replaced {
      return .replaced
    } catch YoError.Refused(let status) where status == 426 {
      return .stopped  // this build too old: kept, for the build after it
    } catch let error as YoError where mediaRetryLater(error: error) {
      return .later
    } catch {
      return .dropped  // not on Yo, blocked, or refused for good
    }
  }
}

/// Photos and videos that came, kept until seen once. Each is noted (with its key) before the core may forget its
/// Yo, then fetched, a few at a time (each fetch counts at the hub, the last deletes the blob), opened and kept here
/// with complete file protection; seen, it is deleted. A fetch the network breaks is tried again soon, and at once
/// when the next Yo comes; one the hub no longer has is dropped. A download whose file does not write stays in memory
/// and is written again, never fetched again. All of it on the main actor but the reading, writing and fetching.
@MainActor
final class ReceivedMedia {
  static let shared = ReceivedMedia()

  private struct Pending: Codable {
    let id: String
    let from: String
    let kind: MediaFile.Kind
    let media: StoredMedia
  }

  private struct Saved: Codable {
    let id: String
    let from: String
    let kind: MediaFile.Kind
  }

  private var pending: [Pending] = []
  private var saved: [Saved] = []
  private var fetching: Set<String> = []
  /// Fetches the network broke, each waiting for its next try: cancelled, it tries now.
  private var pauses: [String: Task<Void, Never>] = [:]
  /// Downloads not written yet (the disk full): the hub may have deleted the blob with that fetch.
  private var unwritten: [String: Data] = [:]
  /// What fetches go by, as `fetchWaiting` was last given it: more start as others end.
  private var client: YoClient?
  private var arrived: (@MainActor (Arrival) -> Void)?
  /// Counts sign-outs: a fetch begun before one keeps nothing.
  private var generation = 0

  /// Fetches at once, at most: a backlog never holds every thread and all the memory.
  private static let together = 3
  /// The longest side of a bar's thumbnail, in pixels.
  nonisolated private static let thumbnailPixels = 300

  private var directory: URL { MediaStorage.directory("Received") }
  private var pendingIndex: URL { directory.appending(path: "pending.json") }
  private var savedIndex: URL { directory.appending(path: "received.json") }

  private init() {
    pending = (try? Data(contentsOf: pendingIndex)).flatMap { try? JSONDecoder().decode([Pending].self, from: $0) } ?? []
    saved = (try? Data(contentsOf: savedIndex)).flatMap { try? JSONDecoder().decode([Saved].self, from: $0) } ?? []
    // Seen but not deleted (the app ended while it showed), or written and never noted: gone now.
    // A few files, once, before any fetch can write one.
    let keep = Set(saved.map { file($0.id, $0.kind).lastPathComponent } + ["pending.json", "received.json"])
    for url in (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
    where !keep.contains(url.lastPathComponent) {
      try? FileManager.default.removeItem(at: url)
    }
  }

  private func file(_ id: String, _ kind: MediaFile.Kind) -> URL {
    let safe = String(id.map { $0.isLetter || $0.isNumber ? $0 : "-" })
    return directory.appending(path: safe + (kind == .photo ? ".jpg" : ".mp4"))
  }

  /// Both indexes to disk; false when either did not write.
  private func store() -> Bool {
    guard let waiting = try? JSONEncoder().encode(pending), let kept = try? JSONEncoder().encode(saved) else {
      return false
    }
    let first = (try? waiting.write(to: pendingIndex, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil
    let second = (try? kept.write(to: savedIndex, options: [.atomic, .completeFileProtection])) != nil
    return first && second
  }

  /// A photo or video came: noted before the core may forget its Yo. False when the note did not write: the core then
  /// keeps the Yo (`Inbox.take`).
  func expect(_ media: YoMedia, id: String, from: String, kind: MediaFile.Kind) -> Bool {
    guard !pending.contains(where: { $0.id == id }), !saved.contains(where: { $0.id == id }) else { return true }
    pending.append(Pending(id: id, from: from, kind: kind, media: StoredMedia(media)))
    guard store() else {
      pending.removeAll { $0.id == id }
      return false
    }
    return true
  }

  /// Fetches what is noted and not under way, a few at a time, oldest first; one waiting to try again (the network
  /// broke it) tries now: the next Yo came, so the network is back. Each one kept goes to `arrived`.
  func fetchWaiting(client: YoClient, arrived: @escaping @MainActor (Arrival) -> Void) {
    self.client = client
    self.arrived = arrived
    for one in pending where fetching.contains(one.id) { pauses[one.id]?.cancel() }
    startMore()
  }

  private func startMore() {
    guard let client, let arrived, !HubTransport.unsupported else { return }
    for one in pending where !fetching.contains(one.id) && fetching.count < Self.together {
      fetching.insert(one.id)
      Task { await fetch(one, client: client, arrived: arrived) }
    }
  }

  /// Tries again after 1, 3, 10 and 30 seconds (or sooner, see `fetchWaiting`); then with the next Yo or opening.
  private static let retries = [0, 1, 3, 10, 30]

  private func fetch(_ one: Pending, client: YoClient, arrived: @MainActor (Arrival) -> Void) async {
    defer {
      fetching.remove(one.id)
      startMore()
    }
    let generation = generation
    let media = one.media.media
    for wait in Self.retries {
      if wait > 0 {
        let pause = Task<Void, Never> { _ = try? await Task.sleep(for: .seconds(wait)) }
        pauses[one.id] = pause
        await pause.value
        pauses[one.id] = nil
        guard generation == self.generation else { return }
      }
      var data = unwritten[one.id]
      if data == nil {
        do {
          data = try await Hub.run { try client.fetchMedia(media: media) }
        } catch YoError.Refused(let status) where status == 426 {
          return  // this build too old: noted still, for the build after it
        } catch let error as YoError where mediaRetryLater(error: error) {
          continue  // noted still: tried again now, or with the next Yo or opening
        } catch {
          data = nil  // does not open: nothing to keep
        }
        guard generation == self.generation else { return }  // signed out meanwhile: nothing kept
      }
      guard let data else {
        // The hub no longer has it, or it does not open: its Yo came without it.
        pending.removeAll { $0.id == one.id }
        _ = store()
        return
      }
      // The file, then the indexes; the note goes only once both are written.
      let url = file(one.id, one.kind)
      guard await MediaStorage.write(data, to: url, .completeFileProtection) else {
        unwritten[one.id] = data
        continue
      }
      guard generation == self.generation else {
        try? FileManager.default.removeItem(at: url)
        return
      }
      let kept = Saved(id: one.id, from: one.from, kind: one.kind)
      saved.append(kept)
      pending.removeAll { $0.id == one.id }
      guard store() else {
        saved.removeAll { $0.id == one.id }
        pending.append(one)
        unwritten[one.id] = data
        continue
      }
      unwritten[one.id] = nil
      if let arrival = await arrival(kept) { arrived(arrival) }
      return
    }
  }

  /// What waits to be seen, after a restart, oldest sent first.
  func waiting() async -> [Arrival] {
    var arrivals: [Arrival] = []
    for one in saved {
      if let arrival = await arrival(one) { arrivals.append(arrival) }
    }
    return arrivals.sorted(by: Arrival.older)
  }

  /// Shown: it will not show again, even if the app ends before the viewer closes (its file goes then, or at the
  /// next start).
  func consumed(_ id: String) {
    saved.removeAll { $0.id == id }
    _ = store()
  }

  /// Seen: gone from this iPhone.
  func forget(_ id: String) {
    for kind in [MediaFile.Kind.photo, .video] {
      let url = file(id, kind)
      Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: url) }
    }
    saved.removeAll { $0.id == id }
    _ = store()
  }

  /// Signed out: nothing kept, nothing noted.
  func forgetAll() {
    generation += 1
    pending = []
    saved = []
    unwritten = [:]
    client = nil
    arrived = nil
    pauses.values.forEach { $0.cancel() }
    pauses = [:]
  }

  /// For the bar: a small picture (the viewer reads the photo itself from its file when it shows it).
  private func arrival(_ one: Saved) async -> Arrival? {
    let url = file(one.id, one.kind)
    switch one.kind {
    case .photo:
      let image = await Task.detached(priority: .userInitiated) { Self.thumbnail(url) }.value
      return image.map { Arrival(id: one.id, from: one.from, photo: $0, file: url) }
    case .video:
      guard FileManager.default.fileExists(atPath: url.path) else { return nil }
      let generator = AVAssetImageGenerator(asset: AVURLAsset(url: url))
      generator.appliesPreferredTrackTransform = true
      let side = CGFloat(Self.thumbnailPixels)
      generator.maximumSize = CGSize(width: side, height: side)
      let frame = (try? await generator.image(at: .zero)).map { UIImage(cgImage: $0.image) }
      return Arrival(id: one.id, from: one.from, photo: frame ?? UIImage(), video: url)
    }
  }

  /// A photo's file made small, read without the whole image in memory.
  nonisolated private static func thumbnail(_ url: URL) -> UIImage? {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceShouldCacheImmediately: true,
      kCGImageSourceThumbnailMaxPixelSize: thumbnailPixels,
    ]
    return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
  }
}
