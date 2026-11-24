import AppIntents
import Foundation

struct FriendEntity: AppEntity {
  static let typeDisplayRepresentation: TypeDisplayRepresentation = "Friend or Group"
  static let defaultQuery = FriendQuery()

  let id: String
  let title: String
  var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(title)") }
}

struct FriendQuery: EntityStringQuery {
  private var friends: [FriendEntity] {
    (WatchState.read()?.friends ?? []).map { FriendEntity(id: $0.to, title: $0.title) }
  }

  func entities(for identifiers: [String]) async throws -> [FriendEntity] {
    friends.filter { identifiers.contains($0.id) }
  }

  func entities(matching string: String) async throws -> [FriendEntity] {
    let name = string.hasPrefix("#") ? String(string.dropFirst()) : string
    return friends.filter { $0.title.localizedCaseInsensitiveContains(name) }
  }

  func suggestedEntities() async throws -> [FriendEntity] { friends }
}

private enum SendError: Error, CustomLocalizedStringResourceConvertible {
  case notSent
  var localizedStringResource: LocalizedStringResource { "Not Sent" }
}

struct SendYoIntent: AppIntent {
  static let title: LocalizedStringResource = "Send Yo"
  static let openAppWhenRun = false

  @Parameter(title: "Friend") var friend: FriendEntity

  func perform() async throws -> some IntentResult & ProvidesDialog {
    guard await WatchLink.shared.send(to: friend.id, back: false) else { throw SendError.notSent }
    return .result(dialog: "Sent Yo!")
  }
}

struct YoWatchShortcuts: AppShortcutsProvider {
  static var appShortcuts: [AppShortcut] {
    AppShortcut(intent: SendYoIntent(), phrases: ["\(.applicationName) \(\.$friend)"],
                shortTitle: "Send Yo", systemImageName: "hand.wave.fill")
  }
}
