// Remove once every keychain item is v3. The decisions are the core's (`legacy_*`); this is the platform's I/O only.
import Foundation
import Security
import YoCore

enum KeychainV1 {
  /// Cheap: an account with old state (or the old preferences' seed) is a candidate, pending until `readSignedIn`
  /// found today's state signed in. A current install reads nothing of its state here.
  @MainActor
  static func pending(account: Account?, saved: Data?) -> Bool {
    legacyPending(hasAccount: account != nil, saved: saved, hasOldState: hasOldState, signedIn: nil)
  }

  /// `signedIn`, read off the main actor (it opens and decrypts the state); only for a candidate of `pending`.
  static func readSignedIn() async -> Bool? {
    await Task.detached { signedIn }.value
  }

  @MainActor
  static func signIn(for account: Account, saved: Data?) async -> LegacyFound {
    let stored: SyncedItem?
    do {
      try moveHostItem()
      stored = try SyncedAccount.read(account.userID)
    } catch { return .notNow }
    let shown = LegacyShown(
      username: account.username, userId: account.userID, byPasskey: account.byPasskey, byCode: account.byCode)
    let (recoveryKey, left) = (RecoveryWords.load(), SyncedAccount.left)
    return await Task.detached {
      legacySignIn(
        stored: stored, saved: saved, sources: sources(), shown: shown, recoveryKey: recoveryKey, left: left)
    }.value
  }

  private static var places: [(URL, Bool)] {
    var places = [(URL.applicationSupportDirectory, false)]
    if let group = SharedStateFile.container { places.insert((group, true), at: 0) }
    return places
  }

  private static func files(_ base: URL, shared: Bool) -> [(URL, UInt8)] {
    let directory = base.appending(path: "yo-state-db", directoryHint: .isDirectory)
    return [
      (directory.appending(path: "tables.sqlite"), 3),
      (directory.appending(path: "state.sqlite"), 2),
      (base.appending(path: "yo-state"), shared ? 1 : 0),
    ]
  }

  /// Every old state file with the state keys of its place (read from the keychain here).
  private static func sources() -> [LegacySource] {
    places.flatMap { base, shared in
      let keys: [Data]?
      do {
        let key = shared ? try StateKey.shared() : try StateKey.load()
        keys = [key].compactMap { $0 } + (shared ? [] : StateKey.others())
      } catch { keys = nil }
      return files(base, shared: shared).map { path, format in
        LegacySource(path: path.path, format: format, keys: keys ?? [], keysUnavailable: keys == nil)
      }
    }
  }

  static var hasOldState: Bool {
    places.contains { base, shared in
      files(base, shared: shared).contains { FileManager.default.fileExists(atPath: $0.0.path) }
    }
  }

  /// Today's state signed in: true; none, or saved before the hub answered: false; not readable now: nil.
  static var signedIn: Bool? {
    guard StateStorage.exists(SharedStateFile.folder ?? StateFile.folder) else { return false }
    return try? (SharedStateFile.app ? SharedStateFile.database() : StateFile.database()).registered()
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

  /// Older builds kept one item per hub (keyed by host): moved to the account's own. Throws when the keychain did not
  /// take it; the old item stays for next time.
  static func moveHostItem() throws {
    let old = SyncedAccount.base.merging([kSecAttrAccount as String: SyncedAccount.host]) { $1 }
    guard let data = try Keychain.read(old) else { return }
    if let item = syncedItemDecode(json: data), try SyncedAccount.read(item.userId) == nil,
      !SyncedAccount.write(item, previous: nil)
    {
      throw Keychain.Failure(status: errSecIO)
    }
    Keychain.delete(old)
  }
}
