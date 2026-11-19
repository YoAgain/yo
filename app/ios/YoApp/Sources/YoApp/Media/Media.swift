// Photos and videos (spec 5, 6.8): sealed by the core, kept in an outbox until the hub has the blob, only then their
// Yo goes. What arrives is fetched at once, kept until seen once, then deleted.
import AVFoundation
import CryptoKit
import Foundation
import ImageIO
import Observation
import Synchronization
import UIKit
import UniformTypeIdentifiers
import YoCore

/// Never in the backup: a photo seen once must not live on there.
enum MediaStorage {
  static func directory(_ name: String) -> URL {
    var dir = URL.applicationSupportDirectory.appending(path: name, directoryHint: .isDirectory)
    Diagnostics.attempt({ try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true) })
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    Diagnostics.attempt({ try dir.setResourceValues(values) })
    return dir
  }

  /// A damaged manifest is moved aside, never read as empty and written over; what it named stays on disk. False
  /// when it could not be moved: then nothing is read or written over it.
  static func quarantine(_ url: URL) -> Bool {
    let aside = url.deletingLastPathComponent().appending(
      path: url.lastPathComponent + ".unreadable-\(Int(Date.now.timeIntervalSince1970))")
    guard (Diagnostics.attempt({ try FileManager.default.moveItem(at: url, to: aside) })) != nil else { return false }
    Perf.mark("media manifest unreadable: \(url.lastPathComponent) kept as \(aside.lastPathComponent)")
    return true
  }

  static func isQuarantined(_ name: String) -> Bool { name.contains(".unreadable-") }

  static func write(_ data: Data, to url: URL, _ protection: Data.WritingOptions) async -> Bool {
    await Task.detached(priority: .utility) { (Diagnostics.attempt({ try data.write(to: url, options: [.atomic, protection]) })) != nil }.value
  }

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

/// The manifests hold media keys, so they are sealed (spec 2): ChaCha20-Poly1305 under HKDF-SHA256("yo media
/// manifest v1") of the state key. On disk: "YOM1" || nonce || ciphertext || tag. Older plain JSON is read once and
/// written again sealed.
enum ManifestSeal {
  private static let magic = Data("YOM1".utf8)
  private static let cache = Mutex<(key: SymmetricKey?, generation: Int)>((nil, 0))
  private static let reader = NSLock()

  enum Opened {
    /// `legacy`: plain JSON, to be written again sealed.
    case json(Data, legacy: Bool)
    /// The keychain does not answer before the first unlock: read later.
    case notNow
    /// Damaged, or sealed under a key that is gone.
    case unreadable
  }

  /// Throws while the keychain is locked; nil when there is no account.
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
    guard let key = try? key() else { return nil }
    return seal(json, key: key)
  }

  static func seal(_ json: Data, key: SymmetricKey) -> Data? {
    Diagnostics.attempt({ try ChaChaPoly.seal(json, using: key) }).map { magic + $0.combined }
  }

  /// Writes a sealed manifest. Once a file was written sealed, a plain one there can only have been planted (it
  /// could name media keys of its choice) and is not taken; tracked per file.
  static func write(_ sealed: Data, to file: URL) -> Bool {
    guard Diagnostics.attempt({
      try sealed.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }) != nil else { return false }
    UserDefaults.standard.set(true, forKey: sealedKey(file))
    return true
  }

  private static func sealedKey(_ file: URL) -> String { "manifests.sealed." + file.lastPathComponent }

  static func open(_ data: Data, of file: URL) -> Opened {
    guard data.starts(with: magic) else {
      let plain = data.first == UInt8(ascii: "[") || data.first == UInt8(ascii: "{")
      let migrated = UserDefaults.standard.bool(forKey: sealedKey(file))
      return plain && !migrated ? .json(data, legacy: true) : .unreadable
    }
    let key: SymmetricKey?
    do { key = try Self.key() } catch { return .notNow }
    guard let key, let box = Diagnostics.attempt({ try ChaChaPoly.SealedBox(combined: data.dropFirst(magic.count)) }),
      let json = Diagnostics.attempt({ try ChaChaPoly.open(box, using: key) })
    else { return .unreadable }
    return .json(json, legacy: false)
  }
}

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

