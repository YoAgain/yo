// "Yo" beside the camera and the friend's name on its other side, in the Dynamic Island (and on the Lock Screen),
// for a moment after a Yo from outside the app: the Action button, Control Center, a widget, Shortcuts or Siri
// (YoIntents.swift). Inside the app the bar says it, so nothing starts there. It starts as the intent starts, before
// the state opens and the Yo goes ("Yo" faded: sending), so our pill stands in the island instead of iOS's running
// mark; then "Yo" in full (sent) or "No Yo" in red (did not go). No alert: it never opens big or sounds by itself.
// The attributes and the start are compiled into the app and into YoControls (linked), whose widget bundle draws it
// (YoLiveActivity.swift); only the app's process may start one, which is why the sending intents are
// LiveActivityIntents.
import ActivityKit
import Foundation
#if canImport(os)
import os
#endif

/// The Yos from outside the app, one after the other in one activity: the newest one's friend, and how it went. All
/// in the state, so a second press updates the one showing (the name switching, or "2× Yo") instead of ending it
/// and sliding in another.
struct YoSentAttributes: ActivityAttributes {
  struct ContentState: Codable, Hashable, Sendable {
    enum Outcome: String, Codable, Hashable, Sendable {
      /// On its way: "Yo" faded.
      case sending
      /// It went: "Yo" in full.
      case sent
      /// Not now (offline), or refused: "No Yo" in red.
      case failed
      /// Yo Back with no Yo to answer: "No Yo", "nobody yet".
      case nobody
    }

    var outcome: Outcome
    /// The friend's or group's name as the list shows it ("" for nobody).
    var name: String
    /// The bar's colour (Color.row); nil: not on the list, Yo's purple.
    var color: Int?
    /// A Yo back to whoever sent the last one.
    var back = false
    /// Yos to this friend in a row while it showed: "2× Yo".
    var count = 1
    /// Friends Yo'd before this one while it showed, newest first ("+ Kevin, lina"); the pill grows, never stacks.
    var others: [String] = []
    /// One of those whose Yo did not go ("No Yo to Kevin").
    var missed: String?
    /// All Yos while it showed: "3 Yo" in the small island once there were others.
    var total = 1
  }
}

enum YoSentActivity {
  /// How long it stays once the Yo went or did not.
  static let shown: Duration = .seconds(3)
  /// How long "sending" may stand: if no outcome came by then (the process was suspended mid-send), the island
  /// says "Yo?" instead of a faded "Yo" for good.
  static let sendingFor: TimeInterval = 30

  /// How long the steps of an intent took (from its start): seen in the device's log under "YoIntentTiming".
  struct Timing: Sendable {
    let start = ContinuousClock.now

    /// The first mark: how long the process had been running when the intent began (a cold launch's cost).
    init() {
      var info = kinfo_proc()
      var size = MemoryLayout<kinfo_proc>.stride
      var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
      var age = -1.0
      if sysctl(&mib, 4, &info, &size, nil, 0) == 0 {
        let started = info.kp_proc.p_un.__p_starttime
        age = (Date().timeIntervalSince1970 - (Double(started.tv_sec) + Double(started.tv_usec) / 1e6)) * 1000
      }
      #if canImport(os)
      Logger(subsystem: "app.justyo", category: "YoIntentTiming")
        .notice("perform start, process age \(age, format: .fixed(precision: 0), privacy: .public) ms")
      #endif
    }

    func mark(_ step: String) {
      let ms = (ContinuousClock.now - start).components
      let elapsed = Double(ms.seconds) * 1000 + Double(ms.attoseconds) / 1e15
      #if canImport(os)
      Logger(subsystem: "app.justyo", category: "YoIntentTiming")
        .notice("\(step, privacy: .public) \(elapsed, format: .fixed(precision: 0), privacy: .public) ms")
      #endif
    }
  }

  /// Starts it at once, "sending", before the Yo goes; or, while one still shows, updates that one: the same friend
  /// counts up ("2× Yo"), another takes its place, and it stays `shown` from this Yo's end. Nil when none may show
  /// (counted).
  static func start(name: String, color: Int?, back: Bool) async -> Activity<YoSentAttributes>? {
    let current = Activity<YoSentAttributes>.activities.first { $0.activityState == .active }
    if let current {
      let old = current.content.state
      var state = YoSentAttributes.ContentState(outcome: .sending, name: name, color: color, back: back)
      if old.outcome != .nobody {
        let again = old.name == name
        state.count = again ? old.count + 1 : 1
        state.others = again ? old.others : [old.name] + old.others.filter { $0 != name && $0 != old.name }
        state.missed = old.outcome == .failed && !again ? old.name : old.missed.flatMap { $0 == name ? nil : $0 }
        state.total = old.total + 1
      }
      _ = Ending.next(current.id)  // its end, due from the Yo before, waits for this one
      await current.update(ActivityContent(state: state, staleDate: .now + sendingFor))
      return current
    }
    return await request(.init(outcome: .sending, name: name, color: color, back: back), staleDate: .now + sendingFor)
  }

