// Photos and videos (spec 5, 6.8): made small and plain before they go (a JPEG of at most 1600 pixels, a video of
// at most 10 seconds and 8 MB, neither with a place or a device in it), sealed by the core under a key of their own,
// and kept in an outbox until the hub has them; only then their Yo goes. A hub that says "not now" (the day's quota,
// no room) keeps them waiting here, and they go later by themselves. What arrives is fetched at once, kept on this
// iPhone until it has been seen once, then deleted.
import AVFoundation
import CryptoKit
import Foundation
import ImageIO
import Observation
import Synchronization
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

  /// A manifest that is there and does not decode (damaged): kept aside under another name, never read as empty and
  /// written over; what it named stays on disk (`isQuarantined`). Only a person can tell what it was. False when it
  /// could not be moved: then nothing is read, nor written over it.
  static func quarantine(_ url: URL) -> Bool {
    let aside = url.deletingLastPathComponent().appending(
      path: url.lastPathComponent + ".unreadable-\(Int(Date.now.timeIntervalSince1970))")
    guard (try? FileManager.default.moveItem(at: url, to: aside)) != nil else { return false }
    Perf.mark("media manifest unreadable: \(url.lastPathComponent) kept as \(aside.lastPathComponent)")
    return true
  }

  /// A manifest kept aside (`quarantine`): whatever it named.
  static func isQuarantined(_ name: String) -> Bool { name.contains(".unreadable-") }

  /// Writes off the main actor.
  static func write(_ data: Data, to url: URL, _ protection: Data.WritingOptions) async -> Bool {
    await Task.detached(priority: .utility) { (try? data.write(to: url, options: [.atomic, protection])) != nil }.value
  }

  /// Signing out: nothing of the account's photos and videos stays, sent or received.
  static func clearAll() async {
    await ReceivedMedia.shared.forgetAll()
    ReceivedMedia.drainWrites()
    for name in ["Outbox", "Received"] {
      try? FileManager.default.removeItem(at: URL.applicationSupportDirectory.appending(path: name))
    }
    Prefetch.clear()
    ManifestSeal.forget()
  }
}

/// The two manifests (what waits to go, what came and waits to be seen) hold the photos' keys: sealed as the core's
/// state is (spec 2), ChaCha20-Poly1305 under a key of their own derived from the state key in the keychain
/// (HKDF-SHA256, "yo media manifest v1"), so the files alone open nothing. On disk: "YOM1" || nonce || ciphertext ||
/// tag. A manifest of the builds before (plain JSON) is read once and written again sealed.
enum ManifestSeal {
  private static let magic = Data("YOM1".utf8)
  private static let cache = Mutex<(key: SymmetricKey?, generation: Int)>((nil, 0))
  private static let reader = NSLock()

  /// What a manifest's bytes open to.
  enum Opened {
    /// Its JSON; `legacy`: it was not sealed (to be written again sealed).
    case json(Data, legacy: Bool)
    /// The keychain does not answer now (before the first unlock): read later.
    case notNow
    /// Damaged, or sealed under a key that is gone: never readable.
    case unreadable
  }

  /// The key; throws while the keychain does not answer (locked), nil when there is none (no account).
  fileprivate static func key() throws -> SymmetricKey? {
    if let cached = cache.withLock({ $0.key }) { return cached }
    // Cache reads never wait behind securityd. Only worker threads take the reader lock.
    guard !Thread.isMainThread else {
      Task.detached(priority: .utility) { _ = try? key() }
      throw SharedAccess.unavailable
    }
    reader.lock()
    defer { reader.unlock() }
    if let cached = cache.withLock({ $0.key }) { return cached }
    let generation = cache.withLock { $0.generation }
    return try Perf.timed("media.manifest.key") {
      var state: Data?
      if SharedStateFile.available { state = try StateKey.shared() }
      if state == nil { state = try StateKey.load() }
      guard let state else { return nil }
      let key = HKDF<SHA256>.deriveKey(
        inputKeyMaterial: SymmetricKey(data: state), info: Data("yo media manifest v1".utf8), outputByteCount: 32)
      return cache.withLock {
        guard $0.generation == generation else { return nil }
        $0.key = key
        return key
      }
    }
  }

