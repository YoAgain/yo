// App Intents for the Action button, controls, widgets, Shortcuts and Siri. Sends open the state as a guest in the App
// Group (readable after first unlock), so they work while locked. Compiled into the app and YoControls; the system
// runs each where it chooses.
import ActivityKit
import AppIntents
import Foundation
import Security
import WidgetKit
import YoCore

struct FriendEntity: AppEntity {
  static let typeDisplayRepresentation: TypeDisplayRepresentation = "Friend or Group"
  static let defaultQuery = FriendQuery()

  /// A user id, a name before the first Yo, or "group:" and the group's id.
  let id: String
  let name: String

  var title: String { ShareBridge.Friend.title(name, to: id) }

  var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(title)") }

  init(_ friend: ShareBridge.Friend) {
    (id, name) = (friend.to, friend.name)
  }
}

extension ShareBridge.Friend {
  /// A group's with one "#", as its bar shows it, whether the name brings its own or not (usernames can't hold one).
  var title: String { Self.title(name, to: to) }

  static func title(_ name: String, to: String) -> String { listBarTitle(name: name, group: to.hasPrefix("group:")) }
}

/// In the list's order; a group is found with or without its "#".
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

  /// After "No Yo to lina.".
  var why: String {
    switch self {
    case .signedOut: "Open Yo once to sign in."
    case .noYoYet: "Nobody sent you a Yo yet."
    case .notNow: "Yo could not reach the internet."
    case .refused(let why): why.prefix(1).uppercased() + why.dropFirst() + "."
    }
  }
}

