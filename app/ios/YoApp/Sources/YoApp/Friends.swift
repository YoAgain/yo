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
  /// A group's bar (spec 4.9): the group's id; its name is the group's. No user id then.
  var group: String?
  /// Blocked (spec 4.8): off the list, nothing from them shown; Settings lists them to unblock.
  var blocked = false
  /// Their account is gone (deleted): the bar says "Not on Yo". The name
  /// added again is asked of the hub anew and may be someone else's now.
  var gone = false
  /// Their account key changed (spec 3.2: a new phone without its backup, or someone else): Contacts says so until
  /// their details are opened.
  var keyChanged = false
  /// A request (spec 4.10): someone not yet a contact wrote, or added this account to a group. The bar asks "Yo?"
  /// instead of sending; nothing goes to them until it is said yes to.
  var request = false
  /// "Undo Before Sending" (Settings › Encryption › Per Contact): a Yo tapped to them waits `FriendList.sendDelay`
  /// under the undo ring, as a photo does, so one sent by mistake can be taken back. Kept on this phone.
  var undoFirst = false
  /// How long that Yo waits: 2 to 10 seconds, set beside the switch.
  var undoSeconds: Double = 2  // FriendList.sendDelay
  /// Who they are on this phone, for good: a new spelling or a user id learned later keeps the bar in its place (a
  /// name as identity made the bar leave and come in again on top).
  var key = UUID()
  var id: UUID { key }

  /// Where a Yo to the bar goes: the user id once known, else the name; a group's, "group:" and its id.
  var target: String { group.map { "group:" + $0 } ?? user ?? name }
  /// Where its photos and videos and its last sender are kept (Inbox): a group by its id ("group:" and it), so a rename
  /// or a namesake never mixes them; a friend by its user id (the name's skeleton before it is known).
  var mediaKey: String { group.map { "group:" + $0 } ?? user.map { "user:" + $0 } ?? Username.skeleton(name) }

  /// One friend is one friend, whatever was learned about them since: by `key` alone.
  static func == (a: Friend, b: Friend) -> Bool { a.key == b.key }
  func hash(into hasher: inout Hasher) { hasher.combine(key) }

  /// Identity equality is for gestures; caches must compare the displayed content too.
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
    undoSeconds = try c.decodeIfPresent(Double.self, forKey: .undoSeconds) ?? 2
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
  @ObservationIgnored private var exactKeys: [String: UUID] = [:]
  @ObservationIgnored private var foldedKeys: [String: UUID] = [:]
  @ObservationIgnored private var mirroredUsers: [String: Friend] = [:]
  @ObservationIgnored private var dirtyUsers: Set<String> = []

  private func updateIndexes(_ old: [Friend]) {
    Perf.timed("friends.index") {
      var users: Set<String> = []
      var typed: Set<String> = []
      var changedNames: Set<String> = []
      var folded: Set<String> = []
      @MainActor func invalidate(_ friend: Friend) {
        if let user = friend.user { users.insert(user); dirtyUsers.insert(user) }
        let skeleton = skeletons[friend.key] ?? Username.skeleton(friend.name)
        if Self.typed(friend) { typed.insert(skeleton) }
        changedNames.insert(friend.name)
        folded.insert(skeleton)
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
        // A lift changes offsets, never the text's skeleton or storage mirror.
        if keyIndexes[friend.key] != index {
          keyIndexes[friend.key] = index
          if let user = friend.user { users.insert(user) }
          if Self.typed(friend), let skeleton = skeletons[friend.key] { typed.insert(skeleton) }
          changedNames.insert(friend.name)
          if let skeleton = skeletons[friend.key] { folded.insert(skeleton) }
        }
      }
      // Only affected entries are replaced; no dictionary or skeleton rebuild on a lift.
      for user in users { userIndexes[user] = nil }
      for skeleton in typed { typedIndexes[skeleton] = nil }
      for name in changedNames { exactKeys[name] = nil; names.remove(name) }
      for skeleton in folded { foldedKeys[skeleton] = nil }
      for (index, friend) in all.enumerated() {
        if let user = friend.user, users.contains(user), userIndexes[user] == nil { userIndexes[user] = index }
        let skeleton = skeletons[friend.key] ?? ""
        if Self.typed(friend), typed.contains(skeleton), typedIndexes[skeleton] == nil { typedIndexes[skeleton] = index }
        if changedNames.contains(friend.name) {
          names.insert(friend.name)
          if !friend.blocked, exactKeys[friend.name] == nil { exactKeys[friend.name] = friend.key }
        }
        if !friend.blocked, folded.contains(skeleton), foldedKeys[skeleton] == nil { foldedKeys[skeleton] = friend.key }
      }
      if quiet { refreshMirror() }
    }
  }

  func liftKey(named name: String) -> UUID? {
    exactKeys[name] ?? foldedKeys[Username.skeleton(name)]
  }

  /// Where friends known by user id go as they are added, change their spelling or are removed: the account's
  /// storage (`AccountStore.store`), set by the app. Blocks go there through the core's own `setBlocked`.
  @ObservationIgnored var stored: (@MainActor (_ changed: [Friend], _ removed: [String]) -> Void)?
  /// A change that is not this person's (signing out, the storage's own list coming in): not written back.
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

  /// A friend known only by the name typed (not yet by user id), not a group's bar.
  private static func typed(_ friend: Friend) -> Bool { friend.user == nil && friend.group == nil }

  /// The group bars as the core has the groups (spec 4.9): one for each group this account is in, named as the group
  /// ("Group" until its name is known), new ones on top; a group left goes.
  func takeGroups(_ groups: [YoGroup]) {
    Perf.timed("friends.groups") {
      var list = all
      for group in groups {
        let name = group.name.isEmpty ? "Group" : group.name
        if let index = list.firstIndex(where: { $0.group == group.id }) {
          if group.left {
            list.remove(at: index)
          } else if list[index].name != name {
            list[index].name = name
          }
        } else if !group.left {
          var bar = Friend(name: name, color: nextColor)
          bar.group = group.id
          bar.request = !group.accepted
          list.insert(bar, at: 0)
        }
        if let index = list.firstIndex(where: { $0.group == group.id }), list[index].request != !group.accepted {
          list[index].request = !group.accepted
        }
      }
      let known = Set(groups.filter { !$0.left }.map(\.id))
      list.removeAll { $0.group.map { !known.contains($0) } ?? false }
      // Friends compare by `key` alone: a new name or a request answered is a change too.
      if list != all || zip(list, all).contains(where: { $0.name != $1.name || $0.request != $1.request }) { all = list }
    }
  }

  /// The bar of a group, by its id.
  func groupBar(_ id: String) -> Friend? { all.first { $0.group == id } }

  /// The friends on the list: all but the blocked.
  var shown: [Friend] { all.filter { !$0.blocked } }
  /// A friend on the list who counts as one (bots, groups, requests, blocked and gone accounts don't).
  var hasFriend: Bool {
    all.contains {
      $0.group == nil && !$0.request && !$0.blocked && !$0.gone && !AddFriendHint.bots.contains($0.name.lowercased())
    }
  }
  var blocked: [Friend] { all.filter(\.blocked) }
  /// The people, blocked ones too: no group's bar (a group has no chat of its own to check).
  var contacts: [Friend] { all.filter { $0.group == nil } }

  /// The colour the next friend gets: the one fewest friends have (the first of those), so the list stays colourful.
  var nextColor: Int {
    let used = Dictionary(grouping: all, by: { $0.color % Color.rows.count }).mapValues(\.count)
    return Color.rows.indices.min { (used[$0] ?? 0, $0) < (used[$1] ?? 0, $1) } ?? 0
  }

  /// Adds a username as typed (only what a name may hold); an empty or known name (in any spelling) is ignored. New
  /// friends come on top, as in the original. A friend whose account is gone gives its place to the name's new owner
  /// (or to the name as typed, asked of the hub again). Where both user ids are known, they decide: a name another
  /// user had before is that other user's bar, not this one's.
  func add(_ typed: String, user: String? = nil) {
    let name = Username.clean(typed)
    let skeleton = Username.skeleton(name)
    func same(_ friend: Friend) -> Bool {
      if friend.group != nil { return false }  // a group's bar is no friend, whatever its name
      if let user, let known = friend.user, !friend.gone { return known == user }
      return Username.skeleton(friend.name) == skeleton || (user != nil && friend.user == user)
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
    return Perf.timed("friends.resolve") {
      let spelled = handles.first { !$0.hasPrefix("+") }  // the username, as its owner writes it
      let skeletons = Set(handles.map(Username.skeleton))
      let index =
        userIndexes[user] ?? skeletons.compactMap { typedIndexes[$0] }.min()
      if let index {
        // Written only when something changed: each write saves the list and redraws it (once per Yo in a burst).
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

  /// Reconcile an inbox snapshot in one publication, including requests before mirroring new friends.
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
      if changed {
        let newKeys = Set(added.map(\.key))
        all = list.filter { newKeys.contains($0.key) }.reversed() + list.filter { !newKeys.contains($0.key) }
      }
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
    var exact: Set<String> = []
    var folded: Set<String> = []
    var mirrored: Set<String> = []
    for (index, friend) in list.prefix(end + 1).enumerated() {
      keyIndexes[friend.key] = index
      if let user = friend.user {
        if users.insert(user).inserted { userIndexes[user] = index }
        // Normally a user has one bar. Retain first-wins storage semantics for older duplicate caches.
        if !friend.request, mirrored.insert(user).inserted, mirroredUsers[user]?.key != friend.key {
          dirtyUsers.insert(user)
        }
      }
      let skeleton = skeletons[friend.key] ?? ""
      if Self.typed(friend), typed.insert(skeleton).inserted { typedIndexes[skeleton] = index }
      if !friend.blocked {
        if exact.insert(friend.name).inserted { exactKeys[friend.name] = friend.key }
        if folded.insert(skeleton).inserted { foldedKeys[skeleton] = friend.key }
      }
    }
    reordering = true
    defer { reordering = false }
    all = list
    if !dirtyUsers.isEmpty { mirrorChanges() }
  }

  /// What the hub says a typed name is (`Inbox.identify`): the friend added by it learns the user id and takes the
  /// owner's spelling. A friend removed meanwhile stays removed.
  func found(_ name: String, user: String, spelled: String?) {
    let skeleton = Username.skeleton(name)
    guard all.contains(where: { Self.typed($0) && Username.skeleton($0.name) == skeleton }) else { return }
    _ = friend(user: user, handles: [spelled ?? name])
  }

  /// The core's contacts: friends already on the list take the spelling it knows now (a friend is never added here).
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

  /// A Yo went to or came from `name`: they go on top (the original's promoteObjectToTop).
  /// The name in any spelling: the friend may have been renamed to its owner's spelling meanwhile. One already first
  /// on the list (only blocked ones above) stays as it is: nothing is written, nothing moves.
  func promote(_ name: String) {
    Perf.timed("friends.promote") {
      let skeleton = Username.skeleton(name)
      let found =
        all.firstIndex(where: { $0.name == name }) ?? all.firstIndex(where: { Username.skeleton($0.name) == skeleton })
      guard let index = found, all[..<index].contains(where: { !$0.blocked }) else { return }
      var list = all
      list.insert(list.remove(at: index), at: 0)
      all = list
    }
  }

  /// A friend's "Undo Before Sending" on or off.
  func setUndoFirst(_ friend: Friend, _ on: Bool) {
    guard let index = all.firstIndex(where: { $0.key == friend.key }), all[index].undoFirst != on else { return }
    all[index].undoFirst = on
  }

  /// How long a friend's Yo waits under "Undo Before Sending" (2 to 10 s).
  func setUndoSeconds(_ friend: Friend, _ seconds: Double) {
    let seconds = min(max(seconds, FriendList.sendDelay), 10)
    guard let index = all.firstIndex(where: { $0.key == friend.key }), all[index].undoSeconds != seconds else { return }
    all[index].undoSeconds = seconds
  }

  /// The core's contacts not yet said yes to (`YoContact.accepted`): their bars are requests (spec 4.10).
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

  /// The core's contacts whose account is gone (`YoContact.gone`): their bars say "Not on Yo".
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

  /// The account's storage (spec 6.9) as the core has it, into this list, which mirrors it: friends kept there that
  /// this phone does not have come on the list (after a restore: the whole list), at the end, in their colour; those
  /// here take its name, colour and block (the core holds to the blocks too); friends known by user id that it no
  /// longer has (taken off on another phone) go. Friends known only by a typed name stay. A friend kept without a name
  /// (a block alone, a name over 64 bytes) takes `names` (the hub's, by user id), else "Yo User". Nothing is written
  /// back, but with `seed` (once, for an account from before the storage) the friends this phone has and the storage
  /// lacks go up, and none go; nor without `prune` (a restored phone whose storage the core may not have read yet).
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
          let name = stored ?? names[friend.user].flatMap(Self.storedName) ?? "Yo User"
          var new = Friend(name: name, color: friend.color.map(Int.init) ?? list.count, user: friend.user)
          new.blocked = friend.blocked
          list.append(new)
        }
      }
      if prune && !seed {
        let users = Set(kept.map(\.user))
        list.removeAll { !$0.request && ($0.user.map { !users.contains($0) } ?? false) }  // a request is not kept there
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

  /// A name as the storage keeps it (spec 6.9): kept as it is (a name the user gave is no username: spaces and all),
  /// 1 to 64 bytes of UTF-8 without control characters; else none.
  static func storedName(_ name: String) -> String? {
    let name = name.precomposedStringWithCanonicalMapping
    guard !name.isEmpty, name.utf8.count <= 64, !name.unicodeScalars.contains(where: { $0.properties.generalCategory == .control })
    else { return nil }
    return name
  }

  /// What the storage mirrors of a friend, to tell whether taking it changed anything.
  private static func mirrored(_ friend: Friend) -> [String] {
    [friend.key.uuidString, friend.name, "\(friend.color)", friend.user ?? "", "\(friend.blocked)"]
  }

  /// A contact's account key changed (spec 3.2): marked for Contacts, and said once (`keyAlerts`).
  func keyChanged(_ friend: Friend) {
    guard let index = all.firstIndex(where: { $0.key == friend.key }) else { return }
    if !all[index].keyChanged { all[index].keyChanged = true }
    if !keyAlerts.contains(friend.key) { keyAlerts.append(friend.key) }
  }

  /// Their details were opened: Contacts no longer marks the change.
  func keyChangeSeen(_ friend: Friend) {
    guard let index = all.firstIndex(where: { $0.key == friend.key }), all[index].keyChanged else { return }
    all[index].keyChanged = false
  }

  /// Friends whose new key is still to be said (an alert, once each), oldest first.
  private(set) var keyAlerts: [UUID] = []

  /// The friend's new key was said.
  func keyAlertShown(_ friend: Friend) {
    keyAlerts.removeAll { $0 == friend.key }
  }

  /// The friend shown under this name now, if any.
  func shownFriend(named name: String) -> Friend? {
    all.first { $0.name == name && !$0.blocked }
  }

  /// Signed out: the friends of a new phone, none. (The account's storage keeps them.)
  func reset() {
    quietly { all = [] }
    keyAlerts = []
  }

  private func quietly(_ change: () -> Void) {
    quiet = true
    defer { quiet = false }
    change()
  }

  /// Friends known by user id that are new, newly named, respelled or recoloured since `old`, and those removed: to
  /// the account's storage.
  private func mirrorCandidate(_ user: String) -> Friend? {
    guard let index = userIndexes[user] else { return nil }
    let first = all[index]
    if !first.request { return first }
    // A duplicate may be accepted even when the first bar is a request.
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

  /// Off the list, or back on it (the core blocks them: `AccountStore.setBlocked`).
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
