// Your account (spec 3, 6.4, 7): a username, a user id, an account key, and the backup that brings them back on a
// new phone, opened by a passkey's PRF output or by the recovery key (Signal's: 64 characters).
//
// The account lives at the hub, through the Rust core (`Hub.swift`), which also makes, fetches and opens the backup.
import AuthenticationServices
import CryptoKit
import Foundation
import Observation
import Security
import SwiftUI
import UIKit
import UserNotifications
import YoCore
import os

/// What opening the account did and why it failed, in the device log (Console, `log stream`): the one place the reason
/// is kept once the alert is gone.
let accountLog = Logger(subsystem: "app.justyo", category: "account")

/// The account as the app keeps it beside the core (in its preferences): what it shows and how the backup opens. Its
/// key's secret is not here: the core's sealed state, the backup and the iCloud Keychain hold it (`client.accountSecret`).
struct Account: Codable, Equatable {
  var username: String
  var userID: Data
  /// How the backup opens: by the passkey (and the recovery key), or by the recovery key only.
  var byPasskey: Bool
  /// The recovery key opens the backup (nil in accounts saved before this was kept: see `AccountStore.init`).
  var byCode: Bool? = false
  /// The backup's fetch may be signed by the recovery key's auth key (spec 7): nil in an account whose record is from
  /// before the auth keys, until the backup is made anew (`addAuthKeys`).
  var codeAuth: Bool? = nil
  /// And by the passkey's: false while the record made anew has no passkey door yet (`confirmPasskey`).
  var passkeyAuth: Bool? = nil
  /// The backup's kind: 3, opened by the recovery key (libsignal's account entropy pool). Nil: an account of the
  /// 12 words, whose backup no longer opens (spec 7): not secured until made anew (`AccountStore.init`).
  var backupVersion: Int? = Account.backupVersion

  static let backupVersion = 3

  /// Backed up (spec 7): the account comes back on a new phone by a recovery method, not only by iCloud Keychain.
  var secured: Bool { byPasskey || byCode == true }
}

/// Usernames (spec 3.3): Latin letters (accents and umlauts too), digits, `_` and emoji; kept and shown as their owner
/// writes them ("YoGPT"). Two names that look alike are one name: they are compared by their skeleton, so "yogpt" and
/// "Y0GPT" are "YoGPT". The rules are the core's (one copy for app, core and hub); which names are reserved only the
/// hub knows: to the app they are taken.
enum Username {
  /// What is wrong with a name, nil when it is fine.
  static func problem(_ name: String) -> String? {
    if name.isEmpty { return "Type a name." }
    if name.count > 16 { return "Use no more than 16 characters." }
    if !usernameValid(name: name) { return "Start with a letter or emoji." }
    return nil
  }

  /// What may be typed: the core's own rule (`username_canonical`), so the field keeps exactly what the hub takes, and
  /// only "too short" and "taken" are left to show.
  static func clean(_ typed: String) -> String { usernameCanonical(typed: typed) }

  /// What a name is compared by (spec 3.3), as the core and the hub compare it: LEO, LE0 and LÉO are one name.
  static func skeleton(_ name: String) -> String { usernameSkeleton(name: name) }
}

/// The recovery key (spec 7): Signal's, libsignal's account entropy pool, 64 characters a–z and 0–9; the core makes
/// and reads it. Shown in 16 blocks of 4.
enum RecoveryCode {
  static func make() -> String { YoCore.newRecoveryKey() }

  /// The recovery key in whatever was pasted, typed or scanned: case, spaces, dashes and line breaks do not matter;
  /// a heading around it ("Yo recovery key for LEO: …") neither (the last 64 characters that make one count).
  static func parse(_ typed: String) -> String? {
    if let key = recoveryKeyParse(typed: typed) { return key }
    let kept = typed.lowercased().filter { ($0.isASCII && ($0.isLetter || $0.isNumber)) }
    guard kept.count >= 64 else { return nil }
    return recoveryKeyParse(typed: String(kept.suffix(64)))
  }

  /// The key in blocks of 4, as shown and printed.
  static func blocks(_ key: String) -> [String] {
    stride(from: 0, to: key.count, by: 4).map { start in
      let from = key.index(key.startIndex, offsetBy: start)
      return String(key[from..<(key.index(from, offsetBy: 4, limitedBy: key.endIndex) ?? key.endIndex)])
    }
  }

  /// The key as text to share: its blocks, space-separated.
  static func shown(_ key: String) -> String { blocks(key).joined(separator: " ") }
}

/// The backup (spec 7): the account under a random key K, and K wrapped by the passkey and by the code. The record
/// itself (`YoBackup`) lives at the hub; the core makes, signs for and opens it (`openBackup`, `putBackup`), and this
/// is what K seals.
enum BackupRecord {
  /// The salt every Yo passkey is asked with: one for all, so a new phone asks once, before it knows who it is.
  static let prfSalt = Data(SHA256.hash(data: Data("yo backup v1".utf8)))

  /// A new K: 32 random bytes.
  static func newKey() -> Data { SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) } }

  /// What K seals: the account and its key's secret (libsignal's identity key; signs the account's devices, spec
  /// 3.1), and the recovery key, so a phone restored by passkey has it to show too. The secret sits in the account's
  /// JSON, as in the backups made while `Account` still held it.
  struct Sealed: Codable {
    var account: Account
    var accountKey: Data
    var code: String?

    init(account: Account, accountKey: Data, code: String?) {
      self.account = account
      self.accountKey = accountKey
      self.code = code
    }

    private enum CodingKeys: String, CodingKey { case account, code }
    private enum SecretKeys: String, CodingKey { case accountKey }

    init(from decoder: Decoder) throws {
      let c = try decoder.container(keyedBy: CodingKeys.self)
      account = try c.decode(Account.self, forKey: .account)
      accountKey = try c.nestedContainer(keyedBy: SecretKeys.self, forKey: .account).decode(Data.self, forKey: .accountKey)
      code = try c.decodeIfPresent(String.self, forKey: .code)
    }

    func encode(to encoder: Encoder) throws {
      var c = encoder.container(keyedBy: CodingKeys.self)
      let inner = c.superEncoder(forKey: .account)
      try account.encode(to: inner)
      var secret = inner.container(keyedBy: SecretKeys.self)
      try secret.encode(accountKey, forKey: .accountKey)
      try c.encodeIfPresent(code, forKey: .code)
    }
  }
}

@MainActor @Observable
final class AccountStore {
  var supportUnread: UInt32 = 0 { didSet { supportRevision &+= 1 } }
  private var supportRevision: UInt64 = 0
  func refreshSupport() async {
    guard let client else { supportUnread = 0; return }
    let revision = supportRevision
    do {
      let thread = try await Hub.run { try client.supportThread() }
      guard self.client === client, revision == supportRevision else { return }
      supportUnread = thread.unread
    } catch { /* Keep the last known count while offline. */ }
  }

  private(set) var account: Account? {
    didSet {
      UserDefaults.standard.set(try? JSONEncoder().encode(account), forKey: Self.key)
      // Kept in the iCloud Keychain as it changes (not removed here: an account taken to another phone stays there).
      if let account, let secret { SyncedAccount.save(account, secret: secret) }
    }
  }

  /// The core, opened from its saved state at launch or made at registration: talks to the hub for this device.
  private(set) var client: YoClient?
  /// The account key's secret, from the core with `client` (in memory only): for the iCloud Keychain's item.
  private var secret: Data?

  /// Read back the synchronizable item, rather than assuming a Keychain write succeeded.
  /// iOS does not expose whether this item has reached another device.
  var keychainRecoveryActive: Bool {
    guard let account, let secret, !SyncedAccount.needsSave(account.userID),
      let item = SyncedAccount.load(account.userID) else { return false }
    return item.accountKey == secret && item.signedOut != true
  }