/// Siri and Shortcuts `wait` for the outcome and say it; the Action button, controls and widgets return once the
/// island stands (YoSend.dispatch).
enum IntentYo {
  /// No friend (a control left unconfigured): `choose` opens Yo.
  static func send(
    _ friend: FriendEntity?, wait: Bool, choose: () async throws -> Void = {}
  ) async throws -> some IntentResult & ProvidesDialog {
    guard let friend else {
      YoSend.count("noFriend", expected: true)
      try await choose()
      return .result(dialog: "Choose a friend in Yo.")
    }
    let timing = YoSentActivity.Timing()
    guard let list = ShareBridge.readList() else {
      YoSend.count("signedOut")
      throw YoIntentError.signedOut
    }
    // The name as the list shows it now: a control keeps the friend as it was picked.
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

/// A LiveActivityIntent: the system runs it in the app's process, where the island may start (YoActivity.swift).
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

/// Send Yo's non-waiting twin for controls and widgets; not offered in Shortcuts. A control with no friend chosen
/// can't be asked "To whom?" (Control Center, the Lock Screen, the Action button): it opens Yo instead, unlocking
/// first, where a tap on a name sends the Yo.
struct SendYoNowIntent: LiveActivityIntent {
  static let title: LocalizedStringResource = "Send Yo"
  static let description = IntentDescription("Sends one Yo to a friend or group, without opening Yo.")
  static let supportedModes: IntentModes = [.background, .foreground(.dynamic)]
  static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed
  static let isDiscoverable = false

  @Parameter(title: "Friend")
  var friend: FriendEntity?

  static var parameterSummary: some ParameterSummary { Summary("Send Yo to \(\.$friend)") }

  init() {}

  init(friend: FriendEntity) {
    self.friend = friend
  }

  func perform() async throws -> some IntentResult & ProvidesDialog {
    try await IntentYo.send(friend, wait: false) { try await continueInForeground(alwaysConfirm: false) }
  }
}

/// Back to ShareBridge.LastYo.
struct YoBackIntent: LiveActivityIntent {
  static let title: LocalizedStringResource = "Yo Back"
  static let description = IntentDescription("Sends a Yo back to whoever sent you the last one, without opening Yo.")
  static let supportedModes: IntentModes = .background
  static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

  func perform() async throws -> some IntentResult & ProvidesDialog { try await IntentYo.back(wait: true) }
}

/// Yo Back's non-waiting twin for controls and widgets; not offered in Shortcuts.
struct YoBackNowIntent: LiveActivityIntent {
  static let title: LocalizedStringResource = "Yo Back"
  static let description = IntentDescription("Sends a Yo back to whoever sent you the last one, without opening Yo.")
  static let supportedModes: IntentModes = .background
  static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed
  static let isDiscoverable = false

  func perform() async throws -> some IntentResult & ProvidesDialog { try await IntentYo.back(wait: false) }
}

/// The widgets' "Sent Yo!" on a friend's bar for a moment (YoWidgets.swift).
struct WidgetSent: Codable, Sendable {
  let to: String
  var back = false
  let until: Date

  static let shown: TimeInterval = Double(productDisplayTiming().widgetSentSecs)

  private static var file: URL? { ShareBridge.container?.appending(path: "widget-sent.json") }

  static func note(to: String, back: Bool = false) {
    guard let file, let data = try? JSONEncoder().encode(WidgetSent(to: to, back: back, until: .now + shown)) else { return }
    try? data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    WidgetCenter.shared.reloadAllTimelines()
  }

  static func read(now: Date = .now) -> WidgetSent? {
    guard let file, let data = try? Data(contentsOf: file),
      let sent = try? JSONDecoder().decode(WidgetSent.self, from: data), sent.until > now
    else { return nil }
    return sent
  }
}

/// `openAppWhenRun`: a custom URL scheme is refused to intents ("launch is prohibited").
struct YoCameraIntent: AppIntent {
  static let title: LocalizedStringResource = "Yo Camera"
  static let description = IntentDescription("Opens Yo in the camera.")
  static let openAppWhenRun = true

  @MainActor func perform() async throws -> some IntentResult {
    AppRoute.shared.camera = true
    return .result()
  }
}

struct OpenYoIntent: AppIntent {
  static let title: LocalizedStringResource = "Open Yo"
  static let description = IntentDescription("Opens Yo.")
  static let openAppWhenRun = true

  func perform() async throws -> some IntentResult { .result() }
}

/// Where an intent that opens the app asks it to go (FriendList takes it).
@MainActor @Observable final class AppRoute {
  static let shared = AppRoute()
  var camera = false
}

/// A Yo from outside the app, sent as a guest; Yos that came meanwhile are notified as the service would.
enum YoSend {
  static func send(to target: String, owner: String) async throws {
    let lifetime = SharedAccess.Lifetime()
    let outcome = await Task.detached(priority: .userInitiated) {
      lifetime.run { sendNow(to: target, owner: owner) }
    }.value
    lifetime.stop()
    if outcome == nil { IntentSent.note(to: target, back: true) }
    if let outcome { throw outcome }
  }

  /// The island first ("sending", as early as iOS lets it), then the Yo. Without `wait` it returns once the island
  /// stands, so iOS's running mark does not stay for the network's time; performExpiringActivity keeps the process up
  /// for the send, and if that time ends first the turn is revoked (no lock held into a suspension) and the island
  /// says "No Yo". With `wait`: nil when it went.
  @discardableResult
  static func dispatch(
    to target: String, owner: String, shown: (name: String, color: Int?, back: Bool), timing: YoSentActivity.Timing,
    wait: Bool = false
  ) async -> YoIntentError? {
    let name = ShareBridge.Friend.title(shown.name, to: target)
    let activity = await YoSentActivity.start(name: name, color: shown.color, back: shown.back)
    timing.mark(activity == nil ? "activity none" : "activity requested")
    let lifetime = SharedAccess.Lifetime()
    let box = YoSentActivity.Box(activity)
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
    // Called expired before it ran (the app's background time is spent): the intent's own time sends it. Ended
    // mid-send: the island is settled before the expiry block returns, as iOS suspends right after.
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

  /// Resumes the intent once, from whichever call of the expiring activity's block came first.
  private final class Begun: @unchecked Sendable {
    private let lock = NSLock()
    private var resume: CheckedContinuation<Bool, Never>?
    init(_ resume: CheckedContinuation<Bool, Never>) { self.resume = resume }

    func first(_ ran: Bool) -> Bool {
      guard let resume = lock.withLock({ () -> CheckedContinuation<Bool, Never>? in
        defer { self.resume = nil }
        return self.resume
      }) else { return false }
      resume.resume(returning: ran)
      return true
    }
  }

  /// Set by the app: a success haptic (plays only in front).
  nonisolated(unsafe) static var sent: (@Sendable () -> Void)?

  /// Nil when it went (or waits in the core's outbox). Every failure is counted for the admin page.
  private static func sendNow(to target: String, owner: String) -> YoIntentError? {
    let client: YoClient
    // Under the notification service's lock wait (NotificationService.lockWait, 10 s), so a push meanwhile is read.
    switch Guest.opened(within: 8, timeout: 8) {
    case .success(let opened): client = opened
    case .failure(let failure):
      // Before the first unlock since a restart: expected.
      let locked: Bool = if case .keychain(let status) = failure { status == errSecInteractionNotAllowed } else { false }
      count("open " + reason(failure), expected: locked)
      return .notNow
    }
    guard Hub.bytes(client.userId())?.base64EncodedString() == owner else {
      count("otherAccount")
      return .signedOut
    }
    do {
      let events = try client.send(target: sendTarget(to: target), kind: .yo)
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

  static func count(_ reason: String, expected: Bool = false) {
    let process = Bundle.main.bundleURL.pathExtension == "appex" ? "extension" : "app"
    CountedErrors.record(domain: "IntentSend", place: "YoSend." + process, code: reason, expected: expected)
  }

  /// Without URLs (they could name someone), at most 120 characters.
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
