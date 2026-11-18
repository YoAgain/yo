// The app: one scene, the list of friends. SwiftUI's own lifecycle; a UIKit app delegate only for the push token and
// silent pushes.
import SwiftUI
import UserNotifications
import YoCore

@MainActor @Observable final class DeleteNotice {
  static let shared = DeleteNotice()
  let model = YoList()
  /// The list model's clock, in ms: it keeps counting while the phone sleeps, so a seen Yo's fade runs in real time.
  nonisolated static var now: UInt64 { clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW) / 1_000_000 }
  var receive: ((ListSnapshot) -> Void)?
  var banner: String?
  var deletion: ListDeletion?
  var undo: (() -> Void)?
}

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
    OneTimeReset.runIfDue()  // before anything opens the state
    Perf.mark("launch.init")
    MediaSignWarmup.start()
    Haptic.prepareArrival()
    // Every turn is revocable, by leaving the foreground or by background time expiring.
    let application = UIApplication.shared
    let foreground = ForegroundTurns(active: application.applicationState == .active)
    SharedAccess.foreground = { foreground.isActive }
    SharedAccess.hold = { expire in
      if let end = foreground.hold(expire) { return end }
      let task = Ended(application)
      let id = application.beginBackgroundTask(withName: "yo.shared") {
        expire()
        task.end()
      }
      guard task.start(id) else { return nil }
      return { task.end() }
    }
    // A Yo from the Action button, Shortcuts or Siri went: a light success tap, only while Yo is in front.
    YoSend.sent = {
      Task { @MainActor in
        guard UIApplication.shared.applicationState == .active else { return }
        UINotificationFeedbackGenerator().notificationOccurred(.success)
      }
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
    // A silent push (the hub's non-urgent deliveries): one short inbox sync in the background (AppDelegate).
    AppDelegate.silentSync = { await inbox.syncOnce(accounts: accounts, friends: friends) }
    UNUserNotificationCenter.current().delegate = notifications
    CrashReports.shared.start()  // crashes iOS hands over (MetricKit) go to the Yo team, if on
  }

  private func updateBanner() {
    let snapshot = DeleteNotice.shared.model.connectivity(
      now: DeleteNotice.now,
      offline: Availability.shared.offline, failure: SendFailure.shared.text)
    if let receive = DeleteNotice.shared.receive { receive(snapshot) }
    else { DeleteNotice.shared.banner = snapshot.banner }
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
      .safeAreaInset(edge: .top, spacing: 0) {
        if DeleteNotice.shared.deletion != nil || DeleteNotice.shared.banner != nil {
          HStack(spacing: 8) {
            if let deletion = DeleteNotice.shared.deletion {
              TimelineView(.animation) { _ in
                Circle().trim(from: 0, to: max(0, min(1,
                  (Double(deletion.deadline) - Double(DeleteNotice.now)) / 3000)))
                  .stroke(.white, lineWidth: 2).rotationEffect(.degrees(-90))
                  .frame(width: 14, height: 14)
              }
              Text("\(deletion.name) deleted").font(.custom("Montserrat-Bold", size: 16))
                .lineLimit(1).minimumScaleFactor(0.5)
              Button("Undo") { DeleteNotice.shared.undo?() }.font(.system(size: 16, weight: .bold))
            } else {
              Circle().fill(.red).frame(width: 7, height: 7)
              Text(DeleteNotice.shared.banner ?? "").font(.custom("Montserrat-Bold", size: 16))
              if DeleteNotice.shared.banner == "Offline" {
                // Trying again, shown as dots that fill up one by one and start over.
                TimelineView(.periodic(from: .now, by: 0.45)) { context in
                  let n = Int(context.date.timeIntervalSinceReferenceDate / 0.45) % 4
                  Text(String(repeating: ".", count: n) + String(repeating: " ", count: 3 - n))
                    .font(.custom("Montserrat-Bold", size: 16)).monospaced()
                }
                .padding(.leading, -6)
              }
            }
          }
          .foregroundStyle(.white)
          .frame(maxWidth: .infinity).frame(height: 36)
          .background(Color.amethyst)
          .onTapGesture { if DeleteNotice.shared.deletion == nil && !Availability.shared.offline { SendFailure.shared.retry?() } }
          .accessibilityAddTraits(SendFailure.shared.retry == nil ? [] : [.isButton])
          .accessibilityAction { if DeleteNotice.shared.deletion == nil && !Availability.shared.offline { SendFailure.shared.retry?() } }
          .transition(.opacity)
        }
      }
      .animation(.easeInOut(duration: 0.2), value: DeleteNotice.shared.banner)
      .animation(.easeInOut(duration: 0.2), value: DeleteNotice.shared.deletion?.id)
      .onChange(of: Availability.shared.offline, initial: true) { _, _ in updateBanner() }
      .onChange(of: SendFailure.shared.text) { _, _ in updateBanner() }
      .task(id: phase) {
        guard phase == .active else { return }
        while !Task.isCancelled {
          await HubTransport.warmAnonymous()
          do { try await Task.sleep(for: .seconds(20)) } catch { return }
        }
      }
      .task(id: phase) {
        guard phase == .active else { return }
        await accounts.watchState()
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
      .onOpenURL { url in
        // yo://camera and yo://open come from Yo's own intents (Shortcuts, Siri, the Action button, controls).
        if url.scheme?.lowercased() == "yo", url.host()?.lowercased() == "camera" { AppRoute.shared.camera = true }
        else if url.scheme?.lowercased() == "yo", url.host()?.lowercased() == "open" {}
        else { Invite.shared.open(url) }
      }
      .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
        if let url = activity.webpageURL { Invite.shared.open(url) }
      }
      // The share extension shows the list as it is (ShareBridge).
      .onChange(of: accounts.shareList(friends), initial: true) { _, list in
        ShareInbox.publish(list)
      }
      // The core opened (after a start or a sign-in): what waits goes.
      // A crash came and the person was never asked (CrashReports): once, then Settings.
      .alert("Yo Crashed", isPresented: Binding(get: { CrashReports.shared.asking }, set: { _ in })) {
        Button("Always Send") { CrashReports.shared.answer(always: true) }
        Button("Not Now", role: .cancel) { CrashReports.shared.answer(always: false) }
      } message: {
        Text("Send anonymous crash and error reports to the Yo team? They hold code details, build, iOS version and model, with no Yos, names or photos.")
      }
      .onChange(of: accounts.client == nil, initial: true) { _, none in
        guard !none, let client = accounts.client else { return }
        outbox.received = { [inbox, friends, accounts] events, contacts, client in
          await inbox.handle(events, contacts: contacts, client: client, accounts: accounts, friends: friends)
        }
        outbox.flush(accounts: accounts)
        CrashReports.shared.flush()
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
          await accounts.refreshSupport()
        }
        // Opening the app: the Yos waiting as notifications come in and show on their bars.
        .onChange(of: phase, initial: true) { _, phase in
          guard phase == .active else { return }
          Task { await inbox.collectDelivered() }
          Task { await accounts.refreshSupport() }
          // In front again: receipts, retries and prekeys due (spec 4.1, 4.5); on the first opening, once the core has opened (above).
          Task { await inbox.maintain(accounts: accounts, friends: friends) }
          // What the hub said "later" to goes now (the outbox's events go to the inbox, set when the core opened).
          outbox.flush(accounts: accounts)
          ShareInbox.take(outbox: outbox, accounts: accounts)  // what was shared meanwhile
          if let client = accounts.client { inbox.fetchMedia(client: client) }
          // The old reminder to secure the account (removed with the rule behind it), still pending from earlier builds.
          UNUserNotificationCenter.current().removePendingNotificationRequests(withIdentifiers: ["yo.secure"])
        }
    }
  }
}

