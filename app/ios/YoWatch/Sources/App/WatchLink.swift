import Foundation
import Observation
import WatchConnectivity
import WatchKit
import WidgetKit

// Preserve the session queue's delivery order when handing snapshots to the UI.
private final class SessionDelegate: NSObject, WCSessionDelegate {
  func session(_ session: WCSession, activationDidCompleteWith activationState: WCSessionActivationState,
               error: (any Error)?) {
    let activated = activationState == .activated
    let cached = activated ? WatchState(context: session.receivedApplicationContext) : nil
    DispatchQueue.main.async { WatchLink.shared.activated(activated, cached: cached) }
  }

  func sessionReachabilityDidChange(_ session: WCSession) {
    if session.isReachable { DispatchQueue.main.async { WatchLink.shared.sync() } }
  }

  func session(_ session: WCSession, didReceiveApplicationContext applicationContext: [String: Any]) {
    guard let state = WatchState(context: applicationContext) else { return }
    DispatchQueue.main.async { WatchLink.shared.received(state) }
  }
}

@MainActor @Observable final class WatchLink {
  static let shared = WatchLink()
  private(set) var state: WatchState
  var yoBackRequest: (to: String, id: UUID)?
  private let delegate = SessionDelegate()
  private var activating = false
  private var syncID: UUID?
  private var saved: WatchState?
  private var backgroundTasks = Set<WKWatchConnectivityRefreshBackgroundTask>()
  private var pendingObservation: NSKeyValueObservation?

  private init() {
    let stored = WatchState.read()
    state = stored ?? .empty
    saved = stored
  }

  func start() {
    guard WCSession.isSupported(), !activating else { return }
    let session = WCSession.default
    guard session.activationState != .activated else { return }
    activating = true
    session.delegate = delegate
    session.activate()
  }

  fileprivate func activated(_ success: Bool, cached: WatchState?) {
    activating = false
    guard success else {
      completeBackgroundTasks()
      return
    }
    if let cached { apply(cached) }
    sync()
    finishIfDrained()
  }

  fileprivate func sync() {
    let session = WCSession.default
    guard session.activationState == .activated, session.isReachable else { return }
    let id = UUID()
    syncID = id
    session.sendMessage(["sync": true], replyHandler: { @Sendable reply in
      guard let state = WatchState(context: reply) else { return }
      Task { @MainActor in
        let link = WatchLink.shared
        if link.syncID == id { link.apply(state) }
      }
    }, errorHandler: nil)
  }

  fileprivate func received(_ next: WatchState) {
    // A sync started before this delivery may still have a reply in flight.
    syncID = nil
    apply(next)
    finishIfDrained()
  }

  private func apply(_ next: WatchState) {
    state = next
    guard saved != next, next.save() else { return }
    let previous = saved
    saved = next
    if previous?.last != next.last { WidgetCenter.shared.reloadAllTimelines() }
    if previous?.friends != next.friends { YoWatchShortcuts.updateAppShortcutParameters() }
  }

  func requestYoBack(to target: String?) {
    guard let to = target ?? state.last?.to else { return }
    yoBackRequest = (to, UUID())
  }

  func send(to: String, back: Bool) async -> Bool {
    guard !to.isEmpty, WCSession.isSupported() else { return false }
    start()
    let session = WCSession.default
    let deadline = ContinuousClock.now + .seconds(2)
    while session.activationState != .activated, activating, ContinuousClock.now < deadline {
      do { try await Task.sleep(for: .milliseconds(50)) } catch { return false }
    }
    // Just opened (a complication, a notification action, Siri), the session says unreachable for a moment, and a
    // queued transfer may wait long: a few seconds for a live link first.
    let reachableDeadline = ContinuousClock.now + .seconds(3)
    while session.activationState == .activated, !session.isReachable, ContinuousClock.now < reachableDeadline {
      do { try await Task.sleep(for: .milliseconds(50)) } catch { return false }
    }
    // WatchConnectivity accepts transfers only after activation.
    guard session.activationState == .activated else { return false }
    let id = UUID().uuidString
    let at = Date().timeIntervalSince1970
    let message: @Sendable () -> [String: Any] = { ["yo": to, "back": back, "id": id, "at": at] }
    guard session.isReachable else {
      session.transferUserInfo(message())
      return true
    }
    return await withCheckedContinuation { continuation in
      session.sendMessage(message(), replyHandler: { @Sendable reply in
        continuation.resume(returning: reply["ok"] as? Bool == true)
      }, errorHandler: { @Sendable _ in
        Task { @MainActor in
          let session = WCSession.default
          guard session.activationState == .activated else {
            continuation.resume(returning: false)
            return
          }
          session.transferUserInfo(message())
          continuation.resume(returning: true)
        }
      })
    }
  }

  func handle(_ task: WKWatchConnectivityRefreshBackgroundTask) {
    guard WCSession.isSupported() else {
      task.setTaskCompletedWithSnapshot(false)
      return
    }
    backgroundTasks.insert(task)
    start()
    if pendingObservation == nil {
      pendingObservation = WCSession.default.observe(\.hasContentPending, options: [.new]) { @Sendable _, _ in
        Task { @MainActor in WatchLink.shared.finishIfDrained() }
      }
    }
    finishIfDrained()
  }

  private func finishIfDrained() {
    guard !backgroundTasks.isEmpty, WCSession.default.activationState == .activated,
      !WCSession.default.hasContentPending else { return }
    // The delegate's context may still be on its way to the main actor when pending clears.
    if let cached = WatchState(context: WCSession.default.receivedApplicationContext) { apply(cached) }
    completeBackgroundTasks()
  }

  private func completeBackgroundTasks() {
    for task in backgroundTasks { task.setTaskCompletedWithSnapshot(false) }
    backgroundTasks.removeAll()
    pendingObservation = nil
  }
}
