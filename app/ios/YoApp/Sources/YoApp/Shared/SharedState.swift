// The core's state shared by the app and its extensions in the App Group; a build without the group keeps its own
// (StateFile in Hub.swift). SQLite in WAL mode, opened by each process for itself.
// - Reads take the last committed state without any turn: never a wait for an extension's step.
// - Each step that may write takes the turn between processes (flock on `yo-state.lock`), takes in what another
//   process wrote, and writes only over the state it started from (compare and swap in `BEGIN IMMEDIATE`). Expiry
//   revokes the turn rather than waiting for the network; a revoked turn writes nothing more (`held`).
// - A connection lives for one access: no SQLite lock is held between them, so iOS never suspends a process with a
//   lock on a group file (0xdead10cc). Protected until first user authentication, never in a backup.
// - A failed write changes nothing: the core's step fails before anything that needed it leaves.
// - After a turn that wrote, a Darwin notification per process kind (`StateChanged`, as Signal-iOS's SDSCrossProcess).
import Foundation
import YoCore

final class SharedStateFile: StateLock, @unchecked Sendable {
  /// Nil without an App Group (Yo Dev from dev/push.sh).
  static var container: URL? { ShareBridge.container }

  static var folder: URL? { container?.appending(path: "yo-state-v3", directoryHint: .isDirectory) }
  static let generationKey = "yo-state.lock-generation"
  static var generation: String {
    let defaults = UserDefaults(suiteName: ShareBridge.group)
    defaults?.synchronize()
    return defaults?.string(forKey: generationKey) ?? ""
  }

  static func rotateLock() {
    let defaults = UserDefaults(suiteName: ShareBridge.group)
    defaults?.set(UUID().uuidString, forKey: generationKey)
    defaults?.synchronize()
  }

  private static func lockURL(_ generation: String) -> URL? {
    container?.appending(path: "yo-state.lock" + (generation.isEmpty ? "" : "." + generation))
  }

  /// A semaphore, not a mutex: a turn may be taken on one thread and given back on another.
  private static let inProcess = DispatchSemaphore(value: 1)
  nonisolated(unsafe) private static var access: SharedAccess?

  static var available: Bool { lockURL(generation) != nil && folder != nil }

  /// How long a step waits for the turn.
  private let wait: Double

  init(wait: Double = 10) {
    self.wait = wait
  }

  // MARK: The database

  private static let opening = NSLock()
  nonisolated(unsafe) private static var opened: StateDb?

  /// Opened once per process, again when it went (signed out, a local reset). Throws before the first unlock or
  /// without a group.
  static func database() throws -> StateDb {
    guard let folder else { throw YoError.State }
    guard let key = try StateKey.shared() else { throw YoError.State }
    return try StateStorage.open(folder, gate: opening, cached: &opened, key: key)
  }

  // MARK: The turn

  /// The process keeps its turn until `give`, even if its background time ends meanwhile. Expiry releases the
  /// file lock and fences all later writes and requests of this turn.
  static func take(within seconds: Double = 10) -> Bool {
    return Perf.timed("state.lock.wait") {
      guard !Thread.isMainThread else { return false }
      let budget = min(seconds, 10)
      let start = ProcessInfo.processInfo.systemUptime
      guard inProcess.wait(timeout: .now() + budget) == .success else { return false }
      // Retry a refused grant/open once, within the same total wait budget.
      for attempt in 0..<2 {
        let remaining = max(0, budget - (ProcessInfo.processInfo.systemUptime - start))
        let knownGeneration = generation
        if let url = lockURL(knownGeneration),
          let turn = SharedAccess.take(url, within: remaining,
            valid: { generation == knownGeneration },
            repair: { if generation == knownGeneration { rotateLock() } }) {
          access = turn
          return true
        }
        if attempt == 0, ProcessInfo.processInfo.systemUptime - start < budget { usleep(10_000) }
        else { break }
      }
      inProcess.signal()
      return false
    }
  }

  static func give() {
    access?.finish()
    access = nil
    inProcess.signal()
  }

  // MARK: StateLock (the core, around each step that may write)

  func lock() throws {
    guard Self.take(within: wait) else { throw SharedAccess.unavailable }
    guard Self.access?.holds == true else {
      Self.give()
      throw SharedAccess.unavailable
    }
  }

