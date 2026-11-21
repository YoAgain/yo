// Opt-in diagnostics contain code metadata only. Never serialize an error's description or userInfo.
import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif
import YoCore

enum Diagnostics {
  private struct LegacyCrash: Codable { let id: UUID; let date: Date; let text: String }

  private static let queue = DispatchQueue(label: "app.justyo.diagnostics", qos: .utility)
  private static let pendingKey = "diagnostics.reports"
  // Recursive: a write to UserDefaults can wake a SwiftUI view that reads here again on the same thread (a plain lock
  // deadlocked the app on Status, killed by the watchdog).
  private static let storage = NSRecursiveLock()
  private static let generation = "diagnostics.generation"

  // HTTP/network failures are recorded even when the core retries internally. Keychain records at creation.
  static func recordUnreported(_ error: Error, file: String = #fileID, line: Int = #line, place: String = #function) {
    if error is Keychain.Failure { return }
    if let yo = error as? YoError {
      switch yo {
      case .Network, .Refused: return
      case .Replaced:
        // Another iPhone signed in to the account: every caller signs out and shows it; no change of the app helps.
        CountedErrors.record(domain: String(reflecting: YoError.self), place: "\(file):\(line) \(place)",
          code: "Replaced", expected: true)
        return
      case .Save:
        // The state's turn not had or ended (another process, background time over): the step runs again. A failure
        // of the database itself is counted by the state (StateFailureCounts).
        CountedErrors.record(domain: String(reflecting: YoError.self), place: "\(file):\(line) \(place)", code: "Save",
          expected: true)
        return
      default: break
      }
    }
    record(error, place: place, file: file, line: line)
  }

  static func failure<E: Error>(_ error: E, file: String = #fileID, line: Int = #line, place: String = #function) -> E {
    record(error, place: place, file: file, line: line)
    return error
  }

  /// Counted as expected, not a problem: an answer the person is shown that no change of the app makes go away (their
  /// password manager without PRF, no backup behind the passkey or key they chose).
  static func expected<E: Error>(_ error: E, file: String = #fileID, line: Int = #line, place: String = #function) -> E {
    CountedErrors.record(domain: String(reflecting: type(of: error)), place: "\(file):\(line) \(place)",
      code: String((error as NSError).code), expected: true)
    return error
  }

  // Use at optional local I/O boundaries, so a swallowed failure is still observable.
  @discardableResult
  static func attempt<T>(file: String = #fileID, line: Int = #line, place: String = #function, _ work: () throws -> T) -> T? {
    do { return try work() }
    catch {
      recordUnreported(error, file: file, line: line, place: place)
      return nil
    }
  }

  @discardableResult
  static func attempt<T>(file: String = #fileID, line: Int = #line, place: String = #function, isolation: isolated (any Actor)? = #isolation, _ work: () async throws -> T) async -> T? {
    do { return try await work() }
    catch {
      recordUnreported(error, file: file, line: line, place: place)
      return nil
    }
  }

  // One private, chronological queue for both kinds; consent controls upload only.
  private static func reports() -> [[String: String]] {
    let defaults = UserDefaults.standard
    let stored = defaults.array(forKey: pendingKey) as? [[String: String]] ?? []
    var rows = stored
    for (key, kind) in [("diagnostics.pending", "error"), ("crashReports.pending", "crash")] {
      if let old = defaults.stringArray(forKey: key) {
        rows += old.map { ["kind": kind, "text": $0] }
        defaults.removeObject(forKey: key)
      }
    }
    // Adopt the former Status history into this same retained store once.
    if let data = defaults.data(forKey: "crashReports.history"),
      let history = try? JSONDecoder().decode([LegacyCrash].self, from: data) {
      for record in history where !rows.contains(where: { $0["text"] == record.text }) {
        rows.append(["kind": "crash", "text": record.text, "id": record.id.uuidString,
          "at": String(record.date.timeIntervalSince1970), "sent": "1"])
      }
      defaults.removeObject(forKey: "crashReports.history")
    }
    rows = rows.map { row in
      var row = row
      if row["id"] == nil { row["id"] = UUID().uuidString }
      if row["at"] == nil { row["at"] = String(Date().timeIntervalSince1970) }
      return row
    }
    rows.sort { (Double($0["at"] ?? "") ?? 0) < (Double($1["at"] ?? "") ?? 0) }
    rows = Array(rows.suffix(500))
    // Written only when something changed: Status reads this in its body, and a write there (UserDefaults change,
    // @AppStorage) asked for the body again, endlessly: the app hung.
    if rows != stored { defaults.set(rows, forKey: pendingKey) }
    return rows
  }

  static func retainedCrashes() -> [[String: String]] {
    storage.withLock { reports().filter { $0["kind"] == "crash" } }
  }

  static func pending(_ kind: String) -> [String] {
    storage.withLock { reports().filter { $0["kind"] == kind && $0["sent"] != "1" }.compactMap { $0["text"] } }
  }

  static func append(_ text: String, kind: String, epoch: String?) {
    storage.withLock {
      guard UserDefaults.standard.string(forKey: generation) == epoch else { return }
      var rows = reports()
      rows.append(["kind": kind, "text": text, "id": UUID().uuidString, "at": String(Date().timeIntervalSince1970)])
      UserDefaults.standard.set(Array(rows.suffix(500)), forKey: pendingKey)
    }
  }

  static func markSent(_ text: String, epoch: String?) {
    storage.withLock {
      guard UserDefaults.standard.string(forKey: generation) == epoch else { return }
      var rows = reports()
      for i in rows.indices where rows[i]["text"] == text { rows[i]["sent"] = "1" }
      UserDefaults.standard.set(rows, forKey: pendingKey)
    }
  }

