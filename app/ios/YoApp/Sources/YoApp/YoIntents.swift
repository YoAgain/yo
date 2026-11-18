// What Yo offers the system (App Intents): the Action button, Control Center and the Lock Screen (YoControls),
// Shortcuts and Siri (YoShortcuts.swift). "Send Yo" to a friend or group and "Yo Back" to whoever sent the last Yo go
// without opening the app, also while the iPhone is locked: as the share extension does, they open the app's state as
// a guest in the App Group (SharedState.swift), whose file and key are readable after the first unlock, and send one
// Yo. "Yo Camera" opens Yo in the camera, "Open Yo" just opens it. Compiled into the app and into YoControls
// (Sources/YoControls links to this file); the system runs each where it chooses.
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
    case .noYoYet: "No Yo to answer yet."
    case .notNow: "Not sent. Yo could not reach the internet."
    case .refused(let why): "Not sent: \(why)."
    }
  }
}

/// One Yo to a friend or group. A LiveActivityIntent: the system runs it in the app's process (also from the
/// controls and widgets), where "Sent Yo! to Mia" may start in the Dynamic Island (YoActivity.swift).
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

  func perform() async throws -> some IntentResult & ProvidesDialog {
    guard let list = ShareBridge.readList() else {
      YoSend.count("signedOut")
      throw YoIntentError.signedOut
    }
    // The name as the list shows it now: the control keeps the friend as it was picked ("lina", typed then).
    let name = list.friends.first { $0.to == friend.id }?.name
      ?? list.friends.first { $0.name.caseInsensitiveCompare(friend.name) == .orderedSame }?.name ?? friend.name
    try await YoSend.send(to: friend.id, owner: list.owner, shown: (name: name, back: false, lastYoAtMs: nil))
    WidgetSent.note(to: friend.id)
    return .result(dialog: "Sent Yo to \(name)!")
  }
}

/// One Yo back to whoever sent the newest (ShareBridge.LastYo); the island names whom it went to.
struct YoBackIntent: LiveActivityIntent {
  static let title: LocalizedStringResource = "Yo Back"
  static let description = IntentDescription("Sends a Yo back to whoever sent you the last one, without opening Yo.")
  static let supportedModes: IntentModes = .background
  static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

  func perform() async throws -> some IntentResult & ProvidesDialog {
    guard let last = ShareBridge.readLastYo() else {
      YoSend.count("noYoYet", expected: true)
      await YoSentActivity.showNobody()
      throw YoIntentError.noYoYet
    }
    try await YoSend.send(to: last.to, owner: last.owner, shown: (name: last.name, back: true, lastYoAtMs: last.sentAtMs))
    WidgetSent.note(to: last.to, back: true)
    return .result(dialog: "Sent Yo to \(last.name)!")
  }
}

/// The widgets' "Sent Yo!" (YoWidgets.swift): whom the newest Yo from an intent went to, shown on that friend's bar
/// (and on Yo Back's, for a Yo back) for a moment, then the name again. A small file in the App Group.
struct WidgetSent: Codable, Sendable {
  let to: String
  var back = false
  let until: Date

  /// How long a bar says "Sent Yo!".
  static let shown: TimeInterval = 3

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
  static func send(
    to target: String, owner: String, shown: (name: String, back: Bool, lastYoAtMs: UInt64?)? = nil
  ) async throws {
    let lifetime = SharedAccess.Lifetime()
    let outcome = await Task.detached(priority: .userInitiated) {
      lifetime.run { sendNow(to: target, owner: owner) }
    }.value
    lifetime.stop()
    if outcome == nil { IntentSent.note(to: target, back: shown?.back ?? false) }
    if let shown {
      await YoSentActivity.show(
        to: target, name: shown.name, back: shown.back, failure: outcome, lastYoAtMs: shown.lastYoAtMs)
    }
    if let outcome { throw outcome }
  }

  /// Nil when it went (or waits in the core's outbox for the app); else why not. Every Yo that did not go is counted
  /// with its reason (CountedErrors "IntentSend"), for the admin page.
  private static func sendNow(to target: String, owner: String) -> YoIntentError? {
    let client: YoClient
    switch Guest.opened(within: 8, timeout: 20) {
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