  /// Signed out: an in-flight keychain read cannot repopulate the old account's cache.
  static func forget() { cache.withLock { $0 = (nil, $0.generation + 1) } }

  /// Nil when there is no key now: the manifest is not written.
  static func seal(_ json: Data) -> Data? {
    guard let key = try? key(), let box = try? ChaChaPoly.seal(json, using: key) else { return nil }
    return magic + box.combined
  }

  /// Notes that `file` was written sealed (after the write went): from then on a plain one there is no build's
  /// before but put there by someone else (who could name media keys of their choice), and is not taken. Per file:
  /// one migrated does not close the other's way.
  static func wrote(_ file: URL) {
    UserDefaults.standard.set(true, forKey: "manifests.sealed." + file.lastPathComponent)
  }

  static func open(_ data: Data, of file: URL) -> Opened {
    guard data.starts(with: magic) else {
      // Builds before wrote plain JSON ("[" or "{"), taken until this file was first written sealed; anything else
      // is damaged.
      let plain = data.first == UInt8(ascii: "[") || data.first == UInt8(ascii: "{")
      let migrated = UserDefaults.standard.bool(forKey: "manifests.sealed." + file.lastPathComponent)
      return plain && !migrated ? .json(data, legacy: true) : .unreadable
    }
    let key: SymmetricKey?
    do { key = try Self.key() } catch { return .notNow }
    guard let key, let box = try? ChaChaPoly.SealedBox(combined: data.dropFirst(magic.count)),
      let json = try? ChaChaPoly.open(box, using: key)
    else { return .unreadable }
    return .json(json, legacy: false)
  }
}

/// What the Yo of a photo or video says, as it waits on disk.
struct StoredMedia: Codable, Sendable {
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
    /// Its Yo waits for the undo time (`release`), its upload goes meanwhile. Still held when read again after a
    /// restart (the app ended within the undo time, or an undo that could not be saved): never sent.
    var held: Bool?
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

  /// Whether a photo or video is still in the queue (not sent, not dropped): a group's "LEO left" waits for it.
  func isWaiting(_ id: UUID) -> Bool { items.contains { $0.id == id } }

  /// Whether the queue on disk was read: until then nothing writes over it.
  @ObservationIgnored private var loaded = false

  init() {
    Task {
      _ = try? await Hub.run { try ManifestSeal.key() }
      load()
    }
  }

  /// Reads the queue, once; false while one is there and does not read now (before the first unlock after a restart).
  /// Read as empty then, the next photo's save would write the waiting ones out of it.
  @discardableResult
  private func load() -> Bool {
    if loaded { return true }
    var data: Data?
    if FileManager.default.fileExists(atPath: Self.index.path) {
      guard let read = try? Data(contentsOf: Self.index) else { return false }
      data = read
    }
    var read: [Item] = []
    var legacy = false
    if let data {
      var json: Data?
      switch ManifestSeal.open(data, of: Self.index) {
      case .notNow: return false
      case .unreadable: break
      case .json(let opened, let plain): (json, legacy) = (opened, plain)
      }
      if let decoded = json.flatMap({ try? JSONDecoder().decode([Item].self, from: $0) }) {
        read = decoded
      } else {
        // Its blobs stay (nothing here deletes one it does not name).
        guard MediaStorage.quarantine(Self.index) else { return false }
      }
    }
    items = read.filter { $0.held != true }
    for gone in read where gone.held == true { try? FileManager.default.removeItem(at: Self.blob(gone.id)) }
    loaded = true
    waiting = items.count
    if legacy { save() }  // its keys out of the plain file
    return true
  }

