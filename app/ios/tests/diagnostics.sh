#!/usr/bin/env bash
# Run the real diagnostic queue with a recording transport: opt-in, privacy, counts, retry and opt-out.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
cat > "$out/YoCore.swift" <<'SWIFT'
public enum YoError: Error {
  case Network(message: String), Refused(status: UInt16), NotOnYo, Blocked, GroupEmpty, Outdated, Unconfirmed, Replaced
  case Clock, Protocol(message: String), State, Save(message: String)
}
SWIFT
swiftc -module-cache-path "$out/modules" -swift-version 6 -emit-library -emit-module -module-name YoCore "$out/YoCore.swift" -o "$out/libYoCore.so"
cat > "$out/main.swift" <<'SWIFT'
import Foundation
import Synchronization
import YoCore

enum Keychain { struct Failure: Error {} }
enum CountedErrors {
  static let calls = Mutex(0)
  static func record(domain: String, place: String, code: String, expected: Bool) { calls.withLock { $0 += 1 } }
}
enum AppUpdate { static let header = "ios/test" }
enum Hub {
  static let transport = Transport()
}
enum ShareBridge { static let group = "app.justyo.diagnostics-test" }
struct Transport {
  struct State { var online = true; var refuse = false; var bodies: [Data] = []; var installs: Set<String> = []; var calls = 0 }
  static let state = Mutex(State())
}
func newInstallId() -> String { UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased() }
// The core's help call: the install id in its header, the queued JSON as the body.
func helpDiagnostic(transport: Transport, install: String, crash: Bool, report: String) throws -> UInt16 {
  precondition(!crash && install.count == 32)
  return Transport.state.withLock {
    $0.calls += 1
    if !$0.online { return 503 }
    if $0.refuse { return 400 }
    $0.bodies.append(Data(report.utf8))
    $0.installs.insert(install)
    return 204
  }
}
func eventually(_ check: () -> Bool) {
  for _ in 0..<500 {
    if check() { return }
    usleep(10_000)
  }
  preconditionFailure("queue did not reach expected state")
}
// Expected: the update asked for, a limit. Problems: a refused signature, the hub failing, a broken TLS handshake.
precondition(Noise.status(426, method: "GET", route: "/v1/inbox", signed: true) && Noise.status(429, method: "GET", route: "/v1/inbox", signed: true))
precondition(!Noise.status(502, method: "GET", route: "/v1/inbox", signed: true))
precondition(Noise.status(401, method: "GET", route: "/v1/inbox", signed: true))
precondition(!Noise.status(401, method: "GET", route: "/v1/inbox", signed: false))
precondition(!Noise.urlError(domain: NSURLErrorDomain, code: NSURLErrorSecureConnectionFailed, badDates: false))
precondition(Noise.status(404, method: "POST", route: "/v1/lookup", signed: true))
precondition(!Noise.status(404, method: "GET", route: "/v1/support", signed: true))
precondition(!Noise.status(500, method: "GET", route: "/v1/inbox", signed: true))
precondition(Noise.urlError(domain: NSURLErrorDomain, code: NSURLErrorTimedOut, badDates: false))
precondition(!Noise.urlError(domain: "other", code: NSURLErrorTimedOut, badDates: true))
precondition(Noise.fallback("network") && !Noise.fallback("decrypt") && !Noise.fallback("hubStatus/503"))
let defaults = UserDefaults.standard
Diagnostics.clear()
defaults.set(false, forKey: "crashReports.on")
Diagnostics.recordUnreported(YoError.Network(message: "offline"))
precondition(CountedErrors.calls.withLock { $0 } == 0)
Diagnostics.record(NSError(domain: "test", code: 1))
usleep(50_000)
precondition(Transport.state.withLock { $0.calls } == 0)
eventually { Diagnostics.pending("error").count == 1 }
defaults.set(true, forKey: "crashReports.on")
for _ in 0..<12 {
  Diagnostics.record(NSError(domain: "test", code: 17, userInfo: [NSLocalizedDescriptionKey: "PRIVATE_SENTINEL", "token": "PRIVATE_SENTINEL"]))
}
Diagnostics.recordUnreported(YoError.Refused(status: 429))
Diagnostics.recordUnreported(YoError.Refused(status: 401))
eventually { Transport.state.withLock { $0.bodies.count } == 14 }
for body in Transport.state.withLock({ $0.bodies }) {
  precondition(!String(decoding: body, as: UTF8.self).contains("PRIVATE_SENTINEL"))
  let payload = try! JSONSerialization.jsonObject(with: body) as! [String: String]
  precondition(Set(payload.keys) == Set(["app", "os", "model", "domain", "code", "place", "uuid"]))
  precondition(payload["app"] == "ios/test")
}
let last = try! JSONSerialization.jsonObject(with: Transport.state.withLock { $0.bodies.last! }) as! [String: String]
precondition(last["code"] == "Refused/401")
// Cases by name, not their NSError number ("9" was Replaced); a session's libsignal text never leaves.
precondition(Diagnostics.name(.Replaced) == "Replaced" && Diagnostics.name(.State) == "State")
precondition(Diagnostics.name(.Protocol(message: "protocol: storage version")) == "Protocol/protocol: storage version")
precondition(Diagnostics.name(.Protocol(message: "session: PRIVATE_SENTINEL.1")) == "Protocol/session")
// A save by the core's fixed words: its class and SQLite's extended code, never another text.
precondition(Diagnostics.name(.Save(message: "Io/5386")) == "Save/Io/5386")
precondition(Diagnostics.name(.Save(message: "Busy")) == "Save/Busy" && Diagnostics.name(.Save(message: "Full")) == "Save/Full")
precondition(Diagnostics.name(.Save(message: "the turn on the state ended")) == "Save/TurnEnded")
precondition(Diagnostics.name(.Save(message: "an older state")) == "Save/Stale")
precondition(Diagnostics.name(.Save(message: "Io/PRIVATE_SENTINEL")) == "Save/Io")
precondition(Diagnostics.name(.Save(message: "PRIVATE_SENTINEL")) == "Save/Other")
// iOS giving no time for a save is expected, counted; a database failure is a problem.
let countedBefore = CountedErrors.calls.withLock { $0 }
Diagnostics.recordUnreported(YoError.Save(message: "Busy"))
Diagnostics.recordUnreported(YoError.Save(message: "the turn on the state ended"))
precondition(CountedErrors.calls.withLock { $0 } == countedBefore + 2)
// Replaced elsewhere is expected (signed out and shown), never a report.
Diagnostics.recordUnreported(YoError.Replaced)
usleep(50_000)
precondition(Transport.state.withLock { $0.bodies.count } == 14 && Diagnostics.pending("error").isEmpty)
Transport.state.withLock { $0.online = false }
for _ in 0..<3 { Diagnostics.record(NSError(domain: "offline", code: 2)) }
eventually { Diagnostics.pending("error").count == 3 }
Transport.state.withLock { $0.online = true }
Diagnostics.flush()
eventually { Transport.state.withLock { $0.bodies.count } == 17 }
Transport.state.withLock { $0.online = false }
Diagnostics.record(NSError(domain: "discard", code: 3))
eventually { Diagnostics.pending("error").count == 1 }
defaults.set(false, forKey: "crashReports.on")
for n in 0..<510 { Diagnostics.record(NSError(domain: "bounded", code: n)) }
eventually {
  let rows = Diagnostics.pending("error")
  guard rows.count == 500,
    let first = try? JSONSerialization.jsonObject(with: Data(rows[0].utf8)) as? [String: String]
  else { return false }
  return first["code"] == "10"
}
let rows = Diagnostics.pending("error")
let first = try! JSONSerialization.jsonObject(with: Data(rows[0].utf8)) as! [String: String]
precondition(first["code"] == "10")
let ids = rows.map { (try! JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String: String])["uuid"]! }
precondition(Set(ids).count == 500 && ids.allSatisfy { UUID(uuidString: $0) != nil })
Transport.state.withLock { $0.online = true }
Diagnostics.flush()
usleep(50_000)
precondition(Diagnostics.pending("error").count == 500)
let epoch = defaults.string(forKey: "diagnostics.generation")
let crash = "{\"uuid\":\"" + UUID().uuidString + "\"}"
Diagnostics.append(crash, kind: "crash", epoch: epoch)
precondition(Diagnostics.pending("error").count == 499)
precondition(Diagnostics.pending("crash") == [crash])
Diagnostics.markSent(crash, epoch: epoch)
precondition(Diagnostics.pending("crash").isEmpty)
precondition(Diagnostics.retainedCrashes().map { $0["text"]! } == [crash])
Diagnostics.flush()
precondition(Diagnostics.pending("crash").isEmpty)
Diagnostics.clear()
Diagnostics.append(crash, kind: "crash", epoch: epoch)
precondition(Diagnostics.pending("crash").isEmpty)
precondition(Diagnostics.pending("error").isEmpty)
defaults.set(["legacy"], forKey: "diagnostics.pending")
precondition(Diagnostics.pending("error") == ["legacy"])
precondition(Diagnostics.pending("error") == ["legacy"])
Diagnostics.clear()
precondition(Transport.state.withLock { $0.bodies.count } == 17)
// One install id for all of them; Send Crashes off forgets it, on again makes a new one on the next upload.
let install = Transport.state.withLock { $0.installs }
precondition(install.count == 1 && InstallID.existing == install.first)
Diagnostics.setReportsOn(false)
precondition(InstallID.existing == nil && !Diagnostics.reportsOn)
// What an earlier build queued (the same store; plain text from the first builds) goes out by the new id.
defaults.set(["legacy"], forKey: "diagnostics.pending")
Diagnostics.flush()
usleep(50_000)
precondition(Diagnostics.pending("error") == ["legacy"] && InstallID.existing == nil)
Diagnostics.setReportsOn(true)
Diagnostics.flush()
eventually { Diagnostics.pending("error").isEmpty }
// As queued: the core turns a first build's plain-text crash into JSON itself.
precondition(String(decoding: Transport.state.withLock { $0.bodies.last! }, as: UTF8.self) == "legacy")
precondition(Transport.state.withLock { $0.installs.count } == 2 && !install.contains(InstallID.existing!))
// A report the hub will never take (4xx) is done with, not kept forever at the queue's head.
Transport.state.withLock { $0.refuse = true }
let before = Transport.state.withLock { $0.calls }
Diagnostics.record(NSError(domain: "refused", code: 4))
eventually { Transport.state.withLock { $0.calls } > before && Diagnostics.pending("error").isEmpty }
Transport.state.withLock { $0.refuse = false }
Diagnostics.clear()
precondition(Transport.state.withLock { $0.bodies.count } == 18)
struct OldCrash: Codable { let id: UUID; let date: Date; let text: String }
let old = OldCrash(id: UUID(), date: Date(timeIntervalSince1970: 1), text: "already sent")
defaults.set(try! JSONEncoder().encode([old]), forKey: "crashReports.history")
defaults.set(["waiting"], forKey: "crashReports.pending")
precondition(Diagnostics.pending("crash") == ["waiting"])
let retained = Diagnostics.retainedCrashes()
precondition(retained.count == 2 && retained[0]["id"] == old.id.uuidString)
precondition(defaults.data(forKey: "crashReports.history") == nil)
precondition(defaults.stringArray(forKey: "crashReports.pending") == nil)
Diagnostics.markSent("waiting", epoch: defaults.string(forKey: "diagnostics.generation"))
precondition(Diagnostics.pending("crash").isEmpty && Diagnostics.retainedCrashes().count == 2)
Diagnostics.clear()
Diagnostics.setReportsOn(true)
InstallID.forget()
print("Diagnostics: consent, install id, every occurrence, privacy, retry, drop, retention and migration passed")
SWIFT
swiftc -module-cache-path "$out/modules" -swift-version 6 -I "$out" -L "$out" -lYoCore \
  "$here/../YoApp/Sources/YoApp/Shared/Diagnostics.swift" "$out/main.swift" -o "$out/check"
XDG_CONFIG_HOME="$out/config" LD_LIBRARY_PATH="$out${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$out/check"
