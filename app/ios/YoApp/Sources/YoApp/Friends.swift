// Your friends: usernames you add, and whoever sends you a Yo, kept on this phone in the original's order: whoever you
// last sent a Yo to or got one from is on top. Each keeps a colour of the original's palette wherever they go. A friend
// learns its user id (the hub's, spec 3) and its owner's spelling as soon as the hub is asked who the typed name is, else
// with the first Yo.
import Foundation
import Observation
import SwiftUI
import YoCore

struct Friend: Codable, Hashable, Identifiable {
  /// Their username, as they write it ("Leo"); as typed until the hub told their spelling.
  var name: String
  /// Their place in the palette (Color.rows), for good.
  var color: Int
  /// Their user id at the hub, once a Yo went or came.
  var user: String?
  /// Blocked (spec 4.8): off the list, nothing from them shown; Settings lists them to unblock.
  var blocked = false
  /// Who they are on this phone, for good: a new spelling or a user id learned later keeps the bar in its place (a
  /// name as identity made the bar leave and come in again on top).
  var key = UUID()
  var id: UUID { key }

  /// One friend is one friend, whatever was learned about them since: by `key` alone.
  static func == (a: Friend, b: Friend) -> Bool { a.key == b.key }
  func hash(into hasher: inout Hasher) { hasher.combine(key) }

  init(name: String, color: Int, user: String? = nil) {
    self.name = name
    self.color = color
    self.user = user
  }

  /// Friends saved before they had a colour get theirs from where they stand (`init()`).
  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    name = try c.decode(String.self, forKey: .name)
    color = try c.decodeIfPresent(Int.self, forKey: .color) ?? -1
    user = try c.decodeIfPresent(String.self, forKey: .user)
    blocked = try c.decodeIfPresent(Bool.self, forKey: .blocked) ?? false
    key = try c.decodeIfPresent(UUID.self, forKey: .key) ?? UUID()
  }
}

@MainActor @Observable
final class Friends {
  private(set) var all: [Friend] {
    didSet { save() }
  }

  private static let key = "friends"

  init() {
    let saved = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode([Friend].self, from: $0) }
    all = (saved ?? []).enumerated().map { index, friend in
      var friend = friend
      if friend.color < 0 { friend.color = index }
      return friend
    }
  }

  /// The friends on the list: all but the blocked.
  var shown: [Friend] { all.filter { !$0.blocked } }
  var blocked: [Friend] { all.filter(\.blocked) }

  /// The colour the next friend gets: the one fewest friends have (the first of those), so the list stays colourful.
  var nextColor: Int {
    let used = Dictionary(grouping: all, by: { $0.color % Color.rows.count }).mapValues(\.count)
    return Color.rows.indices.min { (used[$0] ?? 0, $0) < (used[$1] ?? 0, $1) } ?? 0
  }

  /// Adds a username as typed (only what a name may hold); an empty or known name (in any spelling) is ignored. New
  /// friends come on top, as in the original.
  func add(_ typed: String, user: String? = nil) {
    let name = Username.clean(typed)
    guard !name.isEmpty, !all.contains(where: { Username.skeleton($0.name) == Username.skeleton(name) }) else { return }
    all.insert(Friend(name: name, color: nextColor, user: user), at: 0)
  }

  /// The friend a user id belongs to: known by it, or by one of its handles (a name added before its first Yo), or
  /// new (someone who sent a Yo first: the hub told the core their name). A friend added as "yogpt" is shown as its
  /// owner writes it ("YoGPT") once the core knows that.
  func friend(user: String, handles: [String]) -> Friend {
    let spelled = handles.first { !$0.hasPrefix("+") }  // the username, as its owner writes it
    let skeletons = Set(handles.map(Username.skeleton))
    let index =
      all.firstIndex(where: { $0.user == user })
      ?? all.firstIndex(where: { $0.user == nil && skeletons.contains(Username.skeleton($0.name)) })
    if let index {
      // Written only when something changed: each write saves the list and redraws it (once per Yo in a burst).
      if all[index].user != user { all[index].user = user }
      if let spelled, all[index].name != spelled, Username.skeleton(spelled) == Username.skeleton(all[index].name),
        !all.contains(where: { $0.name == spelled })
      {
        all[index].name = spelled
      }
      return all[index]
    }
    add(spelled ?? handles.first ?? "?", user: user)
    return all.first { $0.user == user } ?? Friend(name: spelled ?? handles.first ?? "?", color: 0, user: user)
  }

  /// What the hub says a typed name is (`Inbox.identify`): the friend added by it learns the user id and takes the
  /// owner's spelling. A friend removed meanwhile stays removed.
  func found(_ name: String, user: String, spelled: String?) {
    let skeleton = Username.skeleton(name)
    guard all.contains(where: { $0.user == nil && Username.skeleton($0.name) == skeleton }) else { return }
    _ = friend(user: user, handles: [spelled ?? name])
  }

  /// The core's contacts: friends already on the list take the spelling it knows now (a friend is never added here).
  func spell(_ contacts: [YoContact]) {
    for contact in contacts {
      let skeletons = Set(contact.handles.map(Username.skeleton))
      if all.contains(where: { $0.user == contact.user || ($0.user == nil && skeletons.contains(Username.skeleton($0.name))) }) {
        _ = friend(user: contact.user, handles: contact.handles)
      }
    }
  }

  /// A Yo went to or came from `name`: they go on top (the original's promoteObjectToTop).
  /// The name in any spelling: the friend may have been renamed to its owner's spelling meanwhile. One already first
  /// on the list (only blocked ones above) stays as it is: nothing is written, nothing moves.
  func promote(_ name: String) {
    let skeleton = Username.skeleton(name)
    let found =
      all.firstIndex(where: { $0.name == name }) ?? all.firstIndex(where: { Username.skeleton($0.name) == skeleton })
    guard let index = found, all[..<index].contains(where: { !$0.blocked }) else { return }
    all.insert(all.remove(at: index), at: 0)
  }

  /// The friend shown under this name now, if any.
  func shownFriend(named name: String) -> Friend? {
    all.first { $0.name == name && !$0.blocked }
  }

  /// Signed out: the friends of a new phone, none.
  func reset() {
    all = []
  }

  func remove(_ friend: Friend) {
    all.removeAll { $0 == friend }
  }

  /// Off the list, or back on it (the core blocks them: `AccountStore.setBlocked`).
  func setBlocked(_ friend: Friend, _ blocked: Bool) {
    guard let index = all.firstIndex(where: { $0.id == friend.id }) else { return }
    all[index].blocked = blocked
  }

  private func save() {
    UserDefaults.standard.set(try? JSONEncoder().encode(all), forKey: Self.key)
  }
}