  /// `client` in use from now on, with its account key's secret (read off the main actor: the core's lock may be held).
  private func use(_ client: YoClient, secret: Data) {
    self.client = client
    self.secret = secret
  }

  /// The recovery words were saved somewhere by the person (through the share sheet), not only made. Accounts from
  /// before this was kept count as saved.
  private(set) var wordsSaved: Bool = UserDefaults.standard.object(forKey: "recovery.saved") as? Bool ?? true {
    didSet { UserDefaults.standard.set(wordsSaved, forKey: "recovery.saved") }
  }

  func markWordsSaved(_ saved: Bool) { wordsSaved = saved }

  /// Without a passkey on purpose (skipped in the guide, or turned off).
  func declinePasskey() { passkeyTurnedOff = true }

  /// The passkey was skipped or turned off on purpose: a choice, so it is not asked for again (no badge, no guide).
  private(set) var passkeyTurnedOff: Bool = UserDefaults.standard.bool(forKey: "passkey.turnedOff") {
    didSet { UserDefaults.standard.set(passkeyTurnedOff, forKey: "passkey.turnedOff") }
  }

  /// What is left to do, for the badges: the passkey, the words saved; 0 once both are done.
  var toDo: Int {
    guard let account else { return 0 }
    let passkeyLeft = !account.byPasskey && !passkeyTurnedOff
    return passkeyLeft || !account.secured || !wordsSaved ? 1 : 0
  }

  private static let key = "account"

  /// What the password manager shows for the passkey: the name, and "(Dev)" for Yo Dev's (its accounts are others).
  private static func passkeyName(_ account: Account) -> String {
    Hub.isDevApp ? "\(account.username) (Dev)" : account.username
  }
  private let passkeys = Passkeys()
  /// Securing under way: K and the recovery key, made once per try.
  private var securing: (k: Data, code: String)?
  /// The guide's passkey once made, before it is signed in with.
  private var newPasskey: Data?
  /// A registration under way (`register(_:secret:)`).
  private var registering = false
  /// Signed in from a backup or the iCloud Keychain, and the account's storage not seen yet: its app icon is this
  /// phone's to take (`takeStorage`).
  private var restored = false
  /// Writes to the storage, one after the other in the order made (`Hub.run` runs calls side by side).
  private var storing: Task<Void, Never>?
  /// The shared state of an account signed out of, being deleted (`reset`): a registration waits for it.
  private var deleting: Task<Void, Never>?
  /// This phone's friends from before the storage went up to it once (`takeStorage`); a new or restored account has
  /// none to bring.
  private var friendsSeeded: Bool = UserDefaults.standard.object(forKey: "storage.seeded") as? Bool ?? false {
    didSet { UserDefaults.standard.set(friendsSeeded, forKey: "storage.seeded") }
  }

  /// The account's storage read into the list since the core opened (`takeStorage`): until then a list without
  /// friends may only not have them yet (the add row keeps Group and Invite closed).
  private(set) var storageRead = false

  init() {
    let saved = UserDefaults.standard.data(forKey: Self.key)
    account = saved.flatMap { try? JSONDecoder().decode(Account.self, from: $0) }
    if var old = account, old.backupVersion != Account.backupVersion {
      // A backup of the 12 words no longer opens (spec 7): not secured, as a new account, its words and backup
      // secrets forgotten; the passkey's door goes with it. Its iCloud Keychain item is written anew at the opening.
      old.byPasskey = false
      old.byCode = false
      old.codeAuth = nil
      old.passkeyAuth = nil
      old.backupVersion = Account.backupVersion
      RecoveryWords.delete()
      SyncedAccount.mustSave(old.userID)
      account = old
      // Observers do not run in `init`: written here.
      UserDefaults.standard.set(try? JSONEncoder().encode(old), forKey: Self.key)
      wordsSaved = false
      UserDefaults.standard.set(false, forKey: "recovery.saved")
      passkeyTurnedOff = false
      UserDefaults.standard.set(false, forKey: "passkey.turnedOff")
    }
    if let account, let saved {
      // Builds before kept the account key's secret here too, in the preferences (and so in the phone's backups):
      // into the iCloud Keychain if the account has no item there yet, then written again without it.
      if let old = try? JSONDecoder().decode(LegacySecret.self, from: saved).accountKey,
        let secret = try? legacyAccountSecret(seed: old)
      {
        SyncedAccount.save(account, secret: secret, onlyIfMissing: true)
        UserDefaults.standard.set(try? JSONEncoder().encode(account), forKey: Self.key)
      }
      // The core opens as the first scene connects (or once Yo is in front), and in a launch in the background only
      // when something asks for it (`openNow`: a silent push's sync, a notification's Yo Back): the Action button's
      // Yo opens the state itself (YoSend), and would wait for ours.
      Launch.whenScene { [weak self] in self?.openNow() }
      Launch.whenActive { [weak self] in self?.openNow() }
    }
  }

  /// The core, opening now if it has not (a launch in the background opens it only when something needs it).
  func openNow() {
    guard client == nil, account != nil, !opening else { return }
    Task { await open() }
  }

  /// Read-only probes retry refused turns. Reopen from the same sealed file and key after repeated failures.
  func watchState() async {
    var failures = 0
    while !Task.isCancelled {
      if let client {
        do {
          _ = try await Hub.run { try client.groups() }
          if client === self.client {
            failures = 0
            Availability.shared.local = false
          }
        } catch {
          if client === self.client, !Task.isCancelled {
            Availability.shared.local = true
            failures += 1
            if failures >= 3, !opening {
              failures = 0
              let userID = account?.userID
              opening = true
              self.client = nil
              do {
                try await openOnce()
              } catch {
                // A soft restart never resets the account or replaces its sealed state.
                if account?.userID == userID, account != nil, self.client == nil { self.client = client }
              }
              opening = false
            }
          }
        }
      } else {
        Availability.shared.local = account != nil
      }
      do { try await Task.sleep(for: .seconds(2)) } catch { return }
    }
  }

  /// The secret as builds before kept it in the account's JSON.
  private struct LegacySecret: Decodable { var accountKey: Data? }

  /// The core from the state it saved last, tried again while it cannot be read yet (the keychain or the file before
  /// the first unlock, a passing error, a save that failed, a registration to finish without the hub), a little later
  /// each time. A state that is gone or does not open: `broken`, and the person signs in again (`recover`). A device
  /// replaced since (the account restored on another phone): out.
  private func open() async {
    guard !opening else { return }
    opening = true
    defer { opening = false }
    var pause = 1.0
    var tries = 0
    var unavailableSince: ContinuousClock.Instant?
    var unavailableTries = 0
    while client == nil, account != nil, !broken {
      do {
        try await openOnce()
        Availability.shared.local = false
        accountLog.notice("opened")
        return
      } catch YoError.Replaced {
        accountLog.error("open: replaced")
        return reset()
      } catch YoError.State where account.flatMap({ SyncedAccount.load($0.userID) }) != nil {
        accountLog.error("open: state does not open, signing in again from the iCloud Keychain")
        // A state that does not open, an older build's among them (before sessions, spec 4): the account comes
        // back from the iCloud Keychain as a new device of it, with new prekeys; contacts write to it again.
        return await recover()
      } catch let error as YoError where !Self.notNow(error) {
        brokenWhy = "\(error)"
        accountLog.error("open: \(self.brokenWhy, privacy: .public) (try \(tries + 1))")
        if await Self.surely(&tries) { broken = true }  // does not open (sealed under another key, or damaged)
      } catch Unopened.gone {
        brokenWhy = await Task.detached { Self.missing() }.value
        accountLog.error("open: \(self.brokenWhy, privacy: .public) (try \(tries + 1))")
        if await Self.surely(&tries) { broken = true }
      } catch {
        Availability.shared.local = true
        if UIApplication.shared.isProtectedDataAvailable {
          unavailableTries += 1
          let now = ContinuousClock.now
          if unavailableSince == nil { unavailableSince = now }
          if unavailableTries >= 3, let since = unavailableSince, since.duration(to: now) >= .seconds(30) {
            brokenWhy = "shared storage unavailable for 30 seconds"
            broken = true
            accountLog.error("open: \(self.brokenWhy, privacy: .public); recovering")
            return await recover()
          }
        } else {
          unavailableSince = nil
          unavailableTries = 0
        }
        accountLog.notice("open: not now: \(String(describing: error), privacy: .public)")
        try? await Task.sleep(for: .seconds(pause))  // not now
        pause = min(pause * 2, 30)
      }
    }
  }

