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

  static func load(_ client: YoClient, user: String) throws -> ContactState {
    ContactState(
      contact: client.contacts().first { $0.user == user },
      accountKey: client.accountKeyOf(user: user),
      safetyNumber: Diagnostics.attempt({ try client.safetyNumber(who: user) }),
      sessions: try client.sessionsWith(user: user))
  }
}

/// Verifying a contact and confirming their new key (spec 3.4), through the core; then what waited for them goes (the
/// core's Yos and the photos in the outbox).
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

  /// Their new key accepted: Yos to them go again (unverified, until the safety numbers are compared again).
  static func confirm(
    _ user: String, accounts: AccountStore, inbox: Inbox, friends: Friends, outbox: MediaOutbox
  )
    async throws
  {
    guard let client = accounts.client else { return }
    try await Hub.run { try client.confirmIdentity(who: user) }
    resume(accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
  }

  private static func resume(
    accounts: AccountStore, inbox: Inbox, friends: Friends, outbox: MediaOutbox
  ) {
    inbox.resume(accounts: accounts, friends: friends)
    outbox.retryNow()
  }
}

/// Each contact with its overall check, a row each, leading to its own checks (ContactDetail): under Settings ›
/// Account › Contacts.
/// A contact in the list: its name and encryption mark (a tap shows the details), and Verified as a switch.
struct ContactListRow: View {
  let friend: Friend
  let state: ContactState?
  let check: Check
  let summary: String
  let details: () -> Void

  /// Name left; its mark and a chevron right; the whole row opens the contact's card.
  var body: some View {
    Button(action: details) {
      SettingsNavigationLabel {
        LabeledContent {
          Image(systemName: check.symbol).foregroundStyle(check.color).accessibilityLabel(summary)
        } label: {
          HStack(spacing: 10) {
            Avatar(name: friend.name, size: 28)
            Text(friend.name).lineLimit(1).minimumScaleFactor(0.5)
          }
        }
      }
    }
    .buttonStyle(.plain)
    .disabled(friend.user != nil && state == nil)
  }
}

/// The groups, each opening its members (the same sheet as the list's menu).
struct GroupRows: View {
  @Environment(Friends.self) private var friends
  @State private var open: Friend?

  var body: some View {
    VStack(spacing: 0) {
      let groups = friends.all.filter { $0.group != nil }
        .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
      ForEach(Array(groups.enumerated()), id: \.element.id) { index, group in
        if index > 0 { Divider().padding(.vertical, 5) }
        Button {
          open = group
        } label: {
          HStack(spacing: 10) {
            Image(systemName: "person.3.fill")
              .font(.system(size: 12, weight: .semibold))
              .foregroundStyle(.white)
              .frame(width: 28, height: 28)
              .background(Color.row(group.color), in: .circle)
            Text(group.name).lineLimit(1).minimumScaleFactor(0.5)
            Spacer(minLength: 0)
          }
          .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(Color(.label))
      }
    }
    .sheet(item: $open) { group in GroupMembersSheet(bar: group) }
  }
}

struct ContactRows: View {
  @Environment(Friends.self) private var friends
  @Environment(AccountStore.self) private var accounts
  @State private var states: [String: ContactState] = [:]
  @State private var unavailable = true
  /// The contact whose card is open: it slides in over the list instead of another page level.
  @State private var open: Friend?
  @State private var cardDetent: PresentationDetent = .medium
  /// The card's height from its rows: bar 64, each row 54, two groups' gaps 28 and 40 padding.
  private func cardHeight(for friend: Friend) -> CGFloat {
    let state = friend.user.flatMap { states[$0] }
    var rows = 3  // Verified, Identity, the wait switch
    if friend.gone || state?.contact?.gone == true { rows += 1 }
    if state?.contact?.outdated == true { rows += 1 }
    return 64 + CGFloat(rows) * 54 + 28 + 40 + 20
  }

