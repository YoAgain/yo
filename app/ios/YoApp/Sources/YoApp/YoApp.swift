// The app: one scene, the list of friends. SwiftUI's own lifecycle; a UIKit app delegate only for the push token.
import SwiftUI
import UserNotifications

@main
struct YoApp: App {
  @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
  @State private var inbox: Inbox
  @State private var friends = Friends()
  @State private var accounts = AccountStore()
  /// Photos and videos on their way out (Media.swift).
  @State private var outbox = MediaOutbox()
  @Environment(\.scenePhase) private var phase
  private let notifications: NotificationDelegate

  init() {
    // The state's lock in the App Group is held inside a background task: never while suspended (SharedState.swift).
    // (Begun and ended on any thread: UIKit allows it for these two.)
    let application = UIApplication.shared
    SharedStateFile.hold = {
      // Ended by its step, or by iOS's warning that time is up (the step then ends as it can; see `hold`).
      let task = Ended()
      task.id = application.beginBackgroundTask(withName: "yo.state") { task.end(application) }
      return { task.end(application) }
    }
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
      .onChange(of: accounts.account == nil) { _, out in
        // Out, however (signed out, deleted, or the account taken to another iPhone): this phone's friends go too.
        if out {
          friends.reset()
          inbox.forgetMedia()
          outbox.clear()
          ShareInbox.clear()
        }
      }
      // The share extension shows the list as it is (ShareBridge).
      .onChange(of: accounts.shareList(friends), initial: true) { _, list in
        ShareInbox.publish(list)
      }
      // The core opened (after a start or a sign-in): what waits goes.
      .onChange(of: accounts.client == nil, initial: true) { _, none in
        guard !none, let client = accounts.client else { return }
        outbox.received = { [inbox, friends] events, contacts, client in
          await inbox.handle(events, contacts: contacts, client: client, friends: friends)
        }
        outbox.flush(accounts: accounts)
        ShareInbox.take(outbox: outbox, accounts: accounts)
        inbox.fetchMedia(client: client)
      }
        .environment(inbox)
        .environment(friends)
        .environment(accounts)
        .environment(outbox)
        // Notifications are asked for once there is someone to get Yos from: with the list, not over the welcome.
        .task(id: accounts.account == nil) {
          guard accounts.account != nil else { return }
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
          // What the hub said "later" to goes now (the outbox's events go to the inbox, set when the core opened).
          outbox.flush(accounts: accounts)
          ShareInbox.take(outbox: outbox, accounts: accounts)  // what was shared meanwhile
          if let client = accounts.client { inbox.fetchMedia(client: client) }
          // Opened: the 365 days start again, and so does the reminder (spec 6.4).
          if let account = accounts.account { SecureReminder.update(secured: account.secured) }
        }
    }
  }
}

/// A background task, ended once.
private final class Ended: @unchecked Sendable {
  private let lock = NSLock()
  var id = UIBackgroundTaskIdentifier.invalid

  func end(_ application: UIApplication) {
    let id = lock.withLock {
      defer { self.id = .invalid }
      return self.id
    }
    if id != .invalid { application.endBackgroundTask(id) }
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
    // The Yo's own id when the notification extension (YoNotify) knew it: the same Yo from the core is not shown twice.
    await inbox.received(id: info["id"] as? String ?? notification.request.identifier, from: sender)
    return []
  }

  /// A tap on the notification opens the app, where the Yo shows on its row. ("Yo back" right on it comes with push.)
  func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
    let info = response.notification.request.content.userInfo
    let sender = info["from"] as? String ?? "Unknown sender"
    switch response.actionIdentifier {
    case UNNotificationDefaultActionIdentifier:
      await inbox.received(id: info["id"] as? String ?? response.notification.request.identifier, from: sender)
    default:
      break
    }
  }
}
