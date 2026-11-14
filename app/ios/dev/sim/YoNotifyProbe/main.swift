// The notification service, run by hand: the Simulator does not start a notification service extension for
// `simctl push`, and has no APNs token without a device identity. This runs YoNotify's own code (the same sources, Yo
// Dev's App Group) in a process of its own, beside the app, on the hub's push as it comes, and prints what it made of
// it as an APNs payload: `simctl push` that to Yo Dev to see the notification (dev/sim/push.sh).
import Foundation
import UserNotifications

let content = UNMutableNotificationContent()
content.title = "Yo"
content.body = "Yo"
content.sound = UNNotificationSound(named: UNNotificationSoundName("yo.caf"))
let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)

// TEST ONLY: YO_PROBE_POST=1 posts what the service made into this process's own Notification Center (provisional:
// no prompt), so the service's next run finds it delivered, merges and replaces it, as on the iPhone, where the
// service and the app share one center. (`simctl push` to Yo Dev cannot be seen nor replaced by the service here.)
let post = ProcessInfo.processInfo.environment["YO_PROBE_POST"] == "1"
final class Shown: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void
  ) { done([.list, .banner]) }
}
let shown = Shown()
if post {
  UNUserNotificationCenter.current().delegate = shown
  let asked = DispatchSemaphore(value: 0)
  UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .provisional]) { ok, error in
    FileHandle.standardError.write(Data("probe: authorized \(ok) \(String(describing: error))\n".utf8))
    asked.signal()
  }
  asked.wait()
}

final class Out: @unchecked Sendable { var content: UNNotificationContent? }
let out = Out()
let done = DispatchSemaphore(value: 0)
let service = NotificationService()
let start = Date.now
service.didReceive(request) { made in
  out.content = made
  done.signal()
}
// iOS gives a service about 30 seconds, then asks for what it has.
if done.wait(timeout: .now() + 28) == .timedOut {
  service.serviceExtensionTimeWillExpire()
  done.wait()
}

let made = out.content!
var aps: [String: Any] = ["alert": ["title": made.title, "body": made.body]]
if made.sound != nil { aps["sound"] = "yo.caf" }
if !made.threadIdentifier.isEmpty { aps["thread-id"] = made.threadIdentifier }
if !made.categoryIdentifier.isEmpty { aps["category"] = made.categoryIdentifier }
if made.interruptionLevel == .passive { aps["interruption-level"] = "passive" }
var payload: [String: Any] = ["aps": aps]
for (key, value) in made.userInfo { if let key = key as? String { payload[key] = value } }
let json = try! JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
FileHandle.standardError.write(Data("probe: \(String(format: "%.2f", Date.now.timeIntervalSince(start))) s\n".utf8))
print(String(decoding: json, as: UTF8.self))
if post {
  let added = DispatchSemaphore(value: 0)
  UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: made, trigger: UNTimeIntervalNotificationTrigger(timeInterval: 1, repeats: false))) {
    FileHandle.standardError.write(Data("probe: posted \(String(describing: $0))\n".utf8))
    added.signal()
  }
  added.wait()
  // Delivered after this process is gone (a process in front gets no notification of its own on the screen).
  let listed = DispatchSemaphore(value: 0)
  UNUserNotificationCenter.current().getDeliveredNotifications { all in
    for n in all.sorted(by: { $0.date > $1.date }) {
      FileHandle.standardError.write(Data("probe: delivered \(n.request.identifier) [\(n.request.content.categoryIdentifier)] \(n.request.content.body.replacingOccurrences(of: "\n", with: " | "))\n".utf8))
    }
    listed.signal()
  }
  listed.wait()
}
// TEST ONLY: YO_PROBE_LINGER=S keeps the process S seconds after delivering (a service iOS lets live on).
if let linger = ProcessInfo.processInfo.environment["YO_PROBE_LINGER"].flatMap(Double.init) {
  fflush(stdout)
  Thread.sleep(forTimeInterval: linger)
  FileHandle.standardError.write(Data("probe: lingered \(linger) s\n".utf8))
}
exit(0)
