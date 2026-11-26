// Opt-in diagnostics contain code metadata only. Never serialize an error's description or userInfo.
import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif
import YoCore

/// The one place that decides which network failures and hub answers are expected, never reported as problems: the
/// phone offline or timing out, a request cancelled, the phone's clock, the update the hub asks for (426), a limit,
/// the device replaced, a signed request's first 401 (the core signs again and retries), and answers the core takes as
/// protocol outcomes. A problem, reported: a 401 left after that retry, the hub failing (5xx), a broken TLS handshake
/// (-1200), a storage failure, a message that does not decrypt.
/// Android mirrors it (CrashReports.noise).
enum Noise {
  static func status(_ status: UInt16, method: String, route: String, signed: Bool) -> Bool {
    if signed && status == 401 { return true }
    // A 410: this device was replaced, which the app shows. 426: the app asks for the update. A limit, or no room
    // now (inbox, blobs or disk): try later.
    if [410, 426, 429, 507].contains(status) { return true }
    switch method {
    case "GET":
      // A username checked while typed, no storage yet, or no backup behind the chosen passkey or key.
      if route.hasPrefix("/v1/username/") { return status == 400 || status == 409 }
      if route == "/v1/storage" || route.hasPrefix("/v1/backup/") { return status == 404 }
      fallthrough
    case "DELETE":
      // A photo or video no longer there: the core says unavailable.
      return route.hasPrefix("/v1/blobs/") && status == 404
    case "PUT":
      // The core merges newer storage, or takes a blob stored by a try whose answer was lost as stored.
      return (route == "/v1/storage" || route.hasPrefix("/v1/blobs/")) && status == 409
    case "POST":
      // No such name or no commitment told: invited instead.
      if route == "/v1/lookup" || route == "/v1/group-credential" { return status == 404 }
      // A name too short or taken at sign-up: the person picks another.
      if route == "/v1/register" { return status == 400 || status == 409 }
      // The account was deleted ("Not on Yo") or its device changed (send again to the one named).
      if route.hasPrefix("/v1/inbox/"), route != "/v1/inbox/ack" { return status == 404 || status == 409 }
      // Removed from the group or it ended (403, 404), or another member changed it (apply to that revision).
      return route.hasPrefix("/v1/groups/") && [403, 404, 409].contains(status)
    default: return false
    }
  }

  // A "bad URL" for the fixed hub address can come from a proxy or VPN. A broken TLS handshake is reported: the
  // certificate or the proxy in front of the hub.
  private static let offline: Set<Int> = [
    NSURLErrorBadURL, NSURLErrorTimedOut, NSURLErrorCannotFindHost, NSURLErrorCannotConnectToHost,
    NSURLErrorNetworkConnectionLost, NSURLErrorDNSLookupFailed, NSURLErrorNotConnectedToInternet,
    NSURLErrorInternationalRoamingOff, NSURLErrorDataNotAllowed,
  ]

  static func urlError(domain: String, code: Int, badDates: Bool) -> Bool {
    domain == NSURLErrorDomain && (code == NSURLErrorCancelled || offline.contains(code) || badDates)
  }

  static func fallback(_ code: String) -> Bool {
    ["network", "hubStatus/426", "hubStatus/429"].contains(code)
  }
}

enum Diagnostics {
  private struct LegacyCrash: Codable { let id: UUID; let date: Date; let text: String }

  private static let queue = DispatchQueue(label: "app.justyo.diagnostics", qos: .utility)
  private static let pendingKey = "diagnostics.reports"
  // Recursive: a write to UserDefaults can wake a SwiftUI view that reads here again on the same thread (a plain lock
  // deadlocked the app on Status, killed by the watchdog).
  private static let storage = NSRecursiveLock()
  private static let generation = "diagnostics.generation"

  // The transport handles HTTP and network diagnostics before the core retries. Keychain records at creation.
  static func recordUnreported(_ error: Error, file: String = #fileID, line: Int = #line, place: String = #function) {
    if error is Keychain.Failure { return }
    if let yo = error as? YoError {
      switch yo {
      // The 401 left after the core signed again and retried: a problem (Android reports it the same way).
      case .Refused(401): break
      case .Network, .Refused: return
      case .Replaced:
        // Another iPhone signed in to the account: every caller signs out and shows it; no change of the app helps.
        CountedErrors.record(domain: String(reflecting: YoError.self), place: "\(file):\(line) \(place)",
          code: "Replaced", expected: true)
        return
      case .Save(let message) where ["Busy", "TurnEnded"].contains(saveKind(message)):
        // iOS gave no background time for the database ("Busy") or took the turn back meanwhile: the receive loop's
        // save as the app left the front. Nothing was written; the step runs again in front. The database's own
        // failures ("Io/5386", "Full", "Stale", ...) stay problems.
        CountedErrors.record(domain: String(reflecting: YoError.self), place: "\(file):\(line) \(place)",
          code: name(yo), expected: true)
        return
      case .NotOnYo, .Blocked, .GroupEmpty, .Clock:
        // An answer the person is shown ("Not on Yo", blocked, the group empty, the phone's date and time): about
        // them, not a fault of the app.
        CountedErrors.record(domain: String(reflecting: YoError.self), place: "\(file):\(line) \(place)",
          code: name(yo), expected: true)
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
    case .Clock: "Clock"
    case .NotOnYo: "NotOnYo"
    case .Blocked: "Blocked"
    case .GroupEmpty: "GroupEmpty"
    case .Outdated: "Outdated"
    case .Unconfirmed: "Unconfirmed"
    case .Replaced: "Replaced"
    case .Protocol(let message):
      "Protocol/" + (message.hasPrefix("session") ? "session" : String(message.prefix(60)))
    case .State: "State"
    case .Save(let message): "Save/" + saveKind(message)
    }
  }

  /// A save's failure by the core's fixed words only (statedb: its class, SQLite's extended code for an I/O or other
  /// failure, "Io/5386"), never another text.
  static func saveKind(_ message: String) -> String {
    if message == "the turn on the state ended" { return "TurnEnded" }  // statedb::TURN_ENDED
    if message == "an older state" { return "Stale" }  // statedb::STALE
    let parts = message.split(separator: "/", maxSplits: 1)
    let classes = ["Busy", "Full", "Io", "InvalidDatabase", "Constraint", "UnsupportedVersion", "Sqlite"]
    guard let first = parts.first, classes.contains(String(first)) else { return "Other" }
    guard parts.count == 2 else { return String(first) }
    return Int(parts[1]).map { "\(first)/\($0)" } ?? String(first)
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

/// The random install id help goes by (support thread, reports, crash and error reports; spec 4.2): made on first use,
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
