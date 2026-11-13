// The friends of the mockup: names you add yourself, kept on this phone, in the original's order: whoever you last
// sent a Yo to or got one from is on top. A tap on one sends you a Yo from them, `replyDelay` later.
import Foundation
import Observation

struct Friend: Codable, Hashable, Identifiable {
  var name: String
  var id: String { name }
}

@MainActor @Observable
final class Friends {
  private(set) var all: [Friend] {
    didSet { save() }
  }

  /// When the fake Yo back comes: time to see "Sent Yo!", or to lock the phone and see it come as a push.
  static let replyDelay = Duration.seconds(3)

  private static let key = "friends"
  /// The names of the 2015 press screens, until you change them.
  private static let starting = ["Leo", "Lukas", "Serena", "Adrian", "Jason"].map(Friend.init(name:))

  init() {
    let saved = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode([Friend].self, from: $0) }
    all = saved ?? Self.starting
  }

  /// Adds a name as typed, upper and lower case as "Yo" itself; an empty or known name is ignored. New friends come on
  /// top, as in the original.
  func add(_ name: String) {
    let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !name.isEmpty, !all.contains(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) else { return }
    all.insert(Friend(name: name), at: 0)
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
