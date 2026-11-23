// Contacts in the profile: every friend and whether its encryption is as it should be (spec 3, 4).
import SwiftUI
import UIKit
import YoCore

typealias Check = ProductCheck

extension ProductCheck {
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

struct ContactState: Sendable {
  var contact: YoContact?
  var accountKey: String?
  var safetyNumber: String?
  var sessions: [YoSession] = []

  var sessionsCheck: Check {
    productSessionsCheck(sessions: UInt32(sessions.count), unready: sessions.contains { $0.retrying || !$0.usable })
  }

  func facts(_ friend: Friend, loaded: Bool = true) -> ProductContact {
    ProductContact(
      blocked: friend.blocked, friendGone: friend.gone, contactGone: contact?.gone == true,
      unconfirmed: contact?.unconfirmed == true, outdated: contact?.outdated == true, keyChanged: friend.keyChanged,
      verified: contact?.verified == true, hasUser: friend.user != nil, loaded: loaded,
      sessions: UInt32(sessions.count), sessionsUnready: sessions.contains { $0.retrying || !$0.usable })
  }

  func overall(_ friend: Friend) -> Check { productContactCheck(contact: facts(friend)) }

  static func load(_ client: YoClient, user: String) throws -> ContactState {
    let contact = client.contacts().first { $0.user == user }
    return ContactState(
      contact: contact,
      accountKey: client.accountKeyOf(user: user),
      // No contact yet: no safety number (the core would answer `NotOnYo`).
      safetyNumber: contact == nil ? nil : Diagnostics.attempt({ try client.safetyNumber(who: user) }),
      sessions: try client.sessionsWith(user: user))
  }
}

/// Verifying a contact and confirming their new key (spec 3.4); then what waited for them goes again.
@MainActor
enum Verification {
  static func setVerified(
    _ user: String, _ verified: Bool, accounts: AccountStore, inbox: Inbox, friends: Friends,
    outbox: MediaOutbox
  ) async throws {
    guard let client = accounts.client else { return }
    try await Hub.run { try client.setVerified(who: user, verified: verified) }
    resume(accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
  }

  /// Yos to them go again (unverified until the safety numbers are compared again).
  static func confirm(
    _ user: String, accounts: AccountStore, inbox: Inbox, friends: Friends, outbox: MediaOutbox
  ) async throws {
    guard let client = accounts.client else { return }
    try await Hub.run { try client.confirmIdentity(who: user) }
    resume(accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
  }

  private static func resume(accounts: AccountStore, inbox: Inbox, friends: Friends, outbox: MediaOutbox) {
    inbox.resume(accounts: accounts, friends: friends)
    outbox.retryNow()
  }
}

extension ProductContactMark {
  var symbol: String { self == .verified ? "checkmark.seal.fill" : "exclamationmark.circle.fill" }
  var color: Color { self == .verified ? .green : .orange }
}

/// A contact under Settings › Account › Contacts: a mark only for an exception (verified, or something to look at).
struct ContactListRow: View {
  let friend: Friend
  let state: ContactState?
  let mark: ProductContactMark?
  let summary: String
  let details: () -> Void

  var body: some View {
    Button(action: details) {
      HStack(spacing: 10) {
        Avatar(name: friend.name, size: 28)
        Text(friend.name).lineLimit(1).minimumScaleFactor(0.5)
        Spacer(minLength: 0)
        if let mark {
          Image(systemName: mark.symbol).foregroundStyle(mark.color).accessibilityHidden(true)
        }
      }
    }
    .tint(Color(.label))
    .accessibilityValue(summary)
    .disabled(friend.user != nil && state == nil)
  }
}

private struct ContactsList: View {
  @Environment(Friends.self) private var friends
  @Environment(AccountStore.self) private var accounts
  @State private var states: [String: ContactState] = [:]
  @State private var unavailable = true
  @State private var open: Friend?
  @State private var openGroup: Friend?

  var body: some View {
    Form {
      Section {
        if friends.contacts.isEmpty {
          Text("No contacts yet.").foregroundStyle(.secondary)
        }
        ForEach(friends.contacts.sorted(by: Self.byName)) { friend in
          let state = friend.user.flatMap { states[$0] }
          let facts = (state ?? ContactState()).facts(friend, loaded: !unavailable && state != nil)
          ContactListRow(
            friend: friend, state: state, mark: productContactMark(contact: facts),
            summary: productContactSummary(contact: facts, lang: .english)
          ) { open = friend }
        }
      }
      let groups = friends.all.filter { $0.group != nil }.sorted(by: Self.byName)
      if !groups.isEmpty {
        Section("Groups") {
          ForEach(groups) { group in
            Button {
              openGroup = group
            } label: {
              HStack(spacing: 10) {
                Image(systemName: "person.3.fill")
                  .font(.system(size: 12, weight: .semibold))
                  .foregroundStyle(.white)
                  .frame(width: 28, height: 28)
                  .background(Color.row(group.color), in: .circle)
                Text(group.name).lineLimit(1).minimumScaleFactor(0.5)
              }
            }
            .tint(Color(.label))
          }
        }
      }
    }
    .navigationTitle("Contacts")
    .navigationBarTitleDisplayMode(.inline)
    .sheet(item: $openGroup) { group in GroupMembersSheet(bar: group).systemAppearance().appObjects() }
    .sheet(item: $open) { friend in
      NavigationStack {
        ContactDetail(friend: friend, state: states[friend.user ?? ""]) { user, loaded in
          states[user] = loaded
        }
        .toolbar {
          ToolbarItem(placement: .confirmationAction) {
            Button("Done", systemImage: "checkmark") { open = nil }
          }
        }
      }
      .presentationDetents([.large])
      .presentationDragIndicator(.hidden)
      .systemAppearance().appObjects()
    }
    .task(id: Reload(accounts.client, friends.contacts)) {
      while !Task.isCancelled {
        if UIApplication.shared.applicationState == .background {
          do { try await Task.sleep(for: .seconds(1)) } catch { return }
          continue
        }
        if let loaded = await ContactState.loadAll(accounts, friends) {
          states = loaded
          unavailable = false
        } else {
          unavailable = true
        }
        do { try await Task.sleep(for: .seconds(5)) } catch { return }
      }
    }
  }

  private static func byName(_ a: Friend, _ b: Friend) -> Bool {
    a.name.localizedCaseInsensitiveCompare(b.name) == .orderedAscending
  }
}

extension ContactState {
  @MainActor static func loadAll(_ accounts: AccountStore, _ friends: Friends) async -> [String: ContactState]? {
    guard let client = accounts.client else { return nil }
    let users = friends.contacts.compactMap(\.user)
    let loaded =
      (try? await Hub.run {
        Dictionary(
          try users.map { ($0, try ContactState.load(client, user: $0)) },
          uniquingKeysWith: { a, _ in a })
      })
    return client === accounts.client ? loaded : nil
  }
}

struct EncryptionTally: Equatable {
  var encrypted = 0
  var total = 0
  /// Contacts with a real problem (red), e.g. a changed key: the only thing the Account row shows.
  var problems = 0

  @MainActor static func load(_ accounts: AccountStore, _ friends: Friends) async -> EncryptionTally? {
    guard let states = await ContactState.loadAll(accounts, friends) else { return nil }
    let contacts = friends.contacts.filter { !$0.blocked }
    // No Yo exchanged yet means no session to check: counts as fine.
    let good = contacts.filter { friend in
      let state = friend.user.flatMap { states[$0] } ?? ContactState()
      let check = state.overall(friend)
      return check == .ok || (check != .problem && state.sessions.isEmpty)
    }
    let problems = contacts.filter { friend in
      (friend.user.flatMap { states[$0] } ?? ContactState()).overall(friend) == .problem
    }
    return EncryptionTally(encrypted: good.count, total: contacts.count, problems: problems.count)
  }
}

struct PublicKeyRow: View {
  let title: String
  let value: String
  @State private var whole = false

  var body: some View {
    VStack(alignment: .leading, spacing: 3) {
      Text(title).font(.subheadline.weight(.semibold))
      Text(value).font(.system(.footnote, design: .monospaced))
        .foregroundStyle(.secondary)
        .lineLimit(whole ? nil : 1)
        .truncationMode(.middle)
        .fixedSize(horizontal: false, vertical: whole)
        .textSelection(.enabled)
    }
    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
    .padding(.vertical, 6)
    .contentShape(Rectangle())
    .onTapGesture { withAnimation(.snappy) { whole.toggle() } }
    .contextMenu {
      Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = value }
    }
  }
}

struct ContactSecurityPage: View {
  var summaryOnly = false
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @State private var tally: EncryptionTally?
  var body: some View {
    Group {
      if summaryOnly {
        // A red mark only for a real problem, no count.
        if let tally, tally.problems > 0 {
          Image(systemName: Check.problem.symbol).foregroundStyle(Check.problem.color)
            .accessibilityLabel("Contacts need attention")
        } else {
          Color.clear.frame(width: 1, height: 1).accessibilityHidden(true)  // keeps the tally task alive
        }
      } else {
        ContactsList()
      }
    }
    .task(id: Reload(accounts.client, friends.contacts)) {
      while !Task.isCancelled {
        tally = await EncryptionTally.load(accounts, friends)
        do { try await Task.sleep(for: .seconds(5)) } catch { return }
      }
    }
  }
}

/// One contact as a small card: the code to compare and Mark as Verified; checks and keys under Technical Details.
private struct ContactDetail: View {
  let friend: Friend
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @Environment(Inbox.self) private var inbox
  @Environment(MediaOutbox.self) private var outbox
  @State private var state: ContactState?
  @State private var problem: String?
  @State private var confirmingVerify = false
  @State private var safetyNumber = ""
  /// Handed to the list so its mark follows at once.
  let updated: (String, ContactState) -> Void

