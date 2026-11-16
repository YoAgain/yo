import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import YoCore

/// A cancellable turn in the shared container. Only short disk operations run under `gate`; never a network wait.
/// Expiry takes the same gate, releases the flock and cancels the request. A late answer cannot save stale state.
final class SharedAccess: @unchecked Sendable {
  static var unavailable: YoError { .Network(message: "shared storage is not available now") }
  /// Installed by the app; extensions instead revoke their turns before completing their request.
  nonisolated(unsafe) static var hold: (@Sendable (@escaping @Sendable () -> Void) -> (@Sendable () -> Void)?)?
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
  private let fd: Int32
  private var revoked = false
  private var request: URLSessionTask?
  private var end: (@Sendable () -> Void)?
  var expired: Bool { gate.withLock { revoked } }

  private init(fd: Int32) { self.fd = fd }

  static func take(_ url: URL, within seconds: Double = .infinity) -> SharedAccess? {
    let fd = open(url.path, O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
    guard fd >= 0 else { return nil }
    let turn = SharedAccess(fd: fd)
    // Every turn is revocable: the app's by its background time (`hold`), an extension's by its request's lifetime.
    // Neither (a thread outside `Lifetime.run` in an extension) is refused: a lock nobody can take back would stay
    // held when iOS suspends the process (0xdead10cc).
    guard hold != nil || turn.lifetime != nil else { close(fd); return nil }
    guard turn.lifetime?.add(turn) != false else { close(fd); return nil }
    if let hold {
      guard let end = hold({ turn.revoke() }) else { turn.finish(); return nil }
      turn.end = end
    }
    let deadline = Date.now.addingTimeInterval(seconds.isFinite ? seconds : 1e10)
    while true {
      let (result, code): (Int32, Int32) = turn.gate.withLock {
        guard !turn.revoked else { return (-1, ECANCELED) }
        let result = flock(fd, LOCK_EX | LOCK_NB)
        return (result, errno)
      }
      if result == 0 { break }
      guard !turn.expired, Date.now < deadline, code == EWOULDBLOCK || code == EINTR else {
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
      guard !revoked else { throw Self.unavailable }
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
