// What Yo offers the system (App Intents): the Action button, Control Center and the Lock Screen (YoControls),
// Shortcuts and Siri (YoShortcuts.swift). "Send Yo" to a friend or group and "Yo Back" to whoever sent the last Yo go
// without opening the app, also while the iPhone is locked: as the share extension does, they open the app's state as
// a guest in the App Group (SharedState.swift), whose file and key are readable after the first unlock, and send one
// Yo. "Yo Camera" opens Yo in the camera, "Open Yo" just opens it. Compiled into the app and into YoControls
// (Sources/YoControls links to this file); the system runs each where it chooses.
import ActivityKit
import AppIntents
import Foundation
import Security
import WidgetKit
import YoCore

/// A friend or group on the list, as the app shows it to the extensions (ShareBridge.List): whom a Yo goes to. Shown
/// as in the app: a group with its "#" ("#Spanien 2024").
struct FriendEntity: AppEntity {
  static let typeDisplayRepresentation: TypeDisplayRepresentation = "Friend or Group"
  static let defaultQuery = FriendQuery()

  /// The target as the app sends to it: a user id, a name before the first Yo, or "group:" and the group's id.
  let id: String
  let name: String

  /// The name as the list shows it: a group's with its "#".
  var title: String { ShareBridge.Friend.title(name, to: id) }

  var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(title)") }

  init(_ friend: ShareBridge.Friend) {
    (id, name) = (friend.to, friend.name)
  }
}

extension ShareBridge.Friend {
  /// The name as the list shows it: a group's with its "#" (usernames can't hold one).
  var title: String { Self.title(name, to: to) }

  static func title(_ name: String, to: String) -> String { to.hasPrefix("group:") ? "#" + name : name }
}

/// The friends and groups in the list's order; Siri and Shortcuts find one by name ("#" or not for a group).
struct FriendQuery: EntityStringQuery {
  private var all: [FriendEntity] { (ShareBridge.readList()?.friends ?? []).map(FriendEntity.init) }

  func entities(for identifiers: [String]) async throws -> [FriendEntity] {
    all.filter { identifiers.contains($0.id) }
  }

  func entities(matching string: String) async throws -> [FriendEntity] {
    let string = string.hasPrefix("#") ? String(string.dropFirst()) : string
    return all.filter { $0.name.localizedCaseInsensitiveContains(string) }
  }

  func suggestedEntities() async throws -> [FriendEntity] { all }
}

/// What a send says when it did not go.
enum YoIntentError: Error, CustomLocalizedStringResourceConvertible {
  case signedOut
  case noYoYet
  case notNow
  case refused(String)

  var localizedStringResource: LocalizedStringResource {
    switch self {
    case .signedOut: "Open Yo once to sign in."
    case .noYoYet: LocalizedStringResource(String.LocalizationValue(productText(text: .noAnswerYet, lang: .english)))
    case .notNow: "Not sent. Yo could not reach the internet."
    case .refused(let why): "Not sent: \(why)."
    }
  }

  /// Why, in plain words, after "No Yo to lina.".
  var why: String {
    switch self {
    case .signedOut: "Open Yo once to sign in."
    case .noYoYet: "Nobody sent you a Yo yet."
    case .notNow: "Yo could not reach the internet."
    case .refused(let why): why.prefix(1).uppercased() + why.dropFirst() + "."
    }
  }
}

/// The two ways a Yo from outside the app goes, one code for both. From Siri and Shortcuts (`wait`) the intent waits
/// for how it went and says so ("Yo to lina!", or "No Yo to lina." and why). From the Action button, a control or a
/// widget it returns as soon as the island stands and the Yo goes on in the background (YoSend.dispatch), so iOS's
/// running mark does not stay for the network's time.
enum IntentYo {
  static func send(_ friend: FriendEntity, wait: Bool) async throws -> some IntentResult & ProvidesDialog {
    let timing = YoSentActivity.Timing()
    guard let list = ShareBridge.readList() else {
      YoSend.count("signedOut")
      throw YoIntentError.signedOut
    }
    // The name as the list shows it now: the control keeps the friend as it was picked ("lina", typed then).
    let shown = list.friends.first { $0.to == friend.id }
      ?? list.friends.first { $0.name.caseInsensitiveCompare(friend.name) == .orderedSame }
    let name = shown?.name ?? friend.name
    let outcome = await YoSend.dispatch(
      to: friend.id, owner: list.owner, shown: (name: name, color: shown?.color, back: false), timing: timing,
      wait: wait)
    if let outcome { return .result(dialog: "No Yo to \(name). \(outcome.why)") }
    return .result(dialog: "Yo to \(name)!")
  }

