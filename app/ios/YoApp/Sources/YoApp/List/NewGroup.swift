// Add Group (spec 4.9): a name and up to 19 friends known by user id (a name the hub never confirmed can't join yet).
import SwiftUI
import YoCore

struct NewGroupSheet: View {
  @Environment(\.dismiss) private var dismiss
  @Environment(Friends.self) private var friends
  @Environment(Inbox.self) private var inbox
  @Environment(AccountStore.self) private var accounts
  @Environment(MediaOutbox.self) private var outbox
  @FocusState private var naming: Bool
  @State private var ownName = "You"
  @State private var search = ""
  @State private var name = ""
  @State private var members: Set<String> = []
  @State private var creating = false
  @State private var failed = false

  /// The most others in a group: 20 with this account.
  private static let most = 19

  private var choices: [Friend] {
    friends.all.filter { $0.user != nil && $0.group == nil && !$0.blocked && !$0.gone }
  }

  private var canCreate: Bool {
    !creating && !members.isEmpty && !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  private var filteredChoices: [Friend] {
    choices.filter { search.isEmpty || $0.name.localizedStandardContains(search) }
  }

  var body: some View {
    NavigationStack {
      form
      .task { ownName = accounts.account?.username ?? "You" }
      .searchable(text: $search, prompt: "Find Friends")
      .navigationTitle("New Group")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Cancel", role: .cancel) { dismiss() }
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("Create") { create() }
            .disabled(!canCreate)
        }
      }
      .interactiveDismissDisabled(creating)
    }
  }

  private var form: some View {
    Form {
      Section {
        TextField("Group Name", text: $name)
          .focused($naming)
          .submitLabel(canCreate ? .done : .next)
          .onSubmit {
            if canCreate { create() } else { naming = false }
          }
          .onChange(of: name) { if name.utf8.count > 32 { name = FriendList.groupName(name) } }
      }
      Section {
        GroupMemberRow(name: ownName, selected: true).disabled(true)
        ForEach(filteredChoices) { friend in
          let user = friend.user ?? ""
          let chosen = members.contains(user)
          let unavailable = !chosen && members.count >= Self.most
          Button {
            naming = false
            if chosen { members.remove(user) } else if members.count < Self.most { members.insert(user) }
          } label: {
            GroupMemberRow(name: friend.name, selected: chosen)
          }
          .buttonStyle(.plain)
          .accessibilityAddTraits(chosen ? [.isSelected] : [])
          .disabled(unavailable)
          .opacity(unavailable ? 0.4 : 1)
        }
      } header: {
        Text("Members · \(members.count + 1) of 20")
      } footer: {
        Text("You can add up to \(Self.most) friends, plus yourself.")
      }
      if failed {
        Section {
          Text("The group could not be made. Try again in a moment.")
            .foregroundStyle(.red)
        }
      }
    }
    .formStyle(.grouped)
    .disabled(creating)
  }

  private func create() {
    guard canCreate, let client = accounts.client else { return }
    naming = false
    let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
    let members = Array(members)
    creating = true
    failed = false
    Task {
      do {
        let made = try await Hub.run { try client.createGroup(name: FriendList.groupName(name), members: members) }
        await inbox.absorb(made.events, client: client, friends: friends)
        if let groups = try? await Hub.run({ try client.groups() }) { friends.takeGroups(groups) }
        let bar = friends.all.first(where: { $0.group == made.id })
        if let bar {
          let names = members.compactMap { id in friends.all.first { $0.user == id }?.name }.sorted()
          await GroupNotice.send(
            [.user(GroupNotice.me(accounts)), " made ", .group(bar.name)] + GroupNotice.with(names), to: bar,
            accounts: accounts, outbox: outbox)
        }
        dismiss()
        // Its first Yo goes to the members once the list shows again, so "Sent Yo" is seen there (none in it: no Yo,
        // the bar waits quietly at the bottom).
        if let bar, !members.isEmpty { DeleteNotice.shared.yo?(bar) }
      } catch {
        failed = true
        Problems.shared.report(error)
      }
      creating = false
    }
  }
}

struct GroupMemberRow: View {
  let name: String
  let selected: Bool
  var body: some View {
    HStack(spacing: 12) {
      Avatar(name: name, size: 36)
      Text(name).font(.body).foregroundStyle(.primary).lineLimit(1)
      Spacer()
      Image(systemName: selected ? "checkmark.circle.fill" : "circle")
        .font(.title2)
        .foregroundStyle(selected ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.tertiary))
    }
    .contentShape(Rectangle())
    .accessibilityAddTraits(selected ? [.isSelected] : [])
  }
}
