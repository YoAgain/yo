// Yo codes the camera sees (Camera.codes): each asks once, as an opened invite link does. A code stays answered
// while it is in view, also after its alert was closed; out of view for `away` seconds (or the camera closed), it
// may ask again.
import Foundation

struct CodeSightings {
  static let away: TimeInterval = 3

  /// Names (lowercased) asked about, with when each was last in view.
  private var asked: [String: Date] = [:]

  /// The names in view now; `canAsk` false (an alert up, a video recording) asks nothing, and a new code waits for
  /// the next sighting. Returns the one name to ask about.
  mutating func sighted(_ names: [String], at now: Date, canAsk: Bool) -> String? {
    asked = asked.filter { now.timeIntervalSince($0.value) < Self.away }
    var ask: String?
    for name in names {
      let key = name.lowercased()
      if asked[key] != nil {
        asked[key] = now
      } else if canAsk, ask == nil {
        asked[key] = now
        ask = name
      }
    }
    return ask
  }

  mutating func reset() { asked = [:] }
}