  static func back(wait: Bool) async throws -> some IntentResult & ProvidesDialog {
    let timing = YoSentActivity.Timing()
    guard let last = ShareBridge.readLastYo() else {
      YoSend.count("noYoYet", expected: true)
      await YoSentActivity.showNobody()
      throw YoIntentError.noYoYet
    }
    let bar = ShareBridge.readList()?.friends.first { $0.to == last.to }
    let name = bar?.name ?? last.name
    let outcome = await YoSend.dispatch(
      to: last.to, owner: last.owner, shown: (name: name, color: bar?.color, back: true), timing: timing, wait: wait)
    if let outcome { return .result(dialog: "No Yo back to \(name). \(outcome.why)") }
    return .result(dialog: "Yo back to \(name)!")
  }
}

/// One Yo to a friend or group, from Siri and Shortcuts (YoShortcuts.swift): it waits for the Yo and says whether it
/// went. A LiveActivityIntent: the system runs it in the app's process, where "Yo lina" shows in the Dynamic Island
/// meanwhile (YoActivity.swift).
struct SendYoIntent: LiveActivityIntent {
  static let title: LocalizedStringResource = "Send Yo"
  static let description = IntentDescription("Sends one Yo to a friend or group, without opening Yo.")
  static let supportedModes: IntentModes = .background
  static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

  @Parameter(title: "Friend", requestValueDialog: "To whom?")
  var friend: FriendEntity

  static var parameterSummary: some ParameterSummary { Summary("Send Yo to \(\.$friend)") }

  init() {}

  init(friend: FriendEntity) {
    self.friend = friend
  }

  func perform() async throws -> some IntentResult & ProvidesDialog { try await IntentYo.send(friend, wait: true) }
}

/// Send Yo's twin for the Action button's control, Control Center and the widgets (YoControls): it returns as soon
/// as the island stands, the Yo going on in the background. Not offered in Shortcuts (the waiting one is).
struct SendYoNowIntent: LiveActivityIntent {
  static let title: LocalizedStringResource = "Send Yo"
  static let description = IntentDescription("Sends one Yo to a friend or group, without opening Yo.")
  static let supportedModes: IntentModes = .background
  static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed
  static let isDiscoverable = false

  @Parameter(title: "Friend", requestValueDialog: "To whom?")
  var friend: FriendEntity

  static var parameterSummary: some ParameterSummary { Summary("Send Yo to \(\.$friend)") }

  init() {}

  init(friend: FriendEntity) {
    self.friend = friend
  }

  func perform() async throws -> some IntentResult & ProvidesDialog { try await IntentYo.send(friend, wait: false) }
}

/// One Yo back to whoever sent the newest (ShareBridge.LastYo), from Siri and Shortcuts: it waits for how it went;
/// the island names whom it went to.
struct YoBackIntent: LiveActivityIntent {
  static let title: LocalizedStringResource = "Yo Back"
  static let description = IntentDescription("Sends a Yo back to whoever sent you the last one, without opening Yo.")
  static let supportedModes: IntentModes = .background
  static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

  func perform() async throws -> some IntentResult & ProvidesDialog { try await IntentYo.back(wait: true) }
}

/// Yo Back's twin for the controls and the widget: it returns once the island stands. Not offered in Shortcuts.
struct YoBackNowIntent: LiveActivityIntent {
  static let title: LocalizedStringResource = "Yo Back"
  static let description = IntentDescription("Sends a Yo back to whoever sent you the last one, without opening Yo.")
  static let supportedModes: IntentModes = .background
  static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed
  static let isDiscoverable = false

  func perform() async throws -> some IntentResult & ProvidesDialog { try await IntentYo.back(wait: false) }
}