  /// An error opening a state that is there and reads: not saved (a full disk), or a registration to finish that did
  /// not reach the hub now. Never a reason to sign in again, which would replace this device.
  nonisolated private static func notNow(_ error: YoError) -> Bool { retryLater(error: error) }  // the core's one rule

  /// A state that did not open is taken as broken only on the third try in a row, a few seconds apart: a file or
  /// keychain the extensions hold for a moment (a burst of pushes after the hub came back) is no reason to sign in again.
  private static func surely(_ tries: inout Int) async -> Bool {
    tries += 1
    guard tries >= 3 else {
      try? await Task.sleep(for: .seconds(2))
      return false
    }
    return true
  }

  /// Whether the hub answers now (its key, which every hub serves): "Sign In Again" replaces this phone's device at
  /// the hub, so with the hub down it waits instead of signing out.
  static func hubAnswers() async -> Bool {
    var request = URLRequest(url: Hub.url.appending(path: ".well-known/yo-hub-key"), timeoutInterval: 8)
    request.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
    guard let (_, response) = await Diagnostics.attempt({ try await URLSession(configuration: .ephemeral).data(for: request) }) else { return false }
    return (response as? HTTPURLResponse)?.statusCode == 200
  }

  /// Why the state did not open, as the alert says it small (to tell a damaged state from a missing file or key).
  private(set) var brokenWhy = ""

  /// What of the state is not there: the file, the key, or neither (the shared one's).
  nonisolated private static func missing() -> String {
    let file = SharedStateFile.app ? "shared" : (StateFile.load() == nil ? "no file" : "file")
    let key = SharedStateFile.app ? ((try? StateKey.shared()) == nil ? "no key" : "key") : ((try? StateKey.load()) == nil ? "no key" : "key")
    return "gone: \(file), \(key)"
  }

  /// "Sign In Again" found the hub down: the alert says so and offers to try again, the account stays.
  private(set) var hubDown = false

  /// Opening under way (`open`).
  private var opening = false
  /// The saved state is gone or does not open: this phone cannot use the account until it signs in again.
  private(set) var broken = false

  /// Why the state did not open: not readable yet (before the first unlock), or not there at all.
  private enum Unopened: Error { case notNow, gone }

  private func openOnce() async throws {
    let userID = account?.userID
    let transport = Hub.transport
    let (client, secret) = try await Hub.run { () throws -> (YoClient, Data) in
      try Perf.timed("launch.core.open") { () throws -> (YoClient, Data) in
        if SharedStateFile.app {
          // Shared with the extensions: opened under the lock, then each step takes it (SharedState.swift).
          let file = SharedStateFile(guest: false)
          let client = try SharedStateFile.locked { () throws -> YoClient in
            guard let key = try StateKey.shared() else { throw Unopened.gone }
            guard let blob = file.read() else { throw Self.unread(SharedStateFile.stateURL) }
            return try YoClient.restore(transport: transport, store: file, stateKey: key, blob: blob)
          }
          Hub.pin(client)
          client.shareState(lock: file)
          client.acknowledgeExplicitly()  // each batch as the app has stored it (Inbox.take)
          return (client, client.accountSecret())
        }
        guard let blob = StateFile.load() else { throw Self.unread(StateFile.url) }
        let own = try StateKey.load()
        let client: YoClient
        do {
          guard let own else { throw Unopened.gone }
          client = try YoClient.restore(transport: transport, store: StateFile(), stateKey: own, blob: blob)
        } catch let error where error is Unopened || (error as? YoError).map({ if case .State = $0 { true } else { false } }) == true {
          // Yo Dev: sealed under the key a build before took from Yo of TestFlight (`StateKey.others`)? Taken over then.
          guard let (key, opened) = StateKey.others().lazy.compactMap({ key in
            (Diagnostics.attempt({ try YoClient.restore(transport: transport, store: StateFile(), stateKey: key, blob: blob) })).map { (key, $0) }
          }).first else { throw error }
          try StateKey.adopt(key)
          accountLog.notice("open: the state opened with a key of another group, now this app's own")
          client = opened
        }
        Hub.pin(client)
        client.acknowledgeExplicitly()  // each batch as the app has stored it (Inbox.take)
        return (client, client.accountSecret())
      }
    }
    guard self.client == nil, account != nil, account?.userID == userID else { return }  // account changed meanwhile
    use(client, secret: secret)
    Perf.mark("launch.client")
    // The user id as the core has it (an earlier build read it wrongly): the passkey's handle and the backup's id.
    if let id = Hub.bytes(client.userId()), account?.userID != id { account?.userID = id }
    // Into the iCloud Keychain if not there (accounts from before it, one signed out on another phone of the
    // person's). Only then: opening asks nothing of the hub, so this phone may have been replaced meanwhile, and the
    // item of the phone that has the account now must not be overwritten by an older one.
    // One whose last change the keychain did not take is written again, whole.
    if let account {
      Perf.timed("launch.keychain.synced") {
        SyncedAccount.save(account, secret: secret, onlyIfMissing: !SyncedAccount.needsSave(account.userID))
      }
    }
    Task { await addAuthKeys() }
  }

  /// A state file that did not read: there but locked (not now), or not there.
  nonisolated private static func unread(_ url: URL?) -> Unopened {
    guard let url, FileManager.default.fileExists(atPath: url.path) else { return .gone }
    return .notNow
  }

  /// "Sign In Again" for a state that does not open (`broken`): this phone forgets it, and registers again with the
  /// account's key from the iCloud Keychain if it is there; else the way in (passkey, recovery key) brings it back.
  func recover() async {
    let item = account.flatMap { SyncedAccount.load($0.userID) }
    // With the hub unreachable a new sign-in cannot finish: nothing is forgotten, the alert says why.
    guard await Self.hubAnswers() else {
      hubDown = true
      return
    }
    hubDown = false
    accountLog.error("recover: \(item == nil ? "no iCloud Keychain item" : "registering again", privacy: .public)")
    Self.keepUnopened()
    reset()
    if let item { _ = try? await restore(synced: item) }
  }

  /// The state that did not open, kept aside beside it (never in the phone's backup) before signing in again deletes
  /// it: the only copy of what this device had, for finding out why, or opening it later.
  private static func keepUnopened() {
    let stamp = Int(Date.now.timeIntervalSince1970)
    for url in [StateFile.url, SharedStateFile.stateURL].compactMap({ $0 })
    where FileManager.default.fileExists(atPath: url.path) {
      let aside = url.deletingLastPathComponent().appending(path: url.lastPathComponent + ".unopened-\(stamp)")
      do {
        try FileManager.default.copyItem(at: url, to: aside)
        SharedStateFile.excludeFromBackup(aside)
        accountLog.notice("recover: the state kept as \(aside.lastPathComponent, privacy: .public)")
      } catch {
        accountLog.error("recover: the state not kept: \(String(describing: error), privacy: .public)")
      }
    }
  }

