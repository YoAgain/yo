import UserNotifications
import WatchKit

@MainActor final class AppDelegate: NSObject, WKApplicationDelegate, UNUserNotificationCenterDelegate {
  func applicationDidFinishLaunching() {
    let center = UNUserNotificationCenter.current()
    center.delegate = self
    let action = UNNotificationAction(identifier: "yo.back", title: "Yo Back", options: [],
                                      icon: UNNotificationActionIcon(templateImageName: "yo.back"))
    center.setNotificationCategories([UNNotificationCategory(identifier: "yo", actions: [action],
                                                            intentIdentifiers: [], options: [])])
    WatchLink.shared.start()
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                          didReceive response: UNNotificationResponse,
                                          withCompletionHandler completionHandler: @escaping @Sendable () -> Void) {
    guard response.actionIdentifier == "yo.back" else {
      completionHandler()
      return
    }
    let user = response.notification.request.content.userInfo["user"] as? String
    Task { @MainActor in
      defer { completionHandler() }
      let link = WatchLink.shared
      guard let to = user ?? link.state.last?.to else {
        WKInterfaceDevice.current().play(.failure)
        return
      }
      let sent = await link.send(to: to, back: true)
      WKInterfaceDevice.current().play(sent ? .success : .failure)
    }
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                          willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
    [.banner, .sound, .list]
  }

  func handle(_ backgroundTasks: Set<WKRefreshBackgroundTask>) {
    for task in backgroundTasks {
      if let transfer = task as? WKWatchConnectivityRefreshBackgroundTask {
        WatchLink.shared.handle(transfer)
      } else {
        task.setTaskCompletedWithSnapshot(false)
      }
    }
  }
}
