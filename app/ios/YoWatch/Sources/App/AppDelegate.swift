import UserNotifications
import WatchKit

@MainActor final class AppDelegate: NSObject, WKApplicationDelegate, UNUserNotificationCenterDelegate {
  func applicationDidFinishLaunching() {
    let center = UNUserNotificationCenter.current()
    center.delegate = self
    let action = UNNotificationAction(identifier: "yo.back", title: "Yo Back", options: [.foreground],
                                      icon: UNNotificationActionIcon(systemImageName: "arrowshape.turn.up.left.fill"))
    center.setNotificationCategories([UNNotificationCategory(identifier: "yo", actions: [action],
                                                            intentIdentifiers: [], options: [])])
    WatchLink.shared.start()
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                          didReceive response: UNNotificationResponse,
                                          withCompletionHandler completionHandler: @escaping @Sendable () -> Void) {
    if response.actionIdentifier == "yo.back" {
      let user = response.notification.request.content.userInfo["user"] as? String
      Task { @MainActor in WatchLink.shared.requestYoBack(to: user) }
    }
    completionHandler()
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