/// Outgoing media, kept on disk until the hub has the blob and the Yo is sent; none is given up. Uploads run a few
/// side by side without holding the core; the Yos to each friend go in the order taken, each once its upload and
/// those before it are done. A Yo resent after a crash goes once: the core knows its blob's id.
@MainActor @Observable
final class MediaOutbox {
  private struct Item: Codable {
    let id: UUID
    /// The sending account's user id: only that one sends it.
    let owner: String
    /// User id, or name before the first Yo.
    let to: String
    let kind: MediaFile.Kind
    let media: StoredMedia
    let durationMs: UInt32
    let created: Date
    var tries = 0
    var next = Date.distantPast
    var uploaded: Bool?
    /// Builds before this one held an item until its undo ended; one found after a restart is discarded.
    var held: Bool?
    /// Camera attempts never retry after a failure or restart.
    var singleAttempt: Bool?
  }

  private(set) var waiting = 0
  /// Events the core returned while sending go to the inbox; awaited before the next core call.
  @ObservationIgnored var received: (@MainActor ([YoEvent], [YoContact], YoClient) async -> Void)?
  /// Counts sign-outs.
  @ObservationIgnored private var generation = 0
  @ObservationIgnored private var items: [Item] = []
  @ObservationIgnored private var running = false
  @ObservationIgnored private var oneShot: [UUID: CheckedContinuation<Bool, Never>] = [:]
  @ObservationIgnored private var wake: Task<Void, Never>?
  @ObservationIgnored private weak var accounts: AccountStore?

  private static var directory: URL { MediaStorage.directory("Outbox") }
  private static func blob(_ id: UUID) -> URL { directory.appending(path: "\(id.uuidString).blob") }
  private static var index: URL { directory.appending(path: "outbox.json") }

  /// A group's "LEO left" waits for it.
  func isWaiting(_ id: UUID) -> Bool { items.contains { $0.id == id } }

  /// Until the queue on disk was read nothing writes over it.
  @ObservationIgnored private var loaded = false

  init() {
    Task {
      _ = try? await Hub.run { try ManifestSeal.key() }
      load()
    }
  }