/// The widgets' "Sent Yo!" (YoWidgets.swift): whom the newest Yo from an intent went to, shown on that friend's bar
/// (and on Yo Back's, for a Yo back) for a moment, then the name again. A small file in the App Group.
struct WidgetSent: Codable, Sendable {
  let to: String
  var back = false
  let until: Date

  /// How long a bar says "Sent Yo!".
  static let shown: TimeInterval = Double(productDisplayTiming().widgetSentSecs)

  private static var file: URL? { ShareBridge.container?.appending(path: "widget-sent.json") }

  static func note(to: String, back: Bool = false) {
    guard let file, let data = try? JSONEncoder().encode(WidgetSent(to: to, back: back, until: .now + shown)) else { return }
    try? data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    WidgetCenter.shared.reloadAllTimelines()
  }

  /// The note while it is still to be shown.
  static func read(now: Date = .now) -> WidgetSent? {
    guard let file, let data = try? Data(contentsOf: file),
      let sent = try? JSONDecoder().decode(WidgetSent.self, from: data), sent.until > now
    else { return nil }
    return sent
  }
}

/// Yo, opened in the camera: the system opens the app first and runs this in it (`openAppWhenRun`); a custom URL
/// scheme is refused to intents ("launch is prohibited").
struct YoCameraIntent: AppIntent {
  static let title: LocalizedStringResource = "Yo Camera"
  static let description = IntentDescription("Opens Yo in the camera.")
  static let openAppWhenRun = true

  @MainActor func perform() async throws -> some IntentResult {
    AppRoute.shared.camera = true
    return .result()
  }
}

/// Yo, opened.
struct OpenYoIntent: AppIntent {
  static let title: LocalizedStringResource = "Open Yo"
  static let description = IntentDescription("Opens Yo.")
  static let openAppWhenRun = true

  func perform() async throws -> some IntentResult { .result() }
}

/// Where an intent that opens the app asks it to go; the list takes it as it shows (FriendList).
@MainActor @Observable final class AppRoute {
  static let shared = AppRoute()
  /// Open the camera, once.
  var camera = false
}

/// A Yo from outside the app: the state as a guest (as the share extension's send), one Yo, the Yos that came
/// meanwhile written as the notification service would. The app, whenever it runs next, sees it as sent.
enum YoSend {
  /// With `shown` (the intents) it then shows in the Dynamic Island for a moment, whether it went or not
  /// (YoSentActivity): the name, a Yo back or not, and when the Yo it answers came.
  static func send(to target: String, owner: String) async throws {
    let lifetime = SharedAccess.Lifetime()
    let outcome = await Task.detached(priority: .userInitiated) {
      lifetime.run { sendNow(to: target, owner: owner) }
    }.value
    lifetime.stop()
    if outcome == nil { IntentSent.note(to: target, back: true) }
    if let outcome { throw outcome }
  }

