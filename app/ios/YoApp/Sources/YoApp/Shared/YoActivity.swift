// The Dynamic Island pill after a Yo from outside the app (YoIntents.swift). It starts as the intent starts, before
// the state opens, so our pill stands in the island instead of iOS's running mark. Only the app's process may start
// one, which is why the sending intents are LiveActivityIntents; YoControls draws it (YoLiveActivity.swift).
import ActivityKit
import Foundation
import YoCore
import os

/// All in the state, so a second press updates the one showing instead of ending it and sliding in another.
struct YoSentAttributes: ActivityAttributes {
  struct ContentState: Codable, Hashable, Sendable {
    enum Outcome: String, Codable, Hashable, Sendable {
      case sending, sent, failed
      /// Yo Back with no Yo to answer.
      case nobody

      var product: ProductOutcome {
        switch self {
        case .sending: .sending
        case .sent: .sent
        case .failed: .failed
        case .nobody: .nobody
        }
      }
    }

    var outcome: Outcome
    var name: String
    /// Nil: not on the list, Yo's purple.
    var color: Int?
    var back = false
    /// Yos to this friend in a row while it showed.
    var count = 1
    /// Friends Yo'd before this one while it showed, newest first.
    var others: [String] = []
    /// One of those whose Yo did not go.
    var missed: String?
    var total = 1
  }
}

enum YoSentActivity {
  static let shown: Duration = .seconds(Int64(productDisplayTiming().activityShownSecs))
  /// If no outcome came by then (suspended mid-send), the island says "Yo?" instead of a faded "Yo" for good.
  static let sendingFor: TimeInterval = Double(productDisplayTiming().activityStaleSecs)

  /// Intent step timings in the device log ("YoIntentTiming").
  struct Timing: Sendable {
    let start = ContinuousClock.now

    /// Logs the process age at the intent's start (a cold launch's cost).
    init() {
      var info = kinfo_proc()
      var size = MemoryLayout<kinfo_proc>.stride
      var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
      var age = -1.0
      if sysctl(&mib, 4, &info, &size, nil, 0) == 0 {
        let started = info.kp_proc.p_un.__p_starttime
        age = (Date().timeIntervalSince1970 - (Double(started.tv_sec) + Double(started.tv_usec) / 1e6)) * 1000
      }
      Logger(subsystem: "app.justyo", category: "YoIntentTiming")
        .notice("perform start, process age \(age, format: .fixed(precision: 0), privacy: .public) ms")
    }

    func mark(_ step: String) {
      let ms = (ContinuousClock.now - start).components
      let elapsed = Double(ms.seconds) * 1000 + Double(ms.attoseconds) / 1e15
      Logger(subsystem: "app.justyo", category: "YoIntentTiming")
        .notice("\(step, privacy: .public) \(elapsed, format: .fixed(precision: 0), privacy: .public) ms")
    }
  }

  /// Starts "sending", or updates the one still showing. Nil when none may show (counted).
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

  static func finish(_ activity: Activity<YoSentAttributes>?, failed: Bool) async {
    guard let activity else { return }
    var state = activity.content.state
    state.outcome = failed ? .failed : .sent
    await activity.update(ActivityContent(state: state, staleDate: nil))
    endLater(activity)
  }

  /// For a performExpiringActivity block, whose return lets iOS suspend the process: waits at most two seconds.
  static func finishWaiting(_ activity: Activity<YoSentAttributes>?, failed: Bool) {
    let box = Box(activity)
    let done = DispatchSemaphore(value: 0)
    Task.detached {
      await finish(box.activity, failed: failed)
      done.signal()
    }
    _ = done.wait(timeout: .now() + 2)
  }

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

  /// An activity handed across to another thread.
  final class Box: @unchecked Sendable {
    let activity: Activity<YoSentAttributes>?
    init(_ activity: Activity<YoSentAttributes>?) { self.activity = activity }
  }

  /// Each update voids the ends planned before it.
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

  /// Every reason it does not show is counted for the admin page.
  private static func request(
    _ state: YoSentAttributes.ContentState, staleDate: Date? = nil
  ) async -> Activity<YoSentAttributes>? {
    // The system refuses an extension (controls' intents run in YoControls when the metadata lacks LiveActivityIntent).
    guard Bundle.main.bundleURL.pathExtension != "appex" else { failed("inExtension"); return nil }
    guard ActivityAuthorizationInfo().areActivitiesEnabled else { failed("disabled", expected: true); return nil }
    // An earlier one no longer active goes now, without holding this one up.
    let earlier = Activity<YoSentAttributes>.activities
    if !earlier.isEmpty {
      Task.detached { for old in earlier { await old.end(nil, dismissalPolicy: .immediate) } }
    }
    do {
      let content = ActivityContent(state: state, staleDate: staleDate)
      let activity = try Activity.request(attributes: YoSentAttributes(), content: content)
      // An alert opens the island big; the silent file keeps it quiet.
      await activity.update(
        content, alertConfiguration: AlertConfiguration(title: "Yo", body: "", sound: .named("YoSilent.wav")))
      return activity
    } catch {
      failed("request " + String(describing: error))
      return nil
    }
  }

  /// Ends it after `shown` within the process's expiring-activity time; if that runs out first, ends it then.
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
    Logger(subsystem: "app.justyo", category: "live-activity").error("not shown: \(reason, privacy: .public)")
  }
}
