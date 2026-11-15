// The app: one scene, the list of friends. SwiftUI's own lifecycle; a UIKit app delegate only for the push token.
import SwiftUI
import UserNotifications

@main
struct YoApp: App {
  @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
  @State private var inbox: Inbox
  @State private var friends: Friends
  @State private var accounts: AccountStore
  /// Photos and videos on their way out (Media.swift).
  @State private var outbox = MediaOutbox()
  @Environment(\.scenePhase) private var phase
  private let notifications: NotificationDelegate

  init() {
    Perf.mark("launch.init")
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
    let friends = Friends()
    let accounts = AccountStore()
    _inbox = State(initialValue: inbox)
    _friends = State(initialValue: friends)
    _accounts = State(initialValue: accounts)
    notifications = NotificationDelegate(inbox: inbox, friends: friends, accounts: accounts)
    // Friends known by user id are kept for the account too (its storage, spec 6.9).
    friends.stored = { [weak accounts] changed, removed in accounts?.store(changed, removed: removed) }
    UNUserNotificationCenter.current().delegate = notifications
    CrashReports.shared.start()  // crashes iOS hands over (MetricKit) go to the Yo team, if on
  }

  var body: some Scene {
    WindowGroup {
      // No account yet (or after "Reset Yo"): the way in; then the list.
      // A build the hub no longer takes (426): only the update screen, nothing behind it (VoiceOver included).
      Group {
        if AppUpdate.shared.needed {
          UpdateScreen().transition(.opacity)
        } else if accounts.account == nil {
          Onboarding().transition(.opacity)
        } else {
          FriendList().transition(.opacity)
        }
      }
      .animation(.smooth(duration: 0.5), value: accounts.account == nil)
      .animation(.smooth(duration: 0.5), value: AppUpdate.shared.needed)
      // The saved state is gone or does not open: the account on this phone is signed in again (AccountStore.recover).
      .alert(accounts.hubDown ? "Server Not Reachable" : "Can’t Open Your Account", isPresented: Binding(get: { accounts.broken }, set: { _ in })) {
        Button(accounts.hubDown ? "Try Again" : "Sign In Again") { Task { await accounts.recover() } }
      } message: {
        Text(accounts.hubDown
          ? "Yo can’t reach its server right now. Your account is fine; try again in a moment."
          : "Yo can’t open your account on this iPhone. Sign in again to keep using it.\n\n(\(accounts.brokenWhy))")
      }
      .onChange(of: accounts.account == nil) { _, out in
        // Out, however (signed out, deleted, or the account taken to another iPhone): this phone's friends go too.
        if out {
          friends.reset()
          inbox.forgetMedia()
          outbox.clear()
          ShareInbox.clear()
        } else {
          // In, with nobody on the list (a new account, or one signed in again here): one friend who always
          // answers, YoGPT (the list learns its user id on its own).
          if friends.all.isEmpty { friends.add("YoGPT") }
          // An invite link opened on the way in comes next (FriendList), unless it is one's own name.
          if let me = accounts.account?.username { Invite.shared.signedIn(as: me) }
        }
      }
      // Invite links (justyo.co/NAME, yo://add/NAME): the name waits for the list (Invite.swift).
      .onOpenURL { Invite.shared.open($0) }
      .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
        if let url = activity.webpageURL { Invite.shared.open(url) }
      }
      // The share extension shows the list as it is (ShareBridge).
      .onChange(of: accounts.shareList(friends), initial: true) { _, list in
        ShareInbox.publish(list)
      }
      // The core opened (after a start or a sign-in): what waits goes.
      .onChange(of: accounts.client == nil, initial: true) { _, none in
        guard !none, let client = accounts.client else { return }
        outbox.received = { [inbox, friends, accounts] events, contacts, client in
          await inbox.handle(events, contacts: contacts, client: client, accounts: accounts, friends: friends)
        }
        outbox.flush(accounts: accounts)
        CrashReports.shared.flush(client)
        ShareInbox.take(outbox: outbox, accounts: accounts)
        inbox.fetchMedia(client: client)
        // The account's storage: after a restore, the friends and the app icon of the phone before.
        Task { await accounts.takeStorage(into: friends) }
        // Receipts, retries and prekeys due (spec 4.1, 4.5): also for a phone that only receives.
        Task { await inbox.maintain(accounts: accounts, friends: friends) }
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
        .task(id: "\(Push.shared.token ?? "")/\(accounts.account?.userID.base64EncodedString() ?? "")/\(accounts.client != nil)") {
          await Push.shared.send(accounts)
        }
        // Opening the app: the Yos waiting as notifications come in and show on their bars.
        .onChange(of: phase, initial: true) { _, phase in
          guard phase == .active else { return }
          Task { await inbox.collectDelivered() }
          // In front again: receipts, retries and prekeys due (spec 4.1, 4.5); on the first opening, once the core has opened (above).
          Task { await inbox.maintain(accounts: accounts, friends: friends) }
          // What the hub said "later" to goes now (the outbox's events go to the inbox, set when the core opened).
          outbox.flush(accounts: accounts)
          ShareInbox.take(outbox: outbox, accounts: accounts)  // what was shared meanwhile
          if let client = accounts.client { inbox.fetchMedia(client: client) }
          // Opened: the reminder set again, from the hub's last signed request (spec 6.4), extensions' too.
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
  let friends: Friends
  let accounts: AccountStore

  init(inbox: Inbox, friends: Friends, accounts: AccountStore) {
    self.inbox = inbox
    self.friends = friends
    self.accounts = accounts
  }

  // The completion handlers, not the async forms: UIKit takes the answer only on the main thread, and an async
  // form answered from the thread its task ended on (a tap on a notification stopped the app, in UIKit's snapshot
  // update). What the answers need is taken out of the notification first, here.
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping @Sendable (UNNotificationPresentationOptions) -> Void
  ) {
    let info = notification.request.content.userInfo
    // A push that names nobody ("Yo"): the app's own wait at the hub brings the Yo, if there is one.
    guard let sender = info["from"] as? String else { return DispatchQueue.main.async { completionHandler([]) } }
    // The Yo's own id when the notification extension (YoNotify) knew it: the same Yo from the core is not shown twice.
    let id = info["id"] as? String ?? notification.request.identifier
    Task { @MainActor [inbox] in
      inbox.received(id: id, from: sender)
      // The app shows it itself, on its bar ("Yo!!" at its end): no banner.
      completionHandler([])
      // Nor left in Notification Center, seen here already (a moment later: it is listed only after this answer).
      try? await Task.sleep(for: .seconds(0.5))
      await inbox.collectDelivered()
    }
  }

  /// A tap on the notification opens the app, where the Yo shows on its row. "Yo Back" right on it sends one to the
  /// sender without opening the app (in the background, once the core has opened).
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping @Sendable () -> Void
  ) {
    let info = response.notification.request.content.userInfo
    // "Yo": the app opens, the core brings the Yo
    guard let sender = info["from"] as? String else { return DispatchQueue.main.async { completionHandler() } }
    let id = info["id"] as? String ?? response.notification.request.identifier
    let user = info["user"] as? String
    let action = response.actionIdentifier
    Task { @MainActor [inbox] in
      switch action {
      case UNNotificationDefaultActionIdentifier:
        inbox.received(id: id, from: sender)
      case Inbox.yoBack:
        await yoBack(to: sender, user: user)
      default:
        break
      }
      completionHandler()
    }
  }

  /// The sender by user id when the notification service knew it (also someone not on the list yet), else by name.
  @MainActor private func yoBack(to sender: String, user: String?) async {
    guard accounts.account != nil,
      let friend = user.flatMap({ $0.hasPrefix("group:") ? friends.groupBar(String($0.dropFirst(6))) : nil })
        ?? user.flatMap({ $0.hasPrefix("group:") ? nil : friends.friend(user: $0, handles: [sender]) })
        ?? friends.shownFriend(named: sender),
      !friend.blocked
    else { return }
    for _ in 0..<50 where accounts.client == nil { try? await Task.sleep(for: .milliseconds(100)) }  // opening
    _ = await inbox.send(to: friend, accounts: accounts, friends: friends)
  }
}