  init(
    friend: Friend, state: ContactState?, updated: @escaping (String, ContactState) -> Void = { _, _ in }
  ) {
    self.friend = friend
    self.updated = updated
    _state = State(initialValue: state)
    _safetyNumber = State(initialValue: state?.safetyNumber.map(Self.grouped) ?? "")
  }

  var body: some View {
    Form {
      if friend.user == nil {
        Section { StatusRow(title: "Chat", value: "Not Yet", check: .open) }
      } else if let state {
        if friend.gone || state.contact?.gone == true || state.contact?.outdated == true
          || state.contact?.unconfirmed == true || friend.keyChanged
        {
          Section { attention(state) }
        }
        if let user = friend.user, state.safetyNumber != nil {
          Section { verification(state, user: user) }
        }
        Section {
          NavigationLink {
            ContactTechnicalDetails(friend: friend, state: state, safetyNumber: safetyNumber)
          } label: {
            Text("Technical Details")
          }
        }
      } else {
        Section { StatusRow(title: "Chat", value: "Updating…", check: .waiting) }
      }
    }
    .navigationTitle(friend.name)
    .navigationBarTitleDisplayMode(.inline)
    .toolbar {
      ToolbarItem(placement: .principal) {
        HStack(spacing: 5) {
          Text(friend.name).font(.headline).lineLimit(1)
          Image(systemName: "lock.fill").font(.caption).foregroundStyle(.secondary)
            .accessibilityLabel("End-to-end encrypted")
        }
        .accessibilityElement(children: .combine)
      }
    }
    .task(id: Reload(accounts.client, friends.all.filter { $0.key == friend.key })) {
      while !Task.isCancelled {
        await load()
        do { try await Task.sleep(for: .seconds(5)) } catch { return }
      }
    }
    .problemAlert($problem)
  }

