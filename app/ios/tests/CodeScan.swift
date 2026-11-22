import Foundation

var failures = 0
@MainActor func check(_ ok: Bool, _ what: String, line: Int = #line) {
  if !ok { failures += 1; print("FAIL \(line): \(what)") }
}
func name(_ text: String) -> String? { URL(string: text).flatMap(Invite.name(in:)) }

// What a code says: YourCode draws Invite.link, so each link it makes reads back as its name.
check(name(Invite.link(for: "Chris").absoluteString) == "Chris", "own link reads back")
check(name("https://justyo.co/yoGPT") == "yoGPT", "justyo.co link")
check(name("https://www.justyo.co/Anna") == "Anna", "www link")
check(name("yo://Anna") == "Anna", "yo:// link")
check(name("https://justyo.co/privacy") == nil, "site page")
check(name("https://justyo.co/") == nil, "home page")
check(name("https://example.com/Anna") == nil, "other site")
check(name("WIFI:S:home;T:WPA;P:secret;;") == nil, "other QR")
check(name("hello") == nil, "plain text")

// One ask per code.
let t0 = Date(timeIntervalSince1970: 1_000)
func at(_ s: Double) -> Date { t0.addingTimeInterval(s) }
var seen = CodeSightings()
check(seen.sighted(["Anna"], at: at(0), canAsk: true) == "Anna", "first sighting asks")
check(seen.sighted(["Anna"], at: at(0.1), canAsk: true) == nil, "same code in view: no second ask")
for step in 1...50 { check(seen.sighted(["anna"], at: at(Double(step) * 0.2), canAsk: true) == nil, "kept in view") }
check(seen.sighted(["Anna"], at: at(10 + 2.9), canAsk: true) == nil, "away less than 3 s: still answered")
check(seen.sighted(["Anna"], at: at(12.9 + 3), canAsk: true) == "Anna", "away 3 s: asks again")

// Asking blocked (alert up, recording): nothing asked, the code asks once it may.
var blocked = CodeSightings()
check(blocked.sighted(["Ben"], at: at(0), canAsk: false) == nil, "no ask while blocked")
check(blocked.sighted(["Ben"], at: at(0.2), canAsk: true) == "Ben", "asks when allowed")

// Two codes: one at a time; the other asks next.
var two = CodeSightings()
check(two.sighted(["Anna", "Ben"], at: at(0), canAsk: true) == "Anna", "first of two")
check(two.sighted(["Anna", "Ben"], at: at(0.1), canAsk: false) == nil, "alert for Anna up")
check(two.sighted(["Anna", "Ben"], at: at(0.2), canAsk: true) == "Ben", "then the second")

// The camera closed: asks again at the next opening.
seen.reset()
check(seen.sighted(["Anna"], at: at(16), canAsk: true) == "Anna", "after reset")

if failures > 0 { print("\(failures) failed"); exit(1) }
print("code-scan: ok")