  var body: some View {
    VStack(spacing: 0) {
      if friends.contacts.isEmpty {
        Text("No contacts yet.").foregroundStyle(.secondary)
      }
      ForEach(Array(sorted.enumerated()), id: \.element.id) { index, friend in
        if index > 0 { Divider().padding(.vertical, 5) }
        let state = friend.user.flatMap { states[$0] } ?? ContactState()
        let check = state.overall(friend)
        ContactListRow(friend: friend, state: friend.user.flatMap { states[$0] }, check: check,
          summary: summary(friend, check)) { open = friend }
      }
    }
    .sheet(item: $open) { friend in
      NavigationStack {
        ContactDetail(friend: friend, state: states[friend.user ?? ""])
          .toolbar {
            ToolbarItem(placement: .confirmationAction) {
              Button("Done", systemImage: "checkmark") { open = nil }
            }
          }
      }
      // Low: the card's few rows (Verified, Encryption, a problem row maybe, the wait switch) and its bar.
      .presentationDetents([.medium, .large])
      .presentationDragIndicator(.visible)
      .presentationBackground(Color(.systemGroupedBackground))
    }
    // Again once the core is open (the page opened before), and when a contact changes (a chat, a new key).
    .task(id: Reload(accounts.client, friends.contacts)) {
      while !Task.isCancelled {
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

  private var sorted: [Friend] {
    friends.contacts.sorted {
      $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
    }
  }

  private func summary(_ friend: Friend, _ check: Check) -> String {
    if friend.blocked { return "Blocked" }
    if friend.gone { return "Not on Yo" }
    if friend.user == nil { return "No Chat Yet" }
    if unavailable || states[friend.user ?? ""] == nil { return "Updating…" }
    if states[friend.user ?? ""]?.contact?.outdated == true { return "Needs Update" }
    if friend.keyChanged && check == .waiting { return "New Key" }
    return switch check {
    case .ok: "Encrypted"
    case .open: "No Chat Yet"
    case .waiting: "Updating"
    case .problem: "Check"
    }
  }
}

extension ContactState {
  /// Every contact's state, by user id; nil when the account changed meanwhile (signed out).
  @MainActor static func loadAll(_ accounts: AccountStore, _ friends: Friends) async -> [String:
    ContactState]?
  {
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

/// How many contacts' chats are end-to-end encrypted as they should be, of all of them ("23/23").
struct EncryptionTally: Equatable {
  var encrypted = 0
  var total = 0
  /// Contacts with a real problem (red), e.g. a changed key: the only thing the Account row shows.
  var problems = 0

  var allGood: Bool { encrypted == total }

  @MainActor static func load(_ accounts: AccountStore, _ friends: Friends) async
    -> EncryptionTally?
  {
    guard let states = await ContactState.loadAll(accounts, friends) else { return nil }
    let contacts = friends.contacts.filter { !$0.blocked }
    // A friend with no Yo exchanged yet has no session to check: it comes with the first Yo, so it counts as fine.
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
  /// One line, cut with "…"; a tap shows it whole.
  @State private var whole = false

  var body: some View {
    // The name above, the key under it in one line (cut in the middle); a tap shows it whole.
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
        // Quiet unless something is wrong: a red mark only for a real problem, no count.
        if let tally, tally.problems > 0 {
          Image(systemName: Check.problem.symbol).foregroundStyle(Check.problem.color)
            .accessibilityLabel("Contacts need attention")
        } else {
          Color.clear.frame(width: 1, height: 1).accessibilityHidden(true)  // keeps the tally task alive
        }
      } else {
        SettingsForm {
          Section { ContactRows() }
          if friends.all.contains(where: { $0.group != nil }) {
            Section("Groups") { GroupRows() }
          }
        }
        .navigationTitle("Contacts")
        .navigationBarTitleDisplayMode(.inline)
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

/// One contact: identity and actions first, with technical checks and values folded away.
private struct ContactDetail: View {
  let friend: Friend
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @Environment(Inbox.self) private var inbox
  @Environment(MediaOutbox.self) private var outbox
  @State private var state: ContactState?
  @State private var problem: String?
  @State private var keysShown = false
  @State private var waitShown = ContactDetail.waitOptionShown
  /// "Wait 5 s Before Sending" is hidden: five quick taps on a contact in the list show it (an expert option).
  static var waitOptionShown: Bool {
    get { UserDefaults.standard.bool(forKey: "settings.waitOptionShown") }
    set { UserDefaults.standard.set(newValue, forKey: "settings.waitOptionShown") }
  }
  /// Asks the presenting sheet to go tall (true) while the encryption card shows, and back (false).
  var tall: ((Bool) -> Void)? = nil
  /// Turning Verified on asks first: only after comparing safety numbers.
  @State private var confirmingVerify = false
  @State private var safetyNumber = ""

  init(friend: Friend, state: ContactState?, tall: ((Bool) -> Void)? = nil) {
    self.friend = friend
    self.tall = tall
    _state = State(initialValue: state)
    _safetyNumber = State(initialValue: state?.safetyNumber.map(Self.grouped) ?? "")
  }

  var body: some View {
    SettingsForm {
      Section {
        if friend.user == nil {
          StatusRow(title: "Chat", value: "Not Yet", check: .open)
        } else if let state {
          if friend.gone || state.contact?.gone == true {
            StatusRow(title: "Account", value: "Deleted", check: .problem)
          }
          if state.contact?.outdated == true {
            StatusRow(title: "Yo", value: "Needs Update", check: .waiting)
          }
          if let user = friend.user, state.safetyNumber != nil {
            verification(state, user: user)
          }
          StatusRow(
            title: "Identity",
            value: state.contact?.unconfirmed == true || friend.keyChanged ? "Changed" : "",
            check: state.contact?.unconfirmed == true ? .problem : friend.keyChanged ? .waiting : .ok)
            // Five quick taps show (or hide) the hidden "Wait 5 s Before Sending".
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
          if state.contact?.unconfirmed == true {
            Text("Their key changed after you verified them. Compare safety numbers again.")
              .font(.footnote).foregroundStyle(.secondary)
          } else if friend.keyChanged {
            Text("Their key changed, possibly on a new phone. Compare safety numbers to check it’s still them.")
              .font(.footnote).foregroundStyle(.secondary)
          }
        } else {
          StatusRow(title: "Chat", value: "Updating…", check: .waiting)
        }
      }
      if let state, friend.user != nil {
        Section("Keys") { keyInfo(state) }
      }
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
    .navigationTitle(friend.name)
    .navigationBarTitleDisplayMode(.inline)
    .task(id: Reload(accounts.client, friends.all.filter { $0.key == friend.key })) {
      while !Task.isCancelled {
        await load()
        do { try await Task.sleep(for: .seconds(5)) } catch { return }
      }
    }
    .alert(
      "Something Went Wrong",
      isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })
    ) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(problem ?? "")
    }
  }

  /// Marking them verified once the safety numbers match; a verified contact's new key confirmed, so Yos go again.
  @ViewBuilder private func verification(_ state: ContactState, user: String) -> some View {
    let verified = state.contact?.verified == true
    Group {
      Toggle(
        "Verified",
        isOn: Binding(
          get: { verified },
          set: { on in
            if on {
              confirmingVerify = true
            } else {
              change {
                try await Verification.setVerified(
                  user, false, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
              }
            }
          })
      )
      .tint(.green)
      .alert("Mark \(friend.name) as Verified?", isPresented: $confirmingVerify) {
        Button("Mark as Verified") {
          change {
            try await Verification.setVerified(
              user, true, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
          }
        }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text("Only if your safety numbers match. Compare them with \(friend.name) in person or on a call.")
      }
      .lineLimit(1)
      if state.contact?.unconfirmed == true {
        Button("Confirm New Key") {
          change {
            try await Verification.confirm(
              user, accounts: accounts, inbox: inbox, friends: friends, outbox: outbox)
          }
        }
      }
      if state.contact?.unconfirmed == true {
        Text("Your Yos to \(friend.name) wait until you confirm. Compare safety numbers first.")
      }
    }
  }

  /// The worst of the checks the details page lists.
  private func overall(_ state: ContactState) -> Check {
    let contact = state.contact
    let all: [Check] = [
      friend.gone || contact?.gone == true ? .problem : .ok,
      contact?.unconfirmed == true ? .problem : friend.keyChanged ? .waiting : .ok,
      state.accountKey == nil ? .waiting : .ok,
      state.safetyNumber == nil ? .waiting : .ok,
      state.sessions.isEmpty ? .open : state.sessionsCheck,
    ]
    for check in [Check.problem, .waiting, .open] where all.contains(check) { return check }
    return .ok
  }

  /// Every key and id of this contact in one place.
  @ViewBuilder private func keyInfo(_ state: ContactState) -> some View {
    PublicKeyRow(title: "User ID", value: friend.user ?? "").accessibilityElement(children: .combine)
    if let key = state.accountKey {
      Divider()
      PublicKeyRow(title: "Account Key", value: key).accessibilityElement(children: .combine)
    }
    if state.safetyNumber != nil {
      Divider()
      PublicKeyRow(title: "Safety Number", value: safetyNumber).accessibilityElement(children: .combine)
    }
    ForEach(state.sessions, id: \.device) { session in
      Divider()
      PublicKeyRow(title: "Device Key", value: session.device).accessibilityElement(children: .combine)
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

  /// The safety number in fives, three to a line, as it is read aloud ("12345 67890 12345"): only its digits, however
  /// the core spaced them (its own spaces cut the fives in the wrong places).
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
    friends.keyChangeSeen(friend)  // seen here: the list no longer marks it (this page shows it as it came)
    guard let client = accounts.client, let user = friend.user else { return }
    let loaded = try? await Hub.run { try ContactState.load(client, user: user) }
    if client === accounts.client, !Task.isCancelled, let loaded {
      safetyNumber = loaded.safetyNumber.map(Self.grouped) ?? ""
      state = loaded
    }
  }
}

/// When a contacts page loads again: another core (opened since, or another account), or its friends changed (a chat
/// started, a key changed, one gone or blocked).
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