  /// The queue to disk; false when it did not write (it stays in memory, written with the next change).
  @discardableResult
  private func save() -> Bool {
    guard loaded else { return false }
    waiting = items.count
    guard let json = try? JSONEncoder().encode(items), let data = ManifestSeal.seal(json) else { return false }
    guard (try? data.write(to: Self.index, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil
    else { return false }
    ManifestSeal.wrote(Self.index)
    return true
  }

  /// Seals a photo or video for a friend and puts it in line, from `client`, the account the gesture began in (never
  /// from one signed in since); it goes as soon as the hub takes it. True once it is on disk, its blob and the queue.
  /// `id`: one already in line is not put in again (a share taken in twice, the app ended before its note went).
  /// `held`: its upload goes at once, its Yo only once released (`release`), or never (`discard`): the undo time.
  /// `at`: when it was taken; to each friend they go in that order, whichever was ready first.
  func send(
    _ file: MediaFile, to friend: String, from client: YoClient?, accounts: AccountStore, id: UUID = UUID(),
    held: Bool = false, at: Date = .now
  ) async -> Bool {
    guard let client, client === accounts.client, load() else { return false }
    guard !items.contains(where: { $0.id == id }) else { return true }
    let width = UInt16(clamping: file.width)
    let height = UInt16(clamping: file.height)
    guard
      let (sealed, owner) = try? await Hub.run({
        (try sealMedia(plain: file.data, width: width, height: height), client.userId())
      })
    else { return false }
    guard !items.contains(where: { $0.id == id }) else { return true }  // taken in meanwhile
    var item = Item(
      id: id, owner: owner, to: friend, kind: file.kind, media: StoredMedia(sealed.media),
      durationMs: file.durationMs, created: at)
    item.held = held ? true : nil
    let blob = Self.blob(item.id)
    guard await MediaStorage.write(sealed.ciphertext, to: blob, .completeFileProtectionUntilFirstUserAuthentication)
    else { return false }
    guard client === accounts.client else {  // signed out meanwhile
      try? FileManager.default.removeItem(at: blob)
      return false
    }
    items.insert(item, at: items.firstIndex { $0.created > item.created } ?? items.endIndex)
    guard save() else {
      items.removeAll { $0.id == item.id }
      waiting = items.count
      try? FileManager.default.removeItem(at: blob)
      return false
    }
    flush(accounts: accounts)
    return true
  }

  /// The undo time is over: its Yo goes as soon as its upload is done.
  func release(_ id: UUID) {
    guard let at = items.firstIndex(where: { $0.id == id }) else { return }
    items[at].held = nil
    // Released only once that is saved: a held item a start finds is an undo never ended, and goes. Not saved now (a
    // full disk), it is tried again until it is, or until the item has gone out and left the queue.
    if !save() { saveRelease(id) }
    if let accounts { flush(accounts: accounts) }
  }

  private func saveRelease(_ id: UUID, after seconds: Double = 2) {
    let generation = generation
    Task { [weak self] in
      try? await Task.sleep(for: .seconds(seconds))
      guard let self, generation == self.generation, self.items.contains(where: { $0.id == id && $0.held == nil })
      else { return }
      if !self.save() { self.saveRelease(id, after: min(seconds * 2, 60)) }
    }
  }

  /// Undone: out of the queue, its Yo never sent (a blob the hub took already is never named, and goes after its
  /// time there).
  func discard(_ id: UUID) {
    guard let at = items.firstIndex(where: { $0.id == id }) else { return }
    items.remove(at: at)
    save()
    let blob = Self.blob(id)
    Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: blob) }
  }

  /// Signed out: nothing waits any more, nothing under way goes on (the files went with MediaStorage.clearAll).
  func clear() {
    generation += 1
    items = []
    loaded = false  // the next account's queue is read anew (none: the files went)
    waiting = 0
    wake?.cancel()
  }

