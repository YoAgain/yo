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
  static let unavailableMessage = "shared storage is not available now"
  static var unavailable: YoError { .Network(message: unavailableMessage) }
  /// Installed by the app; extensions instead revoke their turns before completing their request.
  nonisolated(unsafe) static var hold: (@Sendable (@escaping @Sendable () -> Void) -> (@Sendable () -> Void)?)?
  nonisolated(unsafe) static var foreground: (@Sendable () -> Bool)?
  private static let contentionGate = NSLock()
  nonisolated(unsafe) private static var blockedSince: [String: Double] = [:]
  static func resetContention() {
    contentionGate.withLock { blockedSince.removeAll() }
  }
  /// `expected`: counted, marked so the admin page leaves it out (iOS took the time back, not a fault of the turn).
  private static func log(_ reason: String, _ code: Int32, report: Bool = true, expected: Bool = false,
    file: String = #fileID, line: Int = #line, place: String = #function) {
    #if os(iOS)
    if expected {
      CountedErrors.record(domain: "SharedAccess", place: "\(file):\(line) \(place)", code: String(code), expected: true)
    } else if report {
      Diagnostics.record(NSError(domain: "SharedAccess", code: Int(code)), place: place, file: file, line: line)
    }
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

  /// How long the requests of one turn may stand idle in all. The notification service waits 10 s for the turn
  /// (`NotificationService.lockWait`), the app as long: a request on a dead connection (after a suspension, a network
  /// change) waited out its full timeout (70 s in the app) under the turn, and the other process gave up (SharedAccess
  /// 35, the push shown unread, YoSend "open busy"). Cut off, it fails as the network's and goes in a later turn.
  static let requestBudget: TimeInterval = 6
  /// How long a process coming for the lock lets another one already waiting go first (it then waits as well).
  static let yieldMost: TimeInterval = 1
  /// A turn held longer than this is counted with what it did, to find what keeps the other process waiting.
  static let heldLong: TimeInterval = 5

  private let gate = NSLock()
  private let lifetime = SharedAccess.lifetime
  private var began = ProcessInfo.processInfo.systemUptime
  private let valid: @Sendable () -> Bool
  private let fd: Int32
  private var revoked = false
  private var request: URLSessionTask?
  private var end: (@Sendable () -> Void)?
  private var acquired = false
  private var requests = 0
  private var slowest: (place: String, seconds: Double) = ("", 0)

  /// A request this turn made and how long it took, for `finish`'s count of a turn held long.
  func note(_ place: String, seconds: Double) {
    gate.withLock {
      requests += 1
      if seconds > slowest.seconds { slowest = (place, seconds) }
    }
  }
  var expired: Bool { gate.withLock { revoked } }
  /// Still this process's turn: not taken back, its lock generation current.
  var holds: Bool { gate.withLock { !revoked && valid() } }

  /// The timeout for a request started in this turn: what is left of `requestBudget`, at least a second, at most
  /// `wanted`.
  func requestTimeout(_ wanted: TimeInterval) -> TimeInterval {
    let left = gate.withLock { began } + Self.requestBudget - ProcessInfo.processInfo.systemUptime
    return min(wanted, max(1, left))
  }

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
    // Every turn must be revocable (`hold` or a `Lifetime`): an unrevocable lock would stay held when iOS suspends the
    // process (0xdead10cc).
    guard hold != nil || turn.lifetime != nil else { log("no revocable lock lifetime", ECANCELED); close(fd); return nil }
    guard turn.lifetime?.add(turn) != false else { log("extension lifetime ended", ECANCELED); close(fd); return nil }
    if let hold {
      // No background time left: expected, logged only. A turn within a request's lifetime (an App Intent, YoSend)
      // needs none of its own: the request keeps the process up and its end revokes the turn.
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
    // flock queues nobody: a waiter polls every 10 ms, and the app's next step took the lock again within
    // microseconds of the last one, so in a burst of the app's steps the notification service waited out its 10 s
    // (SharedAccess 35 on RC9, with no request under the turn longer than its budget). Who waits says so with a shared
    // lock on `.waiting`; who comes for the lock lets one waiting go first.
    let waiting = seconds > 0 ? open(url.path + ".waiting", O_RDWR | O_CREAT | O_CLOEXEC, 0o600) : -1
    defer { if waiting >= 0 { close(waiting) } }
    if waiting >= 0 {
      let until = min(deadline, ProcessInfo.processInfo.systemUptime + Self.yieldMost)
      while flock(waiting, LOCK_EX | LOCK_NB) != 0, ProcessInfo.processInfo.systemUptime < until, !turn.expired {
        usleep(2_000)
      }
      flock(waiting, LOCK_UN)
    }
    var announced = false
    while true {
      let (result, code): (Int32, Int32) = turn.gate.withLock {
        guard !turn.revoked else { return (-1, ECANCELED) }
        let result = flock(fd, LOCK_EX | LOCK_NB)
        return (result, errno)
      }
      if result == 0 {
        if announced { flock(waiting, LOCK_UN) }
        _ = contentionGate.withLock { blockedSince.removeValue(forKey: url.path) }
        guard valid() else { log("lock generation retired", ECANCELED); turn.finish(); return nil }
        turn.gate.withLock { (turn.began, turn.acquired) = (ProcessInfo.processInfo.systemUptime, true) }
        break
      }
      if code == EWOULDBLOCK, !announced, waiting >= 0 {
        announced = flock(waiting, LOCK_SH | LOCK_NB) == 0
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
        // Taken back while waiting (ECANCELED): the background time ended, or the extension's request did. A single
        // try (`within` 0: the media journals' locks) finds another process at work: it goes later, nothing to report.
        // Which lock, without its generation: the account state's turn is not a media journal's.
        let lock = url.lastPathComponent.components(separatedBy: ".lock").first.map { $0 + ".lock" } ?? ""
        log("lock acquisition failed or timed out", code, report: seconds > 0, expected: code == ECANCELED,
          place: "take \(lock)")
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
    let (held, requests, slowest) = gate.withLock {
      (acquired ? ProcessInfo.processInfo.systemUptime - began : 0, self.requests, self.slowest)
    }
    if !expired, held > Self.heldLong {
      // The step itself is the core's; what it asked the hub and the longest of it tell where the time went.
      Self.log("turn held long", 0, report: false)
      #if os(iOS)
      CountedErrors.record(domain: "SharedAccess", place: "held over \(Int(Self.heldLong)) s",
        code: requests == 0 ? "no request" : slowest.seconds > Self.heldLong / 2 ? "a slow request" : "many requests",
        details: "held \(Int(held)) s, \(requests) requests, slowest \(Int(slowest.seconds)) s \(slowest.place)")
      #endif
    }
    revoke()
    end?()
    end = nil
    lifetime?.remove(self)
    if Self.current === self { Thread.current.threadDictionary.removeObject(forKey: Self.threadKey) }
    close(fd)
  }
}
