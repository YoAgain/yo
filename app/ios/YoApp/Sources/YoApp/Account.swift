// Your account (spec 3, 6.4, 7): a username, a user id, an account key, and the backup that brings them back on a
// new phone, opened by a passkey's PRF output or by a recovery code.
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

struct Account: Codable, Equatable {
  var username: String
  var userID: Data
  /// The account key's 32-byte secret (Ed25519; signs the account's devices, spec 3.1), from the core.
  var accountKey: Data
  /// How the backup opens: by the passkey (and the words), or by the recovery words only.
  var byPasskey: Bool
  /// The recovery words open the backup (nil in accounts saved before this was kept: see `AccountStore.init`).
  var byCode: Bool? = false
  /// The backup's fetch may be signed by the words' auth key (spec 7): nil in a record from before the auth keys, until
  /// it is written again (`addAuthKeys`).
  var codeAuth: Bool? = nil
  /// And by the passkey's: false until the passkey was asked once for it (a record from before, `confirmPasskey`).
  var passkeyAuth: Bool? = nil

  /// Backed up (spec 7): the name comes back on a new phone. Not yet, it is released after 365 days away (spec 6.4).
  var secured: Bool { byPasskey || byCode == true }
}

/// Usernames (spec 3.3): Latin letters (accents and umlauts too), digits, `_` and emoji; kept and shown as their owner
/// writes them ("YoGPT"). Two names that look alike are one name: they are compared by their skeleton, so "yogpt" and
/// "Y0GPT" are "YoGPT". Which names are reserved only the hub knows: to the app they are taken.
enum Username {
  /// What is wrong with a name, nil when it is fine.
  static func problem(_ name: String) -> String? {
    if name.count < 3 { return "Use at least 3 characters." }
    if name.count > 16 { return "Use no more than 16 characters." }
    if let first = name.first, !isLatinLetter(first), !isEmoji(first) { return "Start with a letter or emoji." }
    return nil
  }

  /// What may be typed: the allowed characters, as typed (NFC), a letter or emoji first, at most 16. So the field
  /// itself keeps the rules, and only "too short" and "taken" are left to show.
  static func clean(_ typed: String) -> String {
    let allowed = typed.precomposedStringWithCanonicalMapping.filter {
      isLatinLetter($0) || ("0"..."9").contains($0) || $0 == "_" || isEmoji($0)
    }
    return String(allowed.drop(while: { !isLatinLetter($0) && !isEmoji($0) }).prefix(16))
  }

  /// What a name is compared by (spec 3.3): upper case, compatibility-decomposed, without accents, emoji variation
  /// selectors and skin tones, and with 0 → O, 1 → I, VV → W. LEO, LE0 and LÉO are one name; so are 👍 and 👍🏿.
  static func skeleton(_ name: String) -> String {
    var scalars = String.UnicodeScalarView()
    for scalar in name.uppercased().decomposedStringWithCompatibilityMapping.unicodeScalars {
      let v = scalar.value
      if scalar.properties.generalCategory == .nonspacingMark || v == 0xFE0E || v == 0xFE0F
        || (0x1F3FB...0x1F3FF).contains(v)
      { continue }
      scalars.append(scalar)
    }
    return String(scalars)
      .replacingOccurrences(of: "0", with: "O")
      .replacingOccurrences(of: "1", with: "I")
      .replacingOccurrences(of: "VV", with: "W")
  }

  /// A letter of the Latin script, with its accents (Basic Latin, Latin-1, Extended-A/B, Extended Additional).
  static func isLatinLetter(_ c: Character) -> Bool {
    c.isLetter && c.unicodeScalars.allSatisfy { s in
      let v = s.value
      return (0x41...0x5A).contains(v) || (0x61...0x7A).contains(v) || (0x300...0x36F).contains(v)
        || ((0xC0...0x24F).contains(v) && v != 0xD7 && v != 0xF7) || (0x1E00...0x1EFF).contains(v)
    }
  }

  /// An emoji as people type it: shown as emoji by default, or a sequence (keycaps, ZWJ, flags, skin tones).
  static func isEmoji(_ c: Character) -> Bool {
    guard let first = c.unicodeScalars.first else { return false }
    return first.properties.isEmojiPresentation || (first.properties.isEmoji && c.unicodeScalars.count > 1)
  }
}