/// A background task, ended once.
private final class Ended: @unchecked Sendable {
  private let application: UIApplication

  init(_ application: UIApplication) { self.application = application }
  private let lock = NSLock()
  private var id = UIBackgroundTaskIdentifier.invalid
  private var ended = false

  func start(_ id: UIBackgroundTaskIdentifier) -> Bool {
    let keep = lock.withLock {
      guard !ended, id != .invalid else { return false }
      self.id = id
      return true
    }
    if !keep { finish(id) }  // expiry may have arrived before beginBackgroundTask returned
    return keep
  }

  func end() {
    let id = lock.withLock {
      defer { self.id = .invalid }
      ended = true
      return self.id
    }
    finish(id)
  }

  private func finish(_ id: UIBackgroundTaskIdentifier) {
    if id != .invalid {
      application.endBackgroundTask(id)
    }
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
    let senderName = info["sender"] as? String
    let id = info["id"] as? String ?? notification.request.identifier
    Task { @MainActor [inbox] in
      inbox.received(id: id, from: sender, senderName: senderName)
      // The app shows it itself, on its bar ("Yo!!" at its end): no banner.
      completionHandler([])
      // Nor left in Notification Center, seen here already (a moment later: it is listed only after this answer).
      try? await Task.sleep(for: .seconds(0.5))
      await inbox.collectDelivered()
    }
  }