  /// Registers `username` at the hub (spec 6.4), or, with `secret`, takes the key's account back (a new phone), with
  /// the name it has now: `username` then only counts if the key's account is gone. One at a time, whichever way in (a
  /// second registration would replace the first).
  private func register(_ username: String, secret: Data?, legacySeed: Data? = nil) async throws -> (
    client: YoClient, secret: Data
  ) {
    // One at a time, and never over an account signed in meanwhile (another way in finished first).
    guard !registering, account == nil else { throw Diagnostics.failure(AccountError.busy) }
    registering = true
    defer { registering = false }
    await deleting?.value  // the state signed out of goes first, never this one
    let transport = Hub.transport
    do {
      return try await Hub.run {
        let make = { (store: StateStore, key: Data) in
          let client = try YoClient.registerUsername(
            transport: transport, store: store, stateKey: key, username: username, accountSecret: secret,
            legacySeed: legacySeed, name: nil, pushPlatform: 0, pushToken: "")
          Hub.pin(client)
          return client
        }
        guard SharedStateFile.app else {
          let client = try make(StateFile(), StateKey.make())
          client.acknowledgeExplicitly()  // each batch as the app has stored it (Inbox.take)
          return (client, client.accountSecret())
        }
        let file = SharedStateFile(guest: false)
        let client = try SharedStateFile.locked {
          let key = try StateKey.makeShared()
          _ = file.read()  // the state there now (none, or a signed-out one's): this one's to replace
          return try make(file, key)
        }
        client.shareState(lock: file)
        client.acknowledgeExplicitly()  // each batch as the app has stored it (Inbox.take)
        return (client, client.accountSecret())
      }
    } catch YoError.Refused(let status) where status == 409 {
      throw AccountError.taken
    } catch YoError.Refused(let status) where status == 429 {
      throw AccountError.tooMany
    } catch let error as YoError {
      throw AccountError.hub(error)
    } catch is Keychain.Failure {
      throw AccountError.keychain
    }
  }

  /// A new account (spec 6.4): the name is yours at once, the backup comes later (`createPasskey`, `keepWords`). Its
  /// key is made here and kept until the hub answered (`NewAccountKey`): trying again after a lost answer keeps the
  /// name.
  func register(_ username: String) async throws {
    let client: YoClient
    let secret: Data
    do {
      (client, secret) = try await register(username, secret: NewAccountKey.secret(for: username))
    } catch is Keychain.Failure {
      throw AccountError.keychain
    } catch AccountError.taken {
      NewAccountKey.forget(username)
      throw AccountError.taken
    }
    guard let id = Hub.bytes(client.userId()) else { throw Diagnostics.failure(AccountError.noBackup) }
    use(client, secret: secret)
    friendsSeeded = true
    SyncedAccount.left = nil
    account = Account(username: client.username() ?? username, userID: id, byPasskey: false)
    // Only now that the account is kept (its preference and keychain item): a kill before this keeps the key, and the
    // next try registers the same account again rather than leaving its state where no launch opens it.
    NewAccountKey.forget(username)
    wordsSaved = false
    passkeyTurnedOff = false
  }

  // MARK: Securing the name (spec 7)

  /// The guide's passkey, in two steps the person sees: made (saved in the password manager)…
  func createPasskey() async throws {
    guard let account, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    if !account.secured, securing == nil { securing = (BackupRecord.newKey(), RecoveryCode.make()) }
    let made = try await passkeys.make(name: Self.passkeyName(account), userID: account.userID).credentialID
    try still(client)
    newPasskey = made
  }

  /// A change of the backup (fetched, rewrapped, put) once the one before has ended: two at once would each put a
  /// record over the other's (the start's `addAuthKeys` over a passkey just confirmed). `work` decides on the account
  /// as it is then.
  private func backupTurn<T>(_ work: @MainActor () async throws -> T) async rethrows -> T {
    while backupBusy { await withCheckedContinuation { backupWaiting.append($0) } }
    backupBusy = true
    defer {
      backupBusy = false
      if !backupWaiting.isEmpty { backupWaiting.removeFirst().resume() }
    }
    return try await work()
  }

  /// A backup change under way (`backupTurn`), and those waiting for it.
  private var backupBusy = false
  private var backupWaiting: [CheckedContinuation<Void, Never>] = []

  /// After waiting for the person or the hub: still the account the step began with (not signed out, or out and in
  /// again meanwhile), else the step ends here.
  private func still(_ client: YoClient) throws {
    guard client === self.client else { throw CancellationError() }
  }

  /// …then signed in with once, which checks it works and gives its PRF output: the backup then opens with it too
  /// (a new backup if there was none, else added to the one the words open).
  func checkPasskey() async throws {
    guard let credential = newPasskey, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    guard let prf = try await passkeys.ask(salt: BackupRecord.prfSalt, only: credential).prf else {
      throw Diagnostics.failure(AccountError.noPRF)
    }
    try await backupTurn {
      try still(client)
      if account?.secured == true {
        try await rewrap(.set(prf))
      } else {
        try await saveBackup(prf: prf)
      }
    }
    newPasskey = nil
  }

  /// The recovery key, to show when the passkey did not work.
  func secureWords() -> String {
    if securing == nil { securing = (BackupRecord.newKey(), RecoveryCode.make()) }
    return securing?.code ?? ""
  }

  /// The recovery key kept: the backup opens with it.
  func keepWords() async throws { try await backupTurn { try await saveBackup(prf: nil) } }

  /// The first backup: K under the words always (passkey and words are equal doors), under the passkey's PRF too;
  /// both doors' auth keys.
  private func saveBackup(prf: Data?) async throws {
    guard var account, let securing, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    account.byPasskey = prf != nil
    account.byCode = true
    account.codeAuth = true
    account.passkeyAuth = prf != nil
    try await put(account, code: securing.code, k: securing.k, passkey: prf.map { .new(prf: $0) } ?? .none, by: client)
    self.account = account
    self.securing = nil
    if prf != nil { passkeyTurnedOff = false }
  }

