// Connecting a service (skills/ifttt, spec 3.5): its page opens yo://connect?origin=…&ticket=… (a phone's "Open Yo") or
// https://justyo.co/connect/approve?t=… (a computer's QR code). Asked first, with the pairing code the page shows; on
// Allow the account key signs the ticket bound to the page's origin, the page gets the signature and goes back to the
// service itself. This app only says yes or no; it never opens the service's return link.
import CryptoKit
import Foundation
import Observation
import SwiftUI
import YoCore

@MainActor @Observable
final class Connect {
  static let shared = Connect()

  struct Request: Identifiable, Equatable {
    let origin: String
    let ticket: String
    var id: String { ticket }
    /// The service's bot on Yo, added as a friend on Allow.
    var service: String { "IFTTT" }
    /// The four digits the page shows: SHA-256("yo pairing " || ticket), its first 4 bytes, mod 10000.
    var code: String {
      let digest = SHA256.hash(data: Data(("yo pairing " + ticket).utf8))
      let number = digest.prefix(4).reduce(UInt32(0)) { $0 << 8 | UInt32($1) }
      return String(format: "%04u", number % 10000)
    }
  }

  var pending: Request?

  /// A ticket is base64url: ASCII letters, digits, "-" and "_".
  nonisolated static let ticketCharacters = CharacterSet(
    charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_")

  /// The pages that may ask: Yo's own site; Yo Dev also a local one (YO_CONNECT_ORIGIN, as YO_HUB).
  nonisolated static var origins: Set<String> {
    var origins: Set<String> = ["https://justyo.co"]
    if Hub.isDevApp, let local = ProcessInfo.processInfo.environment["YO_CONNECT_ORIGIN"] { origins.insert(local) }
    return origins
  }

  /// Takes a link if it is a connect request (also one this app refuses); false for any other link.
  func open(_ url: URL) -> Bool {
    guard Self.isConnect(url) else { return false }
    if let request = Self.request(in: url) { pending = request }
    return true
  }

  nonisolated static func isConnect(_ url: URL) -> Bool {
    switch url.scheme?.lowercased() {
    case "yo": return url.host()?.lowercased() == "connect"
    case "https", "http":
      let host = url.host()?.lowercased()
      return (host == "justyo.co" || host == "www.justyo.co") && url.pathComponents.dropFirst().first == "connect"
    default: return false
    }
  }

  nonisolated static func request(in url: URL) -> Request? {
    let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
    func value(_ name: String) -> String? { items.first { $0.name == name }?.value }
    let origin: String?
    let ticket: String?
    if url.scheme?.lowercased() == "yo" {
      origin = value("origin")
      ticket = value("ticket")
    } else {
      origin = url.scheme?.lowercased() == "https" ? "https://justyo.co" : nil
      ticket = url.pathComponents.dropFirst().dropFirst().first == "approve" ? value("t") : nil
    }
    guard let origin, origins.contains(origin), let ticket, (1...255).contains(ticket.utf8.count),
      ticket.unicodeScalars.allSatisfy({ ticketCharacters.contains($0) })
    else { return nil }
    return Request(origin: origin, ticket: ticket)
  }

  enum Outcome: Equatable { case connected, refused, expired, failed }

  /// Says yes (signed) or no to the page.
  nonisolated static func answer(_ request: Request, allow: Bool, client: YoClient) async -> Outcome {
    var body: [String: Any] = ["ticket": request.ticket, "allow": allow]
    if allow {
      guard let signature = try? await Hub.run({ try client.signConnect(origin: request.origin, ticket: request.ticket) })
      else { return .failed }
      body["user"] = client.userId()
      body["signature"] = signature.base64URL
    }
    guard let url = URL(string: request.origin + "/connect/approve"),
      let data = try? JSONSerialization.data(withJSONObject: body)
    else { return .failed }
    var post = URLRequest(url: url, timeoutInterval: 20)
    post.httpMethod = "POST"
    post.setValue("application/json", forHTTPHeaderField: "Content-Type")
    post.httpBody = data
    let session = URLSession(configuration: .ephemeral)
    defer { session.finishTasksAndInvalidate() }
    guard let (_, response) = try? await session.data(for: post), let http = response as? HTTPURLResponse else {
      return .failed
    }
    switch http.statusCode {
    case 200: return allow ? .connected : .refused
    case 410: return .expired
    default: return .failed
    }
  }
}

extension Data {
  /// base64url without padding.
  var base64URL: String {
    base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
      .replacingOccurrences(of: "=", with: "")
  }
}

/// Asks before a service is connected: who, what it may do, and the page's pairing code to check.
struct ConnectSheet: View {
  let request: Connect.Request
  @Environment(\.dismiss) private var dismiss
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @State private var working = false
  @State private var outcome: Connect.Outcome?

