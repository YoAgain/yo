// Invite links: https://justyo.co/NAME (a universal link; justyo.co's association file sends every path but "/" and
// "/privacy" to the app) and yo://add/NAME open Yo and add NAME as a friend. Your own link is in your code (QR) and in
// Invite on the list.
//
// The name waits here until the list can add it, kept on this phone so it outlives the app being closed: signed in at
// once (FriendList, as if typed in Add Friend); otherwise through the whole way in, once the account exists. A link to
// the name just registered or signed in with is dropped.
import Foundation
import Observation

@MainActor @Observable
final class Invite {
  static let shared = Invite()

  private static let key = "invite.pending"

  /// The name of the newest link opened and not added yet.
  var pending: String? = UserDefaults.standard.string(forKey: key) {
    didSet { UserDefaults.standard.set(pending, forKey: Self.key) }
  }

  /// A link opened: its name waits for the list.
  func open(_ url: URL) {
    if let name = Self.name(in: url) { pending = name }
  }

  /// Signed in as `username`: a link to oneself is not added.
  func signedIn(as username: String) {
    if let pending, Username.skeleton(pending) == Username.skeleton(username) { self.pending = nil }
  }

  /// The name a link names: the first part of its path (yo://add/NAME), as a name may be written; nil for none, for
  /// another site and for justyo.co's own pages.
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

  /// Your link, which adds you: https://justyo.co/NAME.
  nonisolated static func link(for username: String) -> URL {
    let path = username.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? ""
    return URL(string: "https://justyo.co/\(path)") ?? URL(string: "https://justyo.co")!
  }
}
