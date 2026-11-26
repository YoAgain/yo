// Your friends: usernames you add, and whoever sends you a Yo. Friends known by user id are kept for the account by
// the core (its storage at the hub, spec 4.3); this list is the phone's cache of it. Friends known only by a typed name
// stay on this phone until the hub names them.
import Foundation
import Observation
import SwiftUI
import YoCore

struct Friend: Codable, Hashable, Identifiable {
  var name: String
  /// Their place in the palette (Color.rows), for good.
  var color: Int
  var user: String?
  /// A group's bar (spec 3.4): the group's id. No user id then.
  var group: String?
  var blocked = false
  /// Their account is deleted: the bar says "Not on Yo". The name added again may be someone else's now.
  var gone = false
  /// Their account key changed (spec 2.1): Contacts says so until their details are opened.
  var keyChanged = false
  /// A request (spec 3.3): nothing goes to them until it is said yes to.
  var request = false
  /// "Undo Before Sending" (Per Contact): a Yo to them waits `undoSeconds` under the undo ring, as a photo does.
  var undoFirst = false
  var undoSeconds: Double = Double(listTimings().undoMs) / 1000
  /// Who they are on this phone, for good: a new spelling or user id keeps the bar in its place.
  var key = UUID()
  var id: UUID { key }

  var target: String { group.map { "group:" + $0 } ?? user ?? name }
  /// Where its photos and videos are kept (Inbox): never by name, so a rename or a namesake never mixes them.
  var mediaKey: String { group.map { "group:" + $0 } ?? user.map { "user:" + $0 } ?? Username.skeleton(name) }

  static func == (a: Friend, b: Friend) -> Bool { a.key == b.key }
  func hash(into hasher: inout Hasher) { hasher.combine(key) }

  /// `==` is identity (for gestures); caches compare the displayed content too.
  func contentMatches(_ other: Friend) -> Bool {
    key == other.key && name == other.name && color == other.color
      && user == other.user && group == other.group && blocked == other.blocked
      && gone == other.gone && keyChanged == other.keyChanged && request == other.request
      && undoFirst == other.undoFirst && undoSeconds == other.undoSeconds
  }

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
    keyChanged = try c.decodeIfPresent(Bool.self, forKey: .keyChanged) ?? false
    request = try c.decodeIfPresent(Bool.self, forKey: .request) ?? false
    undoFirst = try c.decodeIfPresent(Bool.self, forKey: .undoFirst) ?? false
    undoSeconds = try c.decodeIfPresent(Double.self, forKey: .undoSeconds) ?? Double(listTimings().undoMs) / 1000
    group = try c.decodeIfPresent(String.self, forKey: .group)
    key = try c.decodeIfPresent(UUID.self, forKey: .key) ?? UUID()
  }
}

@MainActor @Observable
final class Friends {
  private(set) var all: [Friend] {
    didSet {
      if !reordering { updateIndexes(oldValue) }
      save()
      if !quiet && !reordering { mirrorChanges() }
    }
  }

  @ObservationIgnored private var userIndexes: [String: Int] = [:]
  @ObservationIgnored private var typedIndexes: [String: Int] = [:]
  @ObservationIgnored private var names: Set<String> = []

  @ObservationIgnored private var reordering = false
  @ObservationIgnored private var indexed: [UUID: Friend] = [:]
  @ObservationIgnored private var skeletons: [UUID: String] = [:]
  @ObservationIgnored private var keyIndexes: [UUID: Int] = [:]
  @ObservationIgnored private var mirroredUsers: [String: Friend] = [:]
  @ObservationIgnored private var dirtyUsers: Set<String> = []

