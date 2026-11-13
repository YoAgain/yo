// Statistics: how many Yos went and came, counted on this iPhone only (the hub keeps no counts, spec 1), and the
// friend with the most. Gone with the account on Sign Out or Delete Account.
import SwiftUI

@MainActor
enum Stats {
  private static let defaults = UserDefaults.standard

  static var sent: Int { defaults.integer(forKey: "stats.sent") }
  static var received: Int { defaults.integer(forKey: "stats.received") }

  /// Yos with each friend (sent and received), by name.
  static var byFriend: [String: Int] { defaults.dictionary(forKey: "stats.friends") as? [String: Int] ?? [:] }

  static func sentYo(to friend: String) { count("stats.sent", friend) }
  static func receivedYos(from friend: String, _ n: Int) { count("stats.received", friend, n) }

  static func reset() {
    for key in ["stats.sent", "stats.received", "stats.friends"] { defaults.removeObject(forKey: key) }
  }

  private static func count(_ key: String, _ friend: String, _ n: Int = 1) {
    defaults.set(defaults.integer(forKey: key) + n, forKey: key)
    var friends = byFriend
    friends[friend, default: 0] += n
    defaults.set(friends, forKey: "stats.friends")
  }
}

/// The numbers, as a plain list.
struct StatsPage: View {
  @Environment(Friends.self) private var friends

  var body: some View {
    let top = Stats.byFriend.max { $0.value < $1.value }
    Form {
      Section {
        row("Yos Sent", "\(Stats.sent)", icon: "arrow.up.circle.fill", color: .amethyst)
        row("Yos Received", "\(Stats.received)", icon: "arrow.down.circle.fill", color: .blue)
        row("Friends", "\(friends.all.count)", icon: "person.2.fill", color: .green)
        if let top {
          row("Most Yos With", "\(top.key) · \(top.value)", icon: "star.fill", color: .orange)
        }
      } footer: {
        Text("Counted on this iPhone.")
      }
    }
    .navigationTitle("Statistics")
    .navigationBarTitleDisplayMode(.inline)
  }

  private func row(_ title: String, _ value: String, icon: String, color: Color) -> some View {
    HStack {
      Label(title, systemImage: icon).labelStyle(Tile(color))
      Spacer()
      Text(value).foregroundStyle(Color(.secondaryLabel)).monospacedDigit()
    }
  }
}
