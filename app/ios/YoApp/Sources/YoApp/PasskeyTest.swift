// A test of passkeys with the PRF extension (spec 7), from the settings: make a passkey, then ask it again and see
// that its PRF output is the same each time. Only a fingerprint of the output is shown and kept, never the output.
import CryptoKit
import SwiftUI

@MainActor @Observable
final class PasskeyTest {
  private(set) var log: [String] = []
  private(set) var busy = false
  var hasPasskey: Bool { defaults.data(forKey: Key.credential) != nil }

  private enum Key {
    static let credential = "prfTest.credential"
    static let salt = "prfTest.salt"
    static let fingerprint = "prfTest.fingerprint"
  }

  private let defaults = UserDefaults.standard
  private let passkeys = Passkeys()

  func create() async {
    busy = true
    defer { busy = false }
    do {
      let credential = try await passkeys.make(name: "Yo Passkey Test", userID: Passkeys.random(16))
      defaults.set(credential, forKey: Key.credential)
      defaults.set(Passkeys.random(32), forKey: Key.salt)
      defaults.removeObject(forKey: Key.fingerprint)
      note("Test passkey created. Choose Check Passkey to continue.")
    } catch {
      note("Couldn’t create the test passkey. Try again.")
    }
  }

  /// The PRF output on the same salt: the first time its fingerprint is kept, every later time compared.
  func check() async {
    guard let credential = defaults.data(forKey: Key.credential), let salt = defaults.data(forKey: Key.salt) else {
      return note("Create a test passkey first.")
    }
    busy = true
    defer { busy = false }
    do {
      guard let output = try await passkeys.ask(salt: salt, only: credential).prf else {
        return note("This passkey didn’t provide a test value. Try a different password manager.")
      }
      let print = SHA256.hash(data: output).prefix(6).map { String(format: "%02x", $0) }.joined()
      switch defaults.string(forKey: Key.fingerprint) {
      case nil:
        defaults.set(print, forKey: Key.fingerprint)
        note("Test value: \(print). Saved for comparison.")
      case print?:
        note("Test value: \(print). It matches the saved value.")
      case let kept?:
        note("Test value: \(print). It doesn’t match the saved value, \(kept).")
      }
    } catch {
      note("Couldn’t check the test passkey. Try again.")
    }
  }

  func forget() {
    for key in [Key.credential, Key.salt, Key.fingerprint] { defaults.removeObject(forKey: key) }
    note("Test data removed from this iPhone. The passkey is still in your password manager.")
  }

  private func note(_ line: String) {
    log.insert("\(Date.now.formatted(date: .omitted, time: .standard))  \(line)", at: 0)
  }
}

/// The settings' section for the test.
struct PasskeyTestSection: View {
  @State private var test = PasskeyTest()

  var body: some View {
    Section {
      Button("Create Test Passkey", systemImage: "person.badge.key") { Task { await test.create() } }
      Button("Check Passkey", systemImage: "checkmark.shield") { Task { await test.check() } }
        .disabled(!test.hasPasskey)
      if test.hasPasskey {
        Button("Remove Test Data", systemImage: "trash", role: .destructive) { test.forget() }
      }
      ForEach(Array(test.log.enumerated()), id: \.offset) { _, line in
        Text(line).font(.footnote.monospaced()).foregroundStyle(.secondary)
      }
    } header: {
      Text("Passkey Compatibility Test")
    } footer: {
      Text(
        "Creates a test passkey for \(Passkeys.domain). Choose Check Passkey twice to confirm that the test values "
          + "match.")
    }
    .disabled(test.busy)
  }
}