  /// The backup to the hub through `client`, the one the change began with: never another account's after a sign-out
  /// and sign-in meanwhile. The core seals the account and the words under K and wraps K (spec 7). New words are
  /// staged on this phone first and settled once the hub has them: a failure on the way leaves the words the backup
  /// opens with here, and the caller marks nothing done.
  private func put(_ account: Account, code: String, k: Data, passkey: YoPasskeyDoor, by client: YoClient)
    async throws
  {
    try still(client)
    do {
      if code != RecoveryWords.load() { try RecoveryWords.stage(code) }
    } catch {
      throw AccountError.keychain
    }
    do {
      try await Hub.run {
        let sealed = try JSONEncoder().encode(
          BackupRecord.Sealed(account: account, accountKey: client.accountSecret(), code: code))
        try client.putBackup(k: k, sealed: sealed, recoveryKey: code, passkey: passkey)
      }
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
    try still(client)
    do {
      try RecoveryWords.settle(code)
    } catch {
      throw AccountError.keychain
    }
  }

  /// A core call for this account, off the main actor; the core's error as an `AccountError`.
  private func hub<T: Sendable>(_ work: @escaping @Sendable (YoClient) throws -> T) async throws -> T {
    guard let client else { throw Diagnostics.failure(AccountError.noBackup) }
    do {
      return try await Hub.run { try work(client) }
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
  }

  /// What a rewrap does with the passkey's door: keeps it as it is, sets it from a passkey's PRF output, or removes it.
  private enum Door { case keep, set(Data), remove }

  /// The backup again with the same K (opened by the words on this phone): its passkey door kept, set or removed, and
  /// optionally under new words (the old locator then finds nothing). Always with the doors' auth keys.
  private func rewrap(_ passkey: Door, code new: String? = nil) async throws {
    guard var account, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    let opened: YoOpenedBackup
    let code: String
    do {
      (opened, code) = try await ownBackup(account, by: client)
    } catch AccountError.noBackup {
      try still(client)
      guard let words = new ?? RecoveryWords.load() else { throw Diagnostics.failure(AccountError.noBackup) }
      let prf: Data? = if case .set(let value) = passkey { value } else { nil }
      let removing = if case .remove = passkey { true } else { false }
      try await makeAnew(words: words, prf: prf, removingPasskey: removing)
      if case .set = passkey { passkeyTurnedOff = false }
      if case .remove = passkey { passkeyTurnedOff = true }
      return
    }
    let record = opened.record
    let words = new ?? code
    let door: YoPasskeyDoor =
      switch passkey {
      case .keep: .keep(record: record)
      case .set(let prf): .new(prf: prf)
      case .remove: .none
      }
    switch passkey {
    case .keep:
      account.byPasskey = !record.byPasskey.isEmpty
      account.passkeyAuth = !record.authPasskey.isEmpty
    case .set:
      account.byPasskey = true
      account.passkeyAuth = true
    case .remove:
      account.byPasskey = false
      account.passkeyAuth = false
    }
    account.byCode = true
    account.codeAuth = true
    try await put(account, code: words, k: opened.k, passkey: door, by: client)
    self.account = account
    if case .set = passkey { passkeyTurnedOff = false }
    if case .remove = passkey { passkeyTurnedOff = true }
  }

  /// The backup, by the words on this phone: new ones staged and not settled first (the hub may have taken them and its
  /// answer been lost), else the ones there; whichever open it are the words from then on.
  /// Signed out (or out and in again) during a fetch: the words on this phone are the next account's, left alone.
  private func ownBackup(_ account: Account, by client: YoClient) async throws -> (YoOpenedBackup, String) {
    var tries = [RecoveryWords.pending(), RecoveryWords.load()].compactMap { $0 }
    if tries.count == 2 && tries[0] == tries[1] { tries.removeLast() }
    for (index, words) in tries.enumerated() {
      do {
        let opened = try await fetchBackup(by: .recoveryKey(key: words), userID: account.userID)
        try still(client)
        Diagnostics.attempt({ try RecoveryWords.settle(words) })
        return (opened, words)
      } catch AccountError.noBackup where index < tries.count - 1 {
        try still(client)
        continue  // not these: the others
      }
    }
    throw Diagnostics.failure(AccountError.noBackup)
  }

  /// An account whose record may be from before the doors' auth keys (spec 7), which the hub hands out to no one: once,
  /// with no step for the person, by the words on this phone. A record the words fetch (one made since, the account
  /// back from the iCloud Keychain) only tells how it opens; else the backup is made anew, the passkey's door left for
  /// the passkey (`confirmPasskey`). Tried again at the next start until it went.
  private func addAuthKeys() async {
    await backupTurn {
      // As the account is now: a change made meanwhile (a passkey confirmed) may have done it already.
      guard let account, account.secured, account.codeAuth != true, let words = RecoveryWords.load(),
        let client
      else { return }
      do {
        try await rewrap(.keep)
      } catch AccountError.noBackup {
        // Still the account the words are of: signed out (or into another) meanwhile, nothing is made.
        guard client === self.client else { return }
        try? await makeAnew(words: words, prf: nil)
      } catch {}
    }
  }

  /// A new K needs fresh wraps; keep the passkey available for replacement unless explicitly removed.
  private func makeAnew(words: String, prf: Data?, removingPasskey: Bool = false) async throws {
    guard var account, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    account.byCode = true
    account.codeAuth = true
    account.passkeyAuth = prf != nil
    if removingPasskey { account.byPasskey = false }
    if prf != nil { account.byPasskey = true }
    let passkey: YoPasskeyDoor = prf.map { .new(prf: $0) } ?? .none
    try await put(account, code: words, k: BackupRecord.newKey(), passkey: passkey, by: client)
    self.account = account
  }

  /// A new passkey in place of the one there (made, then used once for its PRF output).
  func replacePasskey() async throws {
    guard let account, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    let made = try await passkeys.make(name: Self.passkeyName(account), userID: account.userID)
    guard let prf = try await passkeys.ask(salt: BackupRecord.prfSalt, only: made.credentialID).prf else {
      throw Diagnostics.failure(AccountError.noPRF)
    }
    try await backupTurn {
      try still(client)
      try await rewrap(.set(prf))
    }
  }

  /// The passkey no longer opens the backup; the words still do. (The passkey itself is deleted in the password
  /// manager.)
  func removePasskey() async throws { try await backupTurn { try await rewrap(.remove) } }

  /// New recovery words; the old ones stop working (the hub forgets their locator).
  func newRecoveryKey() async throws -> String {
    let code = RecoveryCode.make()
    try await backupTurn { try await rewrap(.keep, code: code) }
    wordsSaved = false
    return code
  }

  // MARK: A new phone

  /// Any Yo passkey of this person, asked once; its user handle says whose backup to open.
  func restoreWithPasskey(quietly: Bool = false) async throws {
    let answer = try await passkeys.ask(salt: BackupRecord.prfSalt, quietly: quietly)
    guard let prf = answer.prf else { throw Diagnostics.failure(AccountError.noPRF) }
    try await restore(fetchBackup(by: .passkey(prf: prf), userID: answer.userID))
  }

  /// By the recovery key alone: it says where the backup is and opens it.
  func restore(code: String) async throws {
    try await restore(fetchBackup(by: .recoveryKey(key: code), userID: nil), typed: code)
    wordsSaved = RecoveryWords.load() != nil  // typed in: the person has them (once they are on this phone too)
  }

  /// The backup a door opens, fetched with its auth key (the core derives, signs and opens): by the user id when
  /// given (a passkey's user handle), else by the words' locator. None it opens: `noBackup` (the hub tells no more).
  private func fetchBackup(by door: YoDoor, userID: Data?) async throws -> YoOpenedBackup {
    let transport = Hub.transport
    do {
      guard let opened = try await Hub.run({ try openBackup(transport: transport, userId: userID, door: door) }) else {
        throw Diagnostics.failure(AccountError.noBackup)
      }
      return opened
    } catch YoError.Refused(let status) where status == 429 {
      throw AccountError.tooMany
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
  }

  /// The account from its backup, and this phone registered for it with the account's key (spec 6.4): the hub finds
  /// the account by the key, not by the name the backup has (changed since, or taken by someone else).
  /// `typed`: the words that opened it, kept when the backup does not hold them (one made before it did).
  private func restore(_ opened: YoOpenedBackup, typed: String? = nil) async throws {
    let sealed = try JSONDecoder().decode(BackupRecord.Sealed.self, from: opened.sealed)
    let (client, secret) = try await register(sealed.account.username, secret: sealed.accountKey)
    use(client, secret: secret)
    // The name as the hub answered it (changed since the backup was made), the user id as registered. Another user id
    // (the account deleted meanwhile, spec 6.4, and taken again with the same key) has no backup yet.
    var account = sealed.account
    account.username = client.username() ?? account.username
    let id = Hub.bytes(client.userId()) ?? account.userID
    let same = id == account.userID
    if !same { account = Account(username: account.username, userID: id, byPasskey: false) }
    // Else none to show: the backup still opens.
    if same, let code = sealed.code ?? typed { Diagnostics.attempt({ try RecoveryWords.save(code) }) }
    SyncedAccount.left = nil
    restored = same
    friendsSeeded = true
    tookPasskeyChoice(of: account)
    self.account = account
    Task { await addAuthKeys() }
  }

  /// An account back on this phone secured by its words alone: its passkey was declined (the guide asks for it
  /// first), on the phone it came from. Without this the new phone said "Not Secured" and asked to secure an account
  /// that is, as the old phone did not.
  private func tookPasskeyChoice(of account: Account) {
    passkeyTurnedOff = account.secured && !account.byPasskey
  }

  /// Back without a word, from the iCloud Keychain (`SyncedAccount`): the app deleted and installed again, or a new
  /// iPhone. Registers this phone with the account's key (spec 6.4), so the phone it was on before is signed out, as
  /// with a backup. Only when exactly one account is there not signed out, and not one this phone left (taken to
  /// another phone); several are listed on Sign In instead. True when back in.
  /// `fresh`: the app's first start after installing it — a new start, so an earlier Sign Out does not hold it back.
  func restoreFromKeychain(fresh: Bool = false) async -> Bool {
    let candidates = SyncedAccount.all().filter {
      (fresh || $0.signedOut != true) && $0.userID != SyncedAccount.left
    }
    guard candidates.count == 1 else { return false }
    return (try? await restore(synced: candidates[0])) == true
  }

  /// An account tapped on Sign In, from the iCloud Keychain: signed in with its key, also one signed out before.
  @discardableResult
  func restore(synced item: SyncedAccount.Item) async throws -> Bool {
    guard account == nil else { return false }
    let client: YoClient
    let secret: Data
    do {
      (client, secret) = try await register(item.username, secret: item.accountKey, legacySeed: item.legacySeed)
    } catch AccountError.taken {
      SyncedAccount.delete(item.userID)  // its account is gone, the name another key's: nothing to bring back
      throw Diagnostics.failure(AccountError.noBackup)
    }
    guard account == nil, let id = Hub.bytes(client.userId()) else { return false }
    SyncedAccount.left = nil
    use(client, secret: secret)
    // The same user, or (the account deleted meanwhile, spec 6.4) a new one with the same key and no backup.
    let same = id == item.userID
    if same, let key = item.recoveryKey { Diagnostics.attempt({ try RecoveryWords.save(key) }) }
    let restored = Account(
      username: client.username() ?? item.username, userID: id, byPasskey: same && item.byPasskey,
      byCode: same && item.byCode == true,
      codeAuth: same ? item.codeAuth : nil, passkeyAuth: same ? item.passkeyAuth : nil)
    if !same { SyncedAccount.delete(item.userID) }  // the old user is gone; the new one gets its own item
    self.restored = same
    friendsSeeded = true
    tookPasskeyChoice(of: restored)
    account = restored
    Task { await addAuthKeys() }
    return true
  }

  // MARK: The account's storage (spec 6.9)

  /// Friends known by user id, to the account's storage (`Friends.stored`): changed ones set, removed ones taken off.
  /// The core keeps them at once and writes them to the hub right after (`toStorage`).
  func store(_ changed: [Friend], removed: [String]) {
    // A friend's name at most 64 bytes in the storage: a longer one (many emoji) is kept by user id alone.
    let set = changed.compactMap { friend in
      friend.user.map { ($0, friend.name.utf8.count <= 64 ? friend.name : nil, UInt8(friend.color % Color.rows.count)) }
    }
    toStorage { client in
      for (user, name, color) in set { Diagnostics.attempt({ try client.setFriend(user: user, name: name, color: color) }) }
      for user in removed { Diagnostics.attempt({ try client.removeFriend(user: user) }) }
    }
  }

  /// The app icon chosen in Appearance through `client`, for the account's next phone: not if another account is in
  /// since.
  func storeAppIcon(_ icon: String?, by client: YoClient?) {
    guard client === self.client else { return }
    toStorage { $0.setAppIcon(icon: icon) }
  }

  func storeLeftHanded(_ leftHanded: Bool) {
    toStorage { $0.setLeftHanded(leftHanded: leftHanded) }
  }

  /// A write to this account's storage, after the ones before it, and to the hub at once: beside the inbox's long
  /// wait, not after its answer.
  private func toStorage(_ write: @escaping @Sendable (YoClient) -> Void = { _ in }) {
    guard let client else { return }
    let before = storing
    storing = Task {
      await before?.value
      _ = try? await Hub.run {
        write(client)
        try client.writeStorage()
      }
    }
  }

  /// The storage as the core has it, into the app: when the core opened, and after the first sync (`read`: the core
  /// has read it by then, if registering could not). Its friends into the list (`Friends.take`; this phone's own go
  /// up once, for an account from before the storage); its left-handed mode, and after a restore, its app icon.
  /// Settings are written only by their controls, never from here.
  func takeStorage(into friends: Friends, read: Bool = false) async {
    guard let client else { return }
    await storing?.value  // this phone's own writes first
    let known = Set(friends.all.compactMap(\.user))
    guard
      let (kept, icon, leftHanded, names) = try? await Hub.run(reportErrors: false, { () throws -> ([YoFriend], String?, Bool?, [String: String]) in
        // `friends()` answers a turn on the shared state it did not get (another step or an extension holding it past
        // the wait, background time refused) as an empty list: a read that throws before and after it tells that
        // the core answered (and a refused one is no read; the next takes it).
        _ = try client.storageSavedAt()
        let kept = client.friends()
        _ = try client.storageSavedAt()
        // A friend kept without a name (a block alone, a name over 64 bytes) and new here: the core's name for them,
        // else the hub's.
        let contacts = client.contacts()
        var names: [String: String] = [:]
        for friend in kept where friend.name == nil && !known.contains(friend.user) {
          names[friend.user] =
            contacts.first { $0.user == friend.user }?.handles.first ?? Diagnostics.attempt({ try client.whois(name: friend.user) }).flatMap({ $0 })?.username
        }
        return (kept, client.appIcon(), client.leftHanded(), names)
      }), client === self.client
    else { return }
    // Pruned only by a list that has friends: an empty one may still be a refused read, and taking it would empty
    // the list of everyone known by user id (and its saved copy, until the next read put them back).
    friends.take(kept, names: names, seed: !friendsSeeded, prune: !restored && !kept.isEmpty)
    friendsSeeded = true
    storageRead = true
    if let leftHanded {
      UserDefaults.standard.set(leftHanded, forKey: SettingKey.leftHanded)
    }
    guard restored, read || !kept.isEmpty || icon != nil || leftHanded != nil else { return }  // maybe not read yet: after the first sync
    restored = false
    if icon != UIApplication.shared.alternateIconName {
      try? await UIApplication.shared.setAlternateIconName(icon)
    }
  }

  // MARK: Blocking, reporting, deleting

  /// Blocks a user (spec 4.8), by user id or name: nothing from them is shown, nothing goes to them. Or unblocks.
  func setBlocked(_ who: String, _ blocked: Bool) async throws {
    try await hub { try $0.setBlocked(who: who, blocked: blocked) }
    toStorage()  // the block to the account's storage, for its next phone
  }

  /// Reports a user to Yo (spec 6.4): who and why, nothing of what they sent.
  func report(_ who: String, reason: ReportReason) async throws {
    try await hub { try $0.report(who: who, reason: reason) }
  }

  /// Deletes the account at the hub with everything it holds (spec 6.4); the name is free again. The password manager
  /// is told that the account's passkeys are no longer valid (one that does not listen keeps it: deleted by hand).
  /// Then this phone forgets it (`reset`, by the caller with the rest of the app's data).
  func deleteAccount() async throws {
    guard let account else { throw Diagnostics.failure(AccountError.noBackup) }
    try await hub { try $0.deleteAccount() }
    SyncedAccount.delete(account.userID)
    try? await ASCredentialUpdater().reportAllAcceptedPublicKeyCredentials(
      relyingPartyIdentifier: Passkeys.domain, userHandle: account.userID, acceptedCredentialIDs: [])
  }

  /// "Sign Out" (and after deleting the account): marked signed out in the iCloud Keychain, so no start brings the
  /// account back by itself, here or on the person's other devices (Sign In lists it); then forgotten here (`reset`).
  func signOut() {
    if let id = account?.userID { SyncedAccount.markSignedOut(id) }
    reset(fullLocalReset: true)
  }

  /// The hub said `client` is no longer the account's device (the account taken to another iPhone): out. Not when
  /// it is an older client than the one in use (a call still under way while signing out and in again).
  func replaced(_ client: YoClient) {
    guard client === self.client else { return }
    reset()
  }

  /// This phone forgets the account, as a new phone would (the hub keeps the backup): signed out, or the account
  /// taken to another phone (`YoError.Replaced`). The iCloud Keychain keeps it then, for the phone that has it now;
  /// this one does not take it back without being asked.
  func reset(fullLocalReset: Bool = false) {
    Diagnostics.clear()
    if let id = account?.userID { SyncedAccount.left = id }
    let old = client
    account = nil
    client = nil
    secret = nil
    broken = false
    hubDown = false
    restored = false
    storing = nil
    storageRead = false
    friendsSeeded = false
    securing = nil
    newPasskey = nil
    passkeyTurnedOff = false
    wordsSaved = true
    Availability.shared.local = false
    // Registration waits for this bounded lock attempt and disk cleanup, never for a stale extension or network step.
    let before = deleting
    deleting = Task.detached(priority: .userInitiated) {
      await before?.value
      await MediaStorage.clearAll()
      // Closing a client may wait for a network step; generation retirement fences its later shared writes.
      Task.detached(priority: .userInitiated) { old?.close() }
      let held = SharedStateFile.take(within: 3)
      defer { if held { SharedStateFile.give() } }
      if fullLocalReset {
        LocalReset.wipe()
      } else {
        // Retire the lock even if an extension never releases its old turn.
        SharedStateFile.rotateLock()
        StateFile.delete()
        if let url = SharedStateFile.stateURL { try? FileManager.default.removeItem(at: url) }
      }
    }
    RecoveryWords.delete()
    Stats.reset()
    // Its notifications too: one tapped later would show (or Yo back) under the next account.
    UNUserNotificationCenter.current().removeAllPendingNotificationRequests()
    UNUserNotificationCenter.current().removeAllDeliveredNotifications()
  }
}

enum AccountError: LocalizedError {
  case taken, noBackup, noPRF, tooMany, busy, keychain, hub(YoError)

  var errorDescription: String? {
    switch self {
    case .taken: "That name is taken. Try another."
    case .noBackup: "We couldn’t find your account. Try another passkey or recovery key."
    case .noPRF: "This passkey can’t be used to sign in to Yo. Try a different password manager."
    case .tooMany: "Too many tries. Try again later."
    case .busy: "Signing in already. Wait a moment."
    case .keychain: "Yo can’t use the keychain right now. Unlock your iPhone and try again."
    case .hub(.Network): "Yo can’t reach its server. Check your connection and try again."
    case .hub(.Replaced): "Your account is in use on another iPhone."
    // The core's reason in Yo Dev only (for development); never in Yo itself.
    case .hub(let error): "Something went wrong. Try again." + (Hub.isDevApp ? " (\(error))" : "")
    }
  }
}

/// A new account's key (spec 6.4), made by the app and kept in the keychain (this device only) until the hub answered:
/// a registration whose answer was lost is tried again with the same key and keeps the name (the hub has it under
/// that key; a new key would find it taken by the lost try). One per name; forgotten once the hub took the name or
/// said it is taken.
enum NewAccountKey {
  private static func query(_ name: String) -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.new-account-key",
      kSecAttrAccount as String: Username.skeleton(name),
    ]
  }

  /// The key kept for this name, else a new one, used only once it reads back as kept. A keychain that does not
  /// answer now throws: a new key then would lose the name to the try whose answer was lost.
  static func secret(for name: String) throws -> Data {
    if let data = try Keychain.read(query(name)), data.count == 32 { return data }
    let secret = Passkeys.random(32)
    try Keychain.write(query(name), secret, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    return secret
  }

  /// Whether a key is kept for this name: a try whose answer was lost, so the name the hub calls taken may be this
  /// phone's own.
  static func has(_ name: String) -> Bool {
    guard let query = Keychain.own(query(name)) else { return false }
    return SecItemCopyMatching(query as CFDictionary, nil) == errSecSuccess
  }

  static func forget(_ name: String) { Keychain.delete(query(name)) }
}

/// The recovery key on this phone, to look up in the profile: in the keychain, for this device only (not in a
/// backup, not synced), readable while the phone is unlocked. The 12 words of before are only ever deleted.
enum RecoveryWords {
  private static var query: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.recovery-words",
      kSecAttrAccount as String: "recovery-key",
    ]
  }