/// The 12-word recovery code (BIP-39: 128 bits and a 4-bit checksum).
enum RecoveryCode {
  static let words: [String] = {
    guard let url = Bundle.main.url(forResource: "bip39-english", withExtension: "txt"),
      let text = try? String(contentsOf: url, encoding: .utf8)
    else { return [] }
    return text.split(separator: "\n").map(String.init)
  }()

  static func make() -> [String] {
    let entropy = Passkeys.random(16)
    let check = Array(SHA256.hash(data: entropy))[0] >> 4
    var bits = entropy.flatMap { byte in (0..<8).map { (byte >> (7 - $0)) & 1 } }
    bits += (0..<4).map { (check >> (3 - $0)) & 1 }
    return (0..<12).map { i in
      words[bits[(i * 11)..<(i * 11 + 11)].reduce(0) { $0 << 1 | Int($1) }]
    }
  }

  /// The 12 words in whatever was pasted, typed or scanned: numbers, a heading ("Yo LEO"), line breaks and other
  /// words around them do not matter; the first run of 12 list words whose checksum matches counts.
  static func parse(_ typed: String) -> [String]? {
    let known = typed.lowercased().split(whereSeparator: { !$0.isLetter }).map(String.init).filter(words.contains)
    guard known.count >= 12 else { return nil }
    return (0...(known.count - 12)).lazy.compactMap { check(Array(known[$0..<($0 + 12)])) }.first
  }

  /// The 12 words if their checksum matches.
  private static func check(_ given: [String]) -> [String]? {
    let indexes = given.compactMap { words.firstIndex(of: $0) }
    guard indexes.count == 12 else { return nil }
    let bits = indexes.flatMap { index in (0..<11).map { UInt8((index >> (10 - $0)) & 1) } }
    let entropy = Data((0..<16).map { byte in bits[(byte * 8)..<(byte * 8 + 8)].reduce(0) { $0 << 1 | $1 } })
    let check = bits[128..<132].reduce(0) { $0 << 1 | $1 }
    return Array(SHA256.hash(data: entropy))[0] >> 4 == check ? given : nil
  }
}

/// The backup (spec 7): the account under a random key K, and K wrapped by the passkey and by the code. The record
/// itself (`YoBackup`) lives at the hub; the core makes, signs for and opens it (`openBackup`, `putBackup`), and this
/// is what K seals.
enum BackupRecord {
  /// The salt every Yo passkey is asked with: one for all, so a new phone asks once, before it knows who it is.
  static let prfSalt = Data(SHA256.hash(data: Data("yo backup v1".utf8)))

  /// A new K: 32 random bytes.
  static func newKey() -> Data { SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) } }

  /// What K seals: the account (its key's secret above all), and the recovery words, so a phone restored by passkey
  /// has them to show too.
  struct Sealed: Codable {
    var account: Account
    var code: [String]?
  }
}

@MainActor @Observable
final class AccountStore {
  private(set) var account: Account? {
    didSet {
      UserDefaults.standard.set(try? JSONEncoder().encode(account), forKey: Self.key)
      // Kept in the iCloud Keychain as it changes (not removed here: an account taken to another phone stays there).
      if let account { SyncedAccount.save(account) }
    }
  }

  /// The core, opened from its saved state at launch or made at registration: talks to the hub for this device.
  private(set) var client: YoClient?

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
  /// Securing under way: K and the words, made once per try.
  private var securing: (k: Data, code: [String])?
  /// The guide's passkey once made, before it is signed in with.
  private var newPasskey: Data?
  /// A registration under way (`register(_:secret:)`).
  private var registering = false
  /// Signed in from a backup or the iCloud Keychain, and the account's storage not seen yet: its app icon is this
  /// phone's to take (`takeStorage`).
  private var restored = false
  /// Writes to the storage, one after the other in the order made (`Hub.run` runs calls side by side).
  private var storing: Task<Void, Never>?
  /// This phone's friends from before the storage went up to it once (`takeStorage`); a new or restored account has
  /// none to bring.
  private var friendsSeeded: Bool = UserDefaults.standard.object(forKey: "storage.seeded") as? Bool ?? false {
    didSet { UserDefaults.standard.set(friendsSeeded, forKey: "storage.seeded") }
  }

