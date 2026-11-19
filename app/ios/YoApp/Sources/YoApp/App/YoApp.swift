// The app: one scene, the list of friends; a UIKit app delegate only for the push token and silent pushes.
import SwiftUI
import UserNotifications
import YoCore

@MainActor @Observable final class DeleteNotice {
  static let shared = DeleteNotice()
  let model = YoList()
  /// The list model's clock in ms: monotonic through sleep, so a seen Yo's fade runs in real time.
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
  @State private var outbox: MediaOutbox
  @Environment(\.scenePhase) private var phase
  private let notifications: NotificationDelegate

  init() {
    Perf.mark("launch.init state \(UIApplication.shared.applicationState.rawValue)")
    PerfProbe.watchLaunchLag()
    PerfProbe.profileLaunch()
    StateStorage.access = StateAccessTime(UIApplication.shared)
    Launch.start()
    // Only once Yo is in front, never in a background launch (Action button, silent push).
    Launch.whenActive { MediaSignWarmup.start() }
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
    // A Yo from the Action button, Shortcuts or Siri: a success tap while Yo is in front.
    YoSend.sent = {
      Task { @MainActor in
        guard UIApplication.shared.applicationState == .active else { return }
        UINotificationFeedbackGenerator().notificationOccurred(.success)
      }
    }
    let inbox = Perf.timed("launch.inbox") { Inbox() }
    let friends = Perf.timed("launch.friends") { Friends() }
    let accounts = Perf.timed("launch.accounts") { AccountStore() }
    _inbox = State(initialValue: inbox)
    _friends = State(initialValue: friends)
    let outbox = MediaOutbox()
    _accounts = State(initialValue: accounts)
    _outbox = State(initialValue: outbox)
    // Another user than the one shown signed in: the list and what waited go.
    accounts.leaving = {
      Self.forget(friends, inbox, outbox)
      friends.add("YoGPT")
    }
    notifications = NotificationDelegate(inbox: inbox, friends: friends, accounts: accounts)
    friends.stored = { [weak accounts] changed, removed in accounts?.store(changed, removed: removed) }
    AppDelegate.silentSync = { await inbox.syncOnce(accounts: accounts, friends: friends) }
    UNUserNotificationCenter.current().delegate = notifications
    // Not in a launch for an intent, nor before the list.
    Launch.later { CrashReports.shared.start() }
    Perf.mark("launch.init.end")
  }

