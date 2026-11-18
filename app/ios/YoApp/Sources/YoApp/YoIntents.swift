// What Yo offers the system (App Intents): the Action button, Control Center and the Lock Screen (YoControls),
// Shortcuts and Siri (YoShortcuts.swift). "Send Yo" to a friend or group and "Yo Back" to whoever sent the last Yo go
// without opening the app, also while the iPhone is locked: as the share extension does, they open the app's state as
// a guest in the App Group (SharedState.swift), whose file and key are readable after the first unlock, and send one
// Yo. "Yo Camera" opens Yo in the camera, "Open Yo" just opens it. Compiled into the app and into YoControls
// (Sources/YoControls links to this file); the system runs each where it chooses.
import AppIntents
import Foundation
import WidgetKit
import YoCore

/// A friend or group on the list, as the app shows it to the extensions (ShareBridge.List): whom a Yo goes to.
struct FriendEntity: AppEntity {
  static let typeDisplayRepresentation: TypeDisplayRepresentation = "Friend"
  static let defaultQuery = FriendQuery()

  /// The target as the app sends to it: a user id, a name before the first Yo, or "group:" and the group's id.
  let id: String
  let name: String

  var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(name)") }

  init(_ friend: ShareBridge.Friend) {
    (id, name) = (friend.to, friend.name)
  }
}

/// The friends in the list's order; Siri and Shortcuts find one by name.
struct FriendQuery: EntityStringQuery {
  private var all: [FriendEntity] { (ShareBridge.readList()?.friends ?? []).map(FriendEntity.init) }

  func entities(for identifiers: [String]) async throws -> [FriendEntity] {
    all.filter { identifiers.contains($0.id) }
  }

  func entities(matching string: String) async throws -> [FriendEntity] {
    all.filter { $0.name.localizedCaseInsensitiveContains(string) }
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
    guard let owner = ShareBridge.readList()?.owner else { throw YoIntentError.signedOut }
    try await YoSend.send(to: friend.id, owner: owner, shown: (name: friend.name, back: false, lastYoAtMs: nil))
    WidgetSent.note(to: friend.id)
    return .result(dialog: "Sent Yo to \(friend.name)!")
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
    if let shown {
      await YoSentActivity.show(
        to: target, name: shown.name, back: shown.back, failure: outcome, lastYoAtMs: shown.lastYoAtMs)
    }
    if let outcome { throw outcome }
  }

  /// Nil when it went (or waits in the core's outbox for the app); else why not.
  private static func sendNow(to target: String, owner: String) -> YoIntentError? {
    guard let client = Guest.open(within: 8, timeout: 20) else { return .notNow }
    guard Hub.bytes(client.userId())?.base64EncodedString() == owner else { return .signedOut }
    do {
      let events =
        target.hasPrefix("group:")
        ? try client.sendGroupYo(group: String(target.dropFirst(6))) : try client.sendYo(to: target)
      notify(events, client: client)
      return nil
    } catch YoError.NotOnYo {
      return .refused("not on Yo")
    } catch YoError.Blocked {
      return .refused("blocked")
    } catch YoError.GroupEmpty {
      return .refused("the group is empty")
    } catch let error as YoError where retryLater(error: error) {
      return .notNow
    } catch {
      Diagnostics.record(error)
      return .refused("try again in Yo")
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
