// Add Group: a name and the friends in it (spec 4.9: up to 20 accounts, this one among them). Friends Yo knows by
// user id only (a Yo went or came): one added by a name the hub never confirmed cannot be in a group yet.
import SwiftUI
import YoCore

struct NewGroupSheet: View {
  @Environment(\.dismiss) private var dismiss
  @Environment(Friends.self) private var friends
  @Environment(AccountStore.self) private var accounts
  @State private var name = ""
  @State private var members: Set<String> = []
  @State private var creating = false
  @State private var failed = false

  /// The most others in a group: 20 with this account.
  private static let most = 19

  private var choices: [Friend] {
    friends.all.filter { $0.user != nil && $0.group == nil && !$0.blocked && !$0.gone }
  }

  var body: some View {
    NavigationStack {
      Form {
        Section {
          TextField("Group Name", text: $name)
            .font(.yo(20, relativeTo: .body))
            .onChange(of: name) { if name.utf8.count > 32 { name = String(name.prefix(32)) } }
        }
        Section {
          ForEach(choices) { friend in
            let user = friend.user ?? ""
            let chosen = members.contains(user)
            Button {
              if chosen { members.remove(user) } else if members.count < Self.most { members.insert(user) }
            } label: {
              HStack {
                Circle().fill(Color.row(friend.color)).frame(width: 12, height: 12)
                Text(friend.name).font(.yo(17, relativeTo: .body)).foregroundStyle(Color(.label))
                  .lineLimit(1).minimumScaleFactor(0.5)
                Spacer()
                Image(systemName: chosen ? "checkmark.circle.fill" : "circle")
                  .foregroundStyle(chosen ? Color.amethyst : Color(.tertiaryLabel))
              }
            }
            .disabled(!chosen && members.count >= Self.most)
          }
        } header: {
          Text("Members")
        } footer: {
          Text(failed ? "The group could not be made. Try again in a moment." : "Up to \(Self.most) friends.")
        }
      }
      .navigationTitle("Add Group")  // as the menu item that opens it
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button(role: .close) { dismiss() }
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("Create") { create() }
            .disabled(creating || members.isEmpty || name.trimmingCharacters(in: .whitespaces).isEmpty)
        }
      }
    }
    .tint(Color.amethyst)
  }

  private func create() {
    guard let client = accounts.client else { return }
    let name = name.trimmingCharacters(in: .whitespaces)
    let members = Array(members)
    creating = true
    failed = false
    Task {
      do {
        _ = try await Hub.run { try client.createGroup(name: name, members: members) }
        if let groups = try? await Hub.run({ client.groups() }) { friends.takeGroups(groups) }
        dismiss()
      } catch {
        failed = true
      }
      creating = false
    }
  }
}
