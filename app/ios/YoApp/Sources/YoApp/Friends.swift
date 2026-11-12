// The friends of the mockup: names you add yourself, in the order you sort them, kept on this phone. A tap on one sends
// you a Yo from them (delay(for:)).
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

  /// When a fake Yo from this friend comes, by the bar: the first at once, the last three after five seconds (time to
  /// lock the phone and see it come as a push), the ones between after two.
  func delay(for friend: Friend) -> Duration {
    guard let index = all.firstIndex(of: friend) else { return .seconds(2) }
    if index == 0 { return .zero }
    if index >= all.count - 3 { return .seconds(5) }
    return .seconds(2)
  }

  private static let key = "friends"
  /// The names of the 2015 press screens, until you change them.
  private static let starting = ["Leo", "Lukas", "Serena", "Adrian", "Jason"].map(Friend.init(name:))

  init() {
    let saved = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode([Friend].self, from: $0) }
    all = saved ?? Self.starting
  }

  /// Adds a name as typed, upper and lower case as "Yo" itself; an empty or known name is ignored.
  func add(_ name: String) {
    let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !name.isEmpty, !all.contains(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) else { return }
    all.append(Friend(name: name))
  }

  /// Sorting by hand: the friend called `name` goes where `target` is.
  func move(_ name: String, to target: Friend) {
    guard let from = all.firstIndex(where: { $0.name == name }), let to = all.firstIndex(of: target), from != to
    else { return }
    let friend = all.remove(at: from)
    all.insert(friend, at: to)
  }

  func remove(_ friend: Friend) {
    all.removeAll { $0 == friend }
  }

  private func save() {
    UserDefaults.standard.set(try? JSONEncoder().encode(all), forKey: Self.key)
  }
}
