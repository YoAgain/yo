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
/// that quits, a hub that says "not now", no network, none of it loses one. Their Yos in the order they were taken;
/// a few uploads side by side, while Yos go on beside them (an upload does not hold the core).
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
  }

  /// How many wait.
  private(set) var waiting = 0
  /// Where what the core received while sending goes (the inbox): Yos that came meanwhile are not lost.
  /// Awaited before the next core call, which lets the core forget them; with the core's contacts, read with them.
  @ObservationIgnored var received: (@MainActor ([YoEvent], [YoContact], YoClient) async -> Void)?
  /// Counts sign-outs: what began before one ends without a trace.
  @ObservationIgnored private var generation = 0
  @ObservationIgnored private var items: [Item] = []
  @ObservationIgnored private var running = false
  @ObservationIgnored private var wake: Task<Void, Never>?
  @ObservationIgnored private weak var accounts: AccountStore?
  /// One left this long is given up.
  static let givenUpAfter: TimeInterval = 3 * 24 * 3600

  private static var directory: URL { MediaStorage.directory("Outbox") }
  private static func blob(_ id: UUID) -> URL { directory.appending(path: "\(id.uuidString).blob") }
  private static var index: URL { directory.appending(path: "outbox.json") }

  init() {
    if let data = try? Data(contentsOf: Self.index), let saved = try? JSONDecoder().decode([Item].self, from: data) {
      items = saved
    }
    waiting = items.count
  }

  private func save() {
    waiting = items.count
    guard let data = try? JSONEncoder().encode(items) else { return }
    try? data.write(to: Self.index, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
  }

  /// Seals a photo or video for a friend and puts it in line; it goes as soon as the hub takes it.
  func send(_ file: MediaFile, to friend: String, accounts: AccountStore) async -> Bool {
    guard let client = accounts.client else { return false }
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
    guard await MediaStorage.write(sealed.ciphertext, to: Self.blob(item.id), .completeFileProtectionUntilFirstUserAuthentication)
    else { return false }
    items.append(item)
    save()
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

  /// Sends what is due: up to `together` uploads side by side (the hub takes a few of one user at once), each Yo in
  /// the order taken, as soon as its own upload and the Yos before it are done. A burst of photos then goes about as
  /// fast as it was taken, not one upload after the other. What the hub refuses for now waits its turn again (30
  /// seconds, doubling to half an hour). Also when the app comes to the foreground.
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
      while let client = accounts.client {
        let due = Array(items.filter { $0.next <= .now }.prefix(Self.together))
        guard !due.isEmpty else { break }
        let uploads = due.map { item in Task { await self.upload(item, client: client, generation: generation) } }
        for (item, upload) in zip(due, uploads) {
          var outcome = await upload.value
          if outcome == .uploaded { outcome = await send(item, client: client, generation: generation) }
          guard generation == self.generation else { return }  // signed out meanwhile
          guard let at = items.firstIndex(where: { $0.id == item.id }) else { continue }
          switch outcome {
          case .sent, .dropped:
            items.remove(at: at)
            let blob = Self.blob(item.id)
            Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: blob) }
          case .later, .uploaded:
            items[at].tries += 1
            items[at].next = .now + min(1800, 30 * pow(2, Double(min(items[at].tries - 1, 6))))
          case .replaced:
            accounts.replaced(client)  // clears this outbox too
            return
          }
          save()
        }
      }
      scheduleWake()
    }
  }

  /// Uploads under way at once, at most (the hub takes 4 of one user).
  private static let together = 3

  private func scheduleWake() {
    wake?.cancel()
    guard let next = items.map(\.next).min() else { return }
    wake = Task { [weak self] in
      try? await Task.sleep(for: .seconds(max(1, next.timeIntervalSinceNow)))
      guard !Task.isCancelled, let self, let accounts = self.accounts else { return }
      self.flush(accounts: accounts)
    }
  }

  private enum Outcome { case uploaded, sent, later, dropped, replaced }

  /// The chat first (a new friend's opens now), then the upload outside the client's lock: Yos go on meanwhile, and
  /// so do the other uploads.
  private func upload(_ item: Item, client: YoClient, generation: Int) async -> Outcome {
    if Date.now.timeIntervalSince(item.created) > Self.givenUpAfter { return .dropped }
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
    } catch let error as YoError where mediaRetryLater(error: error) {
      return .later
    } catch {
      return .dropped  // not on Yo, blocked, or refused for good
    }
  }
}

