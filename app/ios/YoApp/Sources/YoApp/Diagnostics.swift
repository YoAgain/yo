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
      switch yo { case .Network, .Refused: return; default: break }
    }
    record(error, place: place, file: file, line: line)
  }

  static func failure<E: Error>(_ error: E, file: String = #fileID, line: Int = #line, place: String = #function) -> E {
    record(error, place: place, file: file, line: line)
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
    if let yo = error as? YoError, case .Refused(let status) = yo {
      code = "Refused/\(status)"
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
      var machine = utsname()
      uname(&machine)
      let model = withUnsafeBytes(of: machine.machine) { bytes in
        String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self)
      }
      let payload = ["uuid": uuid, "domain": domain, "code": code, "place": tag,
        "app": AppUpdate.header, "os": ProcessInfo.processInfo.operatingSystemVersionString, "model": model]
      guard let data = try? JSONSerialization.data(withJSONObject: payload), let text = String(data: data, encoding: .utf8) else { return }
      append(text, kind: "error", epoch: epoch)
      send(epoch: epoch)
    }
  }

  static func flush() {
    let epoch = UserDefaults.standard.string(forKey: generation)
    queue.async { send(epoch: epoch) }
  }

  private static func send(epoch: String?) {
    while let text = pending("error").first {
      guard UserDefaults.standard.bool(forKey: "crashReports.on"),
        UserDefaults.standard.string(forKey: generation) == epoch else { return }
      let answer = Hub.transport.request(method: "POST", path: "/v1/errors", headers: [], body: Data(text.utf8))
      guard answer.status == 204 else {
        queue.asyncAfter(deadline: .now() + 30) { send(epoch: epoch) }
        return
      }
      markSent(text, epoch: epoch)
    }
  }
}
