// The account (spec 2, 4.2, 2.4): username, user id, account key, and the backup opened by passkey PRF or recovery key.
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

let accountLog = Logger(subsystem: "app.justyo", category: "account")

/// The account's secret is not here: the core's sealed state, the backup and the iCloud Keychain hold it.
struct Account: Codable, Equatable {
  var username: String
  var userID: Data
  var byPasskey: Bool
  var byCode: Bool? = false
  var codeAuth: Bool? = nil
  var passkeyAuth: Bool? = nil
  /// Nil: a 12-word account whose backup no longer opens (spec 2.4).
  var backupVersion: Int? = Account.backupVersion

  static let backupVersion = 3

  var secured: Bool { byPasskey || byCode == true }
}

/// Usernames (spec 2.2): the rules are the core's; names are compared by skeleton ("Y0GPT" is "YoGPT").
enum Username {
  static func problem(_ name: String) -> String? {
    if name.isEmpty { return "Type a name." }
    if name.count > 16 { return "Use no more than 16 characters." }
    if !usernameValid(name: name) { return "Use at least one letter or emoji." }
    return nil
  }

  /// What a field keeps of what is typed. Japanese keyboards hand the field kana and full-width letters while a word
  /// is still being written ("i" arrives as "い"): read as Latin first, else every key press empties the field.
  static func clean(_ typed: String) -> String { usernameCanonical(typed: latin(typed)) }

  static func latin(_ typed: String) -> String {
    let japanese = typed.unicodeScalars.contains { (0x3040...0x30FF).contains($0.value) || (0xFF01...0xFF9F).contains($0.value) }
    guard japanese else { return typed }
    var text = typed.applyingTransform(.latinToHiragana, reverse: true) ?? typed
    text = text.applyingTransform(.latinToKatakana, reverse: true) ?? text
    return text.applyingTransform(.fullwidthToHalfwidth, reverse: false) ?? text
  }

  static func skeleton(_ name: String) -> String { usernameSkeleton(name: name) }
}

/// The recovery key (spec 2.4): libsignal's account entropy pool, 64 characters a–z and 0–9, shown in 16 blocks of 4.
enum RecoveryCode {
  static func make() -> String { YoCore.newRecoveryKey() }

  static func parse(_ typed: String) -> String? {
    if let key = recoveryKeyParse(typed: typed) { return key }
    let kept = typed.lowercased().filter { ($0.isASCII && ($0.isLetter || $0.isNumber)) }
    guard kept.count >= 64 else { return nil }
    return recoveryKeyParse(typed: String(kept.suffix(64)))
  }

  static func blocks(_ key: String) -> [String] {
    stride(from: 0, to: key.count, by: 4).map { start in
      let from = key.index(key.startIndex, offsetBy: start)
      return String(key[from..<(key.index(from, offsetBy: 4, limitedBy: key.endIndex) ?? key.endIndex)])
    }
  }

  static func shown(_ key: String) -> String { blocks(key).joined(separator: " ") }
}

/// The backup (spec 2.4): the core makes and opens the record (`YoBackup`); this is what its key K seals.
enum BackupRecord {
  /// One salt for all Yo passkeys, so a new phone asks once before it knows who it is.
  static let prfSalt = Data(SHA256.hash(data: Data("yo backup v1".utf8)))

  static func newKey() -> Data { SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) } }

  /// The secret sits inside the account's JSON, as in backups made while `Account` still held it.
  struct Sealed: Codable {
    var account: Account
    var accountKey: Data
    var code: String?

    private enum CodingKeys: String, CodingKey { case account, code }
    private enum SecretKeys: String, CodingKey { case accountKey }
  }
}

extension BackupRecord.Sealed {
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

@MainActor @Observable
final class AccountStore {
  var supportUnread: UInt32 = 0 { didSet { supportRevision &+= 1 } }
  private var supportRevision: UInt64 = 0
  /// The thread goes by the install id, not the account: none yet (nothing written, or Send Crashes switched off) is
  /// no thread.
  func refreshSupport() async {
    guard let install = InstallID.existing else { supportUnread = 0; return }
    let revision = supportRevision
    do {
      let thread = try await Hub.run { try helpThread(transport: Hub.transport, install: install) }
      guard revision == supportRevision else { return }
      supportUnread = thread.unread
    } catch {}  // keep the last known count while offline
  }

  private(set) var account: Account? {
    didSet {
      UserDefaults.standard.set(try? JSONEncoder().encode(account), forKey: Self.key)
      // Not removed here: an account taken to another phone stays in the iCloud Keychain.
      if let account, let secret { SyncedAccount.save(account, secret: secret) }
    }
  }

  private(set) var client: YoClient?
  private var secret: Data?

  var keychainRecoveryActive: Bool {
    guard let account, let secret, !SyncedAccount.needsSave(account.userID),
      let item = SyncedAccount.load(account.userID) else { return false }
    return item.accountKey == secret && item.signedOut != true
  }

  private func use(_ client: YoClient, secret: Data) {
    self.client = client
    self.secret = secret
    Attestation.start(client)
  }

  private(set) var wordsSaved: Bool = UserDefaults.standard.object(forKey: "recovery.saved") as? Bool ?? true {
    didSet { UserDefaults.standard.set(wordsSaved, forKey: "recovery.saved") }
  }

  func markWordsSaved(_ saved: Bool) { wordsSaved = saved }

  func declinePasskey() { passkeyTurnedOff = true }

  /// Skipped or turned off on purpose: not asked for again (no badge, no guide).
  private(set) var passkeyTurnedOff: Bool = UserDefaults.standard.bool(forKey: "passkey.turnedOff") {
    didSet { UserDefaults.standard.set(passkeyTurnedOff, forKey: "passkey.turnedOff") }
  }

  private static let key = "account"

