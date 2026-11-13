// Passkeys with the PRF extension (spec 7), for justyo.app: make one (named with the username, its user handle
// the user id) and ask one, getting back its PRF output on a salt. Used by the onboarding and the PRF test.
import AuthenticationServices
import CryptoKit

@MainActor
final class Passkeys: NSObject {
  /// The relying party: the domain whose apple-app-site-association names this app (YoApp/Entitlements.plist).
  static let domain = "justyo.app"

  /// What a passkey answered: which one, the user id it holds, and its PRF output (nil if it gave none).
  struct Answer {
    let credentialID: Data
    let userID: Data
    let prf: Data?
  }

  private var pending: CheckedContinuation<ASAuthorization, Error>?

  /// A new passkey for `name` (what the password manager shows) and `userID` (its user handle). Only asked whether it
  /// supports PRF: asking for an output here (`.inputValues`) makes 1Password fail to save the passkey at all. The
  /// output comes from `ask` afterwards, whatever is said here (spec 7).
  func make(name: String, userID: Data) async throws -> (credentialID: Data, prfSupported: Bool) {
    let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: Self.domain)
    let request = provider.createCredentialRegistrationRequest(challenge: Self.random(32), name: name, userID: userID)
    request.displayName = name
    request.prf = .checkForSupport
    guard let made = try await perform(request).credential as? ASAuthorizationPlatformPublicKeyCredentialRegistration
    else { throw PasskeyError.unexpected }
    return (made.credentialID, made.prf?.isSupported ?? false)
  }

  /// Asks a passkey (`only` that one, or any of this domain the person picks) for its PRF output on `salt`. `quietly`:
  /// only if one is on this phone already, and with no sheet at all when there is none (it fails at once).
  func ask(salt: Data, only: Data? = nil, quietly: Bool = false) async throws -> Answer {
    let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: Self.domain)
    let request = provider.createCredentialAssertionRequest(challenge: Self.random(32))
    if let only { request.allowedCredentials = [ASAuthorizationPlatformPublicKeyCredentialDescriptor(credentialID: only)] }
    request.prf = .inputValues(.init(saltInput1: salt))
    guard let asserted = try await perform(request, quietly: quietly).credential
      as? ASAuthorizationPlatformPublicKeyCredentialAssertion
    else { throw PasskeyError.unexpected }
    let prf = asserted.prf?.first.withUnsafeBytes { Data($0) }
    return Answer(credentialID: asserted.credentialID, userID: asserted.userID, prf: prf)
  }

  private func perform(_ request: ASAuthorizationRequest, quietly: Bool = false) async throws -> ASAuthorization {
    try await withCheckedThrowingContinuation { continuation in
      pending = continuation
      let controller = ASAuthorizationController(authorizationRequests: [request])
      controller.delegate = self
      controller.presentationContextProvider = self
      if quietly {
        controller.performRequests(options: .preferImmediatelyAvailableCredentials)
      } else {
        controller.performRequests()
      }
    }
  }

  nonisolated static func random(_ count: Int) -> Data {
    Data((0..<count).map { _ in UInt8.random(in: .min ... .max) })
  }
}

enum PasskeyError: Error {
  case unexpected
}

extension Passkeys: ASAuthorizationControllerDelegate, ASAuthorizationControllerPresentationContextProviding {
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