  /// The key in place of the one there (updated, never deleted first); throws when the keychain did not keep it.
  static func save(_ key: String) throws {
    try Keychain.write(query, Data(key.utf8), accessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly)
  }

  static func load() -> String? { (try? Keychain.read(query)).flatMap(key) }

  /// A new key on its way into the backup at the hub: kept beside the one there until the hub has it (`settle`), so
  /// that whichever the backup opens with is on this phone, also when the answer or the app is lost between.
  static func stage(_ key: String) throws {
    try Keychain.write(staged, Data(key.utf8), accessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly)
  }

  /// The new key staged and not settled (the hub may have it or not).
  static func pending() -> String? { (try? Keychain.read(staged)).flatMap(key) }

  /// The hub has `key` in its backup now: it is the key from here on.
  static func settle(_ key: String) throws {
    if load() != key { try save(key) }
    Keychain.delete(staged)
  }

  /// This key and the 12 words of before (their items "words", "words.new").
  static func delete() {
    Keychain.delete(query)
    Keychain.delete(staged)
    for old in ["words", "words.new"] {
      Keychain.delete(query.merging([kSecAttrAccount as String: old]) { $1 })
    }
  }

  private static var staged: [String: Any] { query.merging([kSecAttrAccount as String: "recovery-key.new"]) { $1 } }