  private func updateIndexes(_ old: [Friend]) {
    Perf.timed("friends.index") {
      var users: Set<String> = []
      var typed: Set<String> = []
      var changedNames: Set<String> = []
      @MainActor func invalidate(_ friend: Friend) {
        if let user = friend.user { users.insert(user); dirtyUsers.insert(user) }
        let skeleton = skeletons[friend.key] ?? Username.skeleton(friend.name)
        if Self.typed(friend) { typed.insert(skeleton) }
        changedNames.insert(friend.name)
      }
      let present = Set(all.map(\.key))
      for friend in old where !present.contains(friend.key) {
        invalidate(friend)
        indexed[friend.key] = nil
        skeletons[friend.key] = nil
        keyIndexes[friend.key] = nil
      }
      for (index, friend) in all.enumerated() {
        let before = indexed[friend.key]
        let changed = before == nil || before?.name != friend.name || before?.user != friend.user
          || before?.group != friend.group || before?.request != friend.request
          || before?.color != friend.color || before?.blocked != friend.blocked
        if changed {
          if let before { invalidate(before) }
          if before?.name != friend.name { skeletons[friend.key] = Username.skeleton(friend.name) }
          invalidate(friend)
          indexed[friend.key] = friend
        }
        if keyIndexes[friend.key] != index {
          keyIndexes[friend.key] = index
          if let user = friend.user { users.insert(user) }
          if Self.typed(friend), let skeleton = skeletons[friend.key] { typed.insert(skeleton) }
          changedNames.insert(friend.name)
        }
      }
      // Only affected entries are replaced: no rebuild on a lift.
      for user in users { userIndexes[user] = nil }
      for skeleton in typed { typedIndexes[skeleton] = nil }
      for name in changedNames { names.remove(name) }
      for (index, friend) in all.enumerated() {
        if let user = friend.user, users.contains(user), userIndexes[user] == nil { userIndexes[user] = index }
        let skeleton = skeletons[friend.key] ?? ""
        if Self.typed(friend), typed.contains(skeleton), typedIndexes[skeleton] == nil { typedIndexes[skeleton] = index }
        if changedNames.contains(friend.name) { names.insert(friend.name) }
      }
      if quiet { refreshMirror() }
    }
  }

  /// The account's storage (`AccountStore.store`) for friends known by user id. Blocks go through `setBlocked`.
  @ObservationIgnored var stored: (@MainActor (_ changed: [Friend], _ removed: [String]) -> Void)?
  /// A change that is not this person's (signing out, the storage's list coming in): not written back.
  @ObservationIgnored private var quiet = false

  private static let key = "friends"
  private static let persistence = DispatchQueue(label: "app.justyo.friends-cache", qos: .utility)

