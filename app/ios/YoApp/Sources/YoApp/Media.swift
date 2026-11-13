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

/// A photo or video ready to be sealed: its bytes and what the Yo says about it.
struct MediaFile: Sendable {
  enum Kind: String, Codable, Sendable { case photo, video }

  let kind: Kind
  let data: Data
  let width: Int
  let height: Int
  let durationMs: UInt32

  static let longestSide = 1600
  static let quality = 0.7
  static let longestVideo = 10.0
  static let largestVideo: Int64 = 8_000_000

  /// A photo as it goes: a JPEG at most `longestSide` pixels on its longest side, upright, and nothing but its
  /// pixels (no place, no camera, no date).
  static func photo(_ data: Data) -> MediaFile? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,  // upright, as it was taken
      kCGImageSourceThumbnailMaxPixelSize: longestSide,
      kCGImageSourceShouldCacheImmediately: true,
    ]
    guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
    let out = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(out, UTType.jpeg.identifier as CFString, 1, nil)
    else { return nil }
    // Only the quality: no properties, so no metadata.
    CGImageDestinationAddImage(
      destination, image, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
    guard CGImageDestinationFinalize(destination) else { return nil }
    return MediaFile(kind: .photo, data: out as Data, width: image.width, height: image.height, durationMs: 0)
  }

  /// A video as it goes: an MP4 of at most `longestVideo` seconds and `largestVideo` bytes, 960 by 540 at most,
  /// without metadata. The recording is deleted.
  static func video(_ recording: URL) async -> MediaFile? {
    defer { try? FileManager.default.removeItem(at: recording) }
    let asset = AVURLAsset(url: recording)
    guard let export = AVAssetExportSession(asset: asset, presetName: AVAssetExportPreset960x540),
      let duration = try? await asset.load(.duration)
    else { return nil }
    let length = CMTimeMinimum(duration, CMTime(seconds: longestVideo, preferredTimescale: 600))
    export.timeRange = CMTimeRange(start: .zero, duration: length)
    export.metadata = []
    export.metadataItemFilter = .forSharing()  // no place, no device
    export.fileLengthLimit = largestVideo
    export.shouldOptimizeForNetworkUse = true
    let out = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mp4")
    defer { try? FileManager.default.removeItem(at: out) }
    do {
      try await export.export(to: out, as: .mp4)
    } catch {
      return nil
    }
    guard let data = try? Data(contentsOf: out), !data.isEmpty, data.count <= largestVideo + 1_000_000 else {
      return nil
    }
    var size = CGSize(width: 540, height: 960)
    if let track = try? await AVURLAsset(url: out).loadTracks(withMediaType: .video).first,
      let (natural, transform) = try? await track.load(.naturalSize, .preferredTransform)
    {
      let turned = natural.applying(transform)
      size = CGSize(width: abs(turned.width), height: abs(turned.height))
    }
    let ms = UInt32(max(0, min(length.seconds, longestVideo)) * 1000)
    return MediaFile(kind: .video, data: data, width: Int(size.width), height: Int(size.height), durationMs: ms)
  }

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
/// that quits, a hub that says "not now", no network, none of it loses one. In the order they were taken; one at a
/// time, while Yos go on beside them (the upload does not hold the core).
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
  /// Awaited before the next core call, which lets the core forget them.
  @ObservationIgnored var received: (@MainActor ([YoEvent], YoClient) async -> Void)?
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

  /// Sends what is due, one after the other; what the hub refuses for now waits its turn again (30 seconds, doubling
  /// to half an hour). Also when the app comes to the foreground.
  func flush(accounts: AccountStore) {
    self.accounts = accounts
    guard !running else { return }
    running = true
    Task {
      defer { running = false }
      while let client = accounts.client, let index = items.firstIndex(where: { $0.next <= .now }) {
        let item = items[index]
        let outcome = await deliver(item, client: client)
        guard let at = items.firstIndex(where: { $0.id == item.id }) else { continue }
        switch outcome {
        case .sent, .dropped:
          items.remove(at: at)
          let blob = Self.blob(item.id)
          Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: blob) }
        case .later:
          items[at].tries += 1
          items[at].next = .now + min(1800, 30 * pow(2, Double(min(items[at].tries - 1, 6))))
        case .replaced:
          accounts.reset()
          return save()
        }
        save()
      }
      scheduleWake()
    }
  }

  private func scheduleWake() {
    wake?.cancel()
    guard let next = items.map(\.next).min() else { return }
    wake = Task { [weak self] in
      try? await Task.sleep(for: .seconds(max(1, next.timeIntervalSinceNow)))
      guard !Task.isCancelled, let self, let accounts = self.accounts else { return }
      self.flush(accounts: accounts)
    }
  }

  private enum Outcome { case sent, later, dropped, replaced }

  private func deliver(_ item: Item, client: YoClient) async -> Outcome {
    if Date.now.timeIntervalSince(item.created) > Self.givenUpAfter { return .dropped }
    let media = item.media.media
    let blob = Self.blob(item.id)
    do {
      // Another account's (signed out meanwhile): never sent under this one.
      guard try await Hub.run({ client.userId() }) == item.owner else { return .dropped }
      // The chat first (a new friend's opens now), then the upload outside the client's lock: Yos go on meanwhile.
      // The Yo only after the hub has the blob.
      let chat = try await Hub.run { try client.mediaChat(to: item.to) }
      await received?(chat.events, client)
      try await Hub.run {
        try client.uploadMedia(media: media, ciphertext: try Data(contentsOf: blob), fetches: chat.fetches)
      }
      let events = try await Hub.run {
        switch item.kind {
        case .photo: try client.sendPhoto(to: item.to, media: media)
        case .video: try client.sendVideo(to: item.to, media: media, durationMs: item.durationMs)
        }
      }
      await received?(events, client)
      return .sent
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
/// Yo, then fetched (each fetch counts at the hub, the last deletes the blob), opened and kept here with complete
/// file protection; seen, it is deleted. A fetch the network breaks is tried again; one the hub no longer has is
/// dropped. All of it on the main actor but the reading, writing and fetching.
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

  private var directory: URL { MediaStorage.directory("Received") }
  private var pendingIndex: URL { directory.appending(path: "pending.json") }
  private var savedIndex: URL { directory.appending(path: "received.json") }

  private init() {
    pending = (try? Data(contentsOf: pendingIndex)).flatMap { try? JSONDecoder().decode([Pending].self, from: $0) } ?? []
    saved = (try? Data(contentsOf: savedIndex)).flatMap { try? JSONDecoder().decode([Saved].self, from: $0) } ?? []
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

  /// Fetches what is noted and not under way; each one kept goes to `arrived`.
  func fetchWaiting(client: YoClient, arrived: @escaping @MainActor (Arrival) -> Void) {
    for one in pending where fetching.insert(one.id).inserted {
      Task { await fetch(one, client: client, arrived: arrived) }
    }
  }

  private func fetch(_ one: Pending, client: YoClient, arrived: @MainActor (Arrival) -> Void) async {
    defer { fetching.remove(one.id) }
    let media = one.media.media
    for wait in [0, 3, 15, 60] {
      if wait > 0 { try? await Task.sleep(for: .seconds(wait)) }
      let data: Data?
      do {
        data = try await Hub.run { try client.fetchMedia(media: media) }
      } catch let error as YoError where mediaRetryLater(error: error) {
        continue  // noted still: tried again now, or with the next Yo or opening
      } catch {
        data = nil  // does not open: nothing to keep
      }
      if let data, await MediaStorage.write(data, to: file(one.id, one.kind), .completeFileProtection) {
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

  /// Seen: gone from this iPhone.
  func forget(_ id: String) {
    for gone in saved where gone.id == id {
      let url = file(gone.id, gone.kind)
      Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: url) }
    }
    saved.removeAll { $0.id == id }
    store()
  }

  /// Signed out: nothing kept, nothing noted.
  func forgetAll() {
    pending = []
    saved = []
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
