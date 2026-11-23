// Yo's friends and groups in Spotlight (FriendEntity, an IndexedEntity): the names the list shows, nothing else.
// Kept in step with the list ShareInbox publishes: renamed, added, removed and blocked ones (blocked are not in the
// list) change the index; signing out or deleting the account empties it. Core Spotlight's index stays on this
// phone (it is not synced or backed up).
import AppIntents
import CoreSpotlight

@MainActor
enum FriendSpotlight {
  /// What the index holds, by target: the title. Nil before this launch's first list: then it is built anew.
  private static var indexed: [String: String]?
  private static var running: Task<Void, Never>?

  static func update(_ list: ShareBridge.List?) {
    guard let list else { return wipe() }
    var next: [String: String] = [:]
    for friend in list.friends where next[friend.to] == nil { next[friend.to] = friend.title }
    let before = indexed
    guard before != next else { return }
    indexed = next
    let changed = list.friends.filter { before?[$0.to] != $0.title }.map(FriendEntity.init)
    let gone = before.map { Array(Set($0.keys).subtracting(next.keys)) } ?? []
    run { index in
      if before == nil { try await index.deleteAllSearchableItems() }
      if !gone.isEmpty { try await index.deleteAppEntities(identifiedBy: gone, ofType: FriendEntity.self) }
      if !changed.isEmpty { try await index.indexAppEntities(changed) }
    }
  }

  /// Signed out, account deleted.
  static func wipe() {
    guard indexed != [:] else { return }
    indexed = [:]
    run { try await $0.deleteAllSearchableItems() }
  }

  /// One after the other, so a wipe never lands before an older update.
  private static func run(_ work: @escaping @Sendable (CSSearchableIndex) async throws -> Void) {
    let before = running
    running = Task.detached(priority: .utility) {
      await before?.value
      await Diagnostics.attempt({ try await work(.default()) })
    }
  }
}
