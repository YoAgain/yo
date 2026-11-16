// The core's state shared by the app and its extensions (YoShare sends what is shared, YoNotify writes who a push is
// from), in the App Group (ShareBridge.group). A build without the group (Yo Dev from dev/push.sh) keeps its state to
// itself (StateFile, StateKey in Hub.swift).
//
// One state, used in turns. Every step of the core on it (`YoClient.with` in core/bindings) takes a file lock in the
// group (flock on `yo-state.lock`, inside the process's own lock) and first takes in what another process saved since
// (`reload`): so no step ever runs on an old state, and each step's save is the newest when the lock is given back.
// One writer at a time, never a write from an old state. Most waits for the hub (the long poll, deliveries, uploads)
// are outside the turn; some steps still ask the hub within it (opening a chat, a send that receives first). Those
// requests can outlast background time: expiry revokes the turn rather than waiting for the network.
//
// Which save a process has: the file starts with a new random word (36 characters) for each save, written with the
// state in one atomic write, so the word and the state never disagree. Each client keeps the word it read or wrote
// last (`known`); a word that differs means another process saved since. A save that fails (a full disk) changes
// nothing on disk and throws: the core's step fails before anything that needed the save leaves (no message, commit,
// prekey upload or inbox ack), and the others see the state before it, as after a crash of this process.
//
// Background time ending releases the flock before the process can suspend. The old turn may still be unwinding
// a request, but cannot save or send again; its next turn reloads, even if the state on disk did not change.
import Foundation
import YoCore

final class SharedStateFile: StateStore, StateLock, @unchecked Sendable {
  /// The group's folder; nil without an App Group (Yo Dev from dev/push.sh).
  static var container: URL? { ShareBridge.container }

  static var stateURL: URL? { container?.appending(path: "yo-state") }
  private static var lockURL: URL? { container?.appending(path: "yo-state.lock") }

  /// A semaphore, not a mutex: a guest takes it on one thread and gives it back on another.
  private static let inProcess = DispatchSemaphore(value: 1)
  nonisolated(unsafe) private static var access: SharedAccess?

  static var available: Bool { lockURL != nil && stateURL != nil }

  /// The save this client read or made last (its `gen` word); only touched under the lock.
  private var known: String?
  /// A guest (an extension) never makes the state: it saves only over one that is there.
  private let guest: Bool

  init(guest: Bool) {
    self.guest = guest
  }

  // MARK: The lock

  /// The process keeps its turn until `give`, even if its background time ends meanwhile. Expiry releases the
  /// file lock, fences all later saves and requests of this turn, and makes the next turn reload from disk.
  static func take(within seconds: Double = .infinity) -> Bool {
    return Perf.timed("state.lock.wait") {
      guard !Thread.isMainThread else { return false }
      let deadline: DispatchTime = seconds.isFinite ? .now() + seconds : .distantFuture
      guard inProcess.wait(timeout: deadline) == .success else { return false }
      guard let lockURL, let turn = SharedAccess.take(lockURL, within: seconds) else {
        inProcess.signal()
        return false
      }
      access = turn
      return true
    }
  }

  static func give() {
    access?.finish()
    access = nil
    inProcess.signal()
  }

  /// `work` under the lock, off the main thread. Refused background time is a retry, never an unprotected turn.
  static func locked<T>(_ work: () throws -> T) throws -> T {
    guard take() else { throw SharedAccess.unavailable }
    defer { give() }
    let result = try work()
    guard access?.expired == false else { throw SharedAccess.unavailable }
    return result
  }

  // MARK: StateLock (the core, around each step)

  func lock() throws -> StateTurn {
    guard Self.take() else { throw SharedAccess.unavailable }
    do {
      return try Self.access!.perform { lockHeld() }
    } catch {
      Self.give()
      throw error
    }
  }

  private func lockHeld() -> StateTurn {
    guard let gen = Self.word() else {
      // Gone (signed out meanwhile) or not readable: this client is done (it never acts or saves again).
      known = nil
      return .gone
    }
    guard gen != known else { return .same }
    guard let (word, blob) = Self.load() else {
      known = nil
      return .gone
    }
    known = word
    return .newer(blob: blob)
  }

  func unlock() {
    if Self.access?.expired == true { known = nil }  // discard any work after expiry, even if nobody else saved
    Self.give()
  }

  // MARK: StateStore (under the lock)

  /// The state as saved last, now this client's (under the lock); nil when there is none.
  func read() -> Data? {
    guard let access = Self.access else { return nil }
    return try? access.perform {
      guard let (word, blob) = Self.load() else {
        known = nil
        return nil
      }
      known = word
      return blob
    }
  }

