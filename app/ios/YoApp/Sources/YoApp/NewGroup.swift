// Add Group, as it will look: a name and the friends in it. A mockup for now: groups need more of the protocol first
// (spec 1: a message to several accounts lets the hub link them), so Create stays off and says so.
import SwiftUI

struct NewGroupSheet: View {
  @Environment(\.dismiss) private var dismiss
  @Environment(Friends.self) private var friends
  @State private var name = ""
  @State private var members: Set<UUID> = []

  var body: some View {
    NavigationStack {
      Form {
        Section {
          TextField("Group Name", text: $name)
            .font(.yo(20, relativeTo: .body))
        }
        Section {
          ForEach(friends.all.filter { !$0.blocked && !$0.gone }) { friend in
            Button {
              if members.contains(friend.key) { members.remove(friend.key) } else { members.insert(friend.key) }
            } label: {
              HStack {
                Circle().fill(Color.row(friend.color)).frame(width: 12, height: 12)
                Text(friend.name).font(.yo(17, relativeTo: .body)).foregroundStyle(Color(.label))
                  .lineLimit(1).minimumScaleFactor(0.5)
                Spacer()
                Image(systemName: members.contains(friend.key) ? "checkmark.circle.fill" : "circle")
                  .foregroundStyle(members.contains(friend.key) ? Color.amethyst : Color(.tertiaryLabel))
              }
            }
          }
        } header: {
          Text("Members")
        } footer: {
          Text("Groups are coming soon.")
        }
      }
      .navigationTitle("Add Group")  // as the list's bar that opens it
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button(role: .close) { dismiss() }
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("Create") {}.disabled(true)
        }
      }
    }
    .tint(Color.amethyst)
  }
}