  /// False while it is there and does not read (before the first unlock): read as empty, the next save would drop
  /// what waits.
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
        guard MediaStorage.quarantine(Self.index) else { return false }
      }
    }
    items = read.filter { $0.held != true && $0.singleAttempt != true }
    for gone in read where gone.held == true || gone.singleAttempt == true { try? FileManager.default.removeItem(at: Self.blob(gone.id)) }
    loaded = true
    waiting = items.count
    if legacy { save() }
    return true
  }

  /// False when it did not write: it stays in memory, written with the next change.
  @discardableResult
  private func save() -> Bool {
    guard loaded else { return false }
    waiting = items.count
    guard let json = try? JSONEncoder().encode(items), let data = ManifestSeal.seal(json) else { return false }
    return ManifestSeal.write(data, to: Self.index)
  }

  /// From `client`, the account the gesture began in. True once blob and queue are on disk. An `id` already in line
  /// is not added again; `at` orders the Yos to each friend.
  func send(
    _ file: MediaFile, to friend: String, from client: YoClient?, accounts: AccountStore, id: UUID = UUID(),
    at: Date = .now
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
    guard !items.contains(where: { $0.id == id }) else { return true }
    var item = Item(
      id: id, owner: owner, to: friend, kind: file.kind, media: StoredMedia(sealed.media),
      durationMs: file.durationMs, created: at)
    item.singleAttempt = oneShot[id] != nil ? true : nil
    let blob = Self.blob(item.id)
    guard await MediaStorage.write(sealed.ciphertext, to: blob, .completeFileProtectionUntilFirstUserAuthentication)
    else {
      try? FileManager.default.removeItem(at: blob)
      return false
    }
    guard client === accounts.client else {
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

  /// Camera stacks enter only after undo ends; a failed attempt is erased, not retried.
  func sendOnce(
    _ file: MediaFile, to friend: String, from client: YoClient, accounts: AccountStore, at: Date
  ) async -> Bool {
    guard !HubTransport.unsupported else { return false }
    let id = UUID()
    return await withCheckedContinuation { continuation in
      oneShot[id] = continuation
      Task {
        if !(await send(file, to: friend, from: client, accounts: accounts, id: id, at: at)) {
          oneShot.removeValue(forKey: id)?.resume(returning: false)
        }
      }
    }
  }

  func discard(_ id: UUID) {
    guard let at = items.firstIndex(where: { $0.id == id }) else { return }
    items.remove(at: at)
    try? FileManager.default.removeItem(at: Self.blob(id))
    oneShot.removeValue(forKey: id)?.resume(returning: false)
    save()
  }

  /// Signed out (the files went with MediaStorage.clearAll).
  func clear() {
    generation += 1
    let pending = oneShot.values
    oneShot = [:]
    for continuation in pending { continuation.resume(returning: false) }
    items = []
    loaded = false
    waiting = 0
    wake?.cancel()
  }

  /// Up to `together` uploads side by side, then the Yos each friend's queue lets go (`releasable`). "Not now" waits
  /// 30 s, doubling to half an hour. After a 426 nothing more is sent.
  func flush(accounts: AccountStore) {
    self.accounts = accounts
    guard !running, load() else { return }
    running = true
    let generation = generation
    Task {
      defer {
        running = false
        // A stopped flush must not retain failed camera media or leave its caller waiting.
        for id in Array(oneShot.keys) where items.contains(where: { $0.id == id }) { discard(id) }
        // Signed out and in meanwhile: the new account's queue goes now.
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
        var held: Set<String> = []  // a friend whose Yo did not go: their next ones wait behind it
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

  /// The hub takes 4 uploads of one user at once.
  private static let together = 3

  /// Per friend in order taken, up to the first whose upload is not done or that waits.
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

  /// False: stop (this phone was replaced, or this build is too old).
  private func settle(_ item: Item, _ outcome: Outcome, client: YoClient, accounts: AccountStore) -> Bool {
    guard let at = items.firstIndex(where: { $0.id == item.id }) else { return true }
    if oneShot[item.id] != nil, outcome != .uploaded {
      let completion = oneShot.removeValue(forKey: item.id)
      items.remove(at: at)
      try? FileManager.default.removeItem(at: Self.blob(item.id))
      save()
      completion?.resume(returning: outcome == .sent)
      if outcome == .replaced { accounts.replaced(client); return false }
      return outcome != .stopped
    }
    if outcome == .dropped { SendFailure.shared.show() }
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
      items[at].next = .now + Self.heldSeconds
    case .replaced:
      accounts.replaced(client)  // clears this outbox too
      return false
    case .stopped:
      return false
    }
    save()
    return true
  }

  private func scheduleWake() {
    wake?.cancel()
    guard let next = items.map(\.next).filter({ $0 > .now }).min() else { return }
    wake = Task { [weak self] in
      try? await Task.sleep(for: .seconds(max(1, next.timeIntervalSinceNow)))
      guard !Task.isCancelled, let self, let accounts = self.accounts else { return }
      self.flush(accounts: accounts)
    }
  }

  /// `held`: friend blocked or new key unconfirmed (spec 4.8, 3.4); goes after `retryNow`, as the core's outbox does.
  private enum Outcome { case uploaded, sent, later, held, dropped, replaced, stopped }

  /// A local re-check only; nothing goes to the hub.
  private static let heldSeconds: TimeInterval = 60

  /// A friend unblocked or their new key confirmed.
  func retryNow() {
    for at in items.indices { items[at].next = .distantPast }
    if let accounts { flush(accounts: accounts) }
  }

  /// The chat first (a new friend's opens now), then the upload outside the client's lock.
  private func upload(_ item: Item, client: YoClient, generation: Int) async -> Outcome {
    let media = item.media.media
    let blob = Self.blob(item.id)
    return await outcome(strict: oneShot[item.id] != nil) {
      guard try await Hub.run({ client.userId() }) == item.owner else { return .dropped }
      let (events, contacts) = try await Hub.run {
        (try client.uploadMediaTo(target: sendTarget(to: item.to), media: media, ciphertext: try Data(contentsOf: blob)),
         client.contacts())
      }
      await received?(events, contacts, client)
      guard generation == self.generation else { return .dropped }
      return .uploaded
    }
  }

  private func send(_ item: Item, client: YoClient, generation: Int) async -> Outcome {
    guard generation == self.generation else { return .dropped }
    let media = item.media.media
    return await outcome(strict: oneShot[item.id] != nil) {
      guard try await Hub.run({ client.userId() }) == item.owner else { return .dropped }
      let (events, after) = try await Hub.run {
        switch item.kind {
        case .photo: (try client.send(target: sendTarget(to: item.to), kind: .photo(media: media)), client.contacts())
        case .video: (try client.send(target: sendTarget(to: item.to), kind: .video(media: media, durationMs: item.durationMs)), client.contacts())
        }
      }
      await received?(events, after, client)
      return .sent
    }
  }

  private func outcome(strict: Bool = false, _ work: () async throws -> Outcome) async -> Outcome {
    do {
      return try await work()
    } catch YoError.Replaced {
      return .replaced
    } catch YoError.Outdated {
      if strict { return .dropped }
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

/// Received media, kept until seen once. Each is noted (with its key) in `media.json` before the core may forget its
/// Yo, then fetched a few at a time (each fetch counts at the hub), kept with complete protection, deleted once seen.
/// Broken fetches retry soon and at the next Yo, then rest (a minute, doubling to half an hour). A download that does
/// not write stays in memory and is never fetched again. Until the manifest reads (locked since boot) nothing is
/// written or deleted and the core keeps the Yo.
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

  /// One file, written whole: an item is noted or kept, never neither.
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
  private var loaded = false
  private var fetching: Set<String> = []
  /// Cancelled, a waiting retry tries now.
  private var pauses: [String: Task<Void, Never>] = [:]
  /// Fetches that used up their tries, so they do not hold the others' turns.
  private var resting: [String: (until: Date, rests: Int)] = [:]
  /// Opened downloads waiting for disk space; no acknowledgement until written.
  private var unwritten: [String: Data] = [:]
  private var client: YoClient?
  private var arrived: (@MainActor (Arrival) -> Void)?
  /// The hub no longer has it: off its bar.
  private var dropped: (@MainActor (String) -> Void)?
  /// Swiped away before its download finished: still fetched and acknowledged (spec 6.8), then deleted.
  private var unwanted: Set<String> = []
  /// Counts sign-outs: a fetch begun before one keeps nothing.
  private var generation = 0

  /// A backlog never holds every thread and all the memory.
  private static let together = 3
  nonisolated private static let thumbnailPixels = 300

  private lazy var directory = MediaStorage.directory("Received")
  private var manifest: URL { directory.appending(path: "media.json") }
  /// Older builds' indexes, read once into the manifest.
  private var oldIndexes: [URL] { [directory.appending(path: "pending.json"), directory.appending(path: "received.json")] }

  private init() {
    for name in [UIApplication.didBecomeActiveNotification, UIApplication.protectedDataDidBecomeAvailableNotification] {
      _ = NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { _ in
        Task { await ReceivedMedia.shared.wake() }
      }
    }
  }

  private func wake() {
    resting = [:]
    if let client { retryAcks(client: client) }
    startMore()
  }

  /// Nil when it is not there; throws when it is there and does not read now (locked).
  nonisolated private static func contents(_ url: URL) throws -> Data? {
    guard FileManager.default.fileExists(atPath: url.path) else { return nil }
    return try Data(contentsOf: url)
  }

  /// Once; false while it cannot be read. A written file only noted (the app ended in between) is kept; what no note
  /// names is deleted.
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
        case .json(let opened, let plain): (json, migrated) = (opened, plain)
        }
        if let decoded = json.flatMap({ try? decoder.decode(Manifest.self, from: $0) }) {
          found = decoded
        } else {
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
      Diagnostics.record(error)
      return false
    }
    loaded = true
    Prefetch.sweep()
    (pending, saved) = (found.pending, found.saved)
    let written = pending.filter { FileManager.default.fileExists(atPath: file($0.id, $0.kind).path) }
    pending.removeAll { one in written.contains { $0.id == one.id } }
    saved += written.map { Saved(id: $0.id, from: $0.from, kind: $0.kind) }
    // Kept with its file gone: seen, and the manifest did not write before the app ended.
    let seen = saved.filter { !FileManager.default.fileExists(atPath: file($0.id, $0.kind).path) }
    saved.removeAll { one in seen.contains { $0.id == one.id } }
    if (migrated || !written.isEmpty || !seen.isEmpty) && storeNow() {
      oldIndexes.forEach { try? FileManager.default.removeItem(at: $0) }
    }
    // Unless a damaged manifest was kept aside: its files may be what it named.
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
    // ".jpg" kept for older files; ImageIO reads HEIC or JPEG by their bytes.
    return directory.appending(path: safe + (kind == .photo ? ".jpg" : ".mp4"))
  }

  /// Serial writes; migration and sign-out drain the queue before their synchronous work. Protection until first
  /// unlock, so a Yo that comes while locked is still noted (the media files have complete protection).
  private static let writes = DispatchQueue(label: "yo.received.manifest", qos: .utility)
  static func drainWrites() { writes.sync {} }

  /// False when it did not write (or was never read).
  private func store() async -> Bool {
    guard loaded, let key = Diagnostics.attempt({ try ManifestSeal.key() }).flatMap({ $0 }) else { return false }
    let snapshot = Manifest(pending: pending, saved: saved)
    let url = manifest
    return await withCheckedContinuation { continuation in
      Self.writes.async {
        let written = Perf.timed("media.manifest.save") {
          _ = MediaStorage.directory("Received")  // recreate after sign-out, off the main actor
          guard let json = Diagnostics.attempt({ try JSONEncoder().encode(snapshot) }),
            let data = ManifestSeal.seal(json, key: key)
          else { return false }
          return ManifestSeal.write(data, to: url)
        }
        continuation.resume(returning: written)
      }
    }
  }

  private func storeNow() -> Bool {
    Perf.timed("media.manifest.migrate") {
      Self.drainWrites()
      guard loaded, let json = try? JSONEncoder().encode(Manifest(pending: pending, saved: saved)),
        let data = ManifestSeal.seal(json)
      else { return false }
      return ManifestSeal.write(data, to: manifest)
    }
  }

  private var expecting: [String: Task<Bool, Never>] = [:]

  /// Each note is durable before its event is acknowledged; ids not written stay with the core (`Inbox.take`).
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

  /// A waiting retry tries now: the next Yo came, so the network is back.
  func fetchWaiting(
    client: YoClient, arrived: @escaping @MainActor (Arrival) -> Void,
    dropped: @escaping @MainActor (String) -> Void = { _ in }
  ) {
    retryAcks(client: client)
    self.client = client
    self.arrived = arrived
    self.dropped = dropped
    for one in pending where fetching.contains(one.id) { pauses[one.id]?.cancel() }
    startMore()
  }

  private var acknowledging: Task<Void, Never>?
  private var ackRevision = 0

  private func retryAcks(client: YoClient) {
    ackRevision += 1
    guard acknowledging == nil else { return }
    let generation = generation
    acknowledging = Task {
      while generation == self.generation && !Task.isCancelled {
        let revision = ackRevision
        let pending = (try? await Hub.run { Prefetch.retryAcks(client: client) }) ?? true
        if revision != ackRevision { continue }  // a new file arrived during the scan
        if !pending { break }
        try? await Task.sleep(for: .seconds(30))
      }
      if generation == self.generation { acknowledging = nil }
    }
  }

  private func startMore() {
    Perf.timed("media.fetch.start") {
      guard let client, let arrived, !HubTransport.unsupported, load() else { return }
      // While a download is not written (full disk) only what is held in memory is retried: bounded memory, spared
      // GET budget.
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

  /// Seconds before each try; then it rests (`rest`).
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
      var data = unwritten[one.id] ?? Prefetch.take(media)  // the notification service may have fetched it
      // Locked, or the service still fetching: wait for its file to spare the GET budget.
      if data == nil && (Prefetch.has(media) || Prefetch.underWay(media)) { continue }
      // Held until written and noted: given back earlier, the service could fetch it again.
      var claimed: String?
      defer { if let claimed { Prefetch.unclaim(media, token: claimed) } }
      if data == nil {
        // Its file may have arrived just before the claim.
        guard let token = Prefetch.claim(media, again: true) else { continue }
        claimed = token
        data = Prefetch.take(media)
        do {
          if data == nil { data = try await Hub.run { try client.fetchMedia(media: media) } }
          // "No longer there": the service's file may have been written meanwhile.
          if data == nil { data = Prefetch.take(media) }
          if data == nil && Prefetch.has(media) { continue }
        } catch YoError.Refused(let status) where status == 426 {
          return  // this build too old: stays noted
        } catch let error as YoError where retryLater(error: error) {
          continue
        } catch {
          data = nil
        }
        guard generation == self.generation else { return }
      }
      guard let data else {
        // Gone at the hub or does not open: its Yo came without it.
        Prefetch.done(media)
        pending.removeAll { $0.id == one.id }
        resting[one.id] = nil
        unwanted.remove(one.id)
        _ = await store()
        if generation == self.generation, let dropped { await dropped(one.id) }
        return
      }
      // Kept meanwhile from the same blob in another conversation (below).
      guard !saved.contains(where: { $0.id == one.id }) else { return }
      // The file, then the manifest (`load` keeps a written file that is only noted).
      let url = file(one.id, one.kind)
      guard Prefetch.prepareAck(media, file: url) else { unwritten[one.id] = data; continue }
      let written = await MediaStorage.write(data, to: url, .completeFileProtection)
      // Signed out meanwhile: not even kept in memory (it would hold back the next account's downloads).
      guard generation == self.generation else {
        if written { try? FileManager.default.removeItem(at: url) }
        return
      }
      guard written else {
        unwritten[one.id] = data
        continue
      }
      Prefetch.stored(media)
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
      retryAcks(client: client)
      Prefetch.done(media)
      if unwanted.remove(one.id) != nil {
        forget(one.id)  // swiped away while it downloaded
      } else if let arrival = await arrival(kept) {
        if generation == self.generation { await arrived(arrival) }
      } else if generation == self.generation, let dropped {
        await dropped(one.id)  // nothing to show: not left loading on its bar
      }
      // The same blob waiting in other conversations (a photo to two groups) comes from this one fetch.
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
        if unwanted.remove(other.id) != nil {
          forget(other.id)
        } else if let shown = await self.arrival(also), generation == self.generation {
          await arrived(shown)
        }
      }
      return
    }
    rest(one.id)
  }

  /// A minute, doubling to half an hour; the others go meanwhile.
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

  /// After a restart, oldest first; waits for an unlock.
  func waiting() async -> [Arrival] {
    while !(await MainActor.run { UIApplication.shared.isProtectedDataAvailable }) || !load() {
      try? await Task.sleep(for: .seconds(5))
    }
    let generation = generation
    var arrivals: [Arrival] = []
    for one in saved {
      if let arrival = await arrival(one) { arrivals.append(arrival) }
      guard generation == self.generation else { return [] }
    }
    // Noted, not downloaded yet: shown loading.
    arrivals += pending.filter { !unwanted.contains($0.id) }.map { Arrival(id: $0.id, from: $0.from, pending: $0.kind) }
    return arrivals.sorted(by: Arrival.older)
  }

  /// Shown: never again, even if the app ends before the viewer closes (the file goes then or at the next start).
  func consumed(_ id: String) {
    Perf.timed("media.consumed") {
      if pending.contains(where: { $0.id == id }) { unwanted.insert(id) }
      saved.removeAll { $0.id == id }
      storeUntilWritten()
    }
  }

  /// The file first (what makes it seen for good, see `load`), then the manifest.
  func forget(_ id: String) {
    Perf.timed("media.forget") {
      if pending.contains(where: { $0.id == id }) { unwanted.insert(id) }
      for kind in [MediaFile.Kind.photo, .video] {
        try? FileManager.default.removeItem(at: file(id, kind))
      }
      saved.removeAll { $0.id == id }
      storeUntilWritten()
    }
  }

  /// Retries 1 s doubling to 30 s until it writes or the account signs out.
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
          continue  // changed during the write: needs a newer snapshot
        }
        try? await Task.sleep(for: .seconds(pause))
        pause = min(pause * 2, 30)
      }
      if generation == self.generation { rewriting = nil }
    }
  }

  /// Signed out (the files went with MediaStorage.clearAll).
  func forgetAll() {
    generation += 1
    expecting.values.forEach { $0.cancel() }
    expecting = [:]
    acknowledging?.cancel()
    acknowledging = nil
    pending = []
    saved = []
    loaded = true
    resting = [:]
    unwritten = [:]
    client = nil
    arrived = nil
    dropped = nil
    unwanted = []
    pauses.values.forEach { $0.cancel() }
    pauses = [:]
    rewriting?.cancel()
    rewriting = nil
  }

  /// The bar's thumbnail; the viewer reads the file itself.
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

  /// Without the whole image in memory.
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
