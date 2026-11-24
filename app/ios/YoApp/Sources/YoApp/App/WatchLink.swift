// The watch has no hub credentials. Its list and sends belong to the paired iPhone.
import UIKit
import WatchConnectivity

@MainActor
final class WatchLink: NSObject, WCSessionDelegate {
  static let shared = WatchLink()
  private var list: ShareBridge.List?
  private var lastContext: NSDictionary?
  private var sending: [String: Task<Bool, Never>] = [:]

  func start() {
    guard WCSession.isSupported() else { return }
    list = ShareBridge.readList()
    // The notification service can change the sender while the list stays in the same order.
    CFNotificationCenterAddObserver(
      CFNotificationCenterGetDarwinNotifyCenter(), nil,
      { _, _, _, _, _ in Task { @MainActor in WatchLink.shared.refresh() } },
      ShareBridge.lastYoChanged as CFString, nil, .deliverImmediately)
    WCSession.default.delegate = self
    WCSession.default.activate()
  }

  private func context(_ list: ShareBridge.List?) -> [String: Any] {
    guard let list, !list.owner.isEmpty else { return ["v": 1, "owner": "", "friends": []] }
    let friends: [[String: Any]] = list.friends.map {
      ["to": $0.to, "title": $0.title, "color": (($0.color % 8) + 8) % 8]
    }
    var result: [String: Any] = ["v": 1, "owner": list.owner, "friends": friends]
    if let last = ShareBridge.readLastYo(), last.owner == list.owner {
      result["last"] = ["to": last.to, "name": last.name, "at": Double(last.sentAtMs)] as [String: Any]
    }
    return result
  }

  func publish(_ list: ShareBridge.List?) {
    self.list = list
    refresh()
  }

  func refresh() {
    guard WCSession.isSupported() else { return }
    let session = WCSession.default
    guard session.activationState == .activated, session.isPaired, session.isWatchAppInstalled else { return }
    let next = context(list)
    guard lastContext?.isEqual(to: next) != true else { return }
    do {
      try session.updateApplicationContext(next)
      lastContext = next as NSDictionary
    } catch { /* Retry on the next refresh. */ }
  }

  private func republish() {
    lastContext = nil  // A newly active watch needs its own copy.
    refresh()
  }

  nonisolated func session(_ session: WCSession, activationDidCompleteWith state: WCSessionActivationState, error: Error?) {
    if state == .activated { Task { @MainActor in republish() } }
  }

  nonisolated func sessionWatchStateDidChange(_ session: WCSession) {
    Task { @MainActor in republish() }
  }

  nonisolated func sessionDidBecomeInactive(_ session: WCSession) {}
  nonisolated func sessionDidDeactivate(_ session: WCSession) { session.activate() }

  // WCSession owns this escaping reply block. Only the main actor calls it, once per message.
  private struct Reply: @unchecked Sendable {
    let call: ([String: Any]) -> Void
  }

  nonisolated func session(_ session: WCSession, didReceiveMessage message: [String: Any],
    replyHandler: @escaping ([String: Any]) -> Void) {
    let reply = Reply(call: replyHandler)
    if message["sync"] as? Bool == true {
      Task { @MainActor in reply.call(context(ShareBridge.readList())) }
    } else {
      receive(message, reply: reply)
    }
  }

  nonisolated func session(_ session: WCSession, didReceiveUserInfo userInfo: [String: Any]) {
    receive(userInfo, reply: nil)
  }

  nonisolated private func receive(_ message: [String: Any], reply: Reply?) {
    let at = message["at"] as? Double
    guard at.map({ Date().timeIntervalSince1970 - $0 <= 300 }) ?? true,
      let target = message["yo"] as? String, !target.isEmpty, let back = message["back"] as? Bool,
      let raw = message["id"] as? String, let id = UUID(uuidString: raw) else {
      Task { @MainActor in reply?.call(["ok": false]) }
      return
    }
    Task { @MainActor in
      let ok = await send(to: target, back: back, id: id.uuidString)
      reply?.call(["ok": ok])
    }
  }

  private func send(to target: String, back: Bool, id: String) async -> Bool {
    if let pending = sending[id] { return await pending.value }
    let defaults = UserDefaults.standard
    let key = "watch.handled"
    var handled = defaults.array(forKey: key) as? [[String: String]] ?? []
    if let previous = handled.first(where: { $0["id"] == id }) { return previous["ok"] == "true" }

    // Claim before the network call: a lost reply may arrive again as queued user info, even after relaunch.
    handled = Array(handled.suffix(99)) + [["id": id]]
    defaults.set(handled, forKey: key)
    guard let owner = ShareBridge.readList()?.owner ?? ShareBridge.readLastYo()?.owner, !owner.isEmpty else { return false }
    let task = Task {
      let application = UIApplication.shared
      let time = Ended(application)
      _ = time.start(application.beginBackgroundTask(withName: "yo.watch") { time.end() })
      defer { time.end() }
      do {
        try await YoSend.send(to: target, owner: owner)
        WidgetSent.note(to: target, back: back)
        var recent = defaults.array(forKey: key) as? [[String: String]] ?? []
        if let index = recent.firstIndex(where: { $0["id"] == id }) {
          recent[index]["ok"] = "true"
          defaults.set(recent, forKey: key)
        }
        return true
      } catch { return false }
    }
    sending[id] = task
    defer { sending[id] = nil }
    return await task.value
  }
}
