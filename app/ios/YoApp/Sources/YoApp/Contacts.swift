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
  var chats: [YoChat] = []

  /// Keys are updated at least every 7 days (spec 4.4).
  static let updateDays = 7.0

  /// The chat that carries the Yos: the restarted one if there is one, else the first.
  var chat: YoChat? { chats.first { $0.restarted && !$0.lost } ?? chats.first }

  func keysFresh(_ chat: YoChat) -> Check {
    guard chat.updates else { return .open }
    guard chat.lastUpdateMs > 0 else { return .waiting }
    let age = Date.now.timeIntervalSince1970 - Double(chat.lastUpdateMs) / 1000
    return age <= Self.updateDays * 86400 ? .ok : .waiting
  }

  /// The worst of the checks that matter: what the list shows for the contact.
  func overall(_ friend: Friend) -> Check {
    if friend.gone || contact?.gone == true || contact?.unconfirmed == true { return .problem }
    if friend.keyChanged { return .waiting }
    guard friend.user != nil, let chat else { return .open }
    if chat.lost { return .waiting }
    if chat.members != 2 { return .waiting }
    return keysFresh(chat) == .ok ? .ok : .waiting
  }

  static func load(_ client: YoClient, user: String) -> ContactState {
    ContactState(
      contact: client.contacts().first { $0.user == user },
      accountKey: client.accountKeyOf(user: user),
      safetyNumber: try? client.safetyNumber(who: user),
      chats: client.chatsWith(user: user))
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
      if friends.all.isEmpty {
        ContentUnavailableView("No Contacts", systemImage: "person.2")
      } else {
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
                Text(friend.name).font(.yo(17, relativeTo: .body))
              }
            }
          }
        } footer: {
          Text("A green check: the chat is end-to-end encrypted with fresh keys, as it should be.")
        }
      }
    }
    .navigationTitle("Contacts")
    .navigationBarTitleDisplayMode(.inline)
    .task { await load() }
  }

  private var sorted: [Friend] {
    friends.all.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
  }

  private func summary(_ friend: Friend, _ check: Check) -> String {
    if friend.blocked { return "Blocked" }
    if friend.gone { return "Not on Yo" }
    if friend.user == nil { return "No Chat Yet" }
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
    states = (try? await Hub.run {
      Dictionary(users.map { ($0, ContactState.load(client, user: $0)) }, uniquingKeysWith: { a, _ in a })
    }) ?? [:]
  }
}

/// One contact: the checks first, then who they are and their chats in plain values.
private struct ContactDetail: View {
  let friend: Friend
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @State private var state: ContactState?

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
        Section("Account") {
          plain("User ID", friend.user ?? "", mono: true)
          if let key = state.accountKey { plain("Account Key", key, mono: true) }
          if let number = state.safetyNumber { plain("Safety Number", grouped(number), mono: true) }
        }
        ForEach(Array(state.chats.enumerated()), id: \.offset) { index, chat in
          Section(state.chats.count == 1 ? "MLS Chat" : "MLS Chat \(index + 1)") {
            plain("Epoch", "\(chat.epoch) (joined at \(chat.joinedEpoch))")
            plain("Cipher Suite", chat.ciphersuite, mono: true)
            plain("Group ID", chat.group, mono: true)
          }
        }
      }
    }
    .navigationTitle(friend.name)
    .navigationBarTitleDisplayMode(.inline)
    .task { await load() }
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
    if let chat = state.chat {
      Section {
        StatusRow(title: "End-to-End Encrypted", value: "MLS", check: .ok)
        StatusRow(title: "Sealed Sender", value: "On", check: .ok)
        StatusRow(
          title: "Chat",
          value: chat.lost ? "Restarting" : chat.restarted ? "Restarted" : "Active",
          check: chat.lost ? .waiting : .ok)
        StatusRow(title: "Devices", value: "\(chat.members)", check: chat.members == 2 ? .ok : .waiting)
        StatusRow(title: "Fresh Keys", value: keysValue(chat, state.keysFresh(chat)), check: state.keysFresh(chat))
      } header: {
        Text("Encryption")
      } footer: {
        Text("The server delivers your Yos without learning they are from you. Keys are renewed at least weekly.")
      }
    } else {
      Section {
        StatusRow(title: "Chat", value: "Not Yet", check: .open)
      }
    }
  }

  private func keysValue(_ chat: YoChat, _ check: Check) -> String {
    if !chat.updates { return "Off" }
    if chat.lastUpdateMs == 0 { return "At Next Yo" }
    let when = Date(timeIntervalSince1970: Double(chat.lastUpdateMs) / 1000).formatted(.relative(presentation: .named))
    return check == .ok ? when : "Due"
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
    state = try? await Hub.run { ContactState.load(client, user: user) }
  }
}