  @ViewBuilder private func attention(_ state: ContactState) -> some View {
    if friend.gone || state.contact?.gone == true {
      StatusRow(title: "Account", value: "Deleted", check: .problem)
    } else if state.contact?.outdated == true {
      StatusRow(title: "Yo", value: "Needs Update", check: .waiting)
    }
    if state.contact?.unconfirmed == true {
      StatusRow(title: "Key", value: "Changed", check: .problem)
    } else if friend.keyChanged {
      StatusRow(title: "Key", value: "Changed", check: .waiting)
    }
  }

  @ViewBuilder private func verification(_ state: ContactState, user: String) -> some View {
    let verified = state.contact?.verified == true
    VStack(spacing: 8) {
      Text("Compare Code").font(.subheadline).foregroundStyle(.secondary)
      Text(safetyNumber)
        .font(.system(.body, design: .monospaced))
        .multilineTextAlignment(.center)
        .textSelection(.enabled)
        .accessibilityLabel("Code \(safetyNumber)")
    }
    .frame(maxWidth: .infinity)
    .padding(.vertical, 6)
    Group {
      if verified {
        Menu {
          Button("Clear Verification", systemImage: "xmark.seal", role: .destructive) {
            change {
              try await Verification.setVerified(
                user, false, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
            }
          }
        } label: {
          Label("Verified", systemImage: "checkmark.seal.fill").foregroundStyle(.green)
            .frame(maxWidth: .infinity, minHeight: 44)
            .contentShape(Rectangle())
        }
      } else {
        Button("Mark as Verified") { confirmingVerify = true }
          .foregroundStyle(.tint)
          .frame(maxWidth: .infinity, minHeight: 44)
          .contentShape(Rectangle())
      }
    }
    .alert("Mark \(friend.name) as Verified?", isPresented: $confirmingVerify) {
      Button("Mark as Verified") {
        change {
          try await Verification.setVerified(
            user, true, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
        }
      }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text("Only if the codes match. Compare them with \(friend.name) in person or on a call.")
    }
    if state.contact?.unconfirmed == true {
      Button("Confirm New Key") {
        change {
          try await Verification.confirm(
            user, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
        }
      }
      .foregroundStyle(.tint)
      .frame(maxWidth: .infinity, minHeight: 44)
      .contentShape(Rectangle())
    }
  }

  private func change(_ work: @escaping @MainActor () async throws -> Void) {
    Task {
      do {
        try await work()
      } catch {
        problem = Problems.words(for: error)
      }
      await load()
    }
  }

  /// In fives, three to a line, from its digits only: the core's own spacing cuts the fives in the wrong places.
  private static func grouped(_ number: String) -> String {
    let digits = Array(number.filter(\.isNumber))
    let fives = stride(from: 0, to: digits.count, by: 5).map {
      String(digits[$0..<min($0 + 5, digits.count)])
    }
    return stride(from: 0, to: fives.count, by: 3).map {
      fives[$0..<min($0 + 3, fives.count)].joined(separator: " ")
    }
    .joined(separator: "\n")
  }

  private func load() async {
    friends.keyChangeSeen(friend)  // the list no longer marks it
    guard let client = accounts.client, let user = friend.user else { return }
    let loaded = try? await Hub.run { try ContactState.load(client, user: user) }
    if client === accounts.client, !Task.isCancelled, let loaded {
      safetyNumber = loaded.safetyNumber.map(Self.grouped) ?? ""
      state = loaded
      updated(user, loaded)
    }
  }
}

private struct ContactTechnicalDetails: View {
  let friend: Friend
  let state: ContactState
  let safetyNumber: String
  @Environment(Friends.self) private var friends
  @State private var waitShown = ContactTechnicalDetails.waitOptionShown
  /// "Wait 5 s Before Sending" is hidden: five quick taps on Identity show it.
  static var waitOptionShown: Bool {
    get { UserDefaults.standard.bool(forKey: "settings.waitOptionShown") }
    set { UserDefaults.standard.set(newValue, forKey: "settings.waitOptionShown") }
  }

  var body: some View {
    Form {
      Section {
        StatusRow(
          title: "Identity",
          value: state.contact?.unconfirmed == true || friend.keyChanged ? "Changed" : "",
          check: state.contact?.unconfirmed == true ? .problem : friend.keyChanged ? .waiting : .ok)
          .contentShape(Rectangle())
          .onTapGesture(count: 5) {
            withAnimation(.smooth) { waitShown.toggle() }
            Self.waitOptionShown = waitShown
            UINotificationFeedbackGenerator().notificationOccurred(.success)
          }
        if !state.sessions.isEmpty {
          StatusRow(title: "Sealed Yos", value: "", check: .ok)
          StatusRow(title: "Devices", value: "\(state.sessions.count)", check: state.sessionsCheck)
        }
      }
      Section("Keys") { keyInfo }
      if waitShown {
        Section {
          let current = friends.all.first { $0.key == friend.key }
          Toggle(
            "Wait 5 s Before Sending",
            isOn: Binding(
              get: { current?.undoFirst ?? false },
              set: {
                friends.setUndoFirst(friend, $0)
                if $0 { friends.setUndoSeconds(friend, 5) }
              }))
        }
      }
    }
    .navigationTitle("Technical Details")
    .navigationBarTitleDisplayMode(.inline)
  }

  @ViewBuilder private var keyInfo: some View {
    PublicKeyRow(title: "User ID", value: friend.user ?? "").accessibilityElement(children: .combine)
    if let key = state.accountKey {
      PublicKeyRow(title: "Account Key", value: key).accessibilityElement(children: .combine)
    }
    if state.safetyNumber != nil {
      PublicKeyRow(title: "Safety Number", value: safetyNumber).accessibilityElement(children: .combine)
    }
    ForEach(state.sessions, id: \.device) { session in
      PublicKeyRow(title: "Device Key", value: session.device).accessibilityElement(children: .combine)
    }
  }
}

/// Reload trigger for contact pages: another core, or a friend's chat, key, gone or blocked state changed.
struct Reload: Equatable {
  let client: ObjectIdentifier?
  let friends: [String]

  init(_ client: YoClient?, _ friends: [Friend]) {
    self.client = client.map(ObjectIdentifier.init)
    self.friends = friends.map {
      "\($0.key)/\($0.user ?? "")/\($0.keyChanged)/\($0.gone)/\($0.blocked)"
    }
  }
}
