// Your friends: usernames you add, and whoever sends you a Yo, kept on this phone in the original's order: whoever you
// last sent a Yo to or got one from is on top. Each keeps a colour of the original's palette wherever they go. A friend
// learns its user id (the hub's, spec 3) and its owner's spelling as soon as the hub is asked who the typed name is, else
// with the first Yo.
//
// Friends known by user id are also kept for the account by the core (its storage at the hub, spec 6.9), so a new phone
// has them back: this list is the phone's cache of it. Friends known only by a typed name stay on this phone until the
// hub names them.
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
  /// Their account is gone (deleted, or the name released after a year away): the bar says "Not on Yo". The name
  /// added again is asked of the hub anew and may be someone else's now.
  var gone = false
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
    gone = try c.decodeIfPresent(Bool.self, forKey: .gone) ?? false
    key = try c.decodeIfPresent(UUID.self, forKey: .key) ?? UUID()
  }
}

@MainActor @Observable
final class Friends {
  private(set) var all: [Friend] {
    didSet {
      save()
      if !quiet { mirror(oldValue) }
    }
  }

  /// Where friends known by user id go as they are added, change their spelling or are removed: the account's
  /// storage (`AccountStore.store`), set by the app. Blocks go there through the core's own `setBlocked`.
  @ObservationIgnored var stored: (@MainActor (_ changed: [Friend], _ removed: [String]) -> Void)?
  /// A change that is not this person's (signing out, the storage's own list coming in): not written back.
  @ObservationIgnored private var quiet = false

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
  /// friends come on top, as in the original. A friend whose account is gone gives its place to the name's new owner
  /// (or to the name as typed, asked of the hub again).
  func add(_ typed: String, user: String? = nil) {
    let name = Username.clean(typed)
    let skeleton = Username.skeleton(name)
    func same(_ friend: Friend) -> Bool {
      Username.skeleton(friend.name) == skeleton || (user != nil && friend.user == user)
    }
    guard !name.isEmpty, !all.contains(where: { !$0.gone && same($0) }) else { return }
    var list = all.filter { !same($0) }  // gone ones only
    list.insert(Friend(name: name, color: nextColor, user: user), at: 0)
    all = list
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

  /// The core's contacts whose account is gone (`YoContact.gone`): their bars say "Not on Yo".
  func markGone(_ contacts: [YoContact]) {
    let gone = Set(contacts.filter(\.gone).map(\.user))
    guard all.contains(where: { $0.user.map(gone.contains) == true && !$0.gone }) else { return }
    all = all.map { friend in
      var friend = friend
      if let user = friend.user, gone.contains(user) { friend.gone = true }
      return friend
    }
  }

  /// The account's storage (spec 6.9) as the core has it, into this list: friends kept there that this phone does not
  /// have come on the list (after a restore: the whole list), at the end, in their colour and blocked as kept there
  /// (a friend already here keeps its block: this phone's blocks went through the core). Nothing is written back.
  /// Then what this phone has and the storage lacks goes up (friends from before the storage, or changed while it
  /// could not be read).
  func take(_ kept: [YoFriend]) {
    var list = all
    var changed = false
    for friend in kept {
      guard let name = friend.name.map(Username.clean), !name.isEmpty else { continue }  // a block alone: the core's
      let skeleton = Username.skeleton(name)
      if let index = list.firstIndex(where: { $0.user == friend.user })
        ?? list.firstIndex(where: { $0.user == nil && Username.skeleton($0.name) == skeleton })
      {
        if list[index].user != friend.user {
          list[index].user = friend.user
          changed = true
        }
      } else if !list.contains(where: { !$0.gone && Username.skeleton($0.name) == skeleton }) {
        var new = Friend(name: name, color: friend.color.map(Int.init) ?? list.count, user: friend.user)
        new.blocked = friend.blocked
        list.append(new)
        changed = true
      }
    }
    if changed { quietly { all = list } }
    let keptUsers = Dictionary(kept.map { ($0.user, $0) }, uniquingKeysWith: { a, _ in a })
    let missing = all.filter { friend in
      guard let user = friend.user else { return false }
      return keptUsers[user].map { $0.name != friend.name } ?? true
    }
    if !missing.isEmpty { stored?(missing, []) }
  }

  /// The friend shown under this name now, if any.
  func shownFriend(named name: String) -> Friend? {
    all.first { $0.name == name && !$0.blocked }
  }

  /// Signed out: the friends of a new phone, none. (The account's storage keeps them.)
  func reset() {
    quietly { all = [] }
  }

  private func quietly(_ change: () -> Void) {
    quiet = true
    defer { quiet = false }
    change()
  }

  /// Friends known by user id that are new, newly named, respelled or recoloured since `old`, and those removed: to
  /// the account's storage.
  private func mirror(_ old: [Friend]) {
    guard let stored else { return }
    func byUser(_ list: [Friend]) -> [String: Friend] {
      Dictionary(list.compactMap { friend in friend.user.map { ($0, friend) } }, uniquingKeysWith: { a, _ in a })
    }
    let before = byUser(old)
    let now = byUser(all)
    let changed = now.filter { user, friend in
      before[user].map { $0.name != friend.name || $0.color != friend.color } ?? true
    }
    let removed = before.keys.filter { now[$0] == nil }
    if !changed.isEmpty || !removed.isEmpty { stored(Array(changed.values), removed) }
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