  /// What this phone kept of an account: the list, its media and what waited to go.
  private static func forget(_ friends: Friends, _ inbox: Inbox, _ outbox: MediaOutbox) {
    friends.reset()
    inbox.forgetMedia()
    outbox.clear()
    ShareInbox.clear()
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
      // A build the hub no longer takes (426): only the update screen, nothing behind it (VoiceOver included).
      Group {
        if AppUpdate.shared.needed {
          UpdateScreen().transition(.opacity)
        } else if accounts.account == nil {
          Onboarding().transition(.opacity)
        } else if accounts.updating {
          ProgressView("Yo is updating…").transition(.opacity)
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
      // A scene means Yo opens for the screen: open the core now (a background launch opens it only on demand).
      .onAppear { accounts.openNow() }
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
      .alert("Update Not Finished", isPresented: Binding(get: { accounts.updateStalled && !accounts.broken }, set: { _ in })) {
        Button("Try Again") { accounts.retryUpdate() }
      } message: {
        Text("Yo couldn’t finish updating your account on this iPhone. Nothing is lost. Check your connection and try again.")
      }
      .alert(accounts.hubDown ? "Server Not Reachable" : "Can’t Open Your Account", isPresented: Binding(get: { accounts.broken }, set: { _ in })) {
        Button(accounts.hubDown ? "Try Again" : "Sign In Again") { Task { await accounts.recover() } }
      } message: {
        Text(accounts.hubDown
          ? "Yo can’t reach its server right now. Your account is fine; try again in a moment."
          : "Yo can’t open your account on this iPhone. Sign in again to keep using it.\n\n(\(accounts.brokenWhy))")
      }
      .onChange(of: accounts.account == nil) { _, out in
        // Out (signed out, deleted, or taken to another iPhone): this phone's friends go too.
        if out {
          Self.forget(friends, inbox, outbox)
        } else {
          // A new or re-signed-in account with nobody on the list gets YoGPT, who always answers.
          if friends.all.isEmpty { friends.add("YoGPT") }
          // An invite link opened on the way in comes next (FriendList), unless it is one's own name.
          if let me = accounts.account?.username { Invite.shared.signedIn(as: me) }
        }
      }
      // Invite links (justyo.co/NAME, yo://NAME); yo://camera and yo://open come from Yo's own intents.
      .onOpenURL { url in
        if url.scheme?.lowercased() == "yo", url.host()?.lowercased() == "camera" { AppRoute.shared.camera = true }
        else if url.scheme?.lowercased() == "yo", url.host()?.lowercased() == "open" {}
        else { Invite.shared.open(url) }
      }
      .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
        if let url = activity.webpageURL { Invite.shared.open(url) }
      }
      // The extensions, widgets and Siri see the list as it is (ShareBridge); at launch a few seconds after the first frame.
      .onChange(of: accounts.shareList(friends)) { _, list in
        Perf.timed("app.shareList") { ShareInbox.publish(list) }
      }
      .task { Launch.later { ShareInbox.publish(accounts.shareList(friends)) } }
      .onChange(of: accounts.client == nil, initial: true) { _, none in
        guard !none, let client = accounts.client else { return }
        Perf.mark("app.client")
        outbox.received = { [inbox, friends, accounts] events, contacts, client in
          await inbox.handle(events, contacts: contacts, client: client, accounts: accounts, friends: friends)
        }
        outbox.flush(accounts: accounts)
        ShareInbox.take(outbox: outbox, accounts: accounts)
        inbox.fetchMedia(client: client)
        // A list with friends shows its saved copy and takes the storage later; one without takes it at once, and the
        // add row waits for it (`AccountStore.storageRead`).
        if friends.hasFriend {
          Launch.later { await accounts.takeStorage(into: friends) }
        } else {
          Task { await accounts.takeStorage(into: friends) }
        }
        Launch.later { CrashReports.shared.flush() }
        // Receipts, retries and prekeys due (spec 4.1, 4.5), also for a phone that only receives.
        Launch.later { await inbox.maintain(accounts: accounts, friends: friends) }
      }
        .environment(inbox)
        .environment(friends)
        .environment(accounts)
        .environment(outbox)
        .task(id: accounts.account == nil) {
          guard accounts.account != nil else { return }
          await inbox.askForNotifications()
          Push.shared.register()
        }
        // The push token to the hub once there is a token and an account (again for a new account).
        .task(id: "\(Push.shared.token ?? "")/\(accounts.account?.userID.base64EncodedString() ?? "")/\(accounts.client != nil)") {
          await Push.shared.send(accounts)
          Launch.later { await accounts.refreshSupport() }
        }
        .onChange(of: phase, initial: true) { _, phase in
          guard phase == .active else { return }
          Perf.mark("app.active")
          accounts.openNow()  // also resumes an unfinished old-install update
          Task { await inbox.collectDelivered() }
          Launch.later {
            await accounts.refreshSupport()
            await inbox.maintain(accounts: accounts, friends: friends)
          }
          outbox.flush(accounts: accounts)
          ShareInbox.take(outbox: outbox, accounts: accounts)
          if let client = accounts.client { inbox.fetchMedia(client: client) }
          // The secure-account reminder of earlier builds, still pending on some phones.
          UNUserNotificationCenter.current().removePendingNotificationRequests(withIdentifiers: ["yo.secure"])
        }
    }
  }
}

/// Background time of its own around each access to the state database: the app is never suspended inside SQLite with
/// a lock on the group's database (0xdead10cc). Without time left the access is refused (tried again later); when time
/// runs out, the access under way (milliseconds, at most SQLite's busy wait) finishes before the time is given back.
private final class StateAccessTime: StateAccess, @unchecked Sendable {
  private let application: UIApplication
  private static let key = "yo.stateAccess"

  init(_ application: UIApplication) { self.application = application }

  /// An access's background task, and its end for the expiry handler to wait on.
  private final class Under: @unchecked Sendable {
    let task: Ended
    let done = DispatchSemaphore(value: 0)
    init(_ task: Ended) { self.task = task }
  }

