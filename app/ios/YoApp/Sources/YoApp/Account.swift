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

struct Account: Codable, Equatable {
  var username: String
  var userID: Data
  /// Ed25519, raw: signs the account's devices (spec 3.1).
  var accountKey: Data
  /// How the backup opens: by the passkey, or only by the recovery code.
  var byPasskey: Bool
}

enum Username {
  static let reserved: Set<String> = ["YO", "ADMIN", "YOAGAIN", "SUPPORT", "HELP", "ROOT", "JUSTYO"]

  /// What is wrong with a name, nil when it is fine (spec 3.3).
  static func problem(_ name: String) -> String? {
    if name.count < 3 { return "Use at least 3 characters." }
    if name.count > 16 { return "Use no more than 16 characters." }
    if let first = name.first, !first.isLetter { return "Start with a letter." }
    if reserved.contains(name) { return "That name is taken. Try another." }
    return nil
  }

  /// What may be typed: A–Z, 0–9 and _, in upper case, at most 16.
  static func clean(_ typed: String) -> String {
    String(typed.uppercased().filter { ("A"..."Z").contains($0) || ("0"..."9").contains($0) || $0 == "_" }.prefix(16))
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

  static func isTaken(_ username: String) -> Bool {
    store.data(forKey: "name." + username) != nil || Username.reserved.contains(username) || othersHave.contains(username)
  }

  static func save(_ record: BackupRecord, for username: String, locator: String? = nil) {
    store.set(try? JSONEncoder().encode(record), forKey: "name." + username)
    store.set(username, forKey: "user." + record.userID.base64EncodedString())
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
  /// A registration waiting for its recovery words to be kept.
  private var waiting: (account: Account, record: BackupRecord, code: [String])?

  init() {
    account = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode(Account.self, from: $0) }
  }

  /// Registers `username` (spec 6.4) and backs it up (spec 7). Always with a recovery key, the words returned to be
  /// kept (`confirm`); with the passkey too when it gives its PRF output as it is made (iCloud Keychain does; others,
  /// like 1Password, only when used: they can be added later from Settings, `addPasskey`).
  func register(_ username: String) async throws -> [String] {
    guard !MockHub.isTaken(username) else { throw AccountError.taken }
    let userID = Passkeys.random(16)
    var account = Account(
      username: username, userID: userID, accountKey: Curve25519.Signing.PrivateKey().rawRepresentation,
      byPasskey: false)
    let k = SymmetricKey(size: .bits256)
    let code = RecoveryCode.make()
    var record = BackupRecord(userID: userID, byCode: try BackupRecord.wrap(k, in: BackupRecord.key(code: code)),
      sealed: Data())
    if let made = try? await passkeys.make(name: username, userID: userID, salt: BackupRecord.prfSalt),
      let prf = made.prf {
      record.byPasskey = try BackupRecord.wrap(k, in: BackupRecord.key(prf: prf))
      account.byPasskey = true
    }
    record.sealed = try BackupRecord.seal(account, code: code, with: k)
    waiting = (account, record, code)
    return code
  }

  /// A passkey added to the backup afterwards: a Yo passkey of this account if there is one (the one made at
  /// registration, say, that gave no PRF output then), else a new one; K comes from the recovery words on this phone.
  func addPasskey() async throws {
    guard var account, let code = RecoveryWords.load(), var record = MockHub.record(for: account.username),
      let byCode = record.byCode
    else { throw AccountError.noBackup }
    let k = try BackupRecord.unwrap(byCode, with: BackupRecord.key(code: code))
    var prf: Data?
    if let answer = try? await passkeys.ask(salt: BackupRecord.prfSalt, quietly: true), answer.userID == account.userID {
      prf = answer.prf
    } else {
      let made = try await passkeys.make(name: account.username, userID: account.userID, salt: BackupRecord.prfSalt)
      if let given = made.prf { prf = given } else {
        prf = try await passkeys.ask(salt: BackupRecord.prfSalt, only: made.credentialID).prf
      }
    }
    guard let prf else { throw AccountError.noPRF }
    record.byPasskey = try BackupRecord.wrap(k, in: BackupRecord.key(prf: prf))
    account.byPasskey = true
    record.sealed = try BackupRecord.seal(account, code: code, with: k)
    MockHub.save(record, for: account.username)
    self.account = account
  }

  /// Past the recovery words: the account is saved and in use, and the words stay on this phone to look up later.
  func confirm() {
    guard let waiting else { return }
    RecoveryWords.save(waiting.code)
    MockHub.save(waiting.record, for: waiting.account.username, locator: BackupRecord.locator(code: waiting.code))
    account = waiting.account
    self.waiting = nil
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
    waiting = nil
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
