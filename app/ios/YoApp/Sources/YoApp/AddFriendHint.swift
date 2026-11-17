/// The "+" row says "+ Add a Friend" until the first friend is on the list.
enum AddFriendHint {
  /// Accounts that come on the list by themselves and don't count as a friend.
  static let bots: Set<String> = ["yogpt", "ping"]
}
