import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import YoCore
#if canImport(os)
import os
#endif

/// A cancellable turn in the shared container. Only short disk operations run under `gate`; never a network wait.
/// Expiry takes the same gate, releases the flock and cancels the request. A late answer cannot save stale state.
final class SharedAccess: @unchecked Sendable {
  static var unavailable: YoError { .Network(message: "shared storage is not available now") }
  /// Installed by the app; extensions instead revoke their turns before completing their request.
  nonisolated(unsafe) static var hold: (@Sendable (@escaping @Sendable () -> Void) -> (@Sendable () -> Void)?)?
  nonisolated(unsafe) static var foreground: (@Sendable () -> Bool)?
  private static let contentionGate = NSLock()
  nonisolated(unsafe) private static var blockedSince: [String: Double] = [:]
  static func resetContention() {
    contentionGate.withLock { blockedSince.removeAll() }
  }
  private static func log(_ reason: String, _ code: Int32, report: Bool = true, file: String = #fileID, line: Int = #line, place: String = #function) {
    #if os(iOS)
    if report { Diagnostics.record(NSError(domain: "SharedAccess", code: Int(code)), place: place, file: file, line: line) }
    #endif
    #if canImport(os)
    Logger(subsystem: "app.justyo", category: "shared-lock").error("\(reason, privacy: .public); errno=\(code, privacy: .public)")
    #endif
  }
  private static let threadKey = "yo.sharedAccess"
  private static let lifetimeKey = "yo.sharedLifetime"
  /// Rust and its Swift callbacks are synchronous on one OS thread; no Swift task suspension occurs in a turn.
  static var current: SharedAccess? { Thread.current.threadDictionary[threadKey] as? SharedAccess }
  private static var lifetime: Lifetime? { Thread.current.threadDictionary[lifetimeKey] as? Lifetime }

  /// Each extension request has its own lifetime: the system may reuse the process for another request.
  final class Lifetime: @unchecked Sendable {
    private let gate = NSLock()
    private var stopped = false
    private var turns: [SharedAccess] = []

    fileprivate func add(_ turn: SharedAccess) -> Bool {
      gate.withLock {
        guard !stopped else { return false }
        turns.append(turn)
        return true
      }
    }

    fileprivate func remove(_ turn: SharedAccess) {
      gate.withLock { turns.removeAll { $0 === turn } }
    }

    func stop() {
      let active = gate.withLock {
        stopped = true
        return turns
      }
      for turn in active { turn.revoke() }
    }

    func run<T>(_ work: () -> T) -> T {
      Thread.current.threadDictionary[SharedAccess.lifetimeKey] = self
      defer { Thread.current.threadDictionary.removeObject(forKey: SharedAccess.lifetimeKey) }
      return work()
    }
  }

  private let gate = NSLock()
  private let lifetime = SharedAccess.lifetime
  private let valid: @Sendable () -> Bool
  private let fd: Int32
  private var revoked = false
  private var request: URLSessionTask?
  private var end: (@Sendable () -> Void)?
  var expired: Bool { gate.withLock { revoked } }

  private init(fd: Int32, valid: @escaping @Sendable () -> Bool) { self.fd = fd; self.valid = valid }

  static func take(_ url: URL, within seconds: Double = 10,
    valid: @escaping @Sendable () -> Bool = { true }, repair: (@Sendable () -> Void)? = nil
  ) -> SharedAccess? {
    let fd = open(url.path, O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
    guard fd >= 0 else {
      let code = errno
      log("lock open failed", code)
      // A new generation creates a fresh empty file on the caller's next attempt.
      if code != EWOULDBLOCK { repair?() }
      return nil
    }
    let turn = SharedAccess(fd: fd, valid: valid)
    // Every turn is revocable: the app's by its background time (`hold`), an extension's by its request's lifetime.
    // Neither (a thread outside `Lifetime.run` in an extension) is refused: a lock nobody can take back would stay
    // held when iOS suspends the process (0xdead10cc).
    guard hold != nil || turn.lifetime != nil else { log("no revocable lock lifetime", ECANCELED); close(fd); return nil }
    guard turn.lifetime?.add(turn) != false else { log("extension lifetime ended", ECANCELED); close(fd); return nil }
    if let hold {
      // Refused in the background once iOS gives no more time (beginBackgroundTask: invalid): expected, no turn
      // until the app is in front again (the caller tries again then); only logged here, never reported.
      // A turn within a request's lifetime (an App Intent the system runs in the app, launched in the background for
      // it: YoSend) needs no background time of its own: the system keeps the process up while the request runs, and
      // the request's end revokes the turn (`Lifetime.stop`). Refused, a Yo from the Action button would be "not now"
      // whenever iOS gave the app no background time of its own.
      let end = hold({ turn.revoke() })
      if let end {
        turn.end = end
      } else if turn.lifetime == nil {
        log("background time refused", ECANCELED, report: false)
        turn.finish()
        return nil
      }
    }
    let deadline = ProcessInfo.processInfo.systemUptime + min(seconds, 10)
    while true {
      let (result, code): (Int32, Int32) = turn.gate.withLock {
        guard !turn.revoked else { return (-1, ECANCELED) }
        let result = flock(fd, LOCK_EX | LOCK_NB)
        return (result, errno)
      }
      if result == 0 {
        _ = contentionGate.withLock { blockedSince.removeValue(forKey: url.path) }
        guard valid() else { log("lock generation retired", ECANCELED); turn.finish(); return nil }
        break
      }
      if code == EWOULDBLOCK {
        let now = ProcessInfo.processInfo.systemUptime
        let stuck = contentionGate.withLock {
          guard foreground?() == true else { blockedSince.removeValue(forKey: url.path); return false }
          let since = blockedSince[url.path] ?? now
          blockedSince[url.path] = since
          return now - since > 15
        }
        if stuck {
          log("foreground lock contention exceeded 15 seconds; retiring lock", code)
          repair?()
          _ = contentionGate.withLock { blockedSince.removeValue(forKey: url.path) }
          turn.finish()
          return nil
        }
      }
      guard !turn.expired, ProcessInfo.processInfo.systemUptime < deadline, code == EWOULDBLOCK || code == EINTR else {
        log("lock acquisition failed or timed out", code)
        turn.finish()
        return nil
      }
      usleep(10_000)
    }
    Thread.current.threadDictionary[threadKey] = turn
    return turn
  }

  /// Disk access and starting a request are serialized against revocation, not against the network's answer.
  func perform<T>(_ work: () throws -> T) throws -> T {
    try gate.withLock {
      guard !revoked, valid() else { throw Self.unavailable }
      return try work()
    }
  }

  func start(_ task: URLSessionTask) throws {
    try perform {
      request = task
      task.resume()
    }
  }

  func revoke() {
    gate.withLock {
      guard !revoked else { return }
      revoked = true
      flock(fd, LOCK_UN)
      request?.cancel()
      request = nil
    }
  }

  func finish() {
    revoke()
    end?()
    end = nil
    lifetime?.remove(self)
    if Self.current === self { Thread.current.threadDictionary.removeObject(forKey: Self.threadKey) }
    close(fd)
  }
}
