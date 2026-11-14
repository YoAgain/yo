// The core's state shared by the app and its extensions (YoShare sends what is shared, YoNotify writes who a push is
// from), in the App Group (ShareBridge.group). A build without the group (Yo Dev from dev/push.sh) keeps its state to
// itself (StateFile, StateKey in Hub.swift).
//
// One state, used in turns. Every step of the core on it (`YoClient.with` in core/bindings) takes a file lock in the
// group (flock on `yo-state.lock`, inside the process's own lock) and first takes in what another process saved since
// (`reload`): so no step ever runs on an old state, and each step's save is the newest when the lock is given back.
// One writer at a time, never a write from an old state. The steps are short: the core waits for the hub (the long
// poll, deliveries, uploads) outside them; only a few steps ask the hub something within (opening a chat, a send that
// receives first).
//
// Which save a process has: the file starts with a new random word (36 characters) for each save, written with the
// state in one atomic write, so the word and the state never disagree. Each client keeps the word it read or wrote
// last (`known`); a word that differs means another process saved since. A save that fails (a full disk) changes
// nothing on disk and throws: the core's step fails before anything that needed the save leaves (no message, commit,
// KeyPackage or inbox ack), and the others see the state before it, as after a crash of this process.
//
// iOS ends a suspended app or extension that holds a file lock in a shared container (0xdead10cc): the app holds the
// lock only inside a background task (`hold`), and nobody holds it while waiting for anything but the step itself.
// A process that dies holding it gives it back with its file (flock).
import Foundation
import YoCore

final class SharedStateFile: StateStore, StateLock, @unchecked Sendable {
  /// The group's folder; nil without an App Group (Yo Dev from dev/push.sh).
  static var container: URL? { ShareBridge.container }

  static var stateURL: URL? { container?.appending(path: "yo-state") }
  private static var lockURL: URL? { container?.appending(path: "yo-state.lock") }

  /// The app's: a background task while the lock is held, so the app is not suspended holding it. Returns its end.
  /// (UIApplication is not there in an extension; an extension's own sheet or push keeps it running meanwhile.) A
  /// task refused or run out still takes the lock: a step is short, and the core's waits are outside it; should iOS
  /// end the app holding it, the lock goes with the process.
  nonisolated(unsafe) static var hold: (@Sendable () -> @Sendable () -> Void)?

  /// The lock file, open for the process's life; -1 when it does not open (then nothing is shared).
  private static let fd: Int32 = {
    guard let url = lockURL else { return -1 }
    return open(url.path, O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
  }()

  /// The lock within the process: a flock is the open file's, so two threads of one process would both get it.
  /// A semaphore, not a mutex: a guest takes it on one thread and gives it back on another.
  private static let inProcess = DispatchSemaphore(value: 1)
  nonisolated(unsafe) private static var release: (@Sendable () -> Void)?

  /// Whether the lock can be had here at all.
  static var available: Bool { fd >= 0 && stateURL != nil }

  /// The save this client read or made last (its `gen` word); only touched under the lock.
  private var known: String?
  /// A guest (an extension) never makes the state: it saves only over one that is there.
  private let guest: Bool

  init(guest: Bool) {
    self.guest = guest
  }

  // MARK: The lock

  /// Takes the lock, waiting while another process holds it.
  static func take() {
    inProcess.wait()
    let end = hold?()
    while flock(fd, LOCK_EX) != 0 && errno == EINTR {}
    release = end
  }

  /// Takes the lock if it comes within `seconds`: an extension does not wait long for the app.
  static func take(within seconds: Double) -> Bool {
    let deadline = Date.now.addingTimeInterval(seconds)
    guard inProcess.wait(timeout: .now() + seconds) == .success else { return false }
    while flock(fd, LOCK_EX | LOCK_NB) != 0 {
      guard Date.now < deadline else {
        inProcess.signal()
        return false
      }
      usleep(50_000)
    }
    return true
  }

  static func give() {
    flock(fd, LOCK_UN)
    let end = release
    release = nil
    end?()
    inProcess.signal()
  }

  /// `work` under the lock (opening, making or deleting the state), off the main thread.
  static func locked<T>(_ work: () throws -> T) rethrows -> T {
    take()
    defer { give() }
    return try work()
  }

  // MARK: StateLock (the core, around each step)

  func lock() -> StateTurn {
    Self.take()
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

  func unlock() { Self.give() }

  // MARK: StateStore (under the lock)

  /// The state as saved last, now this client's (under the lock); nil when there is none.
  func read() -> Data? {
    guard let (word, blob) = Self.load() else {
      known = nil
      return nil
    }
    known = word
    return blob
  }

  func save(blob: Data) throws {
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
    try? FileManager.default.removeItem(at: stateURL)
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
    return locked {
      let manager = FileManager.default
      if manager.fileExists(atPath: stateURL.path) {
        // Moved in before: a copy left in the app's folder (stopped between the steps below) goes.
        try? manager.removeItem(at: StateFile.url)
        return true
      }
      guard StateKey.moveIn() else { return false }
      guard manager.fileExists(atPath: StateFile.url.path) else { return true }
      // Written whole beside it with its word, then renamed into place in one step, then the old one deleted: never
      // no state, never half of one.
      guard let blob = StateFile.load(),
        (try? (Data(UUID().uuidString.utf8) + blob).write(
          to: stateURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil
      else { return false }
      excludeFromBackup(stateURL)
      try? manager.removeItem(at: StateFile.url)
      return true
    }
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
    client.shareState(lock: file)
    return client
  }
}
