// yo://services/NAME/connect?code=CODE, from a service's connect page (justyo.co/services/NAME/connect): the app asks,
// then tells that service's account the code, end-to-end encrypted.
import Foundation
import Observation

@MainActor @Observable
final class Connect {
  static let shared = Connect()

  struct Request: Equatable, Identifiable {
    let service: String
    let code: String
    var id: String { code }
  }

  var pending: Request?

  func open(_ url: URL) { pending = Self.request(in: url) }

  nonisolated static func link(_ url: URL) -> Bool {
    url.scheme?.lowercased() == "yo" && url.host()?.lowercased() == "services"
  }

  nonisolated static func request(in url: URL) -> Request? {
    let parts = url.pathComponents.filter { $0 != "/" }
    guard link(url), parts.count == 2, parts[1] == "connect",
      let code = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == "code" })?.value,
      code.wholeMatch(of: /[A-Za-z0-9_-]{43}/) != nil,
      parts[0].wholeMatch(of: /[A-Za-z0-9._]{1,32}/) != nil
    else { return nil }
    return Request(service: parts[0].uppercased(), code: code)
  }
}