  private static func passkeyName(_ account: Account) -> String {
    Hub.isDevApp ? "\(account.username) (Dev)" : account.username
  }
  private let passkeys = Passkeys()
  private var securing: (k: Data, code: String)?
  private var newPasskey: Data?
  private var registering = false
  /// Restored and storage not seen yet: its app icon is this phone's to take (`takeStorage`).
  private var restored = false
  private var storing: Task<Void, Never>?
  private var storageWriteGeneration: UInt64 = 0
  /// A registration waits for the signed-out state's deletion (`reset`).
  private var deleting: Task<Void, Never>?
  private var friendsSeeded: Bool = UserDefaults.standard.object(forKey: "storage.seeded") as? Bool ?? false {
    didSet { UserDefaults.standard.set(friendsSeeded, forKey: "storage.seeded") }
  }

  /// Until then a list without friends may only not have them yet (the add row keeps Group and Invite closed).
  private(set) var storageRead = false

  private(set) var updating = false
  private(set) var updateStalled = false

  func retryUpdate() {
    updateStalled = false
    openNow()
  }
  @ObservationIgnored var leaving: @MainActor () -> Void = {}
  /// `reset` wipes the media files: what waited in memory for them goes too.
  @ObservationIgnored var wiping: @MainActor () -> Void = {}

  init() {
    let saved = UserDefaults.standard.data(forKey: Self.key)
    account = saved.flatMap { try? JSONDecoder().decode(Account.self, from: $0) }
    updating = KeychainV1.pending(account: account, saved: saved)
    if account != nil {
      // Opened when the first scene connects; a background launch opens only on demand (`openNow`), because the
      // Action button's Yo (YoSend) opens the state itself and would wait for ours.
      Launch.whenScene { [weak self] in self?.openNow() }
    }
    Launch.whenActive { [weak self] in self?.openNow() }
  }

  func openNow() {
    guard client == nil, !opening else { return }
    Task { await open() }
  }

  /// The core, opened now if it is not open: awaited up to `seconds` while `still` holds (a cold start, a push).
  func clientSoon(seconds: Int, while still: () -> Bool = { true }) async -> YoClient? {
    openNow()
    for _ in 0..<seconds * 10 where client == nil && still() { try? await Task.sleep(for: .milliseconds(100)) }
    return client
  }

