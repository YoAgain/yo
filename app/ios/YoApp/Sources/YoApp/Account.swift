// Your account (spec 3, 6.4, 7): a username, a user id, an account key, and the backup that brings them back on a
// new phone, opened by a passkey's PRF output or by a recovery code.
//
// The mockup has no hub yet: `MockHub` stands in for it on the phone, in a store of its own that "Reset Yo" does not
// touch, so a reset is a new phone and the restore can be tried for real (the cryptography is the real one).
import CryptoKit
import Foundation
import Observation
import Photos
import Security
import UIKit
import UniformTypeIdentifiers
import UserNotifications

struct Account: Codable, Equatable {
  var username: String
  var userID: Data
  /// Ed25519, raw: signs the account's devices (spec 3.1).
  var accountKey: Data
  /// How the backup opens: by the passkey (and the words), or by the recovery words only.
  var byPasskey: Bool
  /// The recovery words open the backup (nil in accounts saved before this was kept: see `AccountStore.init`).
  var byCode: Bool? = false

  /// Backed up (spec 7): the name comes back on a new phone. Not yet, it is released after 30 days away (spec 6.4).
  var secured: Bool { byPasskey || byCode == true }
}

/// Usernames (spec 3.3): Latin letters (accents and umlauts too), digits, `_` and emoji; shown in upper case. Two
/// names that look alike are one name: they are compared by their skeleton.
enum Username {
  static let reserved: Set<String> = ["YO", "ADMIN", "YOAGAIN", "SUPPORT", "HELP", "ROOT", "JUSTYO"]

  /// What is wrong with a name, nil when it is fine.
  static func problem(_ name: String) -> String? {
    if name.count < 3 { return "Use at least 3 characters." }
    if name.count > 16 { return "Use no more than 16 characters." }
    if let first = name.first, !isLatinLetter(first), !isEmoji(first) { return "Start with a letter or emoji." }
    if isReserved(name) { return "That name is taken. Try another." }
    return nil
  }

  static func isReserved(_ name: String) -> Bool { reserved.contains(skeleton(name)) }

