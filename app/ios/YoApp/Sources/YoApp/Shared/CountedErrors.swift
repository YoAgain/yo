// Errors counted on the phone by any process into a file in the App Group (an extension cannot count on the network,
// and before the first unlock only an unprotected file is writable); the app uploads one report per kind and day.
// Code facts only: never a name or id (at most a short hash prefix).
import CryptoKit
import Foundation
import YoCore
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

enum NotifyFallback {
  static let text = productText(text: .notificationFallback, lang: .english)

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

  /// Locked since a restart is expected: still counted, marked so the admin page can leave it out.
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
    // Facts in the code (one admin row per case); who, as a hash prefix, in the details.
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
  private static var folder: URL? {
    ShareBridge.container ?? FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
  }
  private static var file: URL? { folder?.appending(path: "counted-errors") }
  private static var taking: URL? { folder?.appending(path: "counted-errors.taking") }

  static func hashPrefix(_ id: String) -> String {
    SHA256.hash(data: Data(id.utf8)).prefix(3).map { String(format: "%02x", $0) }.joined()
  }

  /// One line "day domain|place|code|expected|details" in one O_APPEND write, so processes never interleave.
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

  /// A new file would take the container's protection, unwritable before the first unlock.
  private static func ensure(_ file: URL) {
    guard !FileManager.default.fileExists(atPath: file.path) else { return }
    FileManager.default.createFile(atPath: file.path, contents: nil, attributes: [.protectionKey: FileProtectionType.none])
  }

  /// Into the diagnostics queue; a crash in between leaves `taking` for the next time.
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