  /// Accesses begin and end on one thread (the core calls are synchronous).
  private var stack: [Under] {
    get { Thread.current.threadDictionary[Self.key] as? [Under] ?? [] }
    set { Thread.current.threadDictionary[Self.key] = newValue.isEmpty ? nil : newValue }
  }

  func begin() -> Bool {
    let under = Under(Ended(application))
    let id = application.beginBackgroundTask(withName: "yo.state") {
      _ = under.done.wait(timeout: .now() + 2)
      under.task.end()
    }
    guard under.task.start(id) else { return false }
    stack.append(under)
    return true
  }

  func end() {
    guard let under = stack.popLast() else { return }
    under.done.signal()
    under.task.end()
  }
}

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

/// In front the app shows a Yo on the sender's row with its sound, not a system banner.
final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate, Sendable {
  let inbox: Inbox
  let friends: Friends
  let accounts: AccountStore

  init(inbox: Inbox, friends: Friends, accounts: AccountStore) {
    self.inbox = inbox
    self.friends = friends
    self.accounts = accounts
  }

  // Completion handlers, not the async forms: UIKit takes the answer only on the main thread (an async answer from
  // another thread crashed in UIKit's snapshot update).
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping @Sendable (UNNotificationPresentationOptions) -> Void
  ) {
    let info = notification.request.content.userInfo
    // A push that names nobody: the app's own wait at the hub brings the Yo.
    guard let sender = info["from"] as? String else { return DispatchQueue.main.async { completionHandler([]) } }
    let senderName = info["sender"] as? String
    let id = info["id"] as? String ?? notification.request.identifier
    Task { @MainActor [inbox] in
      inbox.received(id: id, from: sender, senderName: senderName)
      completionHandler([])
      // Nor left in Notification Center (listed only after this answer).
      try? await Task.sleep(for: .seconds(0.5))
      await inbox.collectDelivered()
    }
  }

  /// "Yo Back" on the notification sends without opening the app, also while locked: as a guest in the App Group
  /// (YoSend), to the notification's `user`, else to the sender of the newest Yo.
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
    guard let sender, action == UNNotificationDefaultActionIdentifier else {
      return DispatchQueue.main.async { completionHandler() }
    }
    Task { @MainActor [inbox] in
      inbox.received(id: id, from: sender, senderName: senderName)
      completionHandler()
    }
  }

  /// Without a `user` (an older notification): by name through the app's own client, once it has opened.
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
    _ = await accounts.clientSoon(seconds: 5)
    _ = await inbox.send(to: friend, accounts: accounts, friends: friends)
  }
}

/// Work for the screen only, never in a background launch (an App Intent, a silent push), where the intent's own Yo
/// should have the CPU and shared state. At `App.init` UIKit says `.active` in any launch: the first
/// `didBecomeActive` is what tells.
@MainActor enum Launch {
  private static var active = false
  private static var waiting: [@MainActor () -> Void] = []
  private static var observer: NSObjectProtocol?
  private static var scene = false
  private static var waitingForScene: [@MainActor () -> Void] = []
  private static var sceneObserver: NSObjectProtocol?

  static func whenScene(_ work: @escaping @MainActor () -> Void) {
    if scene { return work() }
    start()
    waitingForScene.append(work)
  }

  static func start() {
    guard observer == nil else { return }
    sceneObserver = NotificationCenter.default.addObserver(
      forName: UIScene.willConnectNotification, object: nil, queue: .main
    ) { _ in
      MainActor.assumeIsolated {
        guard !scene else { return }
        scene = true
        let due = waitingForScene
        waitingForScene = []
        for work in due { work() }
      }
    }
    observer = NotificationCenter.default.addObserver(
      forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main
    ) { _ in
      MainActor.assumeIsolated {
        guard !active else { return }
        active = true
        let due = waiting
        waiting = []
        for work in due { work() }
      }
    }
  }

  static func whenActive(_ work: @escaping @MainActor () -> Void) {
    if active { return work() }
    start()
    waiting.append(work)
  }

  /// Work the list needs neither to show nor to send: `seconds` after Yo is in front, at low priority.
  static func later(_ seconds: Double = 3, _ work: @escaping @MainActor () async -> Void) {
    whenActive {
      Task(priority: .utility) { @MainActor in
        try? await Task.sleep(for: .seconds(seconds))
        await work()
      }
    }
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