  /// What may be typed: the allowed characters, in upper case, a letter or emoji first, at most 16. So the field
  /// itself keeps the rules, and only "too short" and "taken" are left to show.
  static func clean(_ typed: String) -> String {
    let allowed = typed.uppercased().precomposedStringWithCanonicalMapping.filter {
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

/// The backup (spec 7): the account under a random key K, and K wrapped by the passkey and by the code.
struct BackupRecord: Codable {
  var userID: Data
  var byPasskey: Data?
  var byCode: Data?
  var sealed: Data

  /// The salt every Yo passkey is asked with: one for all, so a new phone asks once, before it knows who it is.
  static let prfSalt = Data(SHA256.hash(data: Data("yo backup v1".utf8)))

  static func key(prf: Data) -> SymmetricKey {
    HKDF<SHA256>.deriveKey(inputKeyMaterial: SymmetricKey(data: prf), info: Data("yo backup v1 passkey".utf8),
      outputByteCount: 32)
  }

  /// Where the backup lies, found from the words alone: no name needed (a name can change), not guessable (128 bits).
  static func locator(code: [String]) -> String {
    HKDF<SHA256>.deriveKey(inputKeyMaterial: SymmetricKey(data: Data(code.joined(separator: " ").utf8)),
      info: Data("yo backup v1 locator".utf8), outputByteCount: 16)
      .withUnsafeBytes { Data($0) }.base64EncodedString()
  }

  static func key(code: [String]) -> SymmetricKey {
    HKDF<SHA256>.deriveKey(inputKeyMaterial: SymmetricKey(data: Data(code.joined(separator: " ").utf8)),
      info: Data("yo backup v1 code".utf8), outputByteCount: 32)
  }

  /// What K seals: the account, and the recovery words, so a phone restored by passkey has them to show too.
  struct Sealed: Codable {
    var account: Account
    var code: [String]?
  }

  static func seal(_ account: Account, code: [String], with k: SymmetricKey) throws -> Data {
    try ChaChaPoly.seal(try JSONEncoder().encode(Sealed(account: account, code: code)), using: k).combined
  }

  static func open(_ sealed: Data, with k: SymmetricKey) throws -> Sealed {
    let plain = try ChaChaPoly.open(ChaChaPoly.SealedBox(combined: sealed), using: k)
    if let both = try? JSONDecoder().decode(Sealed.self, from: plain) { return both }
    return Sealed(account: try JSONDecoder().decode(Account.self, from: plain))  // older mock backups
  }

  static func wrap(_ k: SymmetricKey, in key: SymmetricKey) throws -> Data {
    try ChaChaPoly.seal(k.withUnsafeBytes { Data($0) }, using: key).combined
  }

  static func unwrap(_ wrapped: Data, with key: SymmetricKey) throws -> SymmetricKey {
    SymmetricKey(data: try ChaChaPoly.open(ChaChaPoly.SealedBox(combined: wrapped), using: key))
  }
}

/// The hub's part, simulated on the phone until the app talks to the hub: names taken, and backups by name and by
/// user id. Its own store, which a reset of the app leaves alone.
enum MockHub {
  private static var store: UserDefaults { UserDefaults(suiteName: "co.justyo.mockhub") ?? .standard }

  /// Names someone else has, as if they had registered first (to try what "taken" looks like).
  static let othersHave: Set<String> = ["LEO"]

  /// Taken: by its skeleton, so a name that only looks like one already there is taken too (spec 3.3).
  static func isTaken(_ username: String) -> Bool {
    let skeleton = Username.skeleton(username)
    return store.string(forKey: "skeleton." + skeleton) != nil || store.data(forKey: "name." + username) != nil
      || Username.isReserved(username) || othersHave.map(Username.skeleton).contains(skeleton)
  }

  static func save(_ record: BackupRecord, for username: String, locator: String? = nil) {
    store.set(try? JSONEncoder().encode(record), forKey: "name." + username)
    store.set(username, forKey: "user." + record.userID.base64EncodedString())
    store.set(username, forKey: "skeleton." + Username.skeleton(username))
    if let locator { store.set(username, forKey: "code." + locator) }
  }

  /// The backup the recovery words point to.
  static func username(at locator: String) -> String? {
    store.string(forKey: "code." + locator)
  }

  static func record(for username: String) -> BackupRecord? {
    store.data(forKey: "name." + username).flatMap { try? JSONDecoder().decode(BackupRecord.self, from: $0) }
  }

  static func username(of userID: Data) -> String? {
    store.string(forKey: "user." + userID.base64EncodedString())
  }

  static func forgetAll() {
    store.removePersistentDomain(forName: "co.justyo.mockhub")
  }
}

@MainActor @Observable
final class AccountStore {
  private(set) var account: Account? {
    didSet { UserDefaults.standard.set(try? JSONEncoder().encode(account), forKey: Self.key) }
  }

  private static let key = "account"
  private let passkeys = Passkeys()
  /// Securing under way: K and the words, made once per try; the passkey once made.
  private var securing: (k: SymmetricKey, code: [String], passkey: Data?)?

  init() {
    account = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode(Account.self, from: $0) }
    if account?.byCode == nil, RecoveryWords.load() != nil { account?.byCode = true }  // registered with the words
  }

  /// Registers `username` (spec 6.4): the name is yours at once, the backup comes later (`secure…`).
  func register(_ username: String) throws {
    guard !MockHub.isTaken(username) else { throw AccountError.taken }
    let userID = Passkeys.random(16)
    MockHub.save(BackupRecord(userID: userID, sealed: Data()), for: username)  // the name taken, nothing backed up
    account = Account(
      username: username, userID: userID, accountKey: Curve25519.Signing.PrivateKey().rawRepresentation,
      byPasskey: false)
    SecureReminder.update(secured: false)
  }

  // MARK: Securing the name (spec 7)

  /// A passkey for the name, made and then used, as its PRF output comes only when a passkey is used. Used right away
  /// when the provider says it supports PRF (iCloud Keychain: one more Face ID); else, or cancelled, the caller explains
  /// and calls `confirmPasskey`. True when the name is secured.
  func securePasskey() async throws -> Bool {
    guard let account else { throw AccountError.noBackup }
    if securing == nil { securing = (SymmetricKey(size: .bits256), RecoveryCode.make(), nil) }
    let made = try await passkeys.make(name: account.username, userID: account.userID)
    securing?.passkey = made.credentialID
    guard made.prfSupported else { return false }
    return (try? await confirmPasskey()) != nil
  }

  /// The passkey just made, used once: its PRF output opens the backup, as the words do.
  func confirmPasskey() async throws {
    guard let credential = securing?.passkey else { throw AccountError.noBackup }
    guard let prf = try await passkeys.ask(salt: BackupRecord.prfSalt, only: credential).prf else {
      throw AccountError.noPRF
    }
    try saveBackup(prf: prf)
  }

  /// The words, to show when the passkey did not work.
  func secureWords() -> [String] {
    if securing == nil { securing = (SymmetricKey(size: .bits256), RecoveryCode.make(), nil) }
    return securing?.code ?? []
  }

  /// The words kept: the backup opens with them.
  func keepWords() throws { try saveBackup(prf: nil) }

  /// The backup: K under the words always (passkey and words are equal doors), under the passkey's PRF output too.
  private func saveBackup(prf: Data?) throws {
    guard var account, let securing else { throw AccountError.noBackup }
    let k = securing.k
    var record = BackupRecord(
      userID: account.userID, byCode: try BackupRecord.wrap(k, in: BackupRecord.key(code: securing.code)),
      sealed: Data())
    if let prf {
      record.byPasskey = try BackupRecord.wrap(k, in: BackupRecord.key(prf: prf))
      account.byPasskey = true
    }
    account.byCode = true
    record.sealed = try BackupRecord.seal(account, code: securing.code, with: k)
    RecoveryWords.save(securing.code)
    MockHub.save(record, for: account.username, locator: BackupRecord.locator(code: securing.code))
    self.account = account
    self.securing = nil
    SecureReminder.update(secured: true)
  }

  /// A passkey added to a backup kept with the words: a Yo passkey of this account if there is one, else a new one;
  /// K comes from the recovery words on this phone.
  func addPasskey() async throws {
    guard var account, let code = RecoveryWords.load(), var record = MockHub.record(for: account.username),
      let byCode = record.byCode
    else { throw AccountError.noBackup }
    let k = try BackupRecord.unwrap(byCode, with: BackupRecord.key(code: code))
    var prf: Data?
    if let answer = try? await passkeys.ask(salt: BackupRecord.prfSalt, quietly: true), answer.userID == account.userID {
      prf = answer.prf
    } else {
      let made = try await passkeys.make(name: account.username, userID: account.userID)
      prf = try await passkeys.ask(salt: BackupRecord.prfSalt, only: made.credentialID).prf
    }
    guard let prf else { throw AccountError.noPRF }
    record.byPasskey = try BackupRecord.wrap(k, in: BackupRecord.key(prf: prf))
    account.byPasskey = true
    record.sealed = try BackupRecord.seal(account, code: code, with: k)
    MockHub.save(record, for: account.username)
    self.account = account
  }

  /// A new phone: any Yo passkey of this person, asked once; its user handle says whose backup to open.
  func restoreWithPasskey(quietly: Bool = false) async throws {
    let answer = try await passkeys.ask(salt: BackupRecord.prfSalt, quietly: quietly)
    guard let username = MockHub.username(of: answer.userID), let record = MockHub.record(for: username),
      let wrapped = record.byPasskey, let prf = answer.prf
    else { throw AccountError.noBackup }
    try open(record, with: BackupRecord.unwrap(wrapped, with: BackupRecord.key(prf: prf)))
  }

  /// A new phone, by the recovery words alone: they say where the backup is and open it.
  func restore(code: [String]) throws {
    guard let username = MockHub.username(at: BackupRecord.locator(code: code)),
      let record = MockHub.record(for: username), let wrapped = record.byCode
    else { throw AccountError.noBackup }
    guard let k = try? BackupRecord.unwrap(wrapped, with: BackupRecord.key(code: code)) else { throw AccountError.wrongCode }
    try open(record, with: k)
    RecoveryWords.save(code)
  }

  private func open(_ record: BackupRecord, with k: SymmetricKey) throws {
    let sealed = try BackupRecord.open(record.sealed, with: k)
    if let code = sealed.code { RecoveryWords.save(code) }
    account = sealed.account
  }

  /// "Reset Yo": this phone forgets everything, as a new phone would (the simulated hub keeps the backups).
  func reset() {
    account = nil
    securing = nil
    SecureReminder.update(secured: true)
    RecoveryWords.delete()
  }
}

enum AccountError: LocalizedError {
  case taken, noBackup, wrongCode, noPRF

  var errorDescription: String? {
    switch self {
    case .taken: "That name is taken. Try another."
    case .noBackup: "We couldn’t find your account. Try another passkey or recovery key."
    case .wrongCode: "That recovery key couldn’t restore your account. Check the words and try again."
    case .noPRF: "This passkey can’t be used to sign in to Yo. Try a different password manager."
    }
  }
}

/// The recovery words on this phone, to look up in the profile: in the keychain, for this device only (not in a
/// backup, not synced), readable while the phone is unlocked.
enum RecoveryWords {
  private static var query: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "co.justyo.recovery-words",
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

  /// To the clipboard, on this phone only (not to other devices) and for 10 minutes: enough to paste them into a note
  /// or a password manager, not to lie about.
  static func copy(_ words: [String], name: String) {
    let text = words.enumerated().map { "\($0.offset + 1). \($0.element)" }.joined(separator: "\n")
    UIPasteboard.general.setItems(
      [[UTType.plainText.identifier: "Yo \(name)\n\(text)"]],
      options: [.localOnly: true, .expirationDate: Date.now.addingTimeInterval(600)])
  }

  /// A picture into the photo library, asked once (add only). Not on the main actor: Photos runs the change on a
  /// queue of its own, and a block made on the main actor would trap there.
  nonisolated static func saveToPhotos(_ image: UIImage) async throws {
    let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
    guard status == .authorized || status == .limited else { throw PhotosError.notAllowed }
    try await PHPhotoLibrary.shared().performChanges { PHAssetChangeRequest.creationRequestForAsset(from: image) }
  }

  enum PhotosError: LocalizedError {
    case notAllowed
    var errorDescription: String? { "Yo can’t save to Photos. Copy your recovery key instead." }
  }
}

/// The reminder to secure the name (spec 6.4): a name not backed up is released after 30 days away from Yo, so 23
/// days after the last time Yo was opened, a notification says so. Set again each time Yo opens; gone once secured.
enum SecureReminder {
  static let id = "yo.secure"

  @MainActor static func update(secured: Bool) {
    let center = UNUserNotificationCenter.current()
    center.removePendingNotificationRequests(withIdentifiers: [id])
    guard !secured else { return }
    let content = UNMutableNotificationContent()
    content.title = "Secure Your Name"
    content.body = "Secure your name, or it becomes available to others in 7 days."
    let trigger = UNTimeIntervalNotificationTrigger(timeInterval: 23 * 24 * 3600, repeats: false)
    center.add(UNNotificationRequest(identifier: id, content: content, trigger: trigger))
  }
}
