// Contacts in the profile: every friend, and for each whether its encryption is as it should be (spec 3, 4), one
// check per row as Settings shows a state: green when it is as expected, an empty circle for a step still open, orange
// for something that waits to sort itself out, red for something to act on. Below the checks, the plain values.
import SwiftUI
import YoCore

/// How one check stands.
enum Check {
  case ok, open, waiting, problem

  var symbol: String {
    switch self {
    case .ok: "checkmark.circle.fill"
    case .open: "circle"
    case .waiting: "clock.fill"
    case .problem: "exclamationmark.triangle.fill"
    }
  }

  var color: Color {
    switch self {
    case .ok: .green
    case .open: Color(.tertiaryLabel)
    case .waiting: .orange
    case .problem: .red
    }
  }
}

/// What the core knows of one contact, read off the main actor.
struct ContactState: Sendable {
  var contact: YoContact?
  var accountKey: String?
  var safetyNumber: String?
  /// A session with each of the contact's devices this iPhone writes to.
  var sessions: [YoSession] = []

  /// The sessions as one check: good to send to every device, a new one on its way, or none yet.
  var sessionsCheck: Check {
    if sessions.isEmpty { return .open }
    if sessions.contains(where: { $0.retrying || !$0.usable }) { return .waiting }
    return .ok
  }

  /// The worst of the checks that matter: what the list shows for the contact.
  func overall(_ friend: Friend) -> Check {
    if friend.gone || contact?.gone == true || contact?.unconfirmed == true { return .problem }
    if contact?.outdated == true { return .waiting }
    if friend.keyChanged { return .waiting }
    guard friend.user != nil else { return .open }
    return sessionsCheck
  }

  static func load(_ client: YoClient, user: String) -> ContactState {
    ContactState(
      contact: client.contacts().first { $0.user == user },
      accountKey: client.accountKeyOf(user: user),
      safetyNumber: try? client.safetyNumber(who: user),
      sessions: client.sessionsWith(user: user))
  }
}

