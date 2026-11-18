// Errors counted on the phone (CountedErrors): a push the notification extension could not read, a group Yo whose
// sender has no name ("?"). Each is an error for the Yo team: whoever sees it (the app, an extension) adds a line to
// a file in the App Group (an extension cannot count on the network, and before the first unlock since a restart
// only a file without protection is writable); the app takes them in on its next diagnostics upload, one report per
// kind and day with its count (a burst is one report, not hundreds). Code facts only: never a name, a user id or a
// group id (at most a short hash prefix of one).
//
// A push the extension could not read goes out as Signal's does: "You may have received a Yo" (the hub's alert says
// the same, for an extension that never ran), counted with its reason. Locked since a restart is expected (iOS keeps
// the keys until the first unlock): still counted, marked "expected" so the admin page can leave it out.
import CryptoKit
import Foundation
import YoCore
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

enum NotifyFallback {
  static let text = "You may have received a Yo"

  enum Reason: Error, Equatable, Sendable {
    /// `busy`: the app held the state longer than the extension may wait (its own send, an Action button's Yo).
    case lockedBeforeFirstUnlock, noAccount, network, busy, hubStatus(UInt16), decrypt, timeout, other

    var code: String {
      switch self {
      case .lockedBeforeFirstUnlock: "lockedBeforeFirstUnlock"
      case .noAccount: "noAccount"
      case .network: "network"
      case .busy: "busy"
      case .hubStatus(let status): "hubStatus/\(status)"
      case .decrypt: "decrypt"
      case .timeout: "timeout"
      case .other: "other"
      }
    }
  }

  /// The extension: a push that went out unread, and why.
  static func record(_ reason: Reason) {
    CountedErrors.record(domain: "NotificationFallback", place: "NotificationService", code: reason.code,
      expected: reason == .lockedBeforeFirstUnlock)
  }
}

/// A sender shown without a name ("?", "Someone"): where, and what was known of them, to find why.
enum UnknownSender {
  /// `place`: "list" (the group's bar in the app), "notification" (a notification's text, app or extension).
  static func record(place: String, user: String, contact: YoContactFacts?, group: (members: Int, listed: Bool?)?,
    friend: Bool, groupsRead: Bool? = nil) {
    // The facts in the code: each case its own row on the admin page; who (a hash prefix) in the details.
    var facts: [String] = []
    if let contact {
      facts += ["contact=yes", "handles=\(contact.handles)", "named=\(contact.named ? "yes" : "no")",
        "gone=\(contact.gone ? "yes" : "no")", "accepted=\(contact.accepted ? "yes" : "no")"]
    } else {
      facts.append("contact=no")
    }
    if let group {
      facts += ["members=\(group.members)", "inRoster=\(group.listed.map { $0 ? "yes" : "no" } ?? "unknown")"]
    }
    if let groupsRead { facts.append("groupsRead=\(groupsRead ? "yes" : "no")") }
    facts.append("friend=\(friend ? "yes" : "no")")
    CountedErrors.record(domain: "UnknownSender", place: place,
      code: ([group == nil ? "unknownSender" : "unknownGroupSender"] + facts).joined(separator: " "),
      details: "user=" + CountedErrors.hashPrefix(user))
  }

  /// What a contact entry says, without who it is.
  struct YoContactFacts {
    let handles: Int, named: Bool, gone: Bool, accepted: Bool
  }

  static func facts(_ contact: YoContact) -> YoContactFacts {
    YoContactFacts(handles: contact.handles.count, named: !(contact.name ?? "").isEmpty, gone: contact.gone,
      accepted: contact.accepted)
  }
}

enum CountedErrors {
  private static var file: URL? { ShareBridge.container?.appending(path: "counted-errors") }
  private static var taking: URL? { ShareBridge.container?.appending(path: "counted-errors.taking") }

  /// A short hash prefix of an id: the same id is the same prefix, and it names nobody.
  static func hashPrefix(_ id: String) -> String {
    SHA256.hash(data: Data(id.utf8)).prefix(3).map { String(format: "%02x", $0) }.joined()
  }

  /// One line "day domain|place|code|expected|details", appended in one write (O_APPEND: a line from another process
  /// never interleaves). The file has no protection, so this works before the first unlock too. Without the App
  /// Group (Yo Dev from dev/push.sh) nothing is counted.
  static func record(domain: String, place: String, code: String, expected: Bool = false, details: String = "") {
    guard let file else { return }
    ensure(file)
    let clean = { (text: String) in String(text.map { $0 == "|" || $0.isNewline ? " " : $0 }.prefix(300)) }
    let line = "\(Int(Date.now.timeIntervalSince1970 / 86_400)) "
      + [domain, place, code, expected ? "1" : "", details].map(clean).joined(separator: "|") + "\n"
    let fd = open(file.path, O_WRONLY | O_APPEND)
    guard fd >= 0 else { return }
    defer { close(fd) }
    _ = line.withCString { write(fd, $0, strlen($0)) }
  }

  /// The file, there with no protection (a new file takes the container's, unreadable before the first unlock).
  private static func ensure(_ file: URL) {
    guard !FileManager.default.fileExists(atPath: file.path) else { return }
    FileManager.default.createFile(atPath: file.path, contents: nil, attributes: [.protectionKey: FileProtectionType.none])
  }

  /// The app: what was counted into the diagnostics queue, one report per kind and day; the lines go only once they
  /// are queued (a crash in between leaves `taking` for the next time).
  static func collect() {
    guard let file, let taking else { return }
    let files = FileManager.default
    if !files.fileExists(atPath: taking.path) {
      guard files.fileExists(atPath: file.path), (try? files.moveItem(at: file, to: taking)) != nil else { return }
      ensure(file)
    }
    guard let data = try? Data(contentsOf: taking) else { return }
    var counts: [Int: [String: Int]] = [:]
    for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
      let parts = line.split(separator: " ", maxSplits: 1)
      guard parts.count == 2, let day = Int(parts[0]) else { continue }
      counts[day, default: [:]][String(parts[1]), default: 0] += 1
    }
    let epoch = UserDefaults.standard.string(forKey: "diagnostics.generation")
    let format = Date.ISO8601FormatStyle().year().month().day()
    for (day, kinds) in counts.sorted(by: { $0.key < $1.key }) {
      let date = Date(timeIntervalSince1970: Double(day) * 86_400).formatted(format)
      for (key, count) in kinds.sorted(by: { $0.key < $1.key }) {
        let fields = key.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
        guard fields.count == 5, !fields[0].isEmpty, !fields[1].isEmpty, !fields[2].isEmpty else { continue }
        var payload = ["uuid": UUID().uuidString, "domain": fields[0], "place": fields[1], "code": fields[2],
          "count": String(min(count, 100_000)),
          "details": "\(date): \(count)" + (fields[4].isEmpty ? "" : " · " + fields[4]),
          "app": AppUpdate.header, "os": ProcessInfo.processInfo.operatingSystemVersionString, "model": Diagnostics.model]
        if fields[3] == "1" { payload["severity"] = "expected" }
        guard let json = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]) else { continue }
        Diagnostics.append(String(decoding: json, as: UTF8.self), kind: "error", epoch: epoch)
      }
    }
    try? files.removeItem(at: taking)
  }
}