  func save(blob: Data) throws {
    guard let access = Self.access else { throw SharedAccess.unavailable }
    try access.perform { try saveHeld(blob: blob) }
  }

  private func saveHeld(blob: Data) throws {
    guard let stateURL = Self.stateURL else { throw YoError.Save(message: "no App Group") }
    // Another save since this client's last one (or the state gone, signed out): this one is old and never written
    // over it. Under the lock that is only a client already closed or replaced.
    guard Self.word() == known, !(guest && known == nil) else { throw YoError.Save(message: "an older state") }
    let gen = UUID().uuidString
    do {
      try (Data(gen.utf8) + blob).write(
        to: stateURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    } catch {
      throw YoError.Save(message: error.localizedDescription)  // not saved: the step fails (see the top)
    }
    known = gen
    Self.excludeFromBackup(stateURL)
  }

  /// Signed out: the state goes (under the lock), whatever a step saved last; the next account's is made only after
  /// (`AccountStore.register` waits for it).
  static func delete() {
    guard let stateURL else { return }
    try? access?.perform { try FileManager.default.removeItem(at: stateURL) }
  }

  private static let wordLength = 36

  /// The word of the last save, read without the lock (one write puts word and state in place at once).
  static func word() -> String? {
    guard let stateURL, let handle = try? FileHandle(forReadingFrom: stateURL) else { return nil }
    defer { try? handle.close() }
    guard let head = try? handle.read(upToCount: wordLength), head.count == wordLength else { return nil }
    return String(data: head, encoding: .utf8)
  }

  private static func load() -> (String, Data)? {
    guard let stateURL, let data = try? Data(contentsOf: stateURL), data.count > wordLength,
      let word = String(data: data.prefix(wordLength), encoding: .utf8)
    else { return nil }
    return (word, Data(data.dropFirst(wordLength)))
  }

  /// As the app's own state: the account comes back by its backup (spec 7), never by an old copy of the state.
  static func excludeFromBackup(_ url: URL) {
    var url = url
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? url.setResourceValues(values)
  }

  // MARK: Moving in (the app, once)

  /// Whether the app keeps its state in the group: decided once per launch. Yes once the state is there; else the
  /// key and the state move in now (the key first: the state is never where its key is not), and a new account
  /// starts there. No (and the app's own folder and keychain item as before) without a group, a lock file, or a
  /// keychain that takes the key.
  static let app: Bool = {
    guard available, let stateURL else { return false }
    return (try? locked {
      guard let access else { return false }
      return try access.perform {
        let manager = FileManager.default
        if manager.fileExists(atPath: stateURL.path) {
          // Moved in before: a copy left in the app's folder (stopped between the steps below) goes, and the key with it.
          try? manager.removeItem(at: StateFile.url)
          StateKey.finishMoveIn()
          return true
        }
        guard StateKey.moveIn() else { return false }
        guard manager.fileExists(atPath: StateFile.url.path) else {
          StateKey.finishMoveIn()
          return true
        }
        // Written whole beside it with its word, then renamed into place in one step, then the old one deleted: never
        // no state, never half of one.
        guard let blob = StateFile.load(),
          (try? (Data(UUID().uuidString.utf8) + blob).write(
            to: stateURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil
        else { return false }
        excludeFromBackup(stateURL)
        try? manager.removeItem(at: StateFile.url)
        StateKey.finishMoveIn()  // only now: until here the state that stayed opens with it
        return true
      }
    }) ?? FileManager.default.fileExists(atPath: stateURL.path)  // refused time never switches to a private state
  }()
}

/// An extension's use of the app's state: the client as a guest (`YoClient.restoreGuest`: what it receives stays for
/// the app), sharing the state step by step as the app does. Nil when there is no state to open (signed out, never
/// moved in, the keychain locked) or the app does not give the lock within `seconds`.
enum Guest {
  static func open(within seconds: Double, timeout: TimeInterval = 70) -> YoClient? {
    guard SharedStateFile.available, let key = try? StateKey.shared(),
      SharedStateFile.take(within: seconds)
    else { return nil }
    defer { SharedStateFile.give() }
    let file = SharedStateFile(guest: true)
    guard let blob = file.read(),
      let client = try? YoClient.restoreGuest(transport: HubTransport(base: Hub.url, timeout: timeout), store: file, stateKey: key, blob: blob)
    else { return nil }
    Hub.pin(client)
    client.shareState(lock: file)
    return client
  }
}