  /// A tap on the notification opens the app, where the Yo shows on its row. "Yo Back" right on it sends one to the
  /// sender without opening the app, also from the Lock Screen while the iPhone is locked: as the Yo Back intent
  /// does (YoSend), the state as a guest in the App Group, readable after the first unlock. To the notification's
  /// sender (`user`, as the notification service wrote it), else to whoever sent the newest Yo (a bare "Yo" from
  /// the hub, when the service ran out of time).
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping @Sendable () -> Void
  ) {
    let info = response.notification.request.content.userInfo
    let sender = info["from"] as? String
    let senderName = info["sender"] as? String
    let id = info["id"] as? String ?? response.notification.request.identifier
    let user = info["user"] as? String
    let action = response.actionIdentifier
    if action == Inbox.yoBack {
      Task.detached { [self] in
        await yoBack(to: user, named: sender)
        DispatchQueue.main.async { completionHandler() }
      }
      return
    }
    // "Yo": the app opens, the core brings the Yo
    guard let sender, action == UNNotificationDefaultActionIdentifier else {
      return DispatchQueue.main.async { completionHandler() }
    }
    Task { @MainActor [inbox] in
      inbox.received(id: id, from: sender, senderName: senderName)
      completionHandler()
    }
  }

  /// One Yo back as a guest (YoSend, as the Yo Back intent); without a `user` (an older notification) by name
  /// through the app's own client, once it has opened.
  private func yoBack(to user: String?, named sender: String?) async {
    let last = ShareBridge.readLastYo()
    if let owner = ShareBridge.readList()?.owner ?? last?.owner, let target = user ?? (sender == nil ? last?.to : nil) {
      do {
        try await YoSend.send(to: target, owner: owner)
        WidgetSent.note(to: target, back: true)
      } catch {}
      return
    }
    guard let sender else { return }
    await yoBackInApp(named: sender)
  }

  @MainActor private func yoBackInApp(named sender: String) async {
    guard accounts.account != nil, let friend = friends.shownFriend(named: sender), !friend.blocked else { return }
    for _ in 0..<50 where accounts.client == nil { try? await Task.sleep(for: .milliseconds(100)) }  // opening
    _ = await inbox.send(to: friend, accounts: accounts, friends: friends)
  }
}

/// Force font fallback and color glyph rasterization before the first media bar needs them.
private enum MediaSignWarmup {
  static func start() {
    DispatchQueue.global(qos: .userInitiated).async {
      autoreleasepool {
        Perf.timed("media.sign.warmup") {
          let format = UIGraphicsImageRendererFormat()
          format.scale = 3
          _ = UIGraphicsImageRenderer(size: CGSize(width: 160, height: 64), format: format).image { _ in
            ("📷🎥" as NSString).draw(at: .zero, withAttributes: [.font: UIFont.systemFont(ofSize: 36)])
          }
        }
      }
    }
  }
}

/// No main-queue hop while Rust holds its mutex: main-thread identity reads may be waiting for that mutex.
private final class ForegroundTurns: @unchecked Sendable {
  private let gate = NSLock()
  private var active: Bool
  private var turns: [UUID: @Sendable () -> Void] = [:]
  private var observers: [NSObjectProtocol] = []

  init(active: Bool) {
    self.active = active
    observers = [
      NotificationCenter.default.addObserver(
        forName: UIApplication.didBecomeActiveNotification, object: nil, queue: nil
      ) { [weak self] _ in self?.setActive(true) },
      NotificationCenter.default.addObserver(
        forName: UIApplication.willResignActiveNotification, object: nil, queue: nil
      ) { [weak self] _ in self?.setActive(false) },
    ]
  }

  var isActive: Bool { gate.withLock { active } }

  func hold(_ expire: @escaping @Sendable () -> Void) -> (@Sendable () -> Void)? {
    gate.withLock {
      guard active else { return nil }
      let id = UUID()
      turns[id] = expire
      return { _ = self.gate.withLock { self.turns.removeValue(forKey: id) } }
    }
  }

  private func setActive(_ active: Bool) {
    let expired = gate.withLock {
      self.active = active
      guard !active else { return [@Sendable () -> Void]() }
      let expired = Array(turns.values)
      turns.removeAll()
      return expired
    }
    if !active { SharedAccess.resetContention() }
    for expire in expired { expire() }
  }
}