/// Verifying a contact and confirming their new key (spec 3.4), through the core; then what waited for them goes (the
/// core's Yos and the photos in the outbox).
@MainActor
enum Verification {
  static func setVerified(
    _ user: String, _ verified: Bool, accounts: AccountStore, inbox: Inbox, friends: Friends, outbox: MediaOutbox
  ) async throws {
    guard let client = accounts.client else { return }
    try await Hub.run { try client.setVerified(who: user, verified: verified) }
    resume(accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
  }

  /// Their new key accepted: Yos to them go again (unverified, until the safety numbers are compared again).
  static func confirm(_ user: String, accounts: AccountStore, inbox: Inbox, friends: Friends, outbox: MediaOutbox)
    async throws
  {
    guard let client = accounts.client else { return }
    try await Hub.run { try client.confirmIdentity(who: user) }
    resume(accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
  }

  private static func resume(accounts: AccountStore, inbox: Inbox, friends: Friends, outbox: MediaOutbox) {
    inbox.resume(accounts: accounts, friends: friends)
    outbox.retryNow()
  }
}

/// A row with its state at the end: title, a word, the check.
private struct StatusRow: View {
  let title: String
  let value: String
  let check: Check

  var body: some View {
    LabeledContent {
      HStack(spacing: 6) {
        Text(value)
        Image(systemName: check.symbol).foregroundStyle(check.color).accessibilityHidden(true)
      }
    } label: {
      Text(title)
    }
  }
}

/// The friends, blocked ones too, each with its overall check, each opening its details.
struct ContactsList: View {
  @Environment(Friends.self) private var friends
  @Environment(AccountStore.self) private var accounts
  @State private var states: [String: ContactState] = [:]

  var body: some View {
    Form {
      if !friends.all.isEmpty {
        Section {
          ForEach(sorted) { friend in
            let state = friend.user.flatMap { states[$0] } ?? ContactState()
            let check = state.overall(friend)
            NavigationLink {
              ContactDetail(friend: friend)
            } label: {
              LabeledContent {
                HStack(spacing: 6) {
                  Text(summary(friend, check))
                  Image(systemName: check.symbol).foregroundStyle(check.color).accessibilityHidden(true)
                }
              } label: {
                Text(friend.name).font(.yo(17, relativeTo: .body)).lineLimit(1).minimumScaleFactor(0.5)
              }
            }
          }
        } footer: {
          Text("A green check: the chat is end-to-end encrypted, as it should be.")
        }
      }
    }
    .overlay {
      if friends.all.isEmpty { ContentUnavailableView("No Contacts", systemImage: "person.2") }
    }
    .navigationTitle("Contacts")
    .navigationBarTitleDisplayMode(.inline)
    // Again once the core is open (the page opened before), and when a contact changes (a chat, a new key).
    .task(id: Reload(accounts.client, friends.all)) { await load() }
  }

  private var sorted: [Friend] {
    friends.all.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
  }

  private func summary(_ friend: Friend, _ check: Check) -> String {
    if friend.blocked { return "Blocked" }
    if friend.gone { return "Not on Yo" }
    if friend.user == nil { return "No Chat Yet" }
    if states[friend.user ?? ""]?.contact?.outdated == true { return "Needs Update" }
    if friend.keyChanged && check == .waiting { return "New Key" }
    return switch check {
    case .ok: "Encrypted"
    case .open: "No Chat Yet"
    case .waiting: "Updating"
    case .problem: "Check"
    }
  }

  private func load() async {
    guard let client = accounts.client else { return }
    let users = friends.all.compactMap(\.user)
    let loaded = (try? await Hub.run {
      Dictionary(users.map { ($0, ContactState.load(client, user: $0)) }, uniquingKeysWith: { a, _ in a })
    }) ?? [:]
    if client === accounts.client { states = loaded }  // not an account signed out meanwhile
  }
}

/// One contact: the checks first, then who they are and their devices in plain values.
private struct ContactDetail: View {
  let friend: Friend
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @Environment(Inbox.self) private var inbox
  @Environment(MediaOutbox.self) private var outbox
  @State private var state: ContactState?
  @State private var problem: String?

  var body: some View {
    Form {
      if friend.user == nil {
        Section {
          StatusRow(title: "Chat", value: "Not Yet", check: .open)
        } footer: {
          Text("The first Yo starts an end-to-end encrypted chat.")
        }
      } else if let state {
        checks(state)
        if let user = friend.user, state.safetyNumber != nil { verification(state, user: user) }
        Section("Account") {
          plain("User ID", friend.user ?? "", mono: true)
          if let key = state.accountKey { plain("Account Key", key, mono: true) }
          if let number = state.safetyNumber { plain("Safety Number", grouped(number), mono: true) }
        }
        if !state.sessions.isEmpty {
          Section(state.sessions.count == 1 ? "Device" : "Devices") {
            ForEach(state.sessions, id: \.device) { session in
              plain("Device Key", session.device, mono: true)
            }
          }
        }
      }
    }
    .navigationTitle(friend.name)
    .navigationBarTitleDisplayMode(.inline)
    .task(id: Reload(accounts.client, friends.all.filter { $0.key == friend.key })) { await load() }
    .alert("Something Went Wrong", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(problem ?? "")
    }
  }

  /// Marking them verified once the safety numbers match; a verified contact's new key confirmed, so Yos go again.
  @ViewBuilder private func verification(_ state: ContactState, user: String) -> some View {
    let verified = state.contact?.verified == true
    Section {
      if state.contact?.unconfirmed == true {
        Button("Confirm New Key") {
          change { try await Verification.confirm(user, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox) }
        }
      }
      Button(verified ? "Mark as Not Verified" : "Mark as Verified") {
        change {
          try await Verification.setVerified(
            user, !verified, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
        }
      }
    } footer: {
      if state.contact?.unconfirmed == true {
        Text("Your Yos to \(friend.name) wait until you confirm. Compare safety numbers first.")
      } else if !verified {
        Text(
          "Compare the safety number below with \(friend.name)’s, in person or on a call. If it matches, mark "
            + "them as verified: if their key changes, Yo then holds your Yos until you confirm.")
      }
    }
  }

  /// A change through the core, then the page read again.
  private func change(_ work: @escaping @MainActor () async throws -> Void) {
    Task {
      do {
        try await work()
      } catch {
        problem = error.localizedDescription
      }
      await load()
    }
  }

  /// One row per thing that should hold, each with its check.
  @ViewBuilder private func checks(_ state: ContactState) -> some View {
    let contact = state.contact
    Section {
      StatusRow(
        title: "Account",
        value: friend.gone || contact?.gone == true ? "Deleted" : "On Yo",
        check: friend.gone || contact?.gone == true ? .problem : .ok)
      StatusRow(
        title: "Identity Key",
        value: contact?.unconfirmed == true || friend.keyChanged ? "Changed" : "Unchanged",
        check: contact?.unconfirmed == true ? .problem : friend.keyChanged ? .waiting : .ok)
      StatusRow(title: "Verified", value: contact?.verified == true ? "Yes" : "No", check: contact?.verified == true ? .ok : .open)
      StatusRow(title: "Blocked", value: friend.blocked ? "Yes" : "No", check: friend.blocked ? .open : .ok)
    } header: {
      Text("Identity")
    } footer: {
      if contact?.unconfirmed == true {
        Text("Their key changed after you verified them. Compare safety numbers again.")
      } else if friend.keyChanged {
        Text("Their key changed: they may have a new iPhone. To be sure it’s them, compare safety numbers.")
      }
    }
    if contact?.outdated == true {
      Section {
        StatusRow(title: "Yo", value: "Needs Update", check: .waiting)
      } header: {
        Text("Encryption")
      } footer: {
        Text("\(friend.name) uses an older Yo. Your Yos wait and arrive once they update the app.")
      }
    } else if !state.sessions.isEmpty {
      Section {
        StatusRow(title: "End-to-End Encrypted", value: "Signal Protocol", check: .ok)
        StatusRow(title: "Quantum-Safe", value: "PQXDH, SPQR", check: .ok)
        StatusRow(title: "Sealed Sender", value: "On", check: .ok)
        StatusRow(
          title: "Devices",
          value: "\(state.sessions.count)",
          check: state.sessionsCheck)
      } header: {
        Text("Encryption")
      } footer: {
        Text("The server delivers your Yos without learning they are from you.")
      }
    } else {
      Section {
        StatusRow(title: "Chat", value: "Not Yet", check: .open)
      }
    }
  }

  /// A short value beside its title; a long one (keys, ids) under it, from the left, as Settings' About shows them:
  /// beside the title it was right-aligned in a narrow column and broke at odd places.
  @ViewBuilder private func plain(_ title: String, _ value: String, mono: Bool = false) -> some View {
    if mono {
      VStack(alignment: .leading, spacing: 4) {
        Text(title)
        Text(value)
          .font(.system(.footnote, design: .monospaced))
          .foregroundStyle(.secondary)
          .textSelection(.enabled)
      }
      .accessibilityElement(children: .combine)
    } else {
      LabeledContent(title, value: value)
    }
  }

  /// The safety number in fives, three to a line, as it is read aloud ("12345 67890 12345"): only its digits, however
  /// the core spaced them (its own spaces cut the fives in the wrong places).
  private func grouped(_ number: String) -> String {
    let digits = Array(number.filter(\.isNumber))
    let fives = stride(from: 0, to: digits.count, by: 5).map { String(digits[$0..<min($0 + 5, digits.count)]) }
    return stride(from: 0, to: fives.count, by: 3).map { fives[$0..<min($0 + 3, fives.count)].joined(separator: " ") }
      .joined(separator: "\n")
  }

  private func load() async {
    friends.keyChangeSeen(friend)  // seen here: the list no longer marks it (this page shows it as it came)
    guard let client = accounts.client, let user = friend.user else { return }
    let loaded = try? await Hub.run { ContactState.load(client, user: user) }
    if client === accounts.client { state = loaded }
  }
}

/// When a contacts page loads again: another core (opened since, or another account), or its friends changed (a chat
/// started, a key changed, one gone or blocked).
private struct Reload: Equatable {
  let client: ObjectIdentifier?
  let friends: [String]

  init(_ client: YoClient?, _ friends: [Friend]) {
    self.client = client.map(ObjectIdentifier.init)
    self.friends = friends.map { "\($0.key)/\($0.user ?? "")/\($0.keyChanged)/\($0.gone)/\($0.blocked)" }
  }
}
