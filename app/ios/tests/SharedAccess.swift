import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

final class Background: @unchecked Sendable {
  private let gate = NSLock()
  private var expiry: (@Sendable () -> Void)?
  private var ended = 0
  func begin(_ expire: @escaping @Sendable () -> Void) -> @Sendable () -> Void {
    gate.withLock { expiry = expire }
    return { self.gate.withLock { self.ended += 1 } }
  }
  func expire() { gate.withLock { expiry }?() }
  var ends: Int { gate.withLock { ended } }
}

let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(at: directory) }
let url = directory.appendingPathComponent("state.lock")
let probe = open(url.path, O_RDWR | O_CREAT, 0o600)
precondition(probe >= 0)
defer { close(probe) }
func isFree() -> Bool {
  let free = flock(probe, LOCK_EX | LOCK_NB) == 0
  if free { flock(probe, LOCK_UN) }
  return free
}

// A refused background task never gets a flock and never runs the protected work.
SharedAccess.hold = { _ in nil }
precondition(SharedAccess.take(url) == nil)
precondition(isFree())

let background = Background()
SharedAccess.hold = { background.begin($0) }
let turn = SharedAccess.take(url)!
precondition(!isFree())
precondition(turn.holds, "a turn taken holds")
background.expire()
precondition(isFree(), "expiry gives the other process the lock before ending background time")
precondition(!turn.holds, "a turn taken back never writes again (the core asks `held` before each write)")
precondition((try? turn.perform { preconditionFailure("late save") }) == nil)
let task = URLSession.shared.dataTask(with: URL(string: "https://example.invalid")!)
do {
  try turn.start(task)
  preconditionFailure("a revoked turn must not start another request")
} catch {}
precondition(task.state == .suspended)
turn.finish()
precondition(background.ends == 1)
precondition(SharedAccess.current == nil)

// A new turn after expiry is independent of the revoked one and can read state again.
let recovered = SharedAccess.take(url)!
let recoveredValue = try recovered.perform { 42 }
precondition(recoveredValue == 42)
// Its requests share one short budget, so the other process waiting for the turn is not kept past its wait by a
// request on a dead connection; a shorter timeout asked for stays.
precondition(recovered.requestTimeout(70) <= SharedAccess.requestBudget)
precondition(recovered.requestTimeout(70) > SharedAccess.requestBudget - 1)
precondition(recovered.requestTimeout(2) == 2)
recovered.finish()

// Expiry waits for an atomic disk operation, but never waits for the rest of the core's turn.
let disk = SharedAccess.take(url)!
let entered = DispatchSemaphore(value: 0)
let release = DispatchSemaphore(value: 0)
let written = DispatchSemaphore(value: 0)
let revoked = DispatchSemaphore(value: 0)
DispatchQueue.global().async {
  try! disk.perform {
    entered.signal()
    release.wait()
    precondition(!isFree())
  }
  written.signal()
}
entered.wait()
DispatchQueue.global().async { background.expire(); revoked.signal() }
precondition(!isFree())
release.signal()
written.wait()
revoked.wait()
precondition(isFree())
disk.finish()

// Completion fences all future turns of that extension request, including work queued before expiry.
let lifetime = SharedAccess.Lifetime()
lifetime.run {
  let extensionTurn = SharedAccess.take(url)!
  lifetime.stop()
  precondition(isFree())
  precondition((try? extensionTurn.perform { 1 }) == nil)
  extensionTurn.finish()
  precondition(SharedAccess.take(url) == nil)
}
let next = SharedAccess.Lifetime()
next.run {
  let turn = SharedAccess.take(url)!
  precondition(!isFree(), "a reused extension process can serve its next request")
  turn.finish()
}

// Contention has a deadline; a failed acquisition neither unlocks another owner's flock nor leaks its task.
precondition(flock(probe, LOCK_EX | LOCK_NB) == 0)
let ends = background.ends
precondition(SharedAccess.take(url, within: 0.02) == nil)
precondition(background.ends == ends + 1)
flock(probe, LOCK_UN)
precondition(isFree())
// An in-flight request is cancelled on expiry; its late completion cannot run protected work.
final class Hanging: URLProtocol {
  static let started = DispatchSemaphore(value: 0)
  static let stopped = DispatchSemaphore(value: 0)
  override class func canInit(with request: URLRequest) -> Bool { true }
  override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
  override func startLoading() { Self.started.signal() }
  override func stopLoading() { Self.stopped.signal() }
}
let configuration = URLSessionConfiguration.ephemeral
configuration.protocolClasses = [Hanging.self]
let session = URLSession(configuration: configuration)
let waiting = SharedAccess.take(url)!
let request = session.dataTask(with: URL(string: "https://example.invalid")!)
try waiting.start(request)
precondition(Hanging.started.wait(timeout: .now() + 5) == .success)
background.expire()
precondition(Hanging.stopped.wait(timeout: .now() + 5) == .success)
precondition(isFree())
precondition((try? waiting.perform { 1 }) == nil)
waiting.finish()
session.invalidateAndCancel()

// One waiting in another process goes first: a newcomer lets it (a while), and a waiter says it waits.
let waitingFile = open(url.path + ".waiting", O_RDWR | O_CREAT, 0o600)
precondition(waitingFile >= 0)
func someoneWaits() -> Bool {
  let free = flock(waitingFile, LOCK_EX | LOCK_NB) == 0
  if free { flock(waitingFile, LOCK_UN) }
  return !free
}
var began = ProcessInfo.processInfo.systemUptime
let alone = SharedAccess.take(url)!
precondition(ProcessInfo.processInfo.systemUptime - began < 0.5, "nobody waiting: taken at once")
alone.finish()
precondition(flock(waitingFile, LOCK_SH | LOCK_NB) == 0)
began = ProcessInfo.processInfo.systemUptime
let polite = SharedAccess.take(url, within: 3)!
let yielded = ProcessInfo.processInfo.systemUptime - began
precondition(yielded >= SharedAccess.yieldMost * 0.9 && yielded < 2.5, "another waiting: let first, then taken")
polite.finish()
flock(waitingFile, LOCK_UN)
precondition(flock(probe, LOCK_EX | LOCK_NB) == 0)
let gotIt = DispatchSemaphore(value: 0)
DispatchQueue.global().async {
  let waiter = SharedAccess.take(url, within: 5)!
  waiter.finish()
  gotIt.signal()
}
var announced = false
for _ in 0..<200 where !announced { usleep(10_000); announced = someoneWaits() }
precondition(announced, "a waiter says it waits")
flock(probe, LOCK_UN)
precondition(gotIt.wait(timeout: .now() + 5) == .success)
precondition(!someoneWaits(), "and stops saying so once it has the lock")
close(waitingFile)

SharedAccess.hold = { expire in expire(); return {} }
precondition(SharedAccess.take(url) == nil, "expiry while beginning a task never takes the flock")
precondition(isFree())
print("Shared access: refused time, expiry, request budget, waiters first, disk fencing, request cancellation, extension reuse and contention passed")
