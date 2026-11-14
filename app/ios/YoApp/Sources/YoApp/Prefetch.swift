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
      switch kind {
      case .photo(let photo): return photo
      case .video(let video, _): return video
      default: return nil
      }
    }
    guard !media.isEmpty else { return }
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    for one in media where one.size <= largest {
      guard Date.now < deadline else { return }
      guard let file = file(one), let mark = mark(one), !FileManager.default.fileExists(atPath: file.path) else {
        continue
      }
      // The mark first: from here on the app does not give the photo up when the hub says it is gone.
      guard FileManager.default.createFile(
        atPath: mark.path, contents: nil, attributes: [.protectionKey: FileProtectionType.completeUnlessOpen]) else { continue }
      defer { try? FileManager.default.removeItem(at: mark) }
      guard let data = try? client.fetchMedia(media: one) else { continue }
      try? data.write(to: file, options: [.atomic, .completeFileProtectionUnlessOpen])
    }
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
