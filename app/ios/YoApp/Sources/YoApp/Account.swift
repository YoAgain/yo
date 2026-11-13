// Your account (spec 3, 6.4, 7): a username, a user id, an account key, and the backup that brings them back on a
// new phone, opened by a passkey's PRF output or by a recovery code.
//
// The mockup has no hub yet: `MockHub` stands in for it on the phone, in a store of its own that "Reset Yo" does not
// touch, so a reset is a new phone and the restore can be tried for real (the cryptography is the real one).
import CryptoKit
import Foundation
import Observation

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
    if name.count < 3 { return "At least 3 letters" }
    if name.count > 16 { return "At most 16" }
    if let first = name.first, !first.isLetter { return "Starts with a letter" }
    if reserved.contains(name) { return "Taken" }
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

  /// The words as typed, if they are 12 known words with a matching checksum.
  static func parse(_ typed: String) -> [String]? {
    let given = typed.lowercased().split(whereSeparator: { !$0.isLetter }).map(String.init)
    guard given.count == 12 else { return nil }
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

  static func key(code: [String]) -> SymmetricKey {
    HKDF<SHA256>.deriveKey(inputKeyMaterial: SymmetricKey(data: Data(code.joined(separator: " ").utf8)),
      info: Data("yo backup v1 code".utf8), outputByteCount: 32)
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

  static func save(_ record: BackupRecord, for username: String) {
    store.set(try? JSONEncoder().encode(record), forKey: "name." + username)
    store.set(username, forKey: "user." + record.userID.base64EncodedString())
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
  /// A registration waiting for its recovery code to be confirmed.
  private var waiting: (account: Account, record: BackupRecord)?

  init() {
    account = UserDefaults.standard.data(forKey: Self.key).flatMap { try? JSONDecoder().decode(Account.self, from: $0) }
  }

  enum Started {
    /// Saved with the passkey: in.
    case done
    /// The passkey did not work: these words must be written down and confirmed (`confirm`).
    case code([String])
  }

  /// Registers `username` (spec 6.4) and backs it up (spec 7): a passkey named with it first; only if that gives no
  /// PRF output (no support, or cancelled), a recovery code.
  func register(_ username: String) async throws -> Started {
    guard !MockHub.isTaken(username) else { throw AccountError.taken }
    let userID = Passkeys.random(16)
    let accountKey = Curve25519.Signing.PrivateKey().rawRepresentation
    let k = SymmetricKey(size: .bits256)
    var account = Account(username: username, userID: userID, accountKey: accountKey, byPasskey: false)
    var record = BackupRecord(userID: userID, sealed: Data())

    if let credential = try? await passkeys.make(name: username, userID: userID),
      let prf = try? await passkeys.ask(salt: BackupRecord.prfSalt, only: credential).prf
    {
      record.byPasskey = try BackupRecord.wrap(k, in: BackupRecord.key(prf: prf))
      account.byPasskey = true
    }
    record.sealed = try ChaChaPoly.seal(try JSONEncoder().encode(account), using: k).combined
    if account.byPasskey {
      MockHub.save(record, for: username)
      self.account = account
      return .done
    }
    let code = RecoveryCode.make()
    record.byCode = try BackupRecord.wrap(k, in: BackupRecord.key(code: code))
    waiting = (account, record)
    return .code(code)
  }

  /// The recovery code was written down: the account is saved and in use.
  func confirm() {
    guard let waiting else { return }
    MockHub.save(waiting.record, for: waiting.account.username)
    account = waiting.account
    self.waiting = nil
  }

  /// A new phone: any Yo passkey of this person, asked once; its user handle says whose backup to open.
  func restoreWithPasskey() async throws {
    let answer = try await passkeys.ask(salt: BackupRecord.prfSalt)
    guard let username = MockHub.username(of: answer.userID), let record = MockHub.record(for: username),
      let wrapped = record.byPasskey, let prf = answer.prf
    else { throw AccountError.noBackup }
    try open(record, with: BackupRecord.unwrap(wrapped, with: BackupRecord.key(prf: prf)))
  }

  /// A new phone, by the name and its recovery code.
  func restore(username: String, code: [String]) throws {
    guard let record = MockHub.record(for: username), let wrapped = record.byCode else { throw AccountError.noBackup }
    guard let k = try? BackupRecord.unwrap(wrapped, with: BackupRecord.key(code: code)) else { throw AccountError.wrongCode }
    try open(record, with: k)
  }

  private func open(_ record: BackupRecord, with k: SymmetricKey) throws {
    let plain = try ChaChaPoly.open(ChaChaPoly.SealedBox(combined: record.sealed), using: k)
    account = try JSONDecoder().decode(Account.self, from: plain)
  }

  /// "Reset Yo": this phone forgets everything, as a new phone would (the simulated hub keeps the backups).
  func reset() {
    account = nil
    waiting = nil
  }
}

enum AccountError: LocalizedError {
  case taken, noBackup, wrongCode

  var errorDescription: String? {
    switch self {
    case .taken: "That name is taken"
    case .noBackup: "No Yo found for that"
    case .wrongCode: "These words don't open it"
    }
  }
}