  init() {
    account = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode(Account.self, from: $0) }
    if account != nil { Task { await open() } }
  }

  /// The core from the state it saved last. A device replaced since (the account restored on another phone): out.
  private func open() async {
    guard client == nil else { return }
    let transport = Hub.transport
    do {
      let opened = try await Hub.run { () throws -> YoClient? in
        guard SharedStateFile.app else {
          guard let blob = StateFile.load() else { return nil }
          return try YoClient.restore(transport: transport, store: StateFile(), stateKey: StateKey.load(), blob: blob)
        }
        // Shared with the extensions: opened under the lock, then each step takes it (SharedState.swift).
        let file = SharedStateFile(guest: false)
        let client = try SharedStateFile.locked { () throws -> YoClient? in
          guard let key = StateKey.shared(), let blob = file.read() else { return nil }
          return try YoClient.restore(transport: transport, store: file, stateKey: key, blob: blob)
        }
        client?.shareState(lock: file)
        return client
      }
      guard let client = opened, self.client == nil, account != nil else { return }  // signed out meanwhile
      self.client = client
      // The user id as the core has it (an earlier build read it wrongly): the passkey's handle and the backup's id.
      if let id = Hub.bytes(client.userId()), account?.userID != id { account?.userID = id }
      // Into the iCloud Keychain if not there (accounts from before it, one signed out on another phone of the
      // person's). Only then: opening asks nothing of the hub, so this phone may have been replaced meanwhile, and the
      // item of the phone that has the account now must not be overwritten by an older one.
      if let account { SyncedAccount.save(account, onlyIfMissing: true) }
      Task { await addAuthKeys() }
    } catch YoError.Replaced {
      reset()
    } catch {}
  }

  /// Registers `username` at the hub (spec 6.4), or, with `secret`, takes the name's account back (a new phone). One at
  /// a time, whichever way in (a second registration would replace the first).
  private func register(_ username: String, secret: Data?) async throws -> YoClient {
    // One at a time, and never over an account signed in meanwhile (another way in finished first).
    guard !registering, account == nil else { throw AccountError.busy }
    registering = true
    defer { registering = false }
    let transport = Hub.transport
    do {
      return try await Hub.run {
        let make = { (store: StateStore, key: Data) in
          try YoClient.registerUsername(
            transport: transport, store: store, stateKey: key, username: username, accountSecret: secret,
            name: nil, pushPlatform: 0, pushToken: "")
        }
        guard SharedStateFile.app else { return try make(StateFile(), StateKey.load()) }
        let file = SharedStateFile(guest: false)
        let client = try SharedStateFile.locked {
          guard let key = StateKey.shared() else { throw YoError.State }
          _ = file.read()  // the state there now (none, or a signed-out one's): this one's to replace
          return try make(file, key)
        }
        client.shareState(lock: file)
        return client
      }
    } catch YoError.Refused(let status) where status == 409 {
      throw AccountError.taken
    } catch YoError.Refused(let status) where status == 429 {
      throw AccountError.tooMany
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
  }

  /// A new account (spec 6.4): the name is yours at once, the backup comes later (`createPasskey`, `keepWords`). Its
  /// key is made here and kept until the hub answered (`NewAccountKey`): trying again after a lost answer keeps the
  /// name.
  func register(_ username: String) async throws {
    let client: YoClient
    do {
      client = try await register(username, secret: NewAccountKey.secret(for: username))
    } catch AccountError.taken {
      NewAccountKey.forget(username)
      throw AccountError.taken
    }
    NewAccountKey.forget(username)
    guard let id = Hub.bytes(client.userId()) else { throw AccountError.noBackup }
    self.client = client
    friendsSeeded = true
    SyncedAccount.left = nil
    account = Account(
      username: client.username() ?? username, userID: id, accountKey: client.accountSecret(), byPasskey: false)
    SecureReminder.update(secured: false)
    wordsSaved = false
    passkeyTurnedOff = false
  }

  // MARK: Securing the name (spec 7)

  /// The guide's passkey, in two steps the person sees: made (saved in the password manager)…
  func createPasskey() async throws {
    guard let account, let client else { throw AccountError.noBackup }
    if !account.secured, securing == nil { securing = (BackupRecord.newKey(), RecoveryCode.make()) }
    let made = try await passkeys.make(name: Self.passkeyName(account), userID: account.userID).credentialID
    try still(client)
    newPasskey = made
  }

  /// After waiting for the person or the hub: still the account the step began with (not signed out, or out and in
  /// again meanwhile), else the step ends here.
  private func still(_ client: YoClient) throws {
    guard client === self.client else { throw CancellationError() }
  }

  /// …then signed in with once, which checks it works and gives its PRF output: the backup then opens with it too
  /// (a new backup if there was none, else added to the one the words open).
  func checkPasskey() async throws {
    guard let credential = newPasskey, let client else { throw AccountError.noBackup }
    guard let prf = try await passkeys.ask(salt: BackupRecord.prfSalt, only: credential).prf else {
      throw AccountError.noPRF
    }
    try still(client)
    if account?.secured == true {
      try await rewrap(.set(prf))
    } else {
      try await saveBackup(prf: prf)
    }
    newPasskey = nil
  }

  /// The words, to show when the passkey did not work.
  func secureWords() -> [String] {
    if securing == nil { securing = (BackupRecord.newKey(), RecoveryCode.make()) }
    return securing?.code ?? []
  }

  /// The words kept: the backup opens with them.
  func keepWords() async throws { try await saveBackup(prf: nil) }

  /// The first backup: K under the words always (passkey and words are equal doors), under the passkey's PRF too;
  /// both doors' auth keys.
  private func saveBackup(prf: Data?) async throws {
    guard var account, let securing, let client else { throw AccountError.noBackup }
    account.byPasskey = prf != nil
    account.byCode = true
    account.codeAuth = true
    account.passkeyAuth = prf != nil
    try await put(account, code: securing.code, k: securing.k, passkey: prf.map { .new(prf: $0) } ?? .none, by: client)
    RecoveryWords.save(securing.code)
    self.account = account
    self.securing = nil
    if prf != nil { passkeyTurnedOff = false }
    SecureReminder.update(secured: true)
  }

  /// The backup to the hub through `client`, the one the change began with: never another account's after a sign-out
  /// and sign-in meanwhile. The core seals the account and the words under K and wraps K (spec 7).
  private func put(_ account: Account, code: [String], k: Data, passkey: YoPasskeyDoor, by client: YoClient)
    async throws
  {
    try still(client)
    let sealed = try JSONEncoder().encode(BackupRecord.Sealed(account: account, code: code))
    do {
      try await Hub.run { try client.putBackup(k: k, sealed: sealed, words: code, passkey: passkey) }
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
    try still(client)
  }

  /// A core call for this account, off the main actor; the core's error as an `AccountError`.
  private func hub<T: Sendable>(_ work: @escaping @Sendable (YoClient) throws -> T) async throws -> T {
    guard let client else { throw AccountError.noBackup }
    do {
      return try await Hub.run { try work(client) }
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
  }

  /// What a rewrap does with the passkey's door: keeps it as it is, sets it from a new passkey's PRF output, removes
  /// it, or adds its auth key from the PRF output of the passkey there (`confirmPasskey`).
  private enum Door { case keep, set(Data), remove, confirm(Data) }

  /// The backup again with the same K (opened by the words on this phone): its passkey door kept, set, removed or
  /// confirmed, and optionally under new words (the old locator then finds nothing). Always with the doors' auth keys.
  private func rewrap(_ passkey: Door, code new: [String]? = nil) async throws {
    guard var account, let code = RecoveryWords.load(), let client else { throw AccountError.noBackup }
    let opened = try await fetchBackup(by: .words(words: code), userID: account.userID)
    let record = opened.record
    let words = new ?? code
    let door: YoPasskeyDoor =
      switch passkey {
      case .keep: .keep(record: record)
      case .set(let prf): .new(prf: prf)
      case .remove: .none
      case .confirm(let prf): .confirm(record: record, prf: prf)
      }
    switch passkey {
    case .keep:
      account.byPasskey = !record.byPasskey.isEmpty
      account.passkeyAuth = !record.authPasskey.isEmpty
    case .set, .confirm:
      account.byPasskey = true
      account.passkeyAuth = true
    case .remove:
      account.byPasskey = false
      account.passkeyAuth = false
    }
    account.byCode = true
    account.codeAuth = true
    try await put(account, code: words, k: opened.k, passkey: door, by: client)
    RecoveryWords.save(words)
    self.account = account
    if case .set = passkey { passkeyTurnedOff = false }
    if case .remove = passkey { passkeyTurnedOff = true }
  }

  /// A record from before the doors' auth keys (spec 7), written again with them, once, by the words on this phone
  /// (no step for the person); kept as it is, also the passkey's door. The passkey's auth key waits for the passkey
  /// (`confirmPasskey`). Tried again at the next start until it went.
  private func addAuthKeys() async {
    guard let account, account.secured, account.codeAuth != true, RecoveryWords.load() != nil else { return }
    try? await rewrap(.keep)
  }

  /// The passkey asked once (Face ID) for a record rewritten from before the auth keys: its PRF output gives the key
  /// that signs the record's fetch, so a new phone finds it by the passkey again. It must open the record's K.
  func confirmPasskey() async throws {
    guard let account, let client else { throw AccountError.noBackup }
    let answer = try await passkeys.ask(salt: BackupRecord.prfSalt)
    guard answer.userID == account.userID, let prf = answer.prf else { throw AccountError.noPRF }
    try still(client)
    try await rewrap(.confirm(prf))
  }

  /// A new passkey in place of the one there (made, then used once for its PRF output).
  func replacePasskey() async throws {
    guard let account, let client else { throw AccountError.noBackup }
    let made = try await passkeys.make(name: Self.passkeyName(account), userID: account.userID)
    guard let prf = try await passkeys.ask(salt: BackupRecord.prfSalt, only: made.credentialID).prf else {
      throw AccountError.noPRF
    }
    try still(client)
    try await rewrap(.set(prf))
  }

  /// The passkey no longer opens the backup; the words still do. (The passkey itself is deleted in the password
  /// manager.)
  func removePasskey() async throws { try await rewrap(.remove) }

  /// New recovery words; the old ones stop working (the hub forgets their locator).
  func newRecoveryKey() async throws -> [String] {
    let code = RecoveryCode.make()
    try await rewrap(.keep, code: code)
    wordsSaved = false
    return code
  }

  // MARK: A new phone

  /// Any Yo passkey of this person, asked once; its user handle says whose backup to open.
  func restoreWithPasskey(quietly: Bool = false) async throws {
    let answer = try await passkeys.ask(salt: BackupRecord.prfSalt, quietly: quietly)
    guard let prf = answer.prf else { throw AccountError.noPRF }
    try await restore(fetchBackup(by: .passkey(prf: prf), userID: answer.userID))
  }

  /// By the recovery words alone: they say where the backup is and open it.
  func restore(code: [String]) async throws {
    try await restore(fetchBackup(by: .words(words: code), userID: nil))
    wordsSaved = true  // typed in: the person has them
  }

  /// The backup a door opens, fetched with its auth key (the core derives, signs and opens): by the user id when
  /// given (a passkey's user handle), else by the words' locator. None it opens: `noBackup` (the hub tells no more).
  private func fetchBackup(by door: YoDoor, userID: Data?) async throws -> YoOpenedBackup {
    let transport = Hub.transport
    do {
      guard let opened = try await Hub.run({ try openBackup(transport: transport, userId: userID, door: door) }) else {
        throw AccountError.noBackup
      }
      return opened
    } catch YoError.Refused(let status) where status == 429 {
      throw AccountError.tooMany
    } catch let error as YoError {
      throw AccountError.hub(error)
    }
  }

  /// The account from its backup, and this phone registered for it with the account's key (spec 6.4).
  private func restore(_ opened: YoOpenedBackup) async throws {
    let sealed = try JSONDecoder().decode(BackupRecord.Sealed.self, from: opened.sealed)
    client = try await register(sealed.account.username, secret: sealed.account.accountKey)
    if let code = sealed.code { RecoveryWords.save(code) }
    SyncedAccount.left = nil
    restored = true
    friendsSeeded = true
    account = sealed.account
    SecureReminder.update(secured: true)
    Task { await addAuthKeys() }
  }

  /// Back without a word, from the iCloud Keychain (`SyncedAccount`): the app deleted and installed again, or a new
  /// iPhone. Registers this phone with the account's key (spec 6.4), so the phone it was on before is signed out, as
  /// with a backup. Only when exactly one account is there not signed out, and not one this phone left (taken to
  /// another phone); several are listed on Sign In instead. True when back in.
  func restoreFromKeychain() async -> Bool {
    let candidates = SyncedAccount.all().filter { $0.signedOut != true && $0.userID != SyncedAccount.left }
    guard candidates.count == 1 else { return false }
    return (try? await restore(synced: candidates[0])) == true
  }

  /// An account tapped on Sign In, from the iCloud Keychain: signed in with its key, also one signed out before.
  @discardableResult
  func restore(synced item: SyncedAccount.Item) async throws -> Bool {
    guard account == nil else { return false }
    let client: YoClient
    do {
      client = try await register(item.username, secret: item.accountKey)
    } catch AccountError.taken {
      SyncedAccount.delete(item.userID)  // the name has another key now: this item brings nothing back any more
      throw AccountError.noBackup
    }
    guard account == nil, let id = Hub.bytes(client.userId()) else { return false }
    SyncedAccount.left = nil
    self.client = client
    // The same user, or (its name released after a year away, spec 6.4) a new one with the same key and no backup.
    let same = id == item.userID
    if same, let words = item.words { RecoveryWords.save(words) }
    let restored = Account(
      username: client.username() ?? item.username, userID: id, accountKey: item.accountKey,
      byPasskey: same && item.byPasskey, byCode: same && item.byCode == true)
    if !same { SyncedAccount.delete(item.userID) }  // the old user is gone; the new one gets its own item
    self.restored = same
    friendsSeeded = true
    account = restored
    SecureReminder.update(secured: restored.secured)
    Task { await addAuthKeys() }
    return true
  }

  // MARK: The account's storage (spec 6.9)

  /// Friends known by user id, to the account's storage (`Friends.stored`): changed ones set, removed ones taken off.
  /// The core keeps them at once and writes them to the hub with its next sync.
  func store(_ changed: [Friend], removed: [String]) {
    // A friend's name at most 64 bytes in the storage: a longer one (many emoji) is kept by user id alone.
    let set = changed.compactMap { friend in
      friend.user.map { ($0, friend.name.utf8.count <= 64 ? friend.name : nil, UInt8(friend.color % Color.rows.count)) }
    }
    toStorage { client in
      for (user, name, color) in set { try? client.setFriend(user: user, name: name, color: color) }
      for user in removed { try? client.removeFriend(user: user) }
    }
  }

  /// The app icon chosen in Appearance through `client`, for the account's next phone: not if another account is in
  /// since.
  func storeAppIcon(_ icon: String?, by client: YoClient?) {
    guard client === self.client else { return }
    toStorage { $0.setAppIcon(icon: icon) }
  }

  /// A write to this account's storage, after the ones before it.
  private func toStorage(_ write: @escaping @Sendable (YoClient) -> Void) {
    guard let client else { return }
    let before = storing
    storing = Task {
      await before?.value
      _ = try? await Hub.run { write(client) }
    }
  }

  /// The storage as the core has it, into the app: when the core opened, and after the first sync (`read`: the core
  /// has read it by then, if registering could not). Its friends into the list (`Friends.take`; this phone's own go
  /// up once, for an account from before the storage); after a restore, its app icon. An icon is never written back
  /// from here: only Appearance changes it.
  func takeStorage(into friends: Friends, read: Bool = false) async {
    guard let client else { return }
    await storing?.value  // this phone's own writes first
    guard let (kept, icon) = try? await Hub.run({ (client.friends(), client.appIcon()) }), client === self.client
    else { return }
    friends.take(kept, seed: !friendsSeeded)
    friendsSeeded = true
    guard restored, read || !kept.isEmpty || icon != nil else { return }  // maybe not read yet: after the first sync
    restored = false
    if icon != UIApplication.shared.alternateIconName {
      try? await UIApplication.shared.setAlternateIconName(icon)
    }
  }

  // MARK: Blocking, reporting, deleting

  /// Blocks a user (spec 4.8), by user id or name: nothing from them is shown, nothing goes to them. Or unblocks.
  func setBlocked(_ who: String, _ blocked: Bool) async throws {
    try await hub { try $0.setBlocked(who: who, blocked: blocked) }
  }

  /// Reports a user to Yo (spec 6.4): who and why, nothing of what they sent.
  func report(_ who: String, reason: ReportReason) async throws {
    try await hub { try $0.report(who: who, reason: reason) }
  }

  /// Deletes the account at the hub with everything it holds (spec 6.4); the name is free again. The password manager
  /// is told that the account's passkeys are no longer valid (one that does not listen keeps it: deleted by hand).
  /// Then this phone forgets it (`reset`, by the caller with the rest of the app's data).
  func deleteAccount() async throws {
    guard let account else { throw AccountError.noBackup }
    try await hub { try $0.deleteAccount() }
    SyncedAccount.delete(account.userID)
    try? await ASCredentialUpdater().reportAllAcceptedPublicKeyCredentials(
      relyingPartyIdentifier: Passkeys.domain, userHandle: account.userID, acceptedCredentialIDs: [])
  }

  /// "Sign Out" (and after deleting the account): marked signed out in the iCloud Keychain, so no start brings the
  /// account back by itself, here or on the person's other devices (Sign In lists it); then forgotten here (`reset`).
  func signOut() {
    if let id = account?.userID { SyncedAccount.markSignedOut(id) }
    reset()
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
  func reset() {
    if let id = account?.userID { SyncedAccount.left = id }
    account = nil
    client = nil
    restored = false
    storing = nil
    friendsSeeded = false
    securing = nil
    newPasskey = nil
    passkeyTurnedOff = false
    wordsSaved = true
    StateFile.delete()
    // The group's too, under the lock, off the main thread (an extension's step under way ends first): the state as
    // it is now, not one a new sign-in saves meanwhile. Clients still on it are closed by their next step (the core's
    // `StateTurn.gone`).
    if SharedStateFile.app {
      let word = SharedStateFile.word()
      Task.detached(priority: .userInitiated) { SharedStateFile.locked { SharedStateFile.delete(ifStill: word) } }
    }
    SecureReminder.update(secured: true)
    RecoveryWords.delete()
    Stats.reset()
    MediaStorage.clearAll()
  }
}

enum AccountError: LocalizedError {
  case taken, noBackup, noPRF, tooMany, busy, hub(YoError)

  var errorDescription: String? {
    switch self {
    case .taken: "That name is taken. Try another."
    case .noBackup: "We couldn’t find your account. Try another passkey or recovery key."
    case .noPRF: "This passkey can’t be used to sign in to Yo. Try a different password manager."
    case .tooMany: "Too many tries. Try again later."
    case .busy: "Signing in already. Wait a moment."
    case .hub(.Network): "Yo can’t reach its server. Check your connection and try again."
    case .hub(.Replaced): "Your account is in use on another iPhone."
    case .hub(let error): "Something went wrong. Try again. (\(error))"  // the core's reason, while in development
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

  /// The key kept for this name, else a new one, kept.
  static func secret(for name: String) -> Data {
    var item = query(name)
    item[kSecReturnData as String] = true
    var result: CFTypeRef?
    if SecItemCopyMatching(item as CFDictionary, &result) == errSecSuccess, let data = result as? Data,
      data.count == 32
    {
      return data
    }
    let secret = Passkeys.random(32)
    SecItemDelete(query(name) as CFDictionary)
    var new = query(name)
    new[kSecValueData as String] = secret
    new[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    SecItemAdd(new as CFDictionary, nil)
    return secret
  }

  /// Whether a key is kept for this name: a try whose answer was lost, so the name the hub calls taken may be this
  /// phone's own.
  static func has(_ name: String) -> Bool {
    SecItemCopyMatching(query(name) as CFDictionary, nil) == errSecSuccess
  }

  static func forget(_ name: String) { SecItemDelete(query(name) as CFDictionary) }
}

/// The recovery words on this phone, to look up in the profile: in the keychain, for this device only (not in a
/// backup, not synced), readable while the phone is unlocked.
enum RecoveryWords {
  private static var query: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.recovery-words",
      kSecAttrAccount as String: "words",
    ]
  }

  static func save(_ words: [String]) {
    delete()
    var item = query
    item[kSecValueData as String] = Data(words.joined(separator: " ").utf8)
    item[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
    SecItemAdd(item as CFDictionary, nil)
  }

  static func load() -> [String]? {
    var item = query
    item[kSecReturnData as String] = true
    var result: CFTypeRef?
    guard SecItemCopyMatching(item as CFDictionary, &result) == errSecSuccess, let data = result as? Data else {
      return nil
    }
    return String(decoding: data, as: UTF8.self).split(separator: " ").map(String.init)
  }

  static func delete() {
    SecItemDelete(query as CFDictionary)
  }
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
    /// How the backup opens, and its recovery words (they open nothing the key does not): so a phone brought back
    /// this way shows the same security as the one before, and can still change its passkey or words.
    var byPasskey: Bool
    var byCode: Bool?
    var words: [String]?
    /// Signed out (on any of the person's devices): only signed in again by a tap on Sign In.
    var signedOut: Bool?

    var id: Data { userID }
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
  static func save(_ account: Account, onlyIfMissing: Bool = false) {
    migrate()
    let previous = load(account.userID)
    if onlyIfMissing && previous != nil { return }
    // The words are readable only while the phone is unlocked (a launch in the background may come before): those
    // already in the item are kept then.
    let item = Item(
      username: account.username, userID: account.userID, accountKey: account.accountKey,
      byPasskey: account.byPasskey, byCode: account.byCode, words: RecoveryWords.load() ?? previous?.words,
      signedOut: false)
    write(item, previous: previous)
  }

  @discardableResult
  private static func write(_ item: Item, previous: Item?) -> Bool {
    guard item != previous else { return true }
    guard let data = try? JSONEncoder().encode(item) else { return false }
    let attributes: [String: Any] = [
      kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
      kSecAttrLabel as String: "\(Hub.isDevApp ? "Yo Dev" : "Yo") – \(item.username)",
    ]
    let updated = SecItemUpdate(query(item.userID) as CFDictionary, attributes as CFDictionary)
    guard updated == errSecItemNotFound else { return updated == errSecSuccess }
    return SecItemAdd(query(item.userID).merging(attributes) { $1 } as CFDictionary, nil) == errSecSuccess
  }

  static func load(_ userID: Data) -> Item? {
    var find = query(userID)
    find[kSecReturnData as String] = true
    find[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: CFTypeRef?
    guard SecItemCopyMatching(find as CFDictionary, &result) == errSecSuccess, let data = result as? Data else {
      return nil
    }
    return try? JSONDecoder().decode(Item.self, from: data)
  }

  /// Every account of this hub in the keychain, by name.
  static func all() -> [Item] {
    migrate()
    var find = base
    find[kSecReturnAttributes as String] = true
    find[kSecReturnData as String] = true
    find[kSecMatchLimit as String] = kSecMatchLimitAll
    var result: CFTypeRef?
    guard SecItemCopyMatching(find as CFDictionary, &result) == errSecSuccess, let found = result as? [[String: Any]]
    else { return [] }
    return found
      .filter { ($0[kSecAttrAccount as String] as? String)?.hasPrefix("\(host)/") == true }
      .compactMap { ($0[kSecValueData as String] as? Data).flatMap { try? JSONDecoder().decode(Item.self, from: $0) } }
      .sorted { $0.username.localizedCaseInsensitiveCompare($1.username) == .orderedAscending }
  }

  /// Signed out: kept, to be tapped on Sign In, never brought back silently.
  static func markSignedOut(_ userID: Data) {
    guard var item = load(userID), item.signedOut != true else { return }
    let previous = item
    item.signedOut = true
    write(item, previous: previous)
  }

  /// The account deleted, or its name gone to another key: the item brings nothing back any more.
  static func delete(_ userID: Data) {
    SecItemDelete(query(userID) as CFDictionary)
  }

  /// Builds before one item per account kept one per hub (its host as the item's account): moved to the account's own.
  private static func migrate() {
    let old = base.merging([kSecAttrAccount as String: host]) { $1 }
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

/// The reminder to secure the name (spec 6.4): a name not backed up is released after 365 days away from Yo, so 358
/// days after the last time Yo was opened, a notification says so. Set again each time Yo opens; gone once secured.
enum SecureReminder {
  static let id = "yo.secure"

  @MainActor static func update(secured: Bool) {
    let center = UNUserNotificationCenter.current()
    center.removePendingNotificationRequests(withIdentifiers: [id])
    guard !secured else { return }
    let content = UNMutableNotificationContent()
    content.title = "Secure Your Account"
    content.body = "Secure your account to keep your name. Otherwise it’s freed in 7 days."
    let trigger = UNTimeIntervalNotificationTrigger(timeInterval: 358 * 24 * 3600, repeats: false)
    center.add(UNNotificationRequest(identifier: id, content: content, trigger: trigger))
  }
}
