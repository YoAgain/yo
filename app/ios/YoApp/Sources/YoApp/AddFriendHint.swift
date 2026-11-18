/// The add block stays open (Group, Invite) until the first friend is on the list.
enum AddFriendHint {
  /// Accounts that come on the list by themselves and don't count as a friend.
  static let bots: Set<String> = ["yogpt", "ping"]
}
