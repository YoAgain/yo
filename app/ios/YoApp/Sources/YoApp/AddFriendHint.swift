/// The add block stays open (Group, Invite) until the first friend is on the list.
enum AddFriendHint {
  /// Bot accounts that don't count as a friend.
  static let bots: Set<String> = ["YOGPT", "PING", "PINGTEST"]
}