  var body: some View {
    NavigationStack {
      VStack(spacing: 0) {
        Spacer(minLength: 8)
        lockup
          .padding(.bottom, 28)
        Text(title)
          .font(.yo(30))
          .multilineTextAlignment(.center)
          .padding(.bottom, 10)
        Text(message)
          .font(.body)
          .foregroundStyle(.secondary)
          .multilineTextAlignment(.center)
          .fixedSize(horizontal: false, vertical: true)
        if outcome == nil {
          pairing
            .padding(.top, 28)
        }
        Spacer(minLength: 24)
        if outcome == nil {
          Button {
            answer(allow: true)
          } label: {
            Text("Allow").font(.yo(20)).frame(maxWidth: .infinity).padding(.vertical, 6)
          }
          .buttonStyle(.glassProminent)
          .tint(.amethyst)
          .controlSize(.large)
          .disabled(working || accounts.client == nil)
        } else {
          Button {
            dismiss()
          } label: {
            Text("Done").font(.yo(20)).frame(maxWidth: .infinity).padding(.vertical, 6)
          }
          .buttonStyle(.glassProminent)
          .tint(.amethyst)
          .controlSize(.large)
        }
      }
      .padding(.horizontal, 24)
      .padding(.bottom, 12)
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        if outcome == nil {
          ToolbarItem(placement: .cancellationAction) {
            Button("Cancel", role: .cancel) { answer(allow: false) }
              .disabled(working)
          }
        }
      }
      .overlay { if working { ProgressView().controlSize(.large) } }
      .interactiveDismissDisabled(working)
      .animation(.spring(duration: 0.45, bounce: 0.35), value: outcome)
      .sensoryFeedback(.success, trigger: outcome == .connected)
    }
    .presentationDetents([.large])
    .presentationCornerRadius(36)
  }

  /// Yo and the service side by side, joined while it asks, one "Yo!" once connected.
  private var lockup: some View {
    HStack(spacing: 14) {
      tile("Yo", color: .amethyst, size: outcome == .connected ? 46 : 40)
      Image(systemName: outcome == .connected ? "checkmark" : "plus")
        .font(.title2.bold())
        .foregroundStyle(outcome == .connected ? Color.amethyst : .secondary)
        .contentTransition(.symbolEffect(.replace))
      tile(request.service, color: Color(red: 0.17, green: 0.06, blue: 0.22), size: 22)
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("Yo and \(request.service)")
  }

  private func tile(_ text: String, color: Color, size: CGFloat) -> some View {
    Text(text)
      .font(.yo(size))
      .foregroundStyle(.white)
      .frame(width: 104, height: 104)
      .background(color, in: .rect(cornerRadius: 28))
      .shadow(color: color.opacity(0.35), radius: 16, y: 8)
  }

  /// The page's four digits, to match before saying yes.
  private var pairing: some View {
    VStack(spacing: 10) {
      Text("Pairing code")
        .font(.caption.weight(.semibold))
        .textCase(.uppercase)
        .foregroundStyle(.secondary)
      HStack(spacing: 8) {
        ForEach(Array(request.code.enumerated()), id: \.offset) { _, digit in
          Text(String(digit))
            .font(.yo(28))
            .frame(width: 48, height: 60)
            .background(Color.amethyst.opacity(0.12), in: .rect(cornerRadius: 14))
            .foregroundStyle(Color.amethyst)
        }
      }
      .accessibilityElement(children: .ignore)
      .accessibilityLabel("Pairing code \(request.code.map(String.init).joined(separator: " "))")
      Text("Allow only if \(request.service) shows this code on the page you just opened.")
        .font(.footnote)
        .foregroundStyle(.secondary)
        .multilineTextAlignment(.center)
    }
  }

  private var title: String {
    switch outcome {
    case .connected: "Yo! Connected"
    case .refused: "Not Connected"
    case .expired: "Link Expired"
    case .failed: "Couldn’t Connect"
    case nil: "Connect \(request.service)?"
    }
  }

  private var message: String {
    switch outcome {
    case .connected: "\(request.service) is on your list. Go back to \(request.service) to finish, then Yo it to run your Applets."
    case .refused: "\(request.service) was not connected."
    case .expired: "Start again from \(request.service)."
    case .failed: "Check your connection and try again."
    case nil: "\(request.service) will be able to Yo you and see the Yos you send it."
    }
  }

  private func answer(allow: Bool) {
    guard let client = accounts.client else { return }
    working = true
    Task {
      let result = await Connect.answer(request, allow: allow, client: client)
      working = false
      if result == .connected { friends.add(request.service) }
      if !allow { dismiss() } else { outcome = result }
    }
  }
}
