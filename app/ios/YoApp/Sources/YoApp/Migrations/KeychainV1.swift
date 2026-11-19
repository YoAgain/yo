// Remove once every keychain item is v3.
import Foundation
import Security
import YoCore

extension SyncedAccount.Item {
  func upgraded() -> Self? {
    guard accountKey.count == 32 else { return nil }
    if version == Account.backupVersion { return self }
    guard version == nil, let secret = try? legacyAccountSecret(seed: accountKey) else { return nil }
    return Self(username: username, userID: userID, accountKey: secret, byPasskey: false,
      byCode: false, recoveryKey: nil, signedOut: signedOut, version: Account.backupVersion, legacySeed: accountKey)
  }
}

enum KeychainV1 {
  private struct Preferences: Decodable { let accountKey: Data? }

  /// What the migration found: the item to sign in with (and the key the hub's storage is still sealed under), no
  /// key anywhere, or nothing readable now (busy, the keychain not answering): asked again later.
  enum Found {
    case item(SyncedAccount.Item, storageKey: Data?)
    case missing
    case notNow
  }

  /// An old install not signed in again yet: its old state (or its preferences' seed), and no state of today's that
  /// signed in. One saved and not answered yet, or not readable now, is still the update's (`AccountStore.migrate`):
  /// only it finishes it, in the foreground.
  @MainActor
  static func pending(account: Account?, saved: Data?) -> Bool {
    guard account != nil else { return false }
    let localSeed = saved.flatMap { try? JSONDecoder().decode(Preferences.self, from: $0).accountKey }
    return (hasOldState || localSeed?.count == 32) && signedIn != true
  }

  /// The item to sign `account` in again with: the iCloud Keychain's, else one made from the old local state. Nothing
  /// that could not be read now counts as missing.
  @MainActor
  static func signIn(for account: Account, saved: Data?) async -> Found {
    let stored: SyncedAccount.Item?
    do {
      try moveHostItem()
      stored = try SyncedAccount.read(account.userID)
    } catch { return .notNow }
    let (identity, notNow) = await localIdentity(saved: saved)
    let local = identity.flatMap { item(of: $0, for: account) }
    guard var item = stored ?? local else { return notNow ? .notNow : .missing }
    let same = local?.accountKey == item.accountKey
    // The old storage's key: from the item's seed, else from the local state of the same account, read when it can be.
    if item.legacySeed == nil, same { item.legacySeed = local?.legacySeed }
    if item.legacySeed == nil, !same, notNow { return .notNow }
    return .item(item, storageKey: same ? identity?.storageKey : nil)
  }

  private static var places: [(URL, Bool)] {
    var places = [(URL.applicationSupportDirectory, false)]
    if let group = SharedStateFile.container { places.insert((group, true), at: 0) }
    return places
  }

  private static func sources(_ base: URL, shared: Bool) -> [(URL, UInt8)] {
    let directory = base.appending(path: "yo-state-db", directoryHint: .isDirectory)
    return [
      (directory.appending(path: "tables.sqlite"), 3),
      (directory.appending(path: "state.sqlite"), 2),
      (base.appending(path: "yo-state"), shared ? 1 : 0),
    ]
  }

  static var hasOldState: Bool {
    places.contains { base, shared in
      sources(base, shared: shared).contains { FileManager.default.fileExists(atPath: $0.0.path) }
    }
  }

  /// Today's state signed in (its registration answered): true; none, or one saved before the hub answered: false;
  /// not readable now: nil.
  static var signedIn: Bool? {
    guard StateStorage.exists(SharedStateFile.folder ?? StateFile.folder) else { return false }
    return try? (SharedStateFile.app ? SharedStateFile.database() : StateFile.database()).registered()
  }

  /// The identity in the old local state, and whether some of it could not be read now.
  private static func localIdentity(saved: Data?) async -> (LegacyIdentity?, notNow: Bool) {
    await Task.detached { () -> (LegacyIdentity?, notNow: Bool) in
      if let saved, let preferences = try? JSONDecoder().decode(Preferences.self, from: saved),
        let seed = preferences.accountKey, seed.count == 32
      {
        return (LegacyIdentity(secret: seed, ed25519: true, storageKey: nil), false)
      }
      var notNow = false
      for (base, shared) in places {
        let keys: [Data]
        do {
          let key = shared ? try StateKey.shared() : try StateKey.load()
          keys = [key].compactMap { $0 } + (shared ? [] : StateKey.others())
        } catch {
          notNow = true  // the keychain did not answer
          continue
        }
        for (path, format) in sources(base, shared: shared) where FileManager.default.fileExists(atPath: path.path) {
          for key in keys {
            do {
              return (try readLegacyIdentity(path: path.path, stateKey: key, format: format), false)
            } catch YoError.State {
              continue  // not under this key, or not a state
            } catch {
              notNow = true
            }
          }
        }
      }
      return (nil, notNow)
    }.value
  }

  private static func item(of identity: LegacyIdentity, for account: Account) -> SyncedAccount.Item? {
    SyncedAccount.Item(username: account.username, userID: account.userID, accountKey: identity.secret,
      byPasskey: !identity.ed25519 && account.byPasskey, byCode: !identity.ed25519 && account.byCode == true,
      recoveryKey: identity.ed25519 ? nil : RecoveryWords.load(), signedOut: false,
      version: identity.ed25519 ? nil : Account.backupVersion).upgraded()
  }

  static func discardOldState() async {
    await Task.detached {
      for (base, _) in places {
        for name in ["yo-state-db", "yo-state"] {
          try? FileManager.default.removeItem(at: base.appending(path: name))
        }
      }
    }.value
  }

  /// Builds before one item per account kept one per hub (its host as the item's account): moved to the account's own.
  /// Throws when the keychain did not answer or would not take it: the old item stays, to be moved next time.
  static func moveHostItem() throws {
    let old = SyncedAccount.base.merging([kSecAttrAccount as String: SyncedAccount.host]) { $1 }
    guard let data = try Keychain.read(old) else { return }
    if let item = try? JSONDecoder().decode(SyncedAccount.Item.self, from: data), try SyncedAccount.read(item.userID) == nil,
      !SyncedAccount.write(item, previous: nil)
    {
      throw Keychain.Failure(status: errSecIO)
    }
    Keychain.delete(old)
  }

}
