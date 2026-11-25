// Invite links: https://justyo.co/NAME (universal link; justyo.co's association file sends one path part, not its own
// pages, to the app) and yo://NAME (the website's "Open in Yo", where a universal link does not fire) add NAME as a
// friend. The name is persisted until the list can add it, also
// through onboarding; a link to one's own name is dropped.
import Foundation
import Observation
import YoCore

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

  /// The name in yo://NAME or justyo.co/NAME, cleaned as a username; nil for other sites and justyo.co's own pages.
  nonisolated static func name(in url: URL) -> String? {
    let parts = url.pathComponents.filter { $0 != "/" }  // percent-decoded
    let host = url.host()?.lowercased()
    if Connect.link(url) { return nil }
    let raw: String?
    switch url.scheme?.lowercased() {
    case "yo": raw = url.host(percentEncoded: false)
    case "https", "http": raw = host == "justyo.co" || host == "www.justyo.co" ? parts.first : nil
    default: raw = nil
    }
    guard let raw, raw.lowercased() != "privacy" else { return nil }
    let name = Username.clean(raw)
    return usernameValid(name: name) ? name : nil  // "1abc" is a name, "404" none
  }

  nonisolated static func link(for username: String) -> URL {
    let path = username.addingPercentEncoding(withAllowedCharacters: CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")) ?? ""
    return URL(string: "https://justyo.co/\(path)") ?? URL(string: "https://justyo.co")!
  }
}
