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
exit(0)