  static func clear() {
    storage.withLock {
      UserDefaults.standard.set(UUID().uuidString, forKey: generation)
      for key in [pendingKey, "diagnostics.pending", "crashReports.pending", "crashReports.seen", "crashReports.history"] {
        UserDefaults.standard.removeObject(forKey: key)
      }
    }
  }

  static func record(_ error: Error, place: String = #function, file: String = #fileID, line: Int = #line) {
    // Reflection's enum case name only, never its associated values (which can contain private data).
    let mirror = Mirror(reflecting: error)
    let domain = mirror.displayStyle == .enum ? String(reflecting: type(of: error)) : (error as NSError).domain
    let code: String
    if let yo = error as? YoError {
      code = name(yo)
    } else if mirror.displayStyle == .enum {
      code = mirror.children.first?.label ?? String((error as NSError).code)
    } else {
      code = String((error as NSError).code)
    }
    let tag = "\(file):\(line) \(place)"
    let epoch = UserDefaults.standard.string(forKey: generation)
    let uuid = UUID().uuidString
    queue.async {
      guard UserDefaults.standard.string(forKey: generation) == epoch else { return }
      let payload = ["uuid": uuid, "domain": domain, "code": code, "place": tag,
        "app": AppUpdate.header, "os": ProcessInfo.processInfo.operatingSystemVersionString, "model": model]
      guard let data = try? JSONSerialization.data(withJSONObject: payload), let text = String(data: data, encoding: .utf8) else { return }
      append(text, kind: "error", epoch: epoch)
      send(epoch: epoch)
    }
  }

  /// A core error's case by name, never a number (a case without values had only its NSError code). Protocol carries
  /// its reason: the core's fixed texts; a session's libsignal text (which can name an address) only as "session".
  static func name(_ error: YoError) -> String {
    switch error {
    case .Network: "Network"
    case .Refused(let status): "Refused/\(status)"
    case .NotOnYo: "NotOnYo"
    case .Blocked: "Blocked"
    case .GroupEmpty: "GroupEmpty"
    case .Outdated: "Outdated"
    case .Unconfirmed: "Unconfirmed"
    case .Replaced: "Replaced"
    case .Protocol(let message):
      "Protocol/" + (message.hasPrefix("session") ? "session" : String(message.prefix(60)))
    case .State: "State"
    case .Save: "Save"
    }
  }

  /// The hardware model: "iPhone17,1".
  static var model: String {
    var machine = utsname()
    uname(&machine)
    return withUnsafeBytes(of: machine.machine) { bytes in
      String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self)
    }
  }

  /// Anonymous crash and error reports: on unless switched off (Status › Send Crashes); one setting for both senders.
  /// The app's own choice first; the extensions read the copy in the App Group.
  static var reportsOn: Bool {
    UserDefaults.standard.object(forKey: onKey) as? Bool ?? InstallID.defaults.object(forKey: onKey) as? Bool ?? true
  }
  private static let onKey = "crashReports.on"

  /// Off forgets the install id (the next help or report on makes a new one) and keeps reports on the phone.
  static func setReportsOn(_ on: Bool) {
    UserDefaults.standard.set(on, forKey: onKey)
    InstallID.defaults.set(on, forKey: onKey)
    if !on { InstallID.forget() }
  }

  /// The extensions learn a choice made before they read the App Group's copy.
  static func shareReportsChoice() {
    guard let on = UserDefaults.standard.object(forKey: onKey) as? Bool,
      InstallID.defaults.object(forKey: onKey) as? Bool != on else { return }
    InstallID.defaults.set(on, forKey: onKey)
  }

  /// One queued report (its JSON, as any earlier build queued it) to the hub by the install id: true when done with it
  /// (taken, or refused for good: a 4xx but 426 and 429), false to keep it for later (no answer, 426, 429, 5xx).
  static func upload(_ text: String, crash: Bool) -> Bool {
    // Plain text from the first builds the core sends as the same report in JSON.
    guard let status = try? helpDiagnostic(transport: Hub.transport, install: InstallID.current, crash: crash, report: text)
    else { return false }
    return status == 204 || (400..<500).contains(status) && status != 426 && status != 429
  }

  static func flush() {
    let epoch = UserDefaults.standard.string(forKey: generation)
    queue.async { send(epoch: epoch) }
  }

  private static func send(epoch: String?) {
    while let text = pending("error").first {
      guard reportsOn,
        UserDefaults.standard.string(forKey: generation) == epoch else { return }
      guard upload(text, crash: false) else {
        queue.asyncAfter(deadline: .now() + 30) { send(epoch: epoch) }
        return
      }
      markSent(text, epoch: epoch)
    }
  }
}

/// The random install id help goes by (support thread, reports, crash and error reports; spec 6.4): made on first use,
/// kept on this phone alone in the App Group (the app and its extensions), never sent with the account or put in its
/// storage. Send Crashes off forgets it.
enum InstallID {
  private static let key = "help.install"
  private static let lock = NSLock()
  /// The App Group's defaults; Yo Dev without the group keeps its own.
  static var defaults: UserDefaults { UserDefaults(suiteName: ShareBridge.group) ?? .standard }

  /// The id, made now if there is none.
  static var current: String {
    lock.withLock {
      if let id = defaults.string(forKey: key) { return id }
      let id = newInstallId()
      defaults.set(id, forKey: key)
      return id
    }
  }

  /// The id if there is one: reading a thread never makes one.
  static var existing: String? { lock.withLock { defaults.string(forKey: key) } }

  static func forget() { lock.withLock { defaults.removeObject(forKey: key) } }
}
