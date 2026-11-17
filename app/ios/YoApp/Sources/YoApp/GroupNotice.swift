// What happened in a group, as a picture sent to it: "LEO added MIA to FAMILY", "LEO removed NOAH from FAMILY", "LEO
// renamed the group to FAMILY", "LEO made FAMILY with MIA and NOAH", "LEO left FAMILY". Every member sees it as a
// photo on the group's bar, as Signal shows such changes in the chat. Made on the phone that made the change (YoCard), sent as any group photo (MediaOutbox).
import SwiftUI
import YoCore

@MainActor
enum GroupNotice {
  /// Renders `sentence` and puts it in the outbox to the group `bar`; its id, nil if it could not be made.
  @discardableResult
  static func send(_ sentence: [YoCard.Piece], to bar: Friend, accounts: AccountStore, outbox: MediaOutbox) async -> UUID? {
    guard bar.group != nil, let client = accounts.client else { return nil }
    guard let data = YoCard(sentence: sentence).jpeg(),
      let file = await Task.detached(operation: { MediaFile.photo(data) }).value
    else { return nil }
    let id = UUID()
    let queued = await outbox.send(file, to: bar.target, from: client, accounts: accounts, id: id)
    return queued ? id : nil
  }

  /// Who else is in it, for "LEO made FAMILY with MIA, NOAH and ZOE": at most four by name, then "and 3 more".
  static func with(_ names: [String]) -> [YoCard.Piece] {
    guard !names.isEmpty else { return [] }
    let shown = names.count > 4 ? Array(names.prefix(3)) : names
    var pieces: [YoCard.Piece] = [" with "]
    for (i, name) in shown.enumerated() {
      if i > 0 { pieces.append(i == shown.count - 1 && shown.count == names.count ? " and " : ", ") }
      pieces.append(.user(name))
    }
    if shown.count < names.count { pieces.append(.text(" and \(names.count - shown.count) more")) }
    return pieces
  }

  /// This account's name, as the pictures say who did it.
  static func me(_ accounts: AccountStore) -> String { accounts.account?.username ?? "Someone" }
}

/// A group's members: each by name, removable (asked first), and friends not in it to add, up to 20 in all. A change
/// goes to the group as a picture too (GroupNotice).
struct GroupMembersSheet: View {
  let bar: Friend
  @Environment(\.dismiss) private var dismiss
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @Environment(Inbox.self) private var inbox
  @Environment(MediaOutbox.self) private var outbox
  @State private var members: [String] = []
  @State private var removing: (user: String, name: String)?
  @State private var search = ""
  @State private var ownUser: String?
  @State private var busy = false
  @State private var failed = false

  static let most = 20

  var body: some View {
    NavigationStack {
      Form {
        // Who is in the group first, then the friends who could be added.
        Section("Members · \(members.count) of 20") {
          if let ownUser {
            GroupMemberRow(name: name(of: ownUser), selected: true).disabled(true)
          }
          ForEach(choices.filter { members.contains($0) }, id: \.self) { user in memberButton(user) }
        }
        let others = choices.filter { !members.contains($0) }
        if !others.isEmpty {
          Section("Add Friends") {
            ForEach(others, id: \.self) { user in memberButton(user) }
          }
        }
        if failed {
          Section { Text("That didn’t work. Try again in a moment.").foregroundStyle(.secondary) }
        }
      }
      .searchable(text: $search, prompt: "Find Friends")
      .navigationTitle(bar.name)
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
      }
      .confirmCard(
        $removing, title: { "Remove \($0.name)?" },
        message: { _ in "They won’t receive any more Yos from this group." },
        action: { "Remove \($0.name)" },
        run: { who in
          change(remove: [who.user], line: [.user(GroupNotice.me(accounts)), " removed ", .user(who.name), " from ", .group(bar.name)])
        })
      .task { await load() }
    }
  }

  private func memberButton(_ user: String) -> some View {
    let selected = members.contains(user)
    return Button {
      if selected { removing = (user, name(of: user)) }
      else {
        change(add: [user], line: [.user(GroupNotice.me(accounts)), " added ", .user(name(of: user)), " to ", .group(bar.name)])
      }
    } label: {
      GroupMemberRow(name: name(of: user), selected: selected)
    }
    .buttonStyle(.plain)
    .disabled(busy || (!selected && members.count >= Self.most))
  }

  private var choices: [String] {
    let users = Set(members + friends.all.filter { $0.group == nil && !$0.blocked && !$0.gone }.compactMap(\.user))
    return users.filter { $0 != ownUser && (search.isEmpty || name(of: $0).localizedStandardContains(search)) }
      .sorted { name(of: $0).localizedStandardCompare(name(of: $1)) == .orderedAscending }
  }

  private func name(of user: String) -> String {
    if user == ownUser { return accounts.account?.username ?? "You" }
    return friends.all.first { $0.user == user }?.name ?? "Yo User"
  }

  private func load() async {
    guard let id = bar.group, let client = accounts.client,
      let groups = try? await Hub.run({ try client.groups() })
    else { return }
    ownUser = try? await Hub.run { client.userId() }
    members = (groups.first { $0.id == id }?.members ?? []).sorted { name(of: $0) < name(of: $1) }
    friends.takeGroups(groups)
  }

  /// Adds or removes, then tells the group in a picture (sent after the change, so a new member sees it too).
  private func change(add: [String] = [], remove: [String] = [], line: [YoCard.Piece]) {
    guard let id = bar.group, let client = accounts.client else { return }
    busy = true
    failed = false
    Task {
      do {
        let events = try await Hub.run {
          add.isEmpty ? try client.removeFromGroup(group: id, users: remove) : try client.addToGroup(group: id, users: add)
        }
        await inbox.absorb(events, client: client, friends: friends)
        await load()
        await GroupNotice.send(line, to: friends.all.first { $0.group == id } ?? bar, accounts: accounts, outbox: outbox)
      } catch {
        failed = true
      }
      busy = false
    }
  }
}