  /// Sends what is due: up to `together` uploads side by side (the hub takes a few of one user at once), then the Yos
  /// each friend's queue lets go (`releasable`). A burst of photos then goes about as fast as it was taken, not one
  /// upload after the other. What the hub refuses for now waits its turn again (30 seconds, doubling to half an hour).
  /// Also when the app comes to the foreground. A build the hub calls too old (426) sends nothing more.
  func flush(accounts: AccountStore) {
    self.accounts = accounts
    guard !running, load() else { return }
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
      // Held (its undo time running) holds the friend's after it too: they go in the order taken.
      guard item.uploaded == true, item.held != true, item.next <= .now else {
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
    case .held:
      items[at].next = .now + Self.heldSeconds  // looked at again soon, without a hub: it goes once they may get it
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

  /// Outcomes: `held`, the friend blocked or their new key not confirmed (spec 4.8, 3.4): kept, and goes after an
  /// unblock or a confirmation (`retryNow`), as the core's own outbox does.
  private enum Outcome { case uploaded, sent, later, held, dropped, replaced, stopped }

  /// How long one held for a blocked or unconfirmed friend waits before it is looked at again (a check on this
  /// iPhone, nothing goes to the hub).
  private static let heldSeconds: TimeInterval = 60

  /// A friend unblocked or their new key confirmed: what waits goes now.
  func retryNow() {
    for at in items.indices { items[at].next = .distantPast }
    if let accounts { flush(accounts: accounts) }
  }

  /// The chat first (a new friend's opens now), then the upload outside the client's lock: Yos go on meanwhile, and
  /// so do the other uploads.
  private func upload(_ item: Item, client: YoClient, generation: Int) async -> Outcome {
    let media = item.media.media
    let blob = Self.blob(item.id)
    return await outcome {
      // Another account's (signed out meanwhile): never sent under this one.
      guard try await Hub.run({ client.userId() }) == item.owner else { return .dropped }
      let (chat, contacts) = try await Hub.run { (try client.mediaChat(toTarget: item.to), client.contacts()) }
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
        case .photo: (try client.sendPhoto(toTarget: item.to, media: media), client.contacts())
        case .video: (try client.sendVideo(toTarget: item.to, media: media, durationMs: item.durationMs), client.contacts())
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
    } catch YoError.Outdated {
      return .sent  // the friend's app too old: the Yo waits in the core's outbox until they update
    } catch YoError.Refused(let status) where status == 426 {
      return .stopped  // this build too old: kept, for the build after it
    } catch YoError.Blocked, YoError.Unconfirmed {
      return .held  // kept until they are unblocked or their key confirmed, never dropped
    } catch let error as YoError where retryLater(error: error) {
      return .later
    } catch {
      return .dropped  // not on Yo, or refused for good
    }
  }
}

/// Photos and videos that came, kept until seen once. Each is noted (with its key) before the core may forget its
/// Yo, then fetched, a few at a time (each fetch counts at the hub, the last deletes the blob), opened and kept here
/// with complete file protection; seen, it is deleted. A fetch the network breaks is tried again soon, and at once
/// when the next Yo comes; after the last try it rests a while (a minute, doubling to half an hour) and the others go
/// first. One the hub no longer has is dropped. A download whose file does not write stays in memory and is written
/// again, never fetched again. What is noted and kept is one file (`media.json`), written whole: an item is noted
/// or kept, never neither. It is read before anything else is done here; while it cannot be read (the iPhone locked
/// since it started), nothing is written or deleted, and a note waits (the core keeps its Yo). All of it on the main
/// actor but the reading, writing and fetching.
actor ReceivedMedia {
  static let shared = ReceivedMedia()

  private struct Pending: Codable, Sendable {
    let id: String
    let from: String
    let kind: MediaFile.Kind
    let media: StoredMedia
  }

  private struct Saved: Codable, Sendable {
    let id: String
    let from: String
    let kind: MediaFile.Kind
  }

  /// Noted and kept, in one file.
  private struct Manifest: Codable, Sendable {
    var pending: [Pending] = []
    var saved: [Saved] = []
  }

  private var manifestRevision = 0
  private var pending: [Pending] = [] {
    didSet { manifestRevision += 1 }
  }
  private var saved: [Saved] = [] {
    didSet { manifestRevision += 1 }
  }
  /// The manifest has been read (or is not there): until then nothing here is written or deleted.
  private var loaded = false
  private var fetching: Set<String> = []
  /// Fetches the network broke, each waiting for its next try: cancelled, it tries now.
  private var pauses: [String: Task<Void, Never>] = [:]
  /// Fetches that used up their tries: not tried again before then, so they do not hold the others' turns.
  private var resting: [String: (until: Date, rests: Int)] = [:]
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

  private lazy var directory = MediaStorage.directory("Received")
  private var manifest: URL { directory.appending(path: "media.json") }
  /// The two indexes of the builds before, read once into the manifest.
  private var oldIndexes: [URL] { [directory.appending(path: "pending.json"), directory.appending(path: "received.json")] }

  private init() {
    // In front again, or unlocked: what rests, or waited for the files to open, is tried now.
    for name in [UIApplication.didBecomeActiveNotification, UIApplication.protectedDataDidBecomeAvailableNotification] {
      _ = NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { _ in
        Task { await ReceivedMedia.shared.wake() }
      }
    }
  }

  private func wake() {
    resting = [:]
    startMore()
  }

  /// A file's bytes; nil when it is not there. Throws when it is there and does not read now (locked).
  nonisolated private static func contents(_ url: URL) throws -> Data? {
    guard FileManager.default.fileExists(atPath: url.path) else { return nil }
    return try Data(contentsOf: url)
  }

  /// Reads the manifest, once; false while it cannot be read. Then a download whose file was written and not yet
  /// noted as kept (the app ended in between) is kept, and what no note names (seen but not deleted, or a write cut
  /// off) is deleted. A few files, once, before any fetch can write one.
  @discardableResult
  private func load() -> Bool {
    Perf.timed("media.manifest.load") { loadNow() }
  }

  private func loadNow() -> Bool {
    if loaded { return true }
    let decoder = JSONDecoder()
    let found: Manifest
    var migrated = false
    do {
      if let data = try Self.contents(manifest) {
        var json: Data?
        switch ManifestSeal.open(data, of: manifest) {
        case .notNow: return false
        case .unreadable: break
        case .json(let opened, let plain): (json, migrated) = (opened, plain)  // a plain one is written again sealed
        }
        if let decoded = json.flatMap({ try? decoder.decode(Manifest.self, from: $0) }) {
          found = decoded
        } else {
          // Kept aside with all it named, never read as empty.
          guard MediaStorage.quarantine(manifest) else { return false }
          found = Manifest()
        }
      } else {
        let (old, kept) = (try Self.contents(oldIndexes[0]), try Self.contents(oldIndexes[1]))
        found = Manifest(
          pending: old.flatMap { try? decoder.decode([Pending].self, from: $0) } ?? [],
          saved: kept.flatMap { try? decoder.decode([Saved].self, from: $0) } ?? [])
        migrated = old != nil || kept != nil
      }
    } catch {
      return false
    }
    loaded = true
    Prefetch.sweep()  // once a start: what the service fetched and nothing took
    (pending, saved) = (found.pending, found.saved)
    let written = pending.filter { FileManager.default.fileExists(atPath: file($0.id, $0.kind).path) }
    pending.removeAll { one in written.contains { $0.id == one.id } }
    saved += written.map { Saved(id: $0.id, from: $0.from, kind: $0.kind) }
    // Kept, and its file gone: seen, and the manifest that said so did not write before the app ended. Never again.
    let seen = saved.filter { !FileManager.default.fileExists(atPath: file($0.id, $0.kind).path) }
    saved.removeAll { one in seen.contains { $0.id == one.id } }
    if (migrated || !written.isEmpty || !seen.isEmpty) && storeNow() {
      oldIndexes.forEach { try? FileManager.default.removeItem(at: $0) }
    }
    // What no note names goes, unless a damaged manifest was kept aside: then it may be what that one named.
    let keep = Set(saved.map { file($0.id, $0.kind).lastPathComponent } + ([manifest] + oldIndexes).map(\.lastPathComponent))
    let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
    if !files.contains(where: { MediaStorage.isQuarantined($0.lastPathComponent) }) {
      for url in files where !keep.contains(url.lastPathComponent) {
        try? FileManager.default.removeItem(at: url)
      }
    }
    return true
  }

  private func file(_ id: String, _ kind: MediaFile.Kind) -> URL {
    let safe = String(id.map { $0.isLetter || $0.isNumber ? $0 : "-" })
    return directory.appending(path: safe + (kind == .photo ? ".jpg" : ".mp4"))
  }

  /// The manifest to disk, whole; false when it did not write (or was never read). Readable while the iPhone is
  /// locked once it was unlocked after starting, so a Yo that comes then is noted (the photos and videos themselves
  /// have complete protection).
  // Serial writes; migration and sign-out drain the queue before their synchronous work.
  private static let writes = DispatchQueue(label: "yo.received.manifest", qos: .utility)
  static func drainWrites() { writes.sync {} }

  private func store() async -> Bool {
    guard loaded, let key = try? ManifestSeal.key() else { return false }
    let snapshot = Manifest(pending: pending, saved: saved)
    let url = manifest
    return await withCheckedContinuation { continuation in
      Self.writes.async {
        let written = Perf.timed("media.manifest.save") {
          _ = MediaStorage.directory("Received")  // recreate after sign-out, off the main actor
          guard let json = try? JSONEncoder().encode(snapshot),
            let box = try? ChaChaPoly.seal(json, using: key),
            (try? (Data("YOM1".utf8) + box.combined).write(
              to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil
          else { return false }
          UserDefaults.standard.set(true, forKey: "manifests.sealed." + url.lastPathComponent)
          return true
        }
        continuation.resume(returning: written)
      }
    }
  }

  private func storeNow() -> Bool {
    return Perf.timed("media.manifest.migrate") {
      Self.drainWrites()
      guard loaded, let json = try? JSONEncoder().encode(Manifest(pending: pending, saved: saved)),
        let data = ManifestSeal.seal(json)
      else { return false }
      guard (try? data.write(to: manifest, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil
      else { return false }
      ManifestSeal.wrote(manifest)
      return true
    }
  }

  /// A photo or video came: noted before the core may forget its Yo. False when the note did not write: the core then
  /// keeps the Yo (`Inbox.take`).
  private var expecting: [String: Task<Bool, Never>] = [:]

  /// A batch crosses from the UI once; each successful note is still durable before its event is acknowledged.
  func expectBatch(_ notes: [(YoMedia, String, String, MediaFile.Kind)]) async -> Set<String> {
    var written: Set<String> = []
    for (media, id, from, kind) in notes {
      if await expect(media, id: id, from: from, kind: kind) { written.insert(id) }
    }
    return written
  }

  func expect(_ media: YoMedia, id: String, from: String, kind: MediaFile.Kind) async -> Bool {
    guard load() else { return false }
    if let writing = expecting[id] { return await writing.value }
    guard !pending.contains(where: { $0.id == id }), !saved.contains(where: { $0.id == id }) else { return true }
    pending.append(Pending(id: id, from: from, kind: kind, media: StoredMedia(media)))
    let generation = generation
    let writing = Task {
      guard generation == self.generation else { return false }
      let stored = await store()
      return generation == self.generation && stored
    }
    expecting[id] = writing
    let written = await writing.value
    guard generation == self.generation else { return false }
    expecting[id] = nil
    if !written { pending.removeAll { $0.id == id } }
    return written
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
    Perf.timed("media.fetch.start") {
      guard let client, let arrived, !HubTransport.unsupported, load() else { return }
      // While a download is not written (a full disk), nothing new is fetched: only what is held in memory is tried
      // again, so the memory held stays bounded and nothing fetched for the last time is lost to a kill.
      let holding = !unwritten.isEmpty
      for one in pending
      where !fetching.contains(one.id) && fetching.count < Self.together && (resting[one.id]?.until ?? .distantPast) <= .now
        && (!holding || unwritten[one.id] != nil)
      {
        fetching.insert(one.id)
        Task { await fetch(one, client: client, arrived: arrived) }
      }
    }
  }

  /// Tries again after 1, 3, 10 and 30 seconds (or sooner, see `fetchWaiting`); then it rests (`rest`).
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
      // Fetched by the notification service as its push came: there at once.
      var data = unwritten[one.id] ?? Prefetch.take(media)
      // There and locked: it reads once the iPhone is unlocked (the hub's copy is this device's fetch already). Being
      // fetched by the service: its file, rather than a second fetch (which would take another device's).
      if data == nil && (Prefetch.has(media) || Prefetch.underWay(media)) { continue }
      // The claim is held until the photo is written and noted (or given up): given back before, the service could
      // fetch it again meanwhile, and take another device's fetch.
      var claimed: String?
      defer { if let claimed { Prefetch.unclaim(media, token: claimed) } }
      if data == nil {
        // This device's one fetch, claimed against the notification service's: each takes one of the blob's fetches,
        // and a second would be another device's. Its file may have come just before the claim.
        guard let token = Prefetch.claim(media, again: true) else { continue }
        claimed = token
        data = Prefetch.take(media)
        do {
          if data == nil { data = try await Hub.run { try client.fetchMedia(media: media) } }
          // "No longer there": the service's file, written meanwhile, else (there, locked) once it reads.
          if data == nil { data = Prefetch.take(media) }
          if data == nil && Prefetch.has(media) { continue }
        } catch YoError.Refused(let status) where status == 426 {
          return  // this build too old: noted still, for the build after it
        } catch let error as YoError where retryLater(error: error) {
          continue  // noted still: tried again now, or with the next Yo or opening
        } catch {
          data = nil  // does not open: nothing to keep
        }
        guard generation == self.generation else { return }  // signed out meanwhile: nothing kept
      }
      guard let data else {
        // The hub no longer has it, or it does not open: its Yo came without it.
        Prefetch.done(media)
        pending.removeAll { $0.id == one.id }
        resting[one.id] = nil
        _ = await store()
        return
      }
      // Kept meanwhile from the same blob in another conversation (below): done.
      guard !saved.contains(where: { $0.id == one.id }) else { return }
      // The file, then the manifest: kept once both are written (a start finds a written file that is only noted).
      let url = file(one.id, one.kind)
      let written = await MediaStorage.write(data, to: url, .completeFileProtection)
      // Signed out meanwhile: nothing of it kept, not even in memory (it would hold the next account's downloads).
      guard generation == self.generation else {
        if written { try? FileManager.default.removeItem(at: url) }
        return
      }
      guard written else {
        unwritten[one.id] = data
        continue
      }
      let kept = Saved(id: one.id, from: one.from, kind: one.kind)
      saved.append(kept)
      pending.removeAll { $0.id == one.id }
      let stored = await store()
      guard generation == self.generation else { return }
      guard stored else {
        saved.removeAll { $0.id == one.id }
        pending.append(one)
        unwritten[one.id] = data
        continue
      }
      unwritten[one.id] = nil
      resting[one.id] = nil
      Prefetch.done(media)  // kept in the app's own media now
      let arrival = await arrival(kept)
      // Signed out while the thumbnail was made: not for the account after it.
      if let arrival, generation == self.generation { await arrived(arrival) }
      // The same blob in other conversations waiting too (a photo to two groups): theirs from this one fetch, not
      // one more each (the notification service's file is gone, and its claim would not be given again).
      for other in pending where other.id != one.id && other.media.media.id == media.id {
        let url = file(other.id, other.kind)
        guard await MediaStorage.write(data, to: url, .completeFileProtection), generation == self.generation else {
          continue
        }
        let also = Saved(id: other.id, from: other.from, kind: other.kind)
        saved.append(also)
        pending.removeAll { $0.id == other.id }
        let stored = await store()
        guard generation == self.generation else { return }
        guard stored else {
          saved.removeAll { $0.id == other.id }
          pending.append(other)
          continue
        }
        if let shown = await self.arrival(also), generation == self.generation { await arrived(shown) }
      }
      return
    }
    rest(one.id)
  }

  /// Every try used up: the item rests a minute, then two, doubling to half an hour, and the others go meanwhile.
  private func rest(_ id: String) {
    let rests = (resting[id]?.rests ?? 0) + 1
    let seconds = min(1800, 60 * pow(2, Double(min(rests - 1, 5))))
    resting[id] = (Date.now + seconds, rests)
    let generation = generation
    Task { [weak self] in
      try? await Task.sleep(for: .seconds(seconds))
      await self?.resumeFetches(generation: generation)
    }
  }

  private func resumeFetches(generation: Int) {
    guard generation == self.generation else { return }
    startMore()
  }

  /// What waits to be seen, after a restart, oldest sent first. Once the files can be read: a start while the iPhone
  /// is locked waits for it to be unlocked.
  func waiting() async -> [Arrival] {
    while !(await MainActor.run { UIApplication.shared.isProtectedDataAvailable }) || !load() {
      try? await Task.sleep(for: .seconds(5))
    }
    let generation = generation
    var arrivals: [Arrival] = []
    for one in saved {
      if let arrival = await arrival(one) { arrivals.append(arrival) }
      guard generation == self.generation else { return [] }  // signed out meanwhile: not for the next account
    }
    return arrivals.sorted(by: Arrival.older)
  }

  /// Shown: it will not show again, even if the app ends before the viewer closes (its file goes then, or at the
  /// next start). A manifest that does not write now is written again until it does (`storeUntilWritten`); its file
  /// goes when the viewer closes all the same, and a start drops a kept one without its file (`load`).
  func consumed(_ id: String) {
    Perf.timed("media.consumed") {
      saved.removeAll { $0.id == id }
      storeUntilWritten()
    }
  }

  /// Seen: gone from this iPhone, the file first (what makes it seen for good, see `load`), then the manifest.
  func forget(_ id: String) {
    Perf.timed("media.forget") {
      for kind in [MediaFile.Kind.photo, .video] {
        try? FileManager.default.removeItem(at: file(id, kind))
      }
      saved.removeAll { $0.id == id }
      storeUntilWritten()
    }
  }

  /// Written again while it fails, a second, then longer, up to half a minute apart, until it writes (any later
  /// write of it counts too) or the account signs out.
  private var rewriting: Task<Void, Never>?

  private func storeUntilWritten() {
    guard rewriting == nil else { return }
    let generation = generation
    rewriting = Task {
      var pause = 1.0
      while generation == self.generation {
        let revision = manifestRevision
        let written = await store()
        guard generation == self.generation else { return }
        if written {
          if revision == manifestRevision { break }
          continue  // a mutation during the write needs a newer snapshot
        }
        try? await Task.sleep(for: .seconds(pause))
        pause = min(pause * 2, 30)
      }
      if generation == self.generation { rewriting = nil }
    }
  }

  /// Signed out: nothing kept, nothing noted (the files went with MediaStorage.clearAll).
  func forgetAll() {
    generation += 1
    expecting.values.forEach { $0.cancel() }
    expecting = [:]
    pending = []
    saved = []
    loaded = true
    resting = [:]
    unwritten = [:]
    client = nil
    arrived = nil
    pauses.values.forEach { $0.cancel() }
    pauses = [:]
    rewriting?.cancel()
    rewriting = nil
  }

  /// For the bar: a small picture (the viewer reads the photo itself from its file when it shows it).
  private func arrival(_ one: Saved) async -> Arrival? {
    let url = file(one.id, one.kind)
    switch one.kind {
    case .photo:
      let image = await Task.detached(priority: .userInitiated) { Perf.timed("media.thumbnail.decode") { Self.thumbnail(url) } }.value
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
    return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { image in
      Perf.timed("media.thumbnail.image") {
        let image = UIImage(cgImage: image)
        return image.preparingForDisplay() ?? image
      }
    }
  }
}
