// The app: one scene, the list of friends. SwiftUI's own lifecycle; a UIKit app delegate only for the push token.
import SwiftUI
import UserNotifications

@main
struct YoApp: App {
  @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
  @State private var inbox: Inbox
  @State private var friends = Friends()
  @State private var accounts = AccountStore()
  @Environment(\.scenePhase) private var phase
  private let notifications: NotificationDelegate

  init() {
    let inbox = Inbox()
    _inbox = State(initialValue: inbox)
    notifications = NotificationDelegate(inbox: inbox)
    UNUserNotificationCenter.current().delegate = notifications
  }

  var body: some Scene {
    WindowGroup {
      // No account yet (or after "Reset Yo"): the way in; then the list.
      Group {
        if accounts.account == nil {
          Onboarding().transition(.opacity)
        } else {
          FriendList().transition(.opacity)
        }
      }
      .animation(.smooth(duration: 0.5), value: accounts.account == nil)
        .environment(inbox)
        .environment(friends)
        .environment(accounts)
        .task {
          await inbox.askForNotifications()
          Push.shared.register()
        }
        // The push token to the hub, once there is both a token and an account (and again for a new account).
        .task(id: "\(Push.shared.token ?? "")/\(accounts.account?.userID.base64EncodedString() ?? "")") {
          await Push.shared.send(accounts)
        }
        // Opening the app: the Yos waiting as notifications come in and show on their bars.
        .onChange(of: phase, initial: true) { _, phase in
          guard phase == .active else { return }
          Task { await inbox.collectDelivered() }
          // Opened: the 365 days start again, and so does the reminder (spec 6.4).
          if let account = accounts.account { SecureReminder.update(secured: account.secured) }
        }
    }
  }
}

/// A Yo notification while the app is in front: the app shows it itself (on the sender's row, with the sound), not
/// the system. In the background the system shows it, with the Yo sound and "Yo back".
final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate, Sendable {
  let inbox: Inbox

  init(inbox: Inbox) {
    self.inbox = inbox
  }

  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification
  ) async -> UNNotificationPresentationOptions {
    let info = notification.request.content.userInfo
    let sender = info["from"] as? String ?? "Unknown sender"
    await inbox.received(id: notification.request.identifier, from: sender)
    return []
  }

  /// A tap on the notification opens the app, where the Yo shows on its row. ("Yo back" right on it comes with push.)
  func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
    let sender = response.notification.request.content.userInfo["from"] as? String ?? "Unknown sender"
    switch response.actionIdentifier {
    case UNNotificationDefaultActionIdentifier:
      await inbox.received(id: response.notification.request.identifier, from: sender)
    default:
      break
    }
  }
}