  private static func key(_ data: Data) -> String? { RecoveryCode.parse(String(decoding: data, as: UTF8.self)) }
}

/// The accounts in the iCloud Keychain (spec 7): what brings one back without a word when the app is deleted and
/// installed again, or on a new iPhone with the same Apple Account, and what Sign In lists to tap. End-to-end encrypted
/// by Apple; on a phone without iCloud Keychain they stay on the phone, which still covers the app deleted and installed
/// again. One item per account and hub ("hub host/user id"), so Yo Dev's accounts and Yo's never meet. Signing out
/// keeps the item, marked signed out: it is listed on Sign In, never brought back silently. Deleting the account
/// deletes it.
enum SyncedAccount {
  struct Item: Codable, Equatable, Identifiable {
    var username: String
    var userID: Data
    /// The account key's secret: whoever has it is the account (spec 3.2), so this item is as precious as the phone.
    var accountKey: Data
    /// How the backup opens, and its recovery key (it opens nothing the account key does not): so a phone brought
    /// back this way shows the same security as the one before, and can still change its passkey or recovery key.
    var byPasskey: Bool
    var byCode: Bool?
    /// Nil in older items: restore still checks the backup once to add its auth keys.
    var codeAuth: Bool? = nil
    var passkeyAuth: Bool? = nil
    var recoveryKey: String?
    /// Signed out (on any of the person's devices): only signed in again by a tap on Sign In.
    var signedOut: Bool?
    /// As `Account.backupVersion`. Nil: an item of the Ed25519 account key and the 12 words (`upgraded`).
    var version: Int?
    /// The Ed25519 seed an item of before had (`upgraded`): the account's storage at the hub may still be sealed under
    /// its key (spec 6.9), so the core gets it with a restore. Kept, as the secret it was made into.
    var legacySeed: Data?

    var id: Data { userID }

    /// An item of before as it is now: its secret the libsignal key it was (`legacyAccountSecret`), not secured (its
    /// backup of the 12 words no longer opens).
    func upgraded() -> Item? {
      guard version != Account.backupVersion else { return self }
      guard let secret = try? legacyAccountSecret(seed: accountKey) else { return nil }
      return Item(
        username: username, userID: userID, accountKey: secret, byPasskey: false, byCode: false, recoveryKey: nil,
        signedOut: signedOut, version: Account.backupVersion, legacySeed: accountKey)
    }
  }

