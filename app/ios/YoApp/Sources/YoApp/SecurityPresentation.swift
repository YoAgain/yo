import AuthenticationServices
import Foundation

enum SecurityPresentation {
  static func isCancellation(_ error: Error) -> Bool {
    if error is CancellationError { return true }

    let error = error as NSError
    return error.domain == ASAuthorizationError.errorDomain
      && error.code == ASAuthorizationError.Code.canceled.rawValue
  }

  static func passkeyMessage(_ error: Error, fallback: String) -> String {
    if let error = error as? AccountError, case .tooMany = error {
      return "Too many tries. Try again later."
    }
    if let error = error as? AccountError, case .noPRF = error {
      return "Your password manager can’t do this. Try iCloud Keychain."
    }
    return fallback
  }
}