  /// The Yo went (`failed` false) or did not: the faded "Yo" turns full, or "No Yo"; gone `shown` later.
  static func finish(_ activity: Activity<YoSentAttributes>?, failed: Bool) async {
    guard let activity else { return }
    var state = activity.content.state
    state.outcome = failed ? .failed : .sent
    await activity.update(ActivityContent(state: state, staleDate: nil))
    endLater(activity)
  }

  /// `finish` from a thread that must not return before it is done (a performExpiringActivity block, whose return
  /// lets iOS suspend the process): at most two seconds.
  static func finishWaiting(_ activity: Activity<YoSentAttributes>?, failed: Bool) {
    final class Box: @unchecked Sendable {
      let activity: Activity<YoSentAttributes>?
      init(_ activity: Activity<YoSentAttributes>?) { self.activity = activity }
    }
    let box = Box(activity)
    let done = DispatchSemaphore(value: 0)
    Task.detached {
      await finish(box.activity, failed: failed)
      done.signal()
    }
    _ = done.wait(timeout: .now() + 2)
  }

  /// Yo Back with no Yo to answer.
  static func showNobody() async {
    let state = YoSentAttributes.ContentState(outcome: .nobody, name: "nobody yet", color: nil, back: true)
    if let current = Activity<YoSentAttributes>.activities.first(where: { $0.activityState == .active }) {
      _ = Ending.next(current.id)
      await current.update(ActivityContent(state: state, staleDate: nil))
      return endLater(current)
    }
    guard let activity = await request(state) else { return }
    endLater(activity)
  }

  /// Which end of an activity is due: each update makes the ends planned before it void.
  private enum Ending {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var generations: [String: Int] = [:]

    static func next(_ id: String) -> Int {
      lock.withLock {
        generations[id, default: 0] += 1
        return generations[id]!
      }
    }

    static func due(_ id: String, _ generation: Int) -> Bool { lock.withLock { generations[id] == generation } }
  }

  /// Every reason it does not show is counted (CountedErrors, "LiveActivity"), for the admin page.
  private static func request(
    _ state: YoSentAttributes.ContentState, staleDate: Date? = nil
  ) async -> Activity<YoSentAttributes>? {
    // Only the app's process may start one: an extension (the controls' intents run in YoControls when the App
    // Intents metadata does not mark them LiveActivityIntent) is refused by the system.
    guard Bundle.main.bundleURL.pathExtension != "appex" else { failed("inExtension"); return nil }
    guard ActivityAuthorizationInfo().areActivitiesEnabled else { failed("disabled", expected: true); return nil }
    // One at a time: an earlier one no longer active (ended, stale) goes now, without holding this one up.
    let earlier = Activity<YoSentAttributes>.activities
    if !earlier.isEmpty {
      Task.detached { for old in earlier { await old.end(nil, dismissalPolicy: .immediate) } }
    }
    do {
      let content = ActivityContent(state: state, staleDate: staleDate)
      let activity = try Activity.request(attributes: YoSentAttributes(), content: content)
      // Opened big every time (an update with an alert opens the island), without a sound: a silent file.
      Task {
        await activity.update(
          content, alertConfiguration: AlertConfiguration(title: "Yo", body: "", sound: .named("YoSilent.wav")))
      }
      return activity
    } catch {
      failed("request " + String(describing: error))
      return nil
    }
  }

  /// Ends it after `shown`, with the time the system gives a process that is about to be suspended; if that time
  /// ends first, it ends then (never left on until its next start). An ended activity leaves the island at once.
  private static func endLater(_ activity: Activity<YoSentAttributes>) {
    let id = activity.id
    let generation = Ending.next(id)
    ProcessInfo.processInfo.performExpiringActivity(withReason: "Yo sent") { expired in
      let done = DispatchSemaphore(value: 0)
      Task.detached {
        if !expired { try? await Task.sleep(for: shown) }
        // Updated meanwhile (another Yo): that one's end is due, not this.
        if expired || Ending.due(id, generation) {
          for running in Activity<YoSentAttributes>.activities where running.id == id {
            await running.end(nil, dismissalPolicy: .immediate)
          }
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
