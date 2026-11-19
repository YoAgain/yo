// Statistics: how many Yos went and came, counted on this iPhone only (the hub keeps no counts, spec 1), and the
// friend with the most. Gone with the account on Sign Out or Delete Account.
import Foundation

@MainActor
enum Stats {
  private static let defaults = UserDefaults.standard
  private static let writes = DispatchQueue(label: "yo.stats", qos: .utility)

  static var byFriend: [String: Int] { defaults.dictionary(forKey: "stats.friends") as? [String: Int] ?? [:] }

  static func sentYo(to friend: String) {
    writes.async {
      count("stats.sent", friend)
      stamp("stats.lastSent", friend)
    }
  }
  static func receivedYos(from friend: String, _ n: Int) {
    writes.async {
      count("stats.received", friend, n)
      stamp("stats.lastReceived", friend)
    }
  }

  /// For a bar's menu, by name as the counts.
  static func lastSent(to friend: String) -> Date? { last("stats.lastSent", friend) }
  static func lastReceived(from friend: String) -> Date? { last("stats.lastReceived", friend) }

  static func reset() {
    writes.async {
      for key in ["stats.sent", "stats.received", "stats.friends", "stats.lastSent", "stats.lastReceived"] {
        UserDefaults.standard.removeObject(forKey: key)
      }
    }
  }

  nonisolated private static func stamp(_ key: String, _ friend: String) {
    let defaults = UserDefaults.standard
    var times = defaults.dictionary(forKey: key) as? [String: Double] ?? [:]
    times[friend] = Date.now.timeIntervalSince1970
    defaults.set(times, forKey: key)
  }

  private static func last(_ key: String, _ friend: String) -> Date? {
    (defaults.dictionary(forKey: key) as? [String: Double])?[friend].map { Date(timeIntervalSince1970: $0) }
  }

  nonisolated private static func count(_ key: String, _ friend: String, _ n: Int = 1) {
    let defaults = UserDefaults.standard
    var friends = defaults.dictionary(forKey: "stats.friends") as? [String: Int] ?? [:]
    friends[friend, default: 0] += n
    defaults.set(friends, forKey: "stats.friends")
    defaults.set(defaults.integer(forKey: key) + n, forKey: key)
  }
}

/// File bytes in both writable containers, counted on a utility thread.
struct LocalStorageUsage: Sendable {
  var total: Int64 = 0

  static func load() async -> Self {
    await Task.detached(priority: .utility) { scan() }.value
  }

  private static func scan() -> Self {
      var roots = [URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)]
      if let group = ShareBridge.container { roots.append(group) }
      var result = Self()
      var seen = Set<String>()
      for root in roots {
        guard let files = FileManager.default.enumerator(
          at: root, includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey], options: [.skipsPackageDescendants])
        else { continue }
        for case let file as URL in files {
          guard seen.insert(file.standardizedFileURL.path).inserted,
            let values = try? file.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey]),
            values.isRegularFile == true
          else { continue }
          result.total += Int64(values.fileSize ?? 0)
        }
      }
      return result
  }

  static func formatted(_ bytes: Int64) -> String {
    ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
  }
}