  /// An intent's Yo: the island first ("sending", before the state opens: as early as iOS lets it), then the Yo,
  /// and the island says how it went. Without `wait` (the Action button, controls, widgets) it returns once the
  /// island stands, not when the Yo went: the intent ends at once, so iOS's own running mark (the control's blue
  /// hand) does not stay for the network's time; the process is kept up for the send (performExpiringActivity, in
  /// the app and in an extension alike), and if that time ends first, the turn is revoked (no lock held into a
  /// suspension) and the island says "No Yo". With `wait` (Siri, Shortcuts) it returns how it went: nil when it went.
  @discardableResult
  static func dispatch(
    to target: String, owner: String, shown: (name: String, color: Int?, back: Bool), timing: YoSentActivity.Timing,
    wait: Bool = false
  ) async -> YoIntentError? {
    // A group as the list writes it: "#Hey".
    let name = target.hasPrefix("group:") && !shown.name.hasPrefix("#") ? "#" + shown.name : shown.name
    let activity = await YoSentActivity.start(name: name, color: shown.color, back: shown.back)
    timing.mark(activity == nil ? "activity none" : "activity requested")
    let lifetime = SharedAccess.Lifetime()
    let box = ActivityBox(activity)
    @Sendable func went(_ outcome: YoIntentError?) {
      timing.mark(outcome == nil ? "sent" : "not sent")
      if outcome == nil {
        IntentSent.note(to: target, back: shown.back)
        WidgetSent.note(to: target, back: shown.back)
        sent?()
      }
    }
    // The intent's own request keeps the process up while it runs; its end (or ours) revokes the turn.
    @Sendable func sendHere() async -> YoIntentError? {
      let lifetime = SharedAccess.Lifetime()
      let outcome = await Task.detached(priority: .userInitiated) {
        lifetime.run { sendNow(to: target, owner: owner) }
      }.value
      lifetime.stop()
      went(outcome)
      await YoSentActivity.finish(box.activity, failed: outcome != nil)
      return outcome
    }
    if wait { return await sendHere() }
    // Refused at once (called expired before it ran: the app's background time is spent, which iOS counts for the
    // app as a whole), the intent's own time sends it, as with `wait`. Ended mid-send, the island says how it stands
    // before the expiry block returns (iOS suspends the process after it), never "sending" for good.
    let ran = await withCheckedContinuation { (resume: CheckedContinuation<Bool, Never>) in
      let begun = Begun(resume)
      ProcessInfo.processInfo.performExpiringActivity(withReason: "Yo") { expired in
        guard !expired else {
          lifetime.stop()
          if begun.first(false) { return }
          return YoSentActivity.finishWaiting(box.activity, failed: true)
        }
        guard begun.first(true) else { return }
        let outcome = lifetime.run { sendNow(to: target, owner: owner) }
        lifetime.stop()
        went(outcome)
        YoSentActivity.finishWaiting(box.activity, failed: outcome != nil)
      }
    }
    if !ran {
      timing.mark("no background time")
      return await sendHere()
    }
    return nil
  }

  /// Which call of the expiring activity's block came first: it resumes the intent, once.
  private final class Begun: @unchecked Sendable {
    private let lock = NSLock()
    private var resume: CheckedContinuation<Bool, Never>?
    init(_ resume: CheckedContinuation<Bool, Never>) { self.resume = resume }

    /// True for the first call only: `ran` true, the send runs on the activity's time; false, the intent sends it.
    func first(_ ran: Bool) -> Bool {
      guard let resume = lock.withLock({ () -> CheckedContinuation<Bool, Never>? in
        defer { self.resume = nil }
        return self.resume
      }) else { return false }
      resume.resume(returning: ran)
      return true
    }
  }

  /// Set by the app: a light success tap once a Yo went, while the app is in front (a haptic plays only there; in
  /// the background and in an extension there is none, beyond the system's own tap on the Action button).
  nonisolated(unsafe) static var sent: (@Sendable () -> Void)?

  /// The activity handed to the send's queue.
  private final class ActivityBox: @unchecked Sendable {
    let activity: Activity<YoSentAttributes>?
    init(_ activity: Activity<YoSentAttributes>?) { self.activity = activity }
  }

  /// Nil when it went (or waits in the core's outbox for the app); else why not. Every Yo that did not go is counted
  /// with its reason (CountedErrors "IntentSend"), for the admin page.
  private static func sendNow(to target: String, owner: String) -> YoIntentError? {
    let client: YoClient
    // The hub's answer comes within a step that holds the state's lock: under the notification service's wait for
    // it (NotificationService.lockWait, 10 s), so a push meanwhile is read, not "You may have received a Yo".
    switch Guest.opened(within: 8, timeout: 8) {
    case .success(let opened): client = opened
    case .failure(let failure):
      // Before the first unlock since a restart the keychain does not answer (errSecInteractionNotAllowed): expected.
      let locked: Bool = if case .keychain(let status) = failure { status == errSecInteractionNotAllowed } else { false }
      count("open " + reason(failure), expected: locked)
      return .notNow
    }
    guard Hub.bytes(client.userId())?.base64EncodedString() == owner else {
      count("otherAccount")
      return .signedOut
    }
    do {
      let events =
        target.hasPrefix("group:")
        ? try client.sendGroupYo(group: String(target.dropFirst(6))) : try client.sendYo(to: target)
      notify(events, client: client)
      return nil
    } catch YoError.NotOnYo {
      count("notOnYo", expected: true)
      return .refused("not on Yo")
    } catch YoError.Blocked {
      count("blocked", expected: true)
      return .refused("blocked")
    } catch YoError.GroupEmpty {
      count("groupEmpty", expected: true)
      return .refused("the group is empty")
    } catch let error as YoError where retryLater(error: error) {
      count("later " + described(error))
      return .notNow
    } catch {
      count("error " + described(error))
      Diagnostics.record(error)
      return .refused("try again in Yo")
    }
  }

