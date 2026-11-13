// A test of passkeys with the PRF extension (spec 7), from the settings: make a passkey for hub.justyo.app, then ask
// it again and see that its PRF output is the same each time. The account backup will rest on exactly that.
// Only a fingerprint of the output is shown and kept, never the output itself.
import AuthenticationServices
import CryptoKit
import SwiftUI

@MainActor @Observable
final class PasskeyTest: NSObject {
  /// The relying party: the domain whose apple-app-site-association names this app (YoApp/Entitlements.plist).
  static let domain = "hub.justyo.app"

  private(set) var log: [String] = []
  private(set) var busy = false
  var hasPasskey: Bool { defaults.data(forKey: Key.credential) != nil }

  private enum Key {
    static let credential = "prfTest.credential"
    static let salt = "prfTest.salt"
    static let fingerprint = "prfTest.fingerprint"
  }

  private let defaults = UserDefaults.standard
  private var pending: CheckedContinuation<ASAuthorization, Error>?

  /// A new passkey; asks whether it can do PRF.
  func create() async {
    busy = true
    defer { busy = false }
    let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: Self.domain)
    let request = provider.createCredentialRegistrationRequest(
      challenge: Self.random(32), name: "Yo PRF test", userID: Self.random(16))
    request.prf = .checkForSupport
    do {
      let result = try await perform(request)
      guard let made = result.credential as? ASAuthorizationPlatformPublicKeyCredentialRegistration else {
        return note("Unexpected answer")
      }
      defaults.set(made.credentialID, forKey: Key.credential)
      defaults.set(Self.random(32), forKey: Key.salt)
      defaults.removeObject(forKey: Key.fingerprint)
      note("Passkey made · PRF \(made.prf?.isSupported == true ? "supported ✓" : "not supported ✗")")
    } catch {
      note("Make failed: \(error.localizedDescription)")
    }
  }

  /// Asks the passkey for its PRF output on the same salt; the first time keeps its fingerprint, every later time
  /// compares.
  func check() async {
    guard let credential = defaults.data(forKey: Key.credential), let salt = defaults.data(forKey: Key.salt) else {
      return note("Make a passkey first")
    }
    busy = true
    defer { busy = false }
    let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: Self.domain)
    let request = provider.createCredentialAssertionRequest(challenge: Self.random(32))
    request.allowedCredentials = [ASAuthorizationPlatformPublicKeyCredentialDescriptor(credentialID: credential)]
    request.prf = .inputValues(.init(saltInput1: salt))
    do {
      let result = try await perform(request)
      guard let assertion = result.credential as? ASAuthorizationPlatformPublicKeyCredentialAssertion,
        let output = assertion.prf?.first
      else { return note("No PRF output ✗") }
      let print = output.withUnsafeBytes { SHA256.hash(data: Data($0)) }.prefix(6)
        .map { String(format: "%02x", $0) }.joined()
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
    note("Forgotten here (the passkey stays in Passwords)")
  }

  private func note(_ line: String) {
    log.insert("\(Date.now.formatted(date: .omitted, time: .standard))  \(line)", at: 0)
  }

  private func perform(_ request: ASAuthorizationRequest) async throws -> ASAuthorization {
    try await withCheckedThrowingContinuation { continuation in
      pending = continuation
      let controller = ASAuthorizationController(authorizationRequests: [request])
      controller.delegate = self
      controller.presentationContextProvider = self
      controller.performRequests()
    }
  }

  private static func random(_ count: Int) -> Data {
    Data((0..<count).map { _ in UInt8.random(in: .min ... .max) })
  }
}

extension PasskeyTest: ASAuthorizationControllerDelegate, ASAuthorizationControllerPresentationContextProviding {
  func authorizationController(controller: ASAuthorizationController, didCompleteWithAuthorization authorization: ASAuthorization) {
    pending?.resume(returning: authorization)
    pending = nil
  }

  func authorizationController(controller: ASAuthorizationController, didCompleteWithError error: Error) {
    pending?.resume(throwing: error)
    pending = nil
  }

  func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let windows = scenes.flatMap(\.windows)
    if let window = windows.first(where: \.isKeyWindow) ?? windows.first { return window }
    guard let scene = scenes.first else { preconditionFailure("a passkey sheet with no window to show it in") }
    return ASPresentationAnchor(windowScene: scene)
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
        "Makes a passkey for \(PasskeyTest.domain) and checks that its PRF output is the same every time: the account "
          + "backup will rely on it. Check twice; the second time should say \"the same\"."
      )
    }
    .disabled(test.busy)
  }
}
