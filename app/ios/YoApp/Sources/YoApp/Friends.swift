// The friends of the mockup: names you add yourself, kept on this phone, in the original's order: whoever you last
// sent a Yo to or got one from is on top. Each keeps a colour of the original's palette wherever they go. A tap on one
// sends you a Yo from them when their bar goes on top (FriendList.showSent).
import Foundation
import Observation
import SwiftUI

struct Friend: Codable, Hashable, Identifiable {
  var name: String
  /// Their place in the palette (Color.rows), for good.
  var color: Int
  var id: String { name }

  init(name: String, color: Int) {
    self.name = name
    self.color = color
  }

  /// Friends saved before they had a colour get theirs from where they stand (`init()`).
  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    name = try c.decode(String.self, forKey: .name)
    color = try c.decodeIfPresent(Int.self, forKey: .color) ?? -1
  }
}

@MainActor @Observable
final class Friends {
  private(set) var all: [Friend] {
    didSet { save() }
  }

  private static let key = "friends"
  /// The names of the 2015 press screens, until you change them.
  private static let starting = ["Leo", "Lukas", "Serena", "Adrian", "Jason"].enumerated().map {
    Friend(name: $1, color: $0)
  }

  init() {
    let saved = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode([Friend].self, from: $0) }
    all = (saved ?? Self.starting).enumerated().map { index, friend in
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

  /// Adds a name as typed, upper and lower case as "Yo" itself; an empty or known name is ignored. New friends come on
  /// top, as in the original.
  func add(_ name: String) {
    let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !name.isEmpty, !all.contains(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) else { return }
    all.insert(Friend(name: name, color: nextColor), at: 0)
  }

  /// A Yo went to or came from `name`: they go on top (the original's promoteObjectToTop).
  func promote(_ name: String) {
    guard let index = all.firstIndex(where: { $0.name == name }), index != 0 else { return }
    all.insert(all.remove(at: index), at: 0)
  }

  func remove(_ friend: Friend) {
    all.removeAll { $0 == friend }
  }

  private func save() {
    UserDefaults.standard.set(try? JSONEncoder().encode(all), forKey: Self.key)
  }
}
