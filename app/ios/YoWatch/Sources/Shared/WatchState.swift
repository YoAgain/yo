import Foundation
import Security

struct WatchFriend: Codable, Equatable, Sendable {
  let to: String
  let title: String
  let color: Int
}

struct LastYo: Codable, Equatable, Sendable {
  let to: String
  let name: String
  let at: Double
}

struct WatchState: Codable, Equatable, Sendable {
  let owner: String
  let friends: [WatchFriend]
  let last: LastYo?

  static let empty = Self(owner: "", friends: [], last: nil)
}

extension WatchState {
  init?(context: [String: Any]) {
    // An empty activation context means the phone hasn't published yet.
    guard let owner = context["owner"] as? String else { return nil }
    guard !owner.isEmpty else { self = .empty; return }
    var seen = Set<String>()
    let friends = (context["friends"] as? [Any] ?? []).compactMap { value -> WatchFriend? in
      guard let row = value as? [String: Any], let to = row["to"] as? String, !to.isEmpty,
        let title = row["title"] as? String, seen.insert(to).inserted else { return nil }
      return WatchFriend(to: to, title: title, color: row["color"] as? Int ?? 0)
    }
    var last: LastYo?
    if let row = context["last"] as? [String: Any], let to = row["to"] as? String, !to.isEmpty,
      let name = row["name"] as? String, let at = row["at"] as? Double, at.isFinite {
      last = LastYo(to: to, name: name, at: at)
    }
    self.init(owner: owner, friends: friends, last: last)
  }

  // Both targets have the watch app's group first in their entitlements.
  private static var item: [String: Any] {
    [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "yo.watch.state",
     kSecAttrAccount as String: "state"]
  }

  static func read() -> Self? {
    var query = item
    query[kSecReturnData as String] = true
    var result: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
      let data = result as? Data else { return nil }
    return try? JSONDecoder().decode(Self.self, from: data)
  }

  func save() -> Bool {
    guard let data = try? JSONEncoder().encode(self) else { return false }
    let values: [String: Any] = [kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock]
    let query = Self.item
    let status = SecItemUpdate(query as CFDictionary, values as CFDictionary)
    guard status == errSecItemNotFound else { return status == errSecSuccess }
    return SecItemAdd(query.merging(values) { _, new in new } as CFDictionary, nil) == errSecSuccess
  }
}