  /// One Yo that did not go, and why: in which process (the app, or YoControls) and the reason.
  static func count(_ reason: String, expected: Bool = false) {
    let process = Bundle.main.bundleURL.pathExtension == "appex" ? "extension" : "app"
    CountedErrors.record(domain: "IntentSend", place: "YoSend." + process, code: reason, expected: expected)
  }

  /// The error's case and message, without what could name someone: no URL (an id or a name in its path), at most
  /// 120 characters.
  private static func described(_ error: Error) -> String {
    let text = String(describing: error)
      .replacingOccurrences(of: #"[a-z]+://\S+"#, with: "<url>", options: .regularExpression)
    return String(text.prefix(120))
  }

  private static func reason(_ failure: Guest.Failure) -> String {
    switch failure {
    case .noGroup: "noGroup"
    case .keychain(let status): "keychain/\(status)"
    case .noAccount: "noAccount"
    case .busy: "busy"
    case .restore(let error): "restore " + described(error)
    }
  }

  /// The Yos that came while sending, as the share extension writes them.
  private static func notify(_ events: [YoEvent], client: YoClient) {
    let yos = ShareBridge.yos(
      in: events, name: { ShareBridge.name(of: $0, client: client) },
      group: { id in (try? client.groups())?.first { $0.id == id }?.name ?? "" },
      requests: ShareBridge.requests(of: client))
    guard !yos.isEmpty else { return }
    ShareBridge.noteLast(yos, client: client)
    Task { await ShareBridge.post(yos) }
  }
}

/// The Yos sent from outside the list (the intents, a notification's Yo Back), for the list to show as its own: the
/// bar on top with "Sent Yo" (FriendList). The core's state has the Yo already; the list's order and its "Sent Yo"
/// are the app's and learn of it here. A small file in the App Group (the send may run in YoControls, or in the app
/// before its list is there), and a Darwin notification for a list on screen meanwhile.
struct IntentSent: Codable, Sendable {
  /// Whom it went to, as the bar sends: a user id, a name, or "group:" and the group's id.
  let to: String
  var back = false
  let at: Date

  /// Posted (in the app) when a note came, from whichever process.
  static let changed = Notification.Name("yo.intentSent")
  private static var darwinName: String { ShareBridge.group + ".intentSent" }
  private static var file: URL? { ShareBridge.container?.appending(path: "intent-sent.json") }
  /// How long a note waits for the list: a Yo sent a day ago is no news on opening.
  static let kept: TimeInterval = 24 * 3600

  static func note(to: String, back: Bool) {
    guard let file else { return }
    let notes = (read(file) + [IntentSent(to: to, back: back, at: .now)]).suffix(20)
    guard let data = try? JSONEncoder().encode(Array(notes)) else { return }
    try? data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    CFNotificationCenterPostNotification(
      CFNotificationCenterGetDarwinNotifyCenter(), CFNotificationName(darwinName as CFString), nil, nil, true)
  }

  /// The app: the notes not yet shown, oldest first; taken (the file goes).
  static func take() -> [IntentSent] {
    guard let file else { return [] }
    let notes = read(file)
    try? FileManager.default.removeItem(at: file)
    return notes.filter { $0.at > .now - kept }
  }

  private static func read(_ file: URL) -> [IntentSent] {
    guard let data = try? Data(contentsOf: file) else { return [] }
    return (try? JSONDecoder().decode([IntentSent].self, from: data)) ?? []
  }

  /// The app, once: a note from another process (or this one) posts `changed` here, on the main queue.
  @MainActor static func listen() {
    guard !listening else { return }
    listening = true
    CFNotificationCenterAddObserver(
      CFNotificationCenterGetDarwinNotifyCenter(), nil,
      { _, _, _, _, _ in DispatchQueue.main.async { NotificationCenter.default.post(name: IntentSent.changed, object: nil) } },
      darwinName as CFString, nil, .deliverImmediately)
  }
  @MainActor private static var listening = false
}
