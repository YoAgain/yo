// Your friends: usernames you add, and whoever sends you a Yo, kept on this phone in the original's order: whoever you
// last sent a Yo to or got one from is on top. Each keeps a colour of the original's palette wherever they go. A friend
// learns its user id (the hub's, spec 3) with the first Yo either way.
import Foundation
import Observation
import SwiftUI

struct Friend: Codable, Hashable, Identifiable {
  /// Their username, as shown ("LEO").
  var name: String
  /// Their place in the palette (Color.rows), for good.
  var color: Int
  /// Their user id at the hub, once a Yo went or came.
  var user: String?
  var id: String { name }

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

  /// The colour the next friend gets: the one fewest friends have (the first of those), so the list stays colourful.
  var nextColor: Int {
    let used = Dictionary(grouping: all, by: { $0.color % Color.rows.count }).mapValues(\.count)
    return Color.rows.indices.min { (used[$0] ?? 0, $0) < (used[$1] ?? 0, $1) } ?? 0
  }

  /// Adds a username as typed (made canonical: upper case); an empty or known name is ignored. New friends come on
  /// top, as in the original.
  func add(_ typed: String, user: String? = nil) {
    let name = Username.clean(typed)
    guard !name.isEmpty, !all.contains(where: { Username.skeleton($0.name) == Username.skeleton(name) }) else { return }
    all.insert(Friend(name: name, color: nextColor, user: user), at: 0)
  }

  /// The friend a user id belongs to: known by it, or by one of its handles (a name added before its first Yo), or
  /// new (someone who sent a Yo first).
  func friend(user: String, handles: [String]) -> Friend {
    if let known = all.first(where: { $0.user == user }) { return known }
    let skeletons = Set(handles.map(Username.skeleton))
    if let index = all.firstIndex(where: { $0.user == nil && skeletons.contains(Username.skeleton($0.name)) }) {
      all[index].user = user
      return all[index]
    }
    add(handles.first ?? "?", user: user)
    return all.first { $0.user == user } ?? Friend(name: handles.first ?? "?", color: 0, user: user)
  }

  /// A Yo went to or came from `name`: they go on top (the original's promoteObjectToTop).
  func promote(_ name: String) {
    guard let index = all.firstIndex(where: { $0.name == name }), index != 0 else { return }
    all.insert(all.remove(at: index), at: 0)
  }

  /// Signed out: the friends of a new phone, none.
  func reset() {
    all = []
  }

  func remove(_ friend: Friend) {
    all.removeAll { $0 == friend }
  }

  private func save() {
    UserDefaults.standard.set(try? JSONEncoder().encode(all), forKey: Self.key)
  }
}
