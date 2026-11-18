// "Sent Yo! to Mia" in the Dynamic Island (and on the Lock Screen) for a moment after a Yo from outside the app: the
// Action button, Control Center, a widget, Shortcuts or Siri (YoIntents.swift). Inside the app the bar says it, so
// nothing starts there. The attributes and the start are compiled into the app and into YoControls (linked), whose
// widget bundle draws it (YoLiveActivity.swift); only the app's process may start one, which is why the sending
// intents are LiveActivityIntents.
import ActivityKit
import Foundation

/// One Yo that went (or did not) to one friend or group.
struct YoSentAttributes: ActivityAttributes {
  struct ContentState: Codable, Hashable, Sendable {
    enum Outcome: String, Codable, Hashable, Sendable {
      /// It went: a green check.
      case sent
      /// Not now (offline), or refused: an orange "!".
      case failed
      /// Yo Back with no Yo to answer: a grey "?".
      case nobody
    }

    var outcome: Outcome
    /// "Sent Yo! to Mia", "Yo back to Leo", "Yo to Mia didn't go out", "No one to Yo back".
    var title: String
    /// "just now", "sent you one 2 min ago", "try again", "no Yo received yet".
    var detail: String
  }

  /// The friend's or group's name, as on the bar ("" for nobody).
  let name: String
  /// The bar's colour (Color.row); nil: not on the list, Yo's purple.
  let color: Int?
  /// A Yo back to whoever sent the last one.
  let back: Bool

  var initial: String { name.first.map { String($0).uppercased() } ?? "?" }
}

enum YoSentActivity {
  /// How long it shows: opened big by itself first (the system's moment, about 2 s), then small, then gone.
  static let shown: Duration = .seconds(4)

  /// After a Yo from outside the app: whom it went to, or why it did not.
  static func show(to target: String, name: String, back: Bool, failure: YoIntentError?, lastYoAtMs: UInt64? = nil) async {
    let outcome: YoSentAttributes.ContentState.Outcome = failure == nil ? .sent : .failed
    let title: String
    let detail: String
    switch failure {
    case nil:
      title = back ? "Yo back to \(name)" : "Sent Yo! to \(name)"
      detail = back ? lastYoAtMs.map(sentYouOne) ?? "just now" : "just now"
    case .notNow?:
      (title, detail) = ("Yo to \(name) didn't go out", "try again")
    case let failure?:
      (title, detail) = ("Yo to \(name) not sent", String(localized: failure.localizedStringResource))
    }
    let color = ShareBridge.readList()?.friends.first { $0.to == target }?.color
    await present(YoSentAttributes(name: name, color: color, back: back), .init(outcome: outcome, title: title, detail: detail))
  }

  /// Yo Back with no Yo to answer.
  static func showNobody() async {
    await present(
      YoSentAttributes(name: "", color: nil, back: true),
      .init(outcome: .nobody, title: "No one to Yo back", detail: "no Yo received yet"))
  }

  /// "sent you one 2 min ago" (no he or she: the app does not know).
  private static func sentYouOne(atMs: UInt64) -> String {
    let at = Date(timeIntervalSince1970: TimeInterval(atMs) / 1000)
    let formatter = RelativeDateTimeFormatter()
    formatter.unitsStyle = .short
    return "sent you one \(formatter.localizedString(for: min(at, .now), relativeTo: .now))"
  }

  /// Starts it and opens it big at once (an update with an alert opens the island; a banner on an iPhone without
  /// one), then ends it. Waits while it shows: a process left in the background may be suspended before it could
  /// end it.
  private static func present(_ attributes: YoSentAttributes, _ state: YoSentAttributes.ContentState) async {
    guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
    let content = ActivityContent(state: state, staleDate: .now + 4)
    // One at a time: an earlier one still there (its process suspended) goes now.
    for old in Activity<YoSentAttributes>.activities { await old.end(nil, dismissalPolicy: .immediate) }
    guard let activity = try? Activity.request(attributes: attributes, content: content) else { return }
    await activity.update(
      content,
      alertConfiguration: AlertConfiguration(title: "\(state.title)", body: "\(state.detail)", sound: .default))
    try? await Task.sleep(for: shown)
    await activity.end(nil, dismissalPolicy: .immediate)
  }
}
