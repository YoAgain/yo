// The app: one scene, the list of friends. SwiftUI's own lifecycle, no UIKit app delegate.
import SwiftUI
import UserNotifications

@main
struct YoApp: App {
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
          await yoUpdate()
        }
        // Opening the app: the Yos waiting as notifications come in and show on their bars.
        .onChange(of: phase, initial: true) { _, phase in
          guard phase == .active else { return }
          Task { await inbox.collectDelivered() }
          // Opened: the 30 days start again, and so does the reminder (spec 6.4).
          if let account = accounts.account { SecureReminder.update(secured: account.secured) }
        }
    }
  }
}

extension YoApp {
  /// "Yo Update", a friend of the mockup: a Yo from it whenever a new build of the app starts (after every
  /// `dev/push.sh`), so a new version announces itself. Known by the app's executable changing.
  @MainActor func yoUpdate() async {
    let name = "Yo Update"
    guard accounts.account != nil else { return }  // the welcome says Yo itself; the update waits for the list
    guard let url = Bundle.main.executableURL,
      let built = (try? FileManager.default.attributesOfItem(atPath: url.path))?[.modificationDate] as? Date
    else { return }
    let key = "yoUpdate.built"
    let stamp = built.timeIntervalSince1970
    guard UserDefaults.standard.double(forKey: key) != stamp else { return }
    UserDefaults.standard.set(stamp, forKey: key)
    if !friends.all.contains(where: { $0.name == name }) { friends.add(name) }
    try? await Task.sleep(for: .seconds(1))  // the list is on screen
    await inbox.fakeYo(from: name, after: .zero)
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

  /// "Yo back" on the notification: the Yo goes without the app opening (in the mockup: that friend's Yo comes
  /// back after two seconds). A tap on the notification itself opens the app, where the Yo shows on its row.
  func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
    let sender = response.notification.request.content.userInfo["from"] as? String ?? "Unknown sender"
    switch response.actionIdentifier {
    case Inbox.yoBack:
      await inbox.fakeYo(from: sender, after: .seconds(2))
    case UNNotificationDefaultActionIdentifier:
      await inbox.received(id: response.notification.request.identifier, from: sender)
    default:
      break
    }
  }
}