  init() {
    let saved = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode([Friend].self, from: $0) }
    all = (saved ?? []).enumerated().map { index, friend in
      var friend = friend
      if friend.color < 0 { friend.color = index }
      return friend
    }
    updateIndexes([])
    refreshMirror()
  }

  private static func typed(_ friend: Friend) -> Bool { friend.user == nil && friend.group == nil }

  /// One bar per group this account is in (spec 3.4), new ones at the bottom until a Yo (the one sent as it is made)
  /// lifts them (`yo_ui::list`); a group left goes.
  func takeGroups(_ groups: [YoGroup]) {
    Perf.timed("friends.groups") {
      var list = all
      for group in groups {
        let name = group.name.isEmpty ? productText(text: .group, lang: .english) : group.name
        if let index = list.firstIndex(where: { $0.group == group.id }) {
          if group.left {
            list.remove(at: index)
          } else if list[index].name != name {
            Stats.renamed(from: list[index].name, to: name)
            list[index].name = name
          }
        } else if !group.left {
          var bar = Friend(name: name, color: nextColor)
          bar.group = group.id
          bar.request = !group.accepted
          list.append(bar)
        }
        if let index = list.firstIndex(where: { $0.group == group.id }), list[index].request != !group.accepted {
          list[index].request = !group.accepted
        }
      }
      let known = Set(groups.filter { !$0.left }.map(\.id))
      list.removeAll { $0.group.map { !known.contains($0) } ?? false }
      // `==` is by key alone: a new name or a request answered is a change too.
      if list != all || zip(list, all).contains(where: { $0.name != $1.name || $0.request != $1.request }) { all = list }
    }
  }


  var shown: [Friend] { all.filter { !$0.blocked } }
  var hasFriend: Bool {
    all.contains {
      productIsRealFriend(skeleton: Username.skeleton($0.name), group: $0.group != nil,
        request: $0.request, blocked: $0.blocked, gone: $0.gone)
    }
  }
  var blocked: [Friend] { all.filter(\.blocked) }
  /// The people, blocked ones too, without groups.
  var contacts: [Friend] { all.filter { $0.group == nil } }

  /// The colour fewest friends have, so the list stays colourful.
  var nextColor: Int {
    let used = Dictionary(grouping: all, by: { $0.color % Color.rows.count }).mapValues(\.count)
    return Color.rows.indices.min { (used[$0] ?? 0, $0) < (used[$1] ?? 0, $1) } ?? 0
  }

  /// Adds a name at the bottom (the Yo the add sends lifts it, `yo_ui::list`); an empty or known one (in any
  /// spelling) is ignored. A gone friend gives its place to the name's new owner. Where both user ids are known, they
  /// decide.
  func add(_ typed: String, user: String? = nil) {
    let name = Username.clean(typed)
    let skeleton = Username.skeleton(name)
    func same(_ friend: Friend) -> Bool {
      if friend.group != nil { return false }
      if let user, let known = friend.user, !friend.gone { return known == user }
      return Username.skeleton(friend.name) == skeleton || (user != nil && friend.user == user)
    }
    guard !name.isEmpty, !all.contains(where: { !$0.gone && same($0) }) else { return }
    var list = all.filter { !same($0) }  // gone ones only
    list.append(Friend(name: name, color: nextColor, user: user))
    all = list
  }

  /// The friend a user id belongs to: known by it, or by one of its handles, or new. Takes the owner's spelling.
  func friend(user: String, handles: [String]) -> Friend {
    return Perf.timed("friends.resolve") {
      let spelled = handles.first { !$0.hasPrefix("+") }  // the username, as its owner writes it
      let skeletons = Set(handles.map(Username.skeleton))
      let index =
        userIndexes[user] ?? skeletons.compactMap { typedIndexes[$0] }.min()
      if let index {
        // Written only when changed: each write saves and redraws the list.
        if all[index].user != user { all[index].user = user }
        if let spelled, all[index].name != spelled, Username.skeleton(spelled) == Username.skeleton(all[index].name),
          !names.contains(spelled)
        {
          all[index].name = spelled
        }
        return all[index]
      }
      add(spelled ?? handles.first ?? "?", user: user)
      return userIndexes[user].map { all[$0] } ?? Friend(name: spelled ?? handles.first ?? "?", color: 0, user: user)
    }
  }

  /// An inbox snapshot reconciled in one publication, requests marked before new friends are mirrored.
  func reconcile(_ contacts: [YoContact], users: [String], changedKeys: Set<String>) -> [String: Friend] {
    Perf.timed("friends.reconcile.batch") {
      let contacts = Dictionary(contacts.map { ($0.user, $0) }, uniquingKeysWith: { a, _ in a })
      var list = all
      var indexes = userIndexes
      var typed = typedIndexes
      var usedNames = names
      var colors = Array(repeating: 0, count: Color.rows.count)
      for friend in list { colors[friend.color % colors.count] += 1 }
      var changed = false
      var added: [Friend] = []
      for user in users {
        let handles = contacts[user]?.handles ?? []
        let spelled = handles.first { !$0.hasPrefix("+") }
        let skeletons = handles.map(Username.skeleton)
        if let index = indexes[user] ?? skeletons.compactMap({ typed[$0] }).min() {
          if list[index].user != user { list[index].user = user; changed = true }
          indexes[user] = index
          for skeleton in skeletons { typed.removeValue(forKey: skeleton) }
          if let spelled, list[index].name != spelled,
            Username.skeleton(spelled) == Username.skeleton(list[index].name), !usedNames.contains(spelled) {
            usedNames.remove(list[index].name)
            list[index].name = spelled
            usedNames.insert(spelled)
            changed = true
          }
        } else {
          let color = colors.indices.min { (colors[$0], $0) < (colors[$1], $1) } ?? 0
          colors[color] += 1
          var friend = Friend(name: spelled ?? handles.first ?? "?", color: color, user: user)
          friend.request = contacts[user].map { !$0.accepted && !$0.blocked } ?? false
          indexes[user] = list.count
          list.append(friend)
          added.append(friend)
          changed = true
        }
      }
      var result: [String: Friend] = [:]
      for index in list.indices {
        guard let user = list[index].user else { continue }
        if let contact = contacts[user] {
          let request = !contact.accepted && !contact.blocked
          if list[index].request != request { list[index].request = request; changed = true }
          if contact.gone && !list[index].gone { list[index].gone = true; changed = true }
        }
        if changedKeys.contains(user) {
          if !list[index].keyChanged { list[index].keyChanged = true; changed = true }
          if !keyAlerts.contains(list[index].key) { keyAlerts.append(list[index].key) }
        }
        result[user] = list[index]
      }
      if changed { all = list }
      return result
    }
  }

  /// One stable partition and one cache write for a whole arrival run.
  func promoteBatch(_ keys: [UUID]) {
    let selected = Set(keys)
    let front = keys.reversed().compactMap { keyIndexes[$0].map { all[$0] } }
    let list = front + all.filter { !selected.contains($0.key) }
    guard list != all else { return }
    let end = keys.compactMap { keyIndexes[$0] }.max() ?? 0
    var users: Set<String> = []
    var typed: Set<String> = []
    var mirrored: Set<String> = []
    for (index, friend) in list.prefix(end + 1).enumerated() {
      keyIndexes[friend.key] = index
      if let user = friend.user {
        if users.insert(user).inserted { userIndexes[user] = index }
        // First wins for older caches with duplicate bars.
        if !friend.request, mirrored.insert(user).inserted, mirroredUsers[user]?.key != friend.key {
          dirtyUsers.insert(user)
        }
      }
      let skeleton = skeletons[friend.key] ?? ""
      if Self.typed(friend), typed.insert(skeleton).inserted { typedIndexes[skeleton] = index }
    }
    reordering = true
    defer { reordering = false }
    all = list
    if !dirtyUsers.isEmpty { mirrorChanges() }
  }

  /// The hub's answer for a typed name (`Inbox.identify`); a friend removed meanwhile stays removed.
  func found(_ name: String, user: String, spelled: String?) {
    let skeleton = Username.skeleton(name)
    guard all.contains(where: { Self.typed($0) && Username.skeleton($0.name) == skeleton }) else { return }
    _ = friend(user: user, handles: [spelled ?? name])
  }

  /// Friends already on the list take the core's spelling (never added here).
  func spell(_ contacts: [YoContact]) {
    Perf.timed("friends.spell") {
      for contact in contacts {
        let skeletons = Set(contact.handles.map(Username.skeleton))
        if all.contains(where: { $0.user == contact.user || (Self.typed($0) && skeletons.contains(Username.skeleton($0.name))) }) {
          _ = friend(user: contact.user, handles: contact.handles)
        }
      }
    }
  }

  func setUndoFirst(_ friend: Friend, _ on: Bool) {
    guard let index = all.firstIndex(where: { $0.key == friend.key }), all[index].undoFirst != on else { return }
    all[index].undoFirst = on
  }

  func setUndoSeconds(_ friend: Friend, _ seconds: Double) {
    let seconds = productUndoSeconds(seconds: seconds)
    guard let index = all.firstIndex(where: { $0.key == friend.key }), all[index].undoSeconds != seconds else { return }
    all[index].undoSeconds = seconds
  }

  /// Contacts not yet said yes to are requests (spec 3.3).
  func markRequests(_ contacts: [YoContact]) {
    Perf.timed("friends.requests") {
      let open = Set(contacts.filter { !$0.accepted && !$0.blocked }.map(\.user))
      let changed = all.contains { friend in friend.group == nil && friend.user.map(open.contains) != nil
        && friend.request != open.contains(friend.user ?? "") }
      guard changed else { return }
      all = all.map { friend in
        var friend = friend
        if friend.group == nil, let user = friend.user { friend.request = open.contains(user) }
        return friend
      }
    }
  }

  func markGone(_ contacts: [YoContact]) {
    Perf.timed("friends.gone") {
      let gone = Set(contacts.filter(\.gone).map(\.user))
      guard all.contains(where: { $0.user.map(gone.contains) == true && !$0.gone }) else { return }
      all = all.map { friend in
        var friend = friend
        if let user = friend.user, gone.contains(user) { friend.gone = true }
        return friend
      }
    }
  }

  /// The account's storage (spec 4.3) into this list: missing friends come at the end in their colour, known ones take
  /// its name, colour and block, and with `prune` those it no longer has go. Typed-only friends stay. With `seed`
  /// (once, for an account from before the storage) what the storage lacks goes up and none go.
  func take(_ kept: [YoFriend], names: [String: String] = [:], seed: Bool, prune: Bool) {
    Perf.timed("friends.take") {
      var list = all
      for friend in kept {
        let stored = friend.name.flatMap(Self.storedName)
        if let index = list.firstIndex(where: { $0.user == friend.user }) ?? stored.flatMap({ name in
          list.firstIndex { Self.typed($0) && Username.skeleton($0.name) == Username.skeleton(name) }
        }) {
          list[index].user = friend.user
          list[index].blocked = friend.blocked
          if let stored { list[index].name = stored }
          if let color = friend.color, list[index].color % Color.rows.count != Int(color) { list[index].color = Int(color) }
        } else {
          let name = stored ?? names[friend.user].flatMap(Self.storedName) ?? productText(text: .yoUser, lang: .english)
          var new = Friend(name: name, color: friend.color.map(Int.init) ?? list.count, user: friend.user)
          new.blocked = friend.blocked
          list.append(new)
        }
      }
      if prune && !seed {
        let users = Set(kept.map(\.user))
        list.removeAll { !$0.request && ($0.user.map { !users.contains($0) } ?? false) }  // requests aren't kept there
      }
      if list.map(Self.mirrored) != all.map(Self.mirrored) { quietly { all = list } }
      guard seed else { return }
      let keptUsers = Dictionary(kept.map { ($0.user, $0) }, uniquingKeysWith: { a, _ in a })
      let missing = all.filter { friend in
        guard let user = friend.user, !friend.request else { return false }
        return keptUsers[user].map { $0.name != friend.name } ?? true
      }
      if !missing.isEmpty { stored?(missing, []) }
    }
  }

  /// A name as the storage keeps it (spec 4.3): 1 to 64 bytes of UTF-8 without control characters, else none.
  static func storedName(_ name: String) -> String? {
    let name = name.precomposedStringWithCanonicalMapping
    guard !name.isEmpty, name.utf8.count <= 64, !name.unicodeScalars.contains(where: { $0.properties.generalCategory == .control })
    else { return nil }
    return name
  }

  private static func mirrored(_ friend: Friend) -> [String] {
    [friend.key.uuidString, friend.name, "\(friend.color)", friend.user ?? "", "\(friend.blocked)"]
  }

  func keyChangeSeen(_ friend: Friend) {
    guard let index = all.firstIndex(where: { $0.key == friend.key }), all[index].keyChanged else { return }
    all[index].keyChanged = false
  }

  /// Friends whose new key is still to be said (an alert, once each), oldest first.
  private(set) var keyAlerts: [UUID] = []

  func keyAlertShown(_ friend: Friend) {
    keyAlerts.removeAll { $0 == friend.key }
  }

  func shownFriend(named name: String) -> Friend? {
    all.first { $0.name == name && !$0.blocked }
  }

  /// The bar a Yo arrived on: a group's by its id (a group Yo's id starts with it, a group photo comes by the bar's
  /// `mediaKey`), a person's by name, so a group named as a friend never takes the friend's Yos, nor the friend the
  /// group's.
  func shownBar(for arrival: Arrival) -> Friend? {
    let first = arrival.id.split(separator: "/").first.map(String.init)
    if let bar = all.first(where: {
      !$0.blocked && $0.group != nil && ($0.mediaKey == arrival.from || $0.group == first)
    }) {
      return bar
    }
    return all.first { $0.name == arrival.from && !$0.blocked && $0.group == nil } ?? shownFriend(named: arrival.from)
  }

  /// Signed out (the account's storage keeps them).
  func reset() {
    quietly { all = [] }
    keyAlerts = []
  }

  private func quietly(_ change: () -> Void) {
    quiet = true
    defer { quiet = false }
    change()
  }

  private func mirrorCandidate(_ user: String) -> Friend? {
    guard let index = userIndexes[user] else { return nil }
    let first = all[index]
    if !first.request { return first }
    return all.dropFirst(index + 1).first { $0.user == user && !$0.request }
  }

  private func refreshMirror() {
    for user in dirtyUsers {
      mirroredUsers[user] = mirrorCandidate(user)
    }
    dirtyUsers.removeAll(keepingCapacity: true)
  }

  private func mirrorChanges() {
    Perf.timed("friends.mirror") {
      var changed: [Friend] = []
      var removed: [String] = []
      for user in dirtyUsers {
        let before = mirroredUsers[user]
        let now = mirrorCandidate(user)
        if let now {
          if before == nil || before?.name != now.name || before?.color != now.color { changed.append(now) }
        } else if before != nil { removed.append(user) }
        mirroredUsers[user] = now
      }
      dirtyUsers.removeAll(keepingCapacity: true)
      if !changed.isEmpty || !removed.isEmpty { stored?(changed, removed) }
    }
  }

  func remove(_ friend: Friend) {
    all.removeAll { $0 == friend }
  }

  func setBlocked(_ friend: Friend, _ blocked: Bool) {
    guard let index = all.firstIndex(where: { $0.id == friend.id }) else { return }
    all[index].blocked = blocked
  }

  private func save() {
    let snapshot = all
    let key = Self.key
    Self.persistence.async {
      Perf.timed("friends.cache") {
        UserDefaults.standard.set(try? JSONEncoder().encode(snapshot), forKey: key)
      }
    }
  }
}