  func held() -> Bool { Self.access?.holds == true }

  func unlock(saved: Bool) {
    Self.give()
    if saved { StateChanged.post() }
  }

  /// The account comes back by its backup (spec 7), never by an old copy of the state.
  static func excludeFromBackup(_ url: URL) {
    var url = url
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    Diagnostics.attempt({ try url.setResourceValues(values) })
  }

  static var app: Bool { available }
}

final class StateFailureCounts: StateDiagnostics, Sendable {
  func failure(kind: String) {
    CountedErrors.record(domain: "StateMigration", place: "StateDb", code: "StateMigration V1 \(kind)")
  }
}

/// Opening a state database (the group's or the app's own).
enum StateStorage {
  static func exists(_ folder: URL) -> Bool {
    FileManager.default.fileExists(atPath: folder.appending(path: "tables.sqlite").path)
  }

  static func open(_ folder: URL, gate: NSLock, cached: inout StateDb?, key: Data) throws -> StateDb {
    try gate.withLock {
      let path = folder.appending(path: "tables.sqlite")
      if let cached, exists(folder) { return cached }
      try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true,
        attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
      SharedStateFile.excludeFromBackup(folder)
      let db = try StateDb.openFresh(path: path.path, stateKey: key, diagnostics: StateFailureCounts())
      cached = db
      return db
    }
  }
}

/// Another process wrote the shared state: one Darwin notification per process kind, observed for all but one's own.
enum StateChanged {
  /// Posted on the main queue.
  static let changed = Notification.Name("yo.stateChanged")
  /// xtool.yml's products.
  private static let kinds = ["YoApp", "YoNotify", "YoShare", "YoControls"]
  private static var own: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleExecutable") as? String ?? "YoApp" }
  private static func darwinName(_ kind: String) -> String { ShareBridge.group + ".stateChanged." + kind }

  static func post() {
    CFNotificationCenterPostNotification(
      CFNotificationCenterGetDarwinNotifyCenter(), CFNotificationName(darwinName(own) as CFString), nil, nil, true)
  }

  @MainActor static func listen() {
    guard !listening else { return }
    listening = true
    for kind in kinds where kind != own {
      CFNotificationCenterAddObserver(
        CFNotificationCenterGetDarwinNotifyCenter(), nil,
        { _, _, _, _, _ in DispatchQueue.main.async { NotificationCenter.default.post(name: StateChanged.changed, object: nil) } },
        darwinName(kind) as CFString, nil, .deliverImmediately)
    }
  }
  @MainActor private static var listening = false
}

/// An extension's client as a guest: what it receives stays for the app.
enum Guest {
  enum Failure: Error {
    case noGroup
    /// errSecInteractionNotAllowed before the first unlock since a restart.
    case keychain(OSStatus)
    case noAccount
    /// Not readable now (before the first unlock, a passing error).
    case busy
    case restore(Error)
  }

  static func open(within seconds: Double, timeout: TimeInterval = 70) -> YoClient? {
    try? opened(within: seconds, timeout: timeout).get()
  }

  /// `seconds`: how long each step that may write waits for the turn.
  static func opened(within seconds: Double, timeout: TimeInterval = 70) -> Result<YoClient, Failure> {
    guard let folder = SharedStateFile.folder else { return .failure(.noGroup) }
    guard StateStorage.exists(folder) else { return .failure(.noAccount) }
    let key: Data
    do {
      guard let found = try StateKey.shared() else { return .failure(.noAccount) }
      key = found
    } catch {
      return .failure(.keychain((error as? Keychain.Failure)?.status ?? errSecInternalError))
    }
    let db: StateDb
    do {
      db = try SharedStateFile.database()
      guard try db.hasState() else { return .failure(.noAccount) }
    } catch {
      Diagnostics.recordUnreported(error)
      return .failure(.busy)
    }
    let client: YoClient
    do {
      client = try YoClient.openDb(
        transport: HubTransport(base: Hub.url, timeout: timeout), db: db, stateKey: key, guest: true,
        lock: SharedStateFile(wait: seconds))
    } catch YoError.Network {
      return .failure(.busy)
    } catch {
      Diagnostics.recordUnreported(error)
      return .failure(.restore(error))
    }
    Hub.pin(client)
    return .success(client)
  }
}