/// Photos and videos that came, kept until seen once. Each is noted (with its key) before the core may forget its
/// Yo, then fetched at once, all side by side (each fetch counts at the hub, the last deletes the blob), opened and
/// kept here with complete file protection; seen, it is deleted. A fetch the network breaks is tried again soon, and
/// at once when the next Yo comes; one the hub no longer has is dropped. All of it on the main actor but the
/// reading, writing and fetching.
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
  /// Counts sign-outs: a fetch begun before one keeps nothing.
  private var generation = 0

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

  private func store() {
    if let data = try? JSONEncoder().encode(pending) {
      try? data.write(to: pendingIndex, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
    if let data = try? JSONEncoder().encode(saved) {
      try? data.write(to: savedIndex, options: [.atomic, .completeFileProtection])
    }
  }

  /// A photo or video came: noted before the core may forget its Yo.
  func expect(_ media: YoMedia, id: String, from: String, kind: MediaFile.Kind) {
    guard !pending.contains(where: { $0.id == id }), !saved.contains(where: { $0.id == id }) else { return }
    pending.append(Pending(id: id, from: from, kind: kind, media: StoredMedia(media)))
    store()
  }

  /// Fetches what is noted and not under way, each at once and beside the others; one waiting to try again (the
  /// network broke it) tries now: the next Yo came, so the network is back. Each one kept goes to `arrived`.
  func fetchWaiting(client: YoClient, arrived: @escaping @MainActor (Arrival) -> Void) {
    for one in pending {
      if fetching.insert(one.id).inserted {
        Task { await fetch(one, client: client, arrived: arrived) }
      } else {
        pauses[one.id]?.cancel()
      }
    }
  }

  /// Tries again after 1, 3, 10 and 30 seconds (or sooner, see `fetchWaiting`); then with the next Yo or opening.
  private static let retries = [0, 1, 3, 10, 30]

  private func fetch(_ one: Pending, client: YoClient, arrived: @MainActor (Arrival) -> Void) async {
    defer { fetching.remove(one.id) }
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
      let data: Data?
      do {
        data = try await Hub.run { try client.fetchMedia(media: media) }
      } catch let error as YoError where mediaRetryLater(error: error) {
        continue  // noted still: tried again now, or with the next Yo or opening
      } catch {
        data = nil  // does not open: nothing to keep
      }
      guard generation == self.generation else { return }  // signed out meanwhile: nothing kept
      if let data, await MediaStorage.write(data, to: file(one.id, one.kind), .completeFileProtection) {
        guard generation == self.generation else {
          try? FileManager.default.removeItem(at: file(one.id, one.kind))
          return
        }
        saved.append(Saved(id: one.id, from: one.from, kind: one.kind))
      }
      pending.removeAll { $0.id == one.id }
      store()
      if let kept = saved.last, kept.id == one.id, let arrival = await arrival(kept) { arrived(arrival) }
      return
    }
  }

  /// What waits to be seen, after a restart.
  func waiting() async -> [Arrival] {
    var arrivals: [Arrival] = []
    for one in saved {
      if let arrival = await arrival(one) { arrivals.append(arrival) }
    }
    return arrivals
  }

  /// Shown: it will not show again, even if the app ends before the viewer closes (its file goes then, or at the
  /// next start).
  func consumed(_ id: String) {
    saved.removeAll { $0.id == id }
    store()
  }

  /// Seen: gone from this iPhone.
  func forget(_ id: String) {
    for kind in [MediaFile.Kind.photo, .video] {
      let url = file(id, kind)
      Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: url) }
    }
    saved.removeAll { $0.id == id }
    store()
  }

  /// Signed out: nothing kept, nothing noted.
  func forgetAll() {
    generation += 1
    pending = []
    saved = []
    pauses.values.forEach { $0.cancel() }
    pauses = [:]
  }

  private func arrival(_ one: Saved) async -> Arrival? {
    let url = file(one.id, one.kind)
    switch one.kind {
    case .photo:
      let image = await Task.detached(priority: .userInitiated) { UIImage(contentsOfFile: url.path) }.value
      return image.map { Arrival(id: one.id, from: one.from, photo: $0) }
    case .video:
      guard FileManager.default.fileExists(atPath: url.path) else { return nil }
      let generator = AVAssetImageGenerator(asset: AVURLAsset(url: url))
      generator.appliesPreferredTrackTransform = true
      let frame = (try? await generator.image(at: .zero)).map { UIImage(cgImage: $0.image) }
      return Arrival(id: one.id, from: one.from, photo: frame ?? UIImage(), video: url)
    }
  }
}
