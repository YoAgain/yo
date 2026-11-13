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
      let credential = try await passkeys.make(name: "Yo PRF test", userID: Passkeys.random(16))
      defaults.set(credential, forKey: Key.credential)
      defaults.set(Passkeys.random(32), forKey: Key.salt)
      defaults.removeObject(forKey: Key.fingerprint)
      note("Passkey made · now Check PRF")
    } catch {
      note("Make failed: \(error.localizedDescription)")
    }
  }

  /// The PRF output on the same salt: the first time its fingerprint is kept, every later time compared.
  func check() async {
    guard let credential = defaults.data(forKey: Key.credential), let salt = defaults.data(forKey: Key.salt) else {
      return note("Make a passkey first")
    }
    busy = true
    defer { busy = false }
    do {
      guard let output = try await passkeys.ask(salt: salt, only: credential).prf else {
        return note("No PRF output ✗")
      }
      let print = SHA256.hash(data: output).prefix(6).map { String(format: "%02x", $0) }.joined()
      switch defaults.string(forKey: Key.fingerprint) {
      case nil:
        defaults.set(print, forKey: Key.fingerprint)
        note("PRF output \(print) · kept to compare")
      case print?:
        note("PRF output \(print) · the same ✓")
      case let kept?:
        note("PRF output \(print) · was \(kept) ✗")
      }
    } catch {
      note("Check failed: \(error.localizedDescription)")
    }
  }

  func forget() {
    for key in [Key.credential, Key.salt, Key.fingerprint] { defaults.removeObject(forKey: key) }
    note("Forgotten here (the passkey stays in your password manager)")
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
      Button("Make Passkey", systemImage: "person.badge.key") { Task { await test.create() } }
      Button("Check PRF", systemImage: "checkmark.shield") { Task { await test.check() } }
        .disabled(!test.hasPasskey)
      if test.hasPasskey {
        Button("Forget Test Passkey", systemImage: "trash", role: .destructive) { test.forget() }
      }
      ForEach(Array(test.log.enumerated()), id: \.offset) { _, line in
        Text(line).font(.footnote.monospaced()).foregroundStyle(.secondary)
      }
    } header: {
      Text("Passkey PRF Test")
    } footer: {
      Text(
        "Makes a passkey for \(Passkeys.domain) and checks that its PRF output is the same every time. Check twice; "
          + "the second time should say \"the same\". What a password manager says about PRF when it makes the "
          + "passkey does not count."
      )
    }
    .disabled(test.busy)
  }
}
