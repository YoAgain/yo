// The core's state shared by the app and its extensions (YoShare sends what is shared, YoNotify writes who a push is
// from, YoControls sends from Control Center), in the App Group (ShareBridge.group). A build without the group (Yo Dev
// from dev/push.sh) keeps its state to itself, in the same kind of database (StateFile in Hub.swift).
//
// SQLite in WAL mode stores client state and libsignal records in tables. Each process opens it for itself.
// - Reads (the list's friends, contacts, groups, settings) take the last committed state in one read transaction,
//   without any turn: never a wait for an extension's step, never an empty answer for a refused turn.
// - Each step that may write takes the turn between the processes (flock on `yo-state.lock`, inside the process's
//   own lock), first takes in what another process wrote since, and writes at its end only over the state it
//   started from (a compare and swap in `BEGIN IMMEDIATE`): one writer at a time, never a write from an old state.
//   The database is locked only for that write, never across a request to the hub; most waits for the hub (the long
//   poll, deliveries, uploads) are outside the turn too, some steps still ask the hub within it (opening a chat, a
//   send that receives first). Those requests can outlast background time: expiry revokes the turn rather than
//   waiting for the network, and a turn taken back writes nothing more (`held`); its next step starts again from
//   the database.
// - A connection lives for one access: no SQLite lock is held between them, so iOS never suspends a process with a
//   lock on a file in the group (0xdead10cc). The database, its -wal and -shm are complete until first user
//   authentication, as the state file was (SQLite’s open flag and the folder’s class), and never in a backup.
// - A write failing (a full disk) changes nothing: the core's step fails before anything that needed it leaves (no
//   message, commit, prekey upload or inbox ack), and the others see the state before it, as after a crash.
// - After a turn that wrote, a Darwin notification named for the writing process (`StateChanged`, as Signal-iOS's
//   SDSCrossProcess): each process listens to the others' names; the app takes in what an extension saved.
import Foundation
import YoCore

final class SharedStateFile: StateLock, @unchecked Sendable {
  /// The group's folder; nil without an App Group (Yo Dev from dev/push.sh).
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

  private static var lockURL: URL? {
    container?.appending(path: "yo-state.lock" + (generation.isEmpty ? "" : "." + generation))
  }

  /// A semaphore, not a mutex: a turn may be taken on one thread and given back on another.
  private static let inProcess = DispatchSemaphore(value: 1)
  nonisolated(unsafe) private static var access: SharedAccess?

  static var available: Bool { lockURL != nil && folder != nil }

  /// How long a step waits for the turn.
  private let wait: Double

  init(wait: Double = 10) {
    self.wait = wait
  }

  // MARK: The database

  private static let opening = NSLock()
  nonisolated(unsafe) private static var opened: StateDb?

  /// The group's database, opened once per process (made, and the state file of before moved in, at the first
  /// opening); opened again when it went (signed out, a local reset). Throws while it cannot be opened (the phone
  /// not unlocked since its start) or without a group.
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
        if let lockURL = container?.appending(path: "yo-state.lock" + (knownGeneration.isEmpty ? "" : "." + knownGeneration)),
          let turn = SharedAccess.take(lockURL, within: remaining,
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

  /// As the app's own state: the account comes back by its backup (spec 7), never by an old copy of the state.
  static func excludeFromBackup(_ url: URL) {
    var url = url
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    Diagnostics.attempt({ try url.setResourceValues(values) })
  }

  static var app: Bool { available }
}

/// Opening a state database (the group's or the app's own): its folder made in the state's protection class and
/// kept out of backups, the file of before moved in.
final class StateFailureCounts: StateDiagnostics, Sendable {
  func failure(kind: String) {
    CountedErrors.record(domain: "StateMigration", place: "StateDb", code: "StateMigration V1 \(kind)")
  }
}

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

/// Another process wrote the shared state: the app takes in what an extension saved for it (its Yos, the friends).
/// As Signal-iOS's SDSCrossProcess (SignalServiceKit/Storage/Database/SDSCrossProcess.swift): a Darwin notification
/// per kind of process, posted after each turn that wrote, observed for every kind but one's own.
enum StateChanged {
  /// Posted (in the app, on the main queue) when another process wrote.
  static let changed = Notification.Name("yo.stateChanged")
  /// The processes that share the state: the app and its extensions (xtool.yml's products).
  private static let kinds = ["YoApp", "YoNotify", "YoShare", "YoControls"]
  private static var own: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleExecutable") as? String ?? "YoApp" }
  private static func darwinName(_ kind: String) -> String { ShareBridge.group + ".stateChanged." + kind }

  static func post() {
    CFNotificationCenterPostNotification(
      CFNotificationCenterGetDarwinNotifyCenter(), CFNotificationName(darwinName(own) as CFString), nil, nil, true)
  }

  /// The app, once: every other process's writes.
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

/// An extension's use of the app's state: the client as a guest (`YoClient.openDb`, guest: what it receives stays for
/// the app), sharing the state step by step as the app does. Nil when there is no state to open (signed out, never
/// moved in, the keychain locked) or it is not readable now.
enum Guest {
  /// Why the state did not open.
  enum Failure: Error {
    /// No App Group in the signature.
    case noGroup
    /// The keychain does not answer: before the first unlock since a restart (errSecInteractionNotAllowed), or else.
    case keychain(OSStatus)
    /// No key, or no state: no account on this phone (yet).
    case noAccount
    /// The state is not readable now (the phone not unlocked since its start, a passing error).
    case busy
    /// The state did not open with its key.
    case restore(Error)
  }

  static func open(within seconds: Double, timeout: TimeInterval = 70) -> YoClient? {
    try? opened(within: seconds, timeout: timeout).get()
  }

  /// Opened without the turn (a read of the last committed state); `seconds`: how long each of its steps that may
  /// write waits for the turn.
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
      return .failure(.busy)  // not readable now
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
