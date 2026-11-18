// "Sent Yo! to Mia" in the Dynamic Island (and on the Lock Screen) for a moment after a Yo from outside the app: the
// Action button, Control Center, a widget, Shortcuts or Siri (YoIntents.swift). Inside the app the bar says it, so
// nothing starts there. The attributes and the start are compiled into the app and into YoControls (linked), whose
// widget bundle draws it (YoLiveActivity.swift); only the app's process may start one, which is why the sending
// intents are LiveActivityIntents.
import ActivityKit
import Foundation
#if canImport(os)
import os
#endif

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
    /// Only where it says more than the title: why it was refused ("blocked"); else empty, and not shown.
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
    // One line, the title, wherever a second says nothing more ("just now", "try again"): only a refusal's reason
    // and "open Yo once to sign in" stay under it.
    switch failure {
    case nil:
      (title, detail) = (back ? "Yo back to \(name)" : "Sent Yo! to \(name)", "")
    case .notNow?:
      (title, detail) = ("Yo to \(name) didn't go out", "")
    case .refused(let why)?:
      (title, detail) = ("Yo to \(name) not sent", why)
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
      .init(outcome: .nobody, title: "No one to Yo back", detail: ""))
  }

  /// Starts it and opens it big at once (an update with an alert opens the island; a banner on an iPhone without
  /// one), and ends it `shown` later. The intent does not wait for that: it returns at once, so iOS's own "running"
  /// mark (the control's blue hand) leaves the island right away. Not ended at once with a later dismissal: an ended
  /// activity leaves the Dynamic Island immediately (the dismissal policy holds only on the Lock Screen).
  /// Every reason it does not show is counted (CountedErrors, "LiveActivity"), for the admin page.
  private static func present(_ attributes: YoSentAttributes, _ state: YoSentAttributes.ContentState) async {
    // Only the app's process may start one: an extension (the controls' intents run in YoControls when the App
    // Intents metadata does not mark them LiveActivityIntent) is refused by the system.
    guard Bundle.main.bundleURL.pathExtension != "appex" else { return failed("inExtension") }
    guard ActivityAuthorizationInfo().areActivitiesEnabled else { return failed("disabled", expected: true) }
    let content = ActivityContent(state: state, staleDate: .now + Double(shown.components.seconds))
    // One at a time: an earlier one still there (its process suspended) goes now.
    for old in Activity<YoSentAttributes>.activities { await old.end(nil, dismissalPolicy: .immediate) }
    let activity: Activity<YoSentAttributes>
    do {
      activity = try Activity.request(attributes: attributes, content: content)
    } catch {
      return failed("request " + String(describing: error))
    }
    // Not the system's alert sound: a soft click of Yo's own (Resources/YoClick.caf, in the app's bundle).
    await activity.update(
      content,
      alertConfiguration: AlertConfiguration(
        title: "\(state.title)", body: "\(state.detail)", sound: .named("YoClick.caf")))
    endLater(activity)
  }

  /// Ends it after `shown`, with the time the system gives a process that is about to be suspended; if that time
  /// ends first, it ends then (never left on until its next start).
  private static func endLater(_ activity: Activity<YoSentAttributes>) {
    let id = activity.id
    ProcessInfo.processInfo.performExpiringActivity(withReason: "Yo sent") { expired in
      let done = DispatchSemaphore(value: 0)
      Task.detached {
        if !expired { try? await Task.sleep(for: shown) }
        for running in Activity<YoSentAttributes>.activities where running.id == id {
          await running.end(nil, dismissalPolicy: .immediate)
        }
        done.signal()
      }
      // The block keeps the process awake until it returns; on `expired` it is called again and ends it now.
      _ = done.wait(timeout: .now() + .seconds(Int(shown.components.seconds) + 2))
    }
  }

  private static func failed(_ reason: String, expected: Bool = false) {
    let process = Bundle.main.bundleURL.pathExtension == "appex" ? "extension" : "app"
    CountedErrors.record(domain: "LiveActivity", place: "YoSentActivity." + process, code: reason, expected: expected)
    #if canImport(os)
    Logger(subsystem: "app.justyo", category: "live-activity").error("not shown: \(reason, privacy: .public)")
    #endif
  }
}
