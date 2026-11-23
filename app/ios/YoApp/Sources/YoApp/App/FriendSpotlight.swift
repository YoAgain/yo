// Yo's friends and groups in Spotlight: the names the list shows, nothing else, as Core Spotlight items by target
// (ShareBridge.Friend.to). Tapping one opens Yo on that bar (YoApp's CSSearchableItemActionType). Plain items, not an
// IndexedEntity: the App Intents metadata ship.sh makes (OAD's appintentsmeta.py) does not support IndexedEntity, and
// associateAppEntity takes only one.
// Kept in step with the list ShareInbox publishes: renamed, added, removed and blocked ones (blocked are not in the
// list) change the index; signing out or deleting the account empties it. Core Spotlight's index stays on this
// phone (it is not synced or backed up).
import CoreSpotlight
import UniformTypeIdentifiers

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
    let changed = next.filter { before?[$0.key] != $0.value }
    let gone = before.map { Array(Set($0.keys).subtracting(next.keys)) } ?? []
    run { index in
      if before == nil { try await index.deleteAllSearchableItems() }
      if !gone.isEmpty { try await index.deleteSearchableItems(withIdentifiers: gone) }
      if !changed.isEmpty { try await index.indexSearchableItems(changed.map { item(target: $0.key, title: $0.value) }) }
    }
  }

  private nonisolated static func item(target: String, title: String) -> CSSearchableItem {
    let attributes = CSSearchableItemAttributeSet(contentType: .content)
    attributes.title = title
    attributes.displayName = title
    return CSSearchableItem(uniqueIdentifier: target, domainIdentifier: "friends", attributeSet: attributes)
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
