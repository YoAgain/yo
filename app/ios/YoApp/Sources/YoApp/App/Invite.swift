// Invite links: https://justyo.co/NAME (universal link; justyo.co's association file sends every path but "/" and
// "/privacy" to the app) and yo://add/NAME add NAME as a friend. The name is persisted until the list can add it, also
// through onboarding; a link to one's own name is dropped.
import Foundation
import Observation

@MainActor @Observable
final class Invite {
  static let shared = Invite()

  private static let key = "invite.pending"

  var pending: String? = UserDefaults.standard.string(forKey: key) {
    didSet { UserDefaults.standard.set(pending, forKey: Self.key) }
  }

  func open(_ url: URL) {
    if let name = Self.name(in: url) { pending = name }
  }

  func signedIn(as username: String) {
    if let pending, Username.skeleton(pending) == Username.skeleton(username) { self.pending = nil }
  }

  /// The first path component, cleaned as a username; nil for other sites and justyo.co's own pages.
  nonisolated static func name(in url: URL) -> String? {
    let parts = url.pathComponents.filter { $0 != "/" }  // percent-decoded
    let host = url.host()?.lowercased()
    let raw: String?
    switch url.scheme?.lowercased() {
    case "yo": raw = host == "add" ? parts.first : nil
    case "https", "http": raw = host == "justyo.co" || host == "www.justyo.co" ? parts.first : nil
    default: raw = nil
    }
    guard let raw, raw.lowercased() != "privacy" else { return nil }
    let name = Username.clean(raw)
    return name.isEmpty ? nil : name
  }

  nonisolated static func link(for username: String) -> URL {
    let path = username.addingPercentEncoding(withAllowedCharacters: CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")) ?? ""
    return URL(string: "https://justyo.co/\(path)") ?? URL(string: "https://justyo.co")!
  }
}