  /// Read-only probes retry refused turns. Reopen from the same sealed file and key after repeated failures.
  func watchState() async {
    var failures = 0
    while !Task.isCancelled {
      if UIApplication.shared.applicationState == .background {
        do { try await Task.sleep(for: .seconds(1)) } catch { return }
        continue
      }
      if let client {
        do {
          _ = try await Hub.run { try client.groups() }
          if client === self.client {
            failures = 0
            Availability.shared.local = false
          }
        } catch YoError.Save(message: "Busy") {
          // iOS gave no time for this access. Try on the next probe.
        } catch {
          if client === self.client, !Task.isCancelled, UIApplication.shared.applicationState != .background {
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

  /// Retried with backoff while the state is not readable yet (before first unlock, passing errors). A state that is
  /// gone or does not open: `broken`, sign in again (`recover`). A device replaced since: out.
  private func open() async {
    guard !opening else { return }
    opening = true
    defer { opening = false }
    updating = KeychainV1.pending(account: account, saved: UserDefaults.standard.data(forKey: Self.key))
    if updating, await KeychainV1.readSignedIn() == true { updating = false }
    if updating {
      await migrate()
      if updating { return }
    }
    var pause = 1.0
    var tries = 0
    var unavailableSince: ContinuousClock.Instant?
    var unavailableTries = 0
    while client == nil, account != nil, !broken {
      do {
        try await openOnce()
        if KeychainV1.hasOldState {
          if var current = account, let item = SyncedAccount.load(current.userID) {
            current.backupVersion = Account.backupVersion
            current.byPasskey = item.byPasskey
            current.byCode = item.byCode
            current.codeAuth = item.codeAuth
            current.passkeyAuth = item.passkeyAuth
            if let key = item.recoveryKey { Diagnostics.attempt({ try RecoveryWords.save(key) }) }
            account = current
          }
          await KeychainV1.discardOldState()
        }
        Availability.shared.local = false
        accountLog.notice("opened")
        return
      } catch YoError.Replaced {
        accountLog.error("open: replaced")
        reset()
        replacedHere = true
        return
      } catch YoError.State where account.flatMap({ SyncedAccount.load($0.userID) }) != nil {
        accountLog.error("open: state does not open, signing in again from the iCloud Keychain")
      // A state that does not open (e.g. an older build's): re-register as a new device from the iCloud Keychain.
        return await recover()
      } catch let error as YoError where !Self.notNow(error) {
        brokenWhy = "\(error)"
        accountLog.error("open: \(self.brokenWhy, privacy: .public) (try \(tries + 1))")
        if await Self.surely(&tries) { broken = true }
      } catch Unopened.gone {
        brokenWhy = await Task.detached { Self.missing() }.value
        accountLog.error("open: \(self.brokenWhy, privacy: .public) (try \(tries + 1))")
        if await Self.surely(&tries) { broken = true }
      } catch {
        Availability.shared.local = true
        // Counted only while Yo is on the screen: in the background (launched by a widget's, the Action button's or a
        // push's request) iOS may refuse every access its background time, and 30 s of that signed the account out
        // and registered it again, the Yo just sent from the widget lost with the state.
        if UIApplication.shared.isProtectedDataAvailable, Self.onScreen {
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
        try? await Task.sleep(for: .seconds(pause))
        pause = min(pause * 2, 30)
      }
    }
  }

  /// A full disk or an unfinished registration: never a reason to sign in again, which would replace this device.
  nonisolated private static func notNow(_ error: YoError) -> Bool { retryLater(error: error) }

  /// A scene of Yo's in front (active or not): not a background launch, nor Yo left.
  private static var onScreen: Bool {
    UIApplication.shared.connectedScenes.contains { $0.activationState != .background && $0.activationState != .unattached }
  }

  /// Broken only on the third try in a row: extensions may hold the file or keychain briefly (a burst of pushes).
  private static func surely(_ tries: inout Int) async -> Bool {
    tries += 1
    guard tries >= 3 else {
      try? await Task.sleep(for: .seconds(2))
      return false
    }
    return true
  }

  /// "Sign In Again" replaces this phone's device at the hub, so with the hub down it waits instead of signing out.
  static func hubAnswers() async -> Bool {
    var request = URLRequest(url: Hub.url.appending(path: ".well-known/yo-hub-key"), timeoutInterval: 8)
    request.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
    guard let (_, response) = await Diagnostics.attempt({ try await URLSession(configuration: .ephemeral).data(for: request) }) else { return false }
    return (response as? HTTPURLResponse)?.statusCode == 200
  }

  private(set) var brokenWhy = ""

  nonisolated private static func missing() -> String {
    let file = SharedStateFile.app ? "shared" : (StateFile.exists ? "file" : "no file")
    let key = SharedStateFile.app ? ((try? StateKey.shared()) == nil ? "no key" : "key") : ((try? StateKey.load()) == nil ? "no key" : "key")
    return "gone: \(file), \(key)"
  }

  private(set) var hubDown = false

  private var opening = false
  private(set) var broken = false

  private enum Unopened: Error { case notNow, gone }

  nonisolated private static func openShared(_ transport: HubTransport) throws -> (YoClient, Data) {
    try Perf.timed("launch.core.open") { () throws -> (YoClient, Data) in
      if SharedStateFile.app {
        // Shared with the extensions: opened without the turn; each step that may write takes it (SharedState.swift).
        guard let key = try StateKey.shared() else { throw Unopened.gone }
        let db = try SharedStateFile.database()
        guard try db.hasState() else { throw Unopened.gone }
        let client = try YoClient.openDb(
          transport: transport, db: db, stateKey: key, guest: false, lock: SharedStateFile())
        Hub.pin(client)
        return (client, client.accountSecret())
      }
      guard StateFile.exists, let key = try StateKey.load() else { throw Unopened.gone }
      let db = try StateFile.database()
      guard try db.hasState() else { throw Unopened.gone }
      let client = try YoClient.openDb(transport: transport, db: db, stateKey: key, guest: false, lock: nil)
      Hub.pin(client)
      return (client, client.accountSecret())
    }
  }

  private func openOnce() async throws {
    let userID = account?.userID
    let transport = Hub.transport
    // No state or key under a known account (an app put back, an iCloud Keychain account on a new phone): `open`
    // tries three times and then signs in again, as the person is shown; counted as expected, not a problem.
    let (client, secret) = try await Hub.run(reportErrors: false) { () throws -> (YoClient, Data) in
      do { return try Self.openShared(transport) }
      catch Unopened.gone {
        CountedErrors.record(domain: "AccountStore.Unopened", place: "Account.openOnce", code: "gone", expected: true)
        throw Unopened.gone
      } catch {
        Diagnostics.recordUnreported(error)
        throw error
      }
    }
    guard self.client == nil, account != nil, account?.userID == userID else { return }
    // Another user than the one shown (an interrupted update sign-in): forget the old one first.
    if let id = Hub.bytes(client.userId()), let current = account, current.userID != id {
      await leave()
      guard self.client == nil, account == current else { return }
      account = Account(username: client.username() ?? current.username, userID: id, byPasskey: false)
    }
    use(client, secret: secret)
    Perf.mark("launch.client")
    // Into the iCloud Keychain only if missing: this phone may have been replaced meanwhile, and the current phone's
    // item must not be overwritten. One whose last change the keychain did not take is written again.
    if let account {
      Perf.timed("launch.keychain.synced") {
        SyncedAccount.save(account, secret: secret, onlyIfMissing: !SyncedAccount.needsSave(account.userID))
      }
    }
    Task { await addAuthKeys() }
  }

  func recover() async {
    guard await Self.hubAnswers() else {
      hubDown = true
      return
    }
    hubDown = false
    guard client == nil else {
      broken = false
      return
    }
    // An old install without a key: only a key surely missing everywhere lets its old state go.
    if updating, let previous = account {
      let found = await KeychainV1.signIn(for: previous, saved: UserDefaults.standard.data(forKey: Self.key))
      guard client == nil, updating, account == previous else { return }
      guard case .missing = found else {
        broken = false
        return openNow()
      }
      updating = false
      await KeychainV1.discardOldState()
    }
    let item = account.flatMap { SyncedAccount.load($0.userID) }
    accountLog.error("recover: \(item == nil ? "no iCloud Keychain item" : "registering again", privacy: .public)")
    reset()
    if let item { _ = try? await restore(synced: item) }
  }

  /// Registers `username` (spec 4.2), or with `secret` takes that key's account back under its current name. One at a
  /// time: a second registration would replace the first. `replacing`: the account shown during migration.
  private func register(
    _ username: String, secret: Data?, legacySeed: Data? = nil, legacyStorageKey: Data? = nil,
    replacing: Account? = nil
  ) async throws -> (client: YoClient, secret: Data) {
    guard !registering, account == replacing else { throw Diagnostics.failure(AccountError.busy) }
    registering = true
    defer { registering = false }
    await deleting?.value  // the signed-out state goes first
    let transport = Hub.transport
    let push = Push.shared.registration
    do {
      return try await Hub.run {
        let make = { (db: StateDb, key: Data, lock: StateLock?) in
          let client = try YoClient.registerUsernameDb(
            transport: transport, db: db, stateKey: key, lock: lock, username: username, accountSecret: secret,
            legacySeed: legacySeed, legacyStorageKey: legacyStorageKey, name: nil, pushPlatform: push.platform,
            pushToken: push.token)
          Hub.pin(client)
          return client
        }
        let client: YoClient
        if SharedStateFile.app {
          let key = try StateKey.makeShared()
          client = try make(SharedStateFile.database(), key, SharedStateFile())
        } else {
          let key = try StateKey.make()
          client = try make(StateFile.database(), key, nil)
        }
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

  /// The key is kept until the hub answered (`NewAccountKey`), so retrying after a lost answer keeps the name.
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
    // Forgotten only now: a kill before this retries the same account rather than orphaning its state.
    NewAccountKey.forget(username)
    wordsSaved = false
    passkeyTurnedOff = false
  }

  // MARK: Securing the name (spec 2.4)

  func createPasskey() async throws {
    guard let account, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    if !account.secured, securing == nil { securing = (BackupRecord.newKey(), RecoveryCode.make()) }
    let made = try await passkeys.make(name: Self.passkeyName(account), userID: account.userID)
    try still(client)
    newPasskey = made
  }

  /// Serialises backup changes: two at once would each put a record over the other's.
  private func backupTurn<T>(_ work: @MainActor () async throws -> T) async rethrows -> T {
    while backupBusy { await withCheckedContinuation { backupWaiting.append($0) } }
    backupBusy = true
    defer {
      backupBusy = false
      if !backupWaiting.isEmpty { backupWaiting.removeFirst().resume() }
    }
    return try await work()
  }

  private var backupBusy = false
  private var backupWaiting: [CheckedContinuation<Void, Never>] = []

  private func still(_ client: YoClient) throws {
    guard client === self.client else { throw CancellationError() }
  }

  func checkPasskey() async throws {
    guard let credential = newPasskey, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    guard let prf = try await passkeys.ask(salt: BackupRecord.prfSalt, only: credential).prf else {
      throw Diagnostics.expected(AccountError.noPRF)
    }
    try await retryingBusy {
      try await backupTurn {
        try still(client)
        if account?.secured == true {
          try await rewrap(.set(prf))
        } else {
          try await saveBackup(prf: prf)
        }
      }
    }
    newPasskey = nil
  }

  /// A save the state database refused for a moment ("Busy") is tried again shortly instead of failing the step;
  /// putting the same backup again is harmless.
  private func retryingBusy(_ work: () async throws -> Void) async throws {
    var attempt = 1
    while true {
      do {
        try await work()
        return
      } catch AccountError.hub(.Save(let kind)) where kind == "Busy" && attempt < 3 {
        attempt += 1
        try await Task.sleep(for: .milliseconds(400))
      }
    }
  }

  func secureWords() -> String {
    if securing == nil { securing = (BackupRecord.newKey(), RecoveryCode.make()) }
    return securing?.code ?? ""
  }

  func keepWords() async throws { try await retryingBusy { try await backupTurn { try await saveBackup(prf: nil) } } }

  private func saveBackup(prf: Data?) async throws {
    guard var account, let securing, let client else { throw Diagnostics.failure(AccountError.noBackup) }
    account.byPasskey = prf != nil
    account.byCode = true
    account.codeAuth = true
    account.passkeyAuth = prf != nil
    try await put(account, code: securing.code, passkey: prf.map { .new(prf: $0) } ?? .none, by: client)
    self.account = account
    self.securing = nil
    if prf != nil { passkeyTurnedOff = false }
  }

  /// Through the client the change began with. New words are staged first and settled once the hub has them, so a
  /// failure leaves the words the backup opens with here. K is the core's: the same only while both doors stay
  /// (`keep` under the same words), new with any other door (spec 2.4).
  private func put(_ account: Account, code: String, passkey: YoPasskeyDoor, by client: YoClient)
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
        try client.putBackup(sealed: sealed, recoveryKey: code, passkey: passkey)
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

  private func hub<T: Sendable>(_ work: @escaping @Sendable (YoClient) throws -> T) async throws -> T {
    guard let client else { throw Diagnostics.failure(AccountError.noBackup) }
    do {
      return try await Hub.run { try work(client) }
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
  }

  private enum Door { case keep, set(Data), remove }

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
    let door: YoPasskeyDoor
    switch passkey {
    case .keep where words != code && !record.byPasskey.isEmpty:
      // New words come with a new K (spec 2.4): the passkey's door is made anew, so the passkey is asked once more.
      let answer = try await passkeys.ask(salt: BackupRecord.prfSalt)
      guard let prf = answer.prf else { throw Diagnostics.expected(AccountError.noPRF) }
      guard answer.userID == account.userID else { throw Diagnostics.expected(AccountError.noBackup) }
      try still(client)
      door = .new(prf: prf)
      account.byPasskey = true
      account.passkeyAuth = true
    case .keep where words != code:
      door = .none
      account.byPasskey = false
      account.passkeyAuth = false
    case .keep:
      door = .keep(record: record)
      account.byPasskey = !record.byPasskey.isEmpty
      account.passkeyAuth = !record.authPasskey.isEmpty
    case .set(let prf):
      door = .new(prf: prf)
      account.byPasskey = true
      account.passkeyAuth = true
    case .remove:
      door = .none
      account.byPasskey = false
      account.passkeyAuth = false
    }
    account.byCode = true
    account.codeAuth = true
    try await put(account, code: words, passkey: door, by: client)
    self.account = account
    if case .set = passkey { passkeyTurnedOff = false }
    if case .remove = passkey { passkeyTurnedOff = true }
  }

  /// Tries staged-but-unsettled words first (the hub's answer may have been lost), then the saved ones.
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
        continue
      }
    }
    throw Diagnostics.failure(AccountError.noBackup)
  }

  /// Records from before the doors' auth keys (spec 2.4) are rewrapped once, silently; retried at each start until done.
  private func addAuthKeys() async {
    await backupTurn {
      guard let account, account.secured, account.codeAuth != true, let words = RecoveryWords.load(),
        let client
      else { return }
      do {
        try await rewrap(.keep)
      } catch AccountError.noBackup {
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
    try await put(account, code: words, passkey: passkey, by: client)
    self.account = account
  }

  func removePasskey() async throws { try await backupTurn { try await rewrap(.remove) } }

  func newRecoveryKey() async throws -> String {
    let code = RecoveryCode.make()
    try await backupTurn { try await rewrap(.keep, code: code) }
    wordsSaved = false
    return code
  }

  // MARK: A new phone

  func restoreWithPasskey() async throws {
    let answer = try await passkeys.ask(salt: BackupRecord.prfSalt)
    guard let prf = answer.prf else { throw Diagnostics.expected(AccountError.noPRF) }
    try await restore(fetchBackup(by: .passkey(prf: prf), userID: answer.userID))
  }

  func restore(code: String) async throws {
    try await restore(fetchBackup(by: .recoveryKey(key: code), userID: nil), typed: code)
    wordsSaved = RecoveryWords.load() != nil
  }

  private func fetchBackup(by door: YoDoor, userID: Data?) async throws -> YoOpenedBackup {
    let transport = Hub.transport
    do {
      let pinned = Hub.hubKeys
      guard
        let opened = try await Hub.run({
          try openBackup(transport: transport, userId: userID, door: door, hubKeys: pinned)
        })
      else {
        throw Diagnostics.expected(AccountError.noBackup)
      }
      return opened
    } catch YoError.Refused(let status) where status == 429 {
      throw AccountError.tooMany
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
  }

  /// The hub finds the account by key, not by the backup's (maybe outdated) name. `typed`: kept when the backup has
  /// no words (one made before it held them).
  private func restore(_ opened: YoOpenedBackup, typed: String? = nil) async throws {
    let sealed = try JSONDecoder().decode(BackupRecord.Sealed.self, from: opened.sealed)
    let (client, secret) = try await register(sealed.account.username, secret: sealed.accountKey)
    use(client, secret: secret)
    // A different user id: the account was deleted and taken again with the same key; it has no backup yet.
    var account = sealed.account
    account.username = client.username() ?? account.username
    let id = Hub.bytes(client.userId()) ?? account.userID
    let same = id == account.userID
    if !same { account = Account(username: account.username, userID: id, byPasskey: false) }
    if same, let code = sealed.code ?? typed { Diagnostics.attempt({ try RecoveryWords.save(code) }) }
    SyncedAccount.left = nil
    adopt(account, restored: same)
  }

  private func adopt(_ account: Account, restored: Bool) {
    self.restored = restored
    friendsSeeded = true
    // A words-only account had its passkey declined on the old phone: not asked for again.
    passkeyTurnedOff = account.secured && !account.byPasskey
    self.account = account
    Task { await addAuthKeys() }
  }

  /// One-time sign-in of an old install (KeychainV1), in the foreground once unlocked; retried with backoff while Yo is
  /// in front. Nothing old goes before the new state is in. Never in a background launch (a silent push): that waits
  /// for the next opening (owner's decision, PLAN).
  private func migrate() async {
    var pause = 1.0
    var tries = 0
    while updating, let previous = account, Self.inFront {
      if tries >= 6 {  // a minute or more
        updateStalled = true
        return
      }
      tries += 1
      let signedIn = await KeychainV1.readSignedIn()
      guard updating, account == previous, Self.inFront else { return }
      if signedIn == true {
        updating = false
        updateStalled = false
        return
      }
      var found = LegacyFound.notNow
      if signedIn == false {
        found = await KeychainV1.signIn(for: previous, saved: UserDefaults.standard.data(forKey: Self.key))
      }
      accountLog.notice("migrate: try \(tries, privacy: .public) signedIn \(String(describing: signedIn), privacy: .public) found \(Self.describe(found), privacy: .public)")
      // Re-checked after the keychain answered, before anything irreversible.
      guard updating, account == previous, Self.inFront else { return }
      switch found {
      case .missing:
        updateStalled = false
        brokenWhy = "no account key"
        broken = true
        return
      case .signedOut:
        // Signed out or moved to another phone: never sign in again silently (it would replace that phone).
        accountLog.notice("migrate: signed out or left, not signing in again")
        updating = false
        updateStalled = false
        return reset()
      case .signIn(let item, let storageKey) where SyncedAccount.write(item, previous: nil):
        do {
          if try await restore(synced: item, legacyStorageKey: storageKey, replacing: previous) {
            await KeychainV1.discardOldState()
            updating = false
            updateStalled = false
            broken = false
            return
          }
          accountLog.error("migrate: restore returned false")
        } catch AccountError.noBackup {
          await KeychainV1.discardOldState()
          updating = false
          updateStalled = false
          brokenWhy = "account gone"
          broken = true
          return
        } catch {
          accountLog.error("migrate: restore failed: \(String(describing: error), privacy: .public)")
        }
      case .signIn: accountLog.error("migrate: keychain write refused")
      case .notNow: break
      }
      try? await Task.sleep(for: .seconds(pause))
      pause = min(pause * 2, 30)
    }
  }

  private static func describe(_ found: LegacyFound) -> String {
    switch found {
    case .signIn(_, let key): "signIn(storageKey: \(key != nil))"
    case .signedOut: "signedOut"
    case .missing: "missing"
    case .notNow: "notNow"
    }
  }

  private static var inFront: Bool {
    UIApplication.shared.applicationState != .background && UIApplication.shared.isProtectedDataAvailable
  }

  /// Another user comes in: what this phone kept of the previous one goes first. The old account stays shown until
  /// then, so an interrupted run redoes it at the next opening.
  private func leave() async {
    RecoveryWords.delete()
    await MediaStorage.clearAll()
    leaving()
  }

  /// Silent restore from the iCloud Keychain (app reinstalled, new iPhone): only when exactly one eligible account is
  /// there. `fresh`: first start after install, so an earlier Sign Out does not hold it back.
  func restoreFromKeychain(fresh: Bool = false) async -> Bool {
    let candidates = SyncedAccount.all().filter {
      (fresh || $0.signedOut != true) && $0.userId != SyncedAccount.left
    }
    guard candidates.count == 1 else { return false }
    return (try? await restore(synced: candidates[0])) == true
  }

  @discardableResult
  func restore(
    synced item: SyncedAccount.Item, legacyStorageKey: Data? = nil, replacing previous: Account? = nil
  ) async throws -> Bool {
    guard account == previous else { return false }
    let client: YoClient
    let secret: Data
    do {
      (client, secret) = try await register(
        item.username, secret: item.accountKey, legacySeed: item.legacySeed, legacyStorageKey: legacyStorageKey,
        replacing: previous)
    } catch AccountError.taken {
      SyncedAccount.delete(item.userId)  // account gone, name taken by another key
      throw Diagnostics.failure(AccountError.noBackup)
    }
    guard account == previous, let id = Hub.bytes(client.userId()) else { return false }
    if let previous, previous.userID != id {
      await leave()
      guard account == previous else { return false }
    }
    SyncedAccount.left = nil
    use(client, secret: secret)
    let same = id == item.userId
    if same, let key = item.recoveryKey { Diagnostics.attempt({ try RecoveryWords.save(key) }) }
    let restored = Account(
      username: client.username() ?? item.username, userID: id, byPasskey: same && item.byPasskey,
      byCode: same && item.byCode == true,
      codeAuth: same ? item.codeAuth : nil, passkeyAuth: same ? item.passkeyAuth : nil)
    if !same { SyncedAccount.delete(item.userId) }
    adopt(restored, restored: same)
    return true
  }

  // MARK: The account's storage (spec 4.3)

  func store(_ changed: [Friend], removed: [String]) {
    // Names over 64 bytes are stored by user id alone.
    let set = changed.compactMap { friend in
      friend.user.map { ($0, friend.name.utf8.count <= 64 ? friend.name : nil, UInt8(friend.color % Color.rows.count)) }
    }
    toStorage { client in
      for (user, name, color) in set { Diagnostics.attempt({ try client.setFriend(user: user, name: name, color: color) }) }
      for user in removed { Diagnostics.attempt({ try client.removeFriend(user: user) }) }
    }
  }

  func storeAppIcon(_ icon: String?, by client: YoClient?) {
    guard client === self.client else { return }
    toStorage { $0.setAppIcon(icon: icon) }
  }

  func storeLeftHanded(_ leftHanded: Bool) {
    toStorage { $0.setLeftHanded(leftHanded: leftHanded) }
  }

  private func toStorage(_ write: @escaping @Sendable (YoClient) -> Void = { _ in }) {
    guard let client else { return }
    let before = storing
    storageWriteGeneration &+= 1
    storing = Task {
      await before?.value
      // A settings or list change that did not reach the hub is said (the core keeps it and writes it later).
      do {
        try await Hub.run {
          write(client)
          try client.writeStorage()
        }
      } catch {
        Problems.shared.report(error)
      }
    }
  }

  /// The storage into the app: friends, left-handed mode, and after a restore the app icon. Settings are written only
  /// by their controls, never from here.
  func takeStorage(into friends: Friends, read: Bool = false) async {
    guard let client else { return }
    while client === self.client {
      let generation = storageWriteGeneration
      await storing?.value
      guard client === self.client else { return }
      guard generation == storageWriteGeneration else { continue }
      let known = Set(friends.all.compactMap(\.user))
      guard
        let (kept, icon, leftHanded, names) = try? await Hub.run(reportErrors: false, { () throws -> ([YoFriend], String?, Bool?, [String: String]) in
          // `friends()` answers an unreadable store as empty: reads that throw around it show the core answered.
          _ = try client.storageSavedAt()
          let kept = client.friends()
          _ = try client.storageSavedAt()
          let contacts = client.contacts()
          var names: [String: String] = [:]
          for friend in kept where friend.name == nil && !known.contains(friend.user) {
            names[friend.user] =
              contacts.first { $0.user == friend.user }?.handles.first ?? Diagnostics.attempt({ try client.whois(name: friend.user) }).flatMap({ $0 })?.username
          }
          return (kept, client.appIcon(), client.leftHanded(), names)
        }), client === self.client
      else { return }
      guard generation == storageWriteGeneration else { continue }
      // Pruned only by a non-empty list: an empty one may be a refused read.
      friends.take(kept, names: names, seed: !friendsSeeded, prune: !restored && !kept.isEmpty)
      friendsSeeded = true
      storageRead = true
      if let leftHanded {
        UserDefaults.standard.set(leftHanded, forKey: SettingKey.leftHanded)
      }
      guard restored, read || !kept.isEmpty || icon != nil || leftHanded != nil else { return }  // maybe not read yet
      restored = false
      if icon != UIApplication.shared.alternateIconName {
        try? await UIApplication.shared.setAlternateIconName(icon)
      }
      return
    }
  }

  // MARK: Blocking, reporting, deleting

  func setBlocked(_ who: String, _ blocked: Bool) async throws {
    try await hub { try $0.setBlocked(who: who, blocked: blocked) }
    toStorage()  // for the account's next phone
  }

  /// By the install id, not this account.
  func report(_ who: String, reason: ReportReason) async throws {
    let install = InstallID.current
    try await hub { try $0.report(who: who, reason: reason, install: install) }
  }

  /// Spec 4.2. Also tells the password manager the passkeys are no longer valid; the caller then resets.
  func deleteAccount() async throws {
    guard let account else { throw Diagnostics.failure(AccountError.noBackup) }
    try await hub { try $0.deleteAccount() }
    SyncedAccount.delete(account.userID)
    try? await ASCredentialUpdater().reportAllAcceptedPublicKeyCredentials(
      relyingPartyIdentifier: Passkeys.domain, userHandle: account.userID, acceptedCredentialIDs: [])
  }

  /// Marked signed out in the iCloud Keychain so no start brings it back by itself (Sign In lists it).
  func signOut() {
    guard !updating else { return }
    if let id = account?.userID { SyncedAccount.markSignedOut(id) }
    reset(fullLocalReset: true, signedOut: true)
  }

  func replaced(_ client: YoClient) {
    guard client === self.client else { return }
    reset()
    replacedHere = true
  }

  /// The hub said another phone took the account (410): the welcome says so, once, instead of only showing the way
  /// in as if signed out. Kept over a restart (the app may be ended before the welcome showed it).
  var replacedHere: Bool {
    get { UserDefaults.standard.bool(forKey: "account.replacedHere") }
    set { UserDefaults.standard.set(newValue, forKey: "account.replacedHere") }
  }

  /// This phone forgets the account (the hub keeps the backup; the iCloud Keychain keeps it for the phone that has it).
  /// `signedOut`: this phone's device stays the account's at the hub, so the hub is told to stop waking the phone for
  /// it (as Android's sign-out); else each of its Yos showed "You may have received a Yo" here.
  func reset(fullLocalReset: Bool = false, signedOut: Bool = false) {
    Diagnostics.clear()
    if let id = account?.userID { SyncedAccount.left = id; Push.shared.forget(id) }
    let old = client
    // Started first, so it has the turn before the state goes below (it fails quietly if not).
    if signedOut, let old { Task.detached(priority: .userInitiated) { _ = try? old.setPush(platform: 0, token: "") } }
    account = nil
    client = nil
    secret = nil
    broken = false
    hubDown = false
    updateStalled = false
    restored = false
    storing = nil
    storageRead = false
    friendsSeeded = false
    securing = nil
    newPasskey = nil
    passkeyTurnedOff = false
    wordsSaved = true
    Availability.shared.local = false
    // Else the same account signed in again (recovery, replaced) sends the outbox's items without their files.
    wiping()
    // Registration waits for this bounded lock attempt and disk cleanup, never for a stale extension or network step.
    let before = deleting
    deleting = Task.detached(priority: .userInitiated) {
      await before?.value
      await MediaStorage.clearAll()
      // Closing a client may wait for a network step; generation retirement fences its later shared writes.
      Task.detached(priority: .userInitiated) { old?.close() }
      let available = SharedStateFile.available
      let shared = available ? (try? SharedStateFile.database()) : nil
      let held = SharedStateFile.take(within: 3)
      defer { if held { SharedStateFile.give() } }
      // Retire the lock even if an extension never releases its old turn; cleared before a full wipe as well.
      SharedStateFile.rotateLock()
      StateFile.forget()
      // A database that no longer opens ("file is not a database") cannot be cleared: it goes, else every sign-in
      // and sign-up after registered into it and failed ("Something went wrong") until Yo was deleted.
      if available, shared.flatMap({ db in Diagnostics.attempt({ try db.clear() }) }) == nil {
        SharedStateFile.removeDatabase()
      }
      if fullLocalReset { LocalReset.wipe() }
    }
    RecoveryWords.delete()
    Stats.reset()
      // Else one tapped later would show under the next account.
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
    case .keychain: "Yo can’t use the keychain right now. Unlock your device and try again."
    case .hub(.Network): "Yo can’t reach its server. Check your connection and try again."
    case .hub(.Replaced): "Your account is in use on another device."
    case .hub(.Clock): productProblemText(problem: .wrongClock, lang: .english)
    // The raw error goes to the error log (Hub.run records it), never on screen.
    case .hub: "Something went wrong. Try again."
    }
  }
}

/// A new account's key, kept on this device until the hub answered: a retry after a lost answer uses the same key
/// and keeps the name.
enum NewAccountKey {
  private static func query(_ name: String) -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.new-account-key",
      kSecAttrAccount as String: Username.skeleton(name),
    ]
  }

  /// A keychain that does not answer throws: a new key would lose the name to the lost try.
  static func secret(for name: String) throws -> Data {
    if let data = try Keychain.read(query(name)), data.count == 32 { return data }
    let secret = Passkeys.random(32)
    try Keychain.write(query(name), secret, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    return secret
  }

  static func has(_ name: String) -> Bool {
    guard let query = Keychain.own(query(name)) else { return false }
    return SecItemCopyMatching(query as CFDictionary, nil) == errSecSuccess
  }

  static func forget(_ name: String) { Keychain.delete(query(name)) }
}

/// The recovery key on this phone (this device only, while unlocked). The 12 words of before are only ever deleted.
enum RecoveryWords {
  private static var query: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.recovery-words",
      kSecAttrAccount as String: "recovery-key",
    ]
  }

  static func save(_ key: String) throws {
    try Keychain.write(query, Data(key.utf8), accessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly)
  }

  static func load() -> String? { (try? Keychain.read(query)).flatMap(key) }

  /// Kept beside the current key until the hub has it (`settle`), so whichever the backup opens with is here.
  static func stage(_ key: String) throws {
    try Keychain.write(staged, Data(key.utf8), accessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly)
  }

  static func pending() -> String? { (try? Keychain.read(staged)).flatMap(key) }

  static func settle(_ key: String) throws {
    if load() != key { try save(key) }
    Keychain.delete(staged)
  }

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

/// Accounts in the iCloud Keychain (spec 5.1): bring one back after reinstall or on a new iPhone, and list them on Sign
/// In. One item per hub and user id; signed-out items stay, marked, and are never restored silently.
enum SyncedAccount {
  typealias Item = SyncedItem

  static var host: String { Hub.home.host() ?? Hub.home.absoluteString }

  static var base: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.account",
      kSecAttrSynchronizable as String: kCFBooleanTrue as Any,
    ]
  }

  private static func query(_ userID: Data) -> [String: Any] {
    base.merging([kSecAttrAccount as String: "\(host)/\(userID.map { String(format: "%02x", $0) }.joined())"]) { $1 }
  }

  static func save(_ account: Account, secret: Data, onlyIfMissing: Bool = false) {
    try? KeychainV1.moveHostItem()
    pendingSignOut.remove(account.userID)
    let stored = try? raw(account.userID)
    let previous = stored.flatMap { syncedItemUpgraded(item: $0) }
    // An item of before (Ed25519, the 12 words) counts as missing.
    if onlyIfMissing && previous != nil && stored?.version == syncedItemVersion() { return }
    // The recovery key is unreadable while locked: keep the one already in the item then.
    let item = Item(
      username: account.username, userId: account.userID, accountKey: secret,
      byPasskey: account.byPasskey, byCode: account.byCode,
      codeAuth: account.codeAuth, passkeyAuth: account.passkeyAuth,
      recoveryKey: RecoveryWords.load() ?? previous?.recoveryKey, signedOut: false, version: syncedItemVersion(),
      legacySeed: previous?.legacySeed)
    // Not taken now (locked, passing error): retried at the next opening (`needsSave`).
    if write(item, previous: previous) {
      pendingSave.remove(account.userID)
    } else {
      pendingSave.insert(account.userID)
    }
  }

  static func needsSave(_ userID: Data) -> Bool { pendingSave.contains(userID) }

  @discardableResult
  static func write(_ item: Item, previous: Item?) -> Bool {
    guard item != previous else { return true }
    let data = syncedItemEncode(item: item)
    let attributes: [String: Any] = [
      kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
      kSecAttrLabel as String: "\(Hub.isDevApp ? "Yo Dev" : "Yo") – \(item.username)",
    ]
    guard let query = Keychain.own(query(item.userId)) else { return false }
    let updated = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    guard updated == errSecItemNotFound else { return updated == errSecSuccess }
    return SecItemAdd(query.merging(attributes) { $1 } as CFDictionary, nil) == errSecSuccess
  }

  static func load(_ userID: Data) -> Item? { try? read(userID) }

  static func read(_ userID: Data) throws -> Item? { try raw(userID).flatMap { syncedItemUpgraded(item: $0) } }

  private static func raw(_ userID: Data) throws -> Item? {
    try Keychain.read(query(userID)).flatMap { syncedItemDecode(json: $0) }
  }

  static func all() -> [Item] {
    try? KeychainV1.moveHostItem()
    retryPending()
    let (signedOut, deleted) = (pendingSignOut, pendingDelete)
    return stored().filter { !deleted.contains($0.userId) }.map { item in
      var item = item
      if signedOut.contains(item.userId) { item.signedOut = true }
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
      .compactMap { ($0[kSecValueData as String] as? Data).flatMap { syncedItemDecode(json: $0) } }
      .compactMap { syncedItemUpgraded(item: $0) }
      .sorted { $0.username.localizedCaseInsensitiveCompare($1.username) == .orderedAscending }
  }

  /// Not taken now: noted, counted as signed out, retried at the next look (`retryPending`).
  static func markSignedOut(_ userID: Data) {
    if !signOut(userID) { pendingSignOut.insert(userID) }
  }

  private static func signOut(_ userID: Data) -> Bool {
    let found: Data?
    do { found = try Keychain.read(query(userID)) } catch { return false }
    guard let found else { return true }
    guard var item = syncedItemDecode(json: found) else { return false }
    guard item.signedOut != true else { return true }
    let previous = item
    item.signedOut = true
    return write(item, previous: previous)
  }

  /// Not deleted now: noted, never offered, retried at the next look (`retryPending`).
  static func delete(_ userID: Data) {
    let status = Keychain.delete(query(userID))
    if status == errSecSuccess || status == errSecItemNotFound {
      pendingDelete.remove(userID)
    } else {
      pendingDelete.insert(userID)
    }
  }

  static func retryPending() {
    for id in pendingDelete { delete(id) }
    for id in pendingSignOut where signOut(id) { pendingSignOut.remove(id) }
  }

  private static var pendingSignOut: Set<Data> {
    get { ids("account.pendingSignOut") } set { setIDs(newValue, "account.pendingSignOut") }
  }
  private static var pendingSave: Set<Data> {
    get { ids("account.pendingSave") } set { setIDs(newValue, "account.pendingSave") }
  }
  private static var pendingDelete: Set<Data> {
    get { ids("account.pendingDelete") } set { setIDs(newValue, "account.pendingDelete") }
  }
  private static func ids(_ key: String) -> Set<Data> { Set(UserDefaults.standard.array(forKey: key) as? [Data] ?? []) }
  private static func setIDs(_ ids: Set<Data>, _ key: String) { UserDefaults.standard.set(Array(ids), forKey: key) }

  /// Never brought back here silently: else two phones on one Apple Account would take it from each other.
  static var left: Data? {
    get { UserDefaults.standard.data(forKey: "account.left") }
    set { UserDefaults.standard.set(newValue, forKey: "account.left") }
  }
}

extension SyncedItem: Identifiable {
  public var id: Data { userId }
}