  private static var host: String { Hub.home.host() ?? Hub.home.absoluteString }

  private static var base: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.account",
      kSecAttrSynchronizable as String: kCFBooleanTrue as Any,
    ]
  }

  private static func query(_ userID: Data) -> [String: Any] {
    base.merging([kSecAttrAccount as String: "\(host)/\(userID.map { String(format: "%02x", $0) }.joined())"]) { $1 }
  }

  /// The account in use here, written as it changes (signed in, so not signed out).
  static func save(_ account: Account, secret: Data, onlyIfMissing: Bool = false) {
    migrate()
    pendingSignOut.remove(account.userID)  // in use here again: not signed out
    let stored = raw(account.userID)
    let previous = stored?.upgraded()
    // An item of before (Ed25519, the 12 words) counts as missing: written anew.
    if onlyIfMissing && previous != nil && stored?.version == Account.backupVersion { return }
    // The key is readable only while the phone is unlocked (a launch in the background may come before): the one
    // already in the item is kept then.
    let item = Item(
      username: account.username, userID: account.userID, accountKey: secret,
      byPasskey: account.byPasskey, byCode: account.byCode,
      codeAuth: account.codeAuth, passkeyAuth: account.passkeyAuth,
      recoveryKey: RecoveryWords.load() ?? previous?.recoveryKey, signedOut: false, version: Account.backupVersion,
      legacySeed: previous?.legacySeed)
    // One the keychain does not take now (locked, a passing error) is noted and written again at the next opening
    // (`needsSave`), not taken as there: a new phone would miss it, or find old words.
    if write(item, previous: previous) {
      pendingSave.remove(account.userID)
    } else {
      pendingSave.insert(account.userID)
    }
  }

  /// Whether this account's item was not written the last time it changed: write it again, whole.
  static func needsSave(_ userID: Data) -> Bool { pendingSave.contains(userID) }

  /// This account's item to be written again, whole, at the next opening.
  static func mustSave(_ userID: Data) { pendingSave.insert(userID) }

  @discardableResult
  private static func write(_ item: Item, previous: Item?) -> Bool {
    guard item != previous else { return true }
    guard let data = try? JSONEncoder().encode(item) else { return false }
    let attributes: [String: Any] = [
      kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
      kSecAttrLabel as String: "\(Hub.isDevApp ? "Yo Dev" : "Yo") – \(item.username)",
    ]
    guard let query = Keychain.own(query(item.userID)) else { return false }
    let updated = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    guard updated == errSecItemNotFound else { return updated == errSecSuccess }
    return SecItemAdd(query.merging(attributes) { $1 } as CFDictionary, nil) == errSecSuccess
  }

  /// The item as it is now (`Item.upgraded`).
  static func load(_ userID: Data) -> Item? { raw(userID)?.upgraded() }

  /// The item as stored.
  private static func raw(_ userID: Data) -> Item? {
    guard var find = Keychain.own(query(userID)) else { return nil }
    find[kSecReturnData as String] = true
    find[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: CFTypeRef?
    guard SecItemCopyMatching(find as CFDictionary, &result) == errSecSuccess, let data = result as? Data else {
      return nil
    }
    return try? JSONDecoder().decode(Item.self, from: data)
  }

  /// Every account of this hub in the keychain, by name; one whose sign-out or deletion the keychain has not taken yet
  /// counts as signed out, or gone.
  static func all() -> [Item] {
    migrate()
    retryPending()
    let (signedOut, deleted) = (pendingSignOut, pendingDelete)
    return stored().filter { !deleted.contains($0.userID) }.map { item in
      var item = item
      if signedOut.contains(item.userID) { item.signedOut = true }
      return item
    }
  }

  private static func stored() -> [Item] {
    guard var find = Keychain.own(base) else { return [] }
    find[kSecReturnAttributes as String] = true
    find[kSecReturnData as String] = true
    find[kSecMatchLimit as String] = kSecMatchLimitAll
    var result: CFTypeRef?
    guard SecItemCopyMatching(find as CFDictionary, &result) == errSecSuccess, let found = result as? [[String: Any]]
    else { return [] }
    return found
      .filter { ($0[kSecAttrAccount as String] as? String)?.hasPrefix("\(host)/") == true }
      .compactMap { ($0[kSecValueData as String] as? Data).flatMap { try? JSONDecoder().decode(Item.self, from: $0) } }
      .compactMap { $0.upgraded() }
      .sorted { $0.username.localizedCaseInsensitiveCompare($1.username) == .orderedAscending }
  }

  /// Signed out: kept, to be tapped on Sign In, never brought back silently. One the keychain does not take now
  /// (locked, a passing error) is noted on this phone, counted as signed out meanwhile, and marked at the next look
  /// (`retryPending`).
  static func markSignedOut(_ userID: Data) {
    if !signOut(userID) { pendingSignOut.insert(userID) }
  }

  /// True once marked (or there is no item to mark).
  private static func signOut(_ userID: Data) -> Bool {
    let found: Data?
    do { found = try Keychain.read(query(userID)) } catch { return false }
    guard let found else { return true }
    guard var item = try? JSONDecoder().decode(Item.self, from: found) else { return false }
    guard item.signedOut != true else { return true }
    let previous = item
    item.signedOut = true
    return write(item, previous: previous)
  }

  /// The account deleted, or its name gone to another key: the item brings nothing back any more. One the keychain
  /// does not delete now is noted, never offered meanwhile, and deleted at the next look (`retryPending`).
  static func delete(_ userID: Data) {
    let status = Keychain.delete(query(userID))
    if status == errSecSuccess || status == errSecItemNotFound {
      pendingDelete.remove(userID)
    } else {
      pendingDelete.insert(userID)
    }
  }

  /// What the keychain did not take before, tried again.
  static func retryPending() {
    for id in pendingDelete { delete(id) }
    for id in pendingSignOut where signOut(id) { pendingSignOut.remove(id) }
  }

  /// Sign-outs and deletions the keychain has not taken yet (this phone's preferences).
  private static var pendingSignOut: Set<Data> {
    get { Set(UserDefaults.standard.array(forKey: "account.pendingSignOut") as? [Data] ?? []) }
    set { UserDefaults.standard.set(Array(newValue), forKey: "account.pendingSignOut") }
  }

  private static var pendingSave: Set<Data> {
    get { Set(UserDefaults.standard.array(forKey: "account.pendingSave") as? [Data] ?? []) }
    set { UserDefaults.standard.set(Array(newValue), forKey: "account.pendingSave") }
  }

  private static var pendingDelete: Set<Data> {
    get { Set(UserDefaults.standard.array(forKey: "account.pendingDelete") as? [Data] ?? []) }
    set { UserDefaults.standard.set(Array(newValue), forKey: "account.pendingDelete") }
  }

  /// Builds before one item per account kept one per hub (its host as the item's account): moved to the account's own.
  private static func migrate() {
    guard let old = Keychain.own(base.merging([kSecAttrAccount as String: host]) { $1 }) else { return }
    var find = old
    find[kSecReturnData as String] = true
    find[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: CFTypeRef?
    guard SecItemCopyMatching(find as CFDictionary, &result) == errSecSuccess else { return }
    if let data = result as? Data, let item = try? JSONDecoder().decode(Item.self, from: data),
      load(item.userID) == nil, !write(item, previous: nil)
    {
      return  // not moved (the keychain would not): the old item stays, to be moved next time
    }
    SecItemDelete(old as CFDictionary)
  }

  /// The account this phone left, or that was taken to another phone: never brought back here silently (on this
  /// phone only, until the app is deleted). Else two phones on one Apple Account would take it from each other at
  /// every start.
  static var left: Data? {
    get { UserDefaults.standard.data(forKey: "account.left") }
    set { UserDefaults.standard.set(newValue, forKey: "account.left") }
  }
}
