// Passkeys with the PRF extension (spec 7) for justyo.app: make one, or ask one for its PRF output on a salt.
import AuthenticationServices
import CryptoKit

@MainActor
final class Passkeys: NSObject {
  /// The relying party: the domain whose apple-app-site-association names this app (YoApp/Entitlements.plist).
  static let domain = "justyo.app"

  struct Answer {
    let credentialID: Data
    let userID: Data
    let prf: Data?
  }

  /// The request under way and who waits for it. A new one cancels it (its caller gets a cancellation), so an
  /// answer never reaches the wrong caller and no caller waits forever.
  private var pending: (controller: ASAuthorizationController, continuation: CheckedContinuation<ASAuthorization, Error>)?

  /// A new passkey; its credential id. Only checks PRF support: asking for an output here (`.inputValues`) makes
  /// 1Password fail to save the passkey. The output comes from `ask` afterwards (spec 7).
  func make(name: String, userID: Data) async throws -> Data {
    let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: Self.domain)
    let request = provider.createCredentialRegistrationRequest(challenge: Self.random(32), name: name, userID: userID)
    request.displayName = name
    request.prf = .checkForSupport
    guard let made = try await perform(request).credential as? ASAuthorizationPlatformPublicKeyCredentialRegistration
    else { throw Diagnostics.failure(PasskeyError.unexpected) }
    return made.credentialID
  }

  /// Asks a passkey (`only` that one, or any of this domain the person picks) for its PRF output on `salt`.
  func ask(salt: Data, only: Data? = nil) async throws -> Answer {
    let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: Self.domain)
    let request = provider.createCredentialAssertionRequest(challenge: Self.random(32))
    if let only { request.allowedCredentials = [ASAuthorizationPlatformPublicKeyCredentialDescriptor(credentialID: only)] }
    request.prf = .inputValues(.init(saltInput1: salt))
    guard let asserted = try await perform(request).credential
      as? ASAuthorizationPlatformPublicKeyCredentialAssertion
    else { throw Diagnostics.failure(PasskeyError.unexpected) }
    let prf = asserted.prf?.first.withUnsafeBytes { Data($0) }
    return Answer(credentialID: asserted.credentialID, userID: asserted.userID, prf: prf)
  }

  private func perform(_ request: ASAuthorizationRequest) async throws -> ASAuthorization {
    if let old = pending {
      pending = nil
      old.controller.cancel()
      old.continuation.resume(throwing: CancellationError())
    }
    return try await withCheckedThrowingContinuation { continuation in
      let controller = ASAuthorizationController(authorizationRequests: [request])
      pending = (controller, continuation)
      controller.delegate = self
      controller.presentationContextProvider = self
      controller.performRequests()
    }
  }

  nonisolated static func random(_ count: Int) -> Data {
    Data((0..<count).map { _ in UInt8.random(in: .min ... .max) })
  }

  /// Cancelled by the person in the system sheet, or by a newer request.
  nonisolated static func isCancellation(_ error: Error) -> Bool {
    if error is CancellationError { return true }
    let error = error as NSError
    return error.domain == ASAuthorizationError.errorDomain && error.code == ASAuthorizationError.Code.canceled.rawValue
  }
}

enum PasskeyError: Error {
  case unexpected
}

extension Passkeys: ASAuthorizationControllerDelegate, ASAuthorizationControllerPresentationContextProviding {
  func authorizationController(controller: ASAuthorizationController, didCompleteWithAuthorization authorization: ASAuthorization) {
    finish(controller) { $0.resume(returning: authorization) }
  }

  func authorizationController(controller: ASAuthorizationController, didCompleteWithError error: Error) {
    let nsError = error as NSError
    if Self.isCancellation(error) {
      // The sheet closed by the person (or no passkey to offer): counted, not a problem.
      CountedErrors.record(domain: ASAuthorizationError.errorDomain, place: "Passkeys", code: "canceled", expected: true)
    } else if nsError.domain.contains("ASCABLE") {
      // A passkey from another device by its QR code: Bluetooth off, the phone out of reach or its side ended.
      CountedErrors.record(domain: nsError.domain, place: "Passkeys", code: String(nsError.code), expected: true)
    } else if nsError.domain == ASAuthorizationError.errorDomain, nsError.code == ASAuthorizationError.Code.failed.rawValue {
      // "Failed" says nothing alone: which step, and the error under it (domain and code only, never its text).
      let under = nsError.userInfo[NSUnderlyingErrorKey] as? NSError
      let making = controller.authorizationRequests.first is ASAuthorizationPlatformPublicKeyCredentialRegistrationRequest
      CountedErrors.record(domain: nsError.domain, place: making ? "Passkeys.make" : "Passkeys.ask",
        code: "1004" + (under.map { " \($0.domain) \($0.code)" } ?? ""))
    } else {
      Diagnostics.record(error)
    }
    finish(controller) { $0.resume(throwing: error) }
  }

  private func finish(_ controller: ASAuthorizationController, _ resume: (CheckedContinuation<ASAuthorization, Error>) -> Void) {
    guard let current = pending, current.controller === controller else { return }
    pending = nil
    resume(current.continuation)
  }

  func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let windows = scenes.flatMap(\.windows)
    if let window = windows.first(where: \.isKeyWindow) ?? windows.first { return window }
    guard let scene = scenes.first else { preconditionFailure("a passkey sheet with no window to show it in") }
    return ASPresentationAnchor(windowScene: scene)
  }
}
