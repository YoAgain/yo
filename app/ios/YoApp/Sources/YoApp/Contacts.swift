// Contacts in the profile: every friend, and for each what the encryption knows (spec 3, 4): the pinned account key
// and safety number, sealed sender, and each MLS chat with its epoch, devices and last key update.
import SwiftUI
import YoCore

/// The friends, blocked ones too, each opening its details.
struct ContactsList: View {
  @Environment(Friends.self) private var friends

  var body: some View {
    Form {
      if friends.all.isEmpty {
        ContentUnavailableView("No Contacts", systemImage: "person.2")
      } else {
        Section {
          ForEach(friends.all.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }) {
            friend in
            NavigationLink {
              ContactDetail(friend: friend)
            } label: {
              LabeledContent {
                Text(friend.blocked ? "Blocked" : friend.gone ? "Not on Yo" : friend.user == nil ? "No Chat Yet" : "")
              } label: {
                Text(friend.name).font(.yo(17, relativeTo: .body))
              }
            }
          }
        } footer: {
          Text("How each chat is encrypted.")
        }
      }
    }
    .navigationTitle("Contacts")
    .navigationBarTitleDisplayMode(.inline)
  }
}

/// One contact: who they are to the hub and to this phone, and their chats as MLS has them.
private struct ContactDetail: View {
  let friend: Friend
  @Environment(AccountStore.self) private var accounts
  @State private var info: Info?

  /// What the core says, read once the page shows.
  private struct Info: Sendable {
    var accountKey: String?
    var safetyNumber: String?
    var verified = false
    var chats: [YoChat] = []
  }

  var body: some View {
    Form {
      Section("Account") {
        row("Name", friend.name)
        if let user = friend.user { row("User ID", user, mono: true) }
        if let key = info?.accountKey { row("Account Key", key, mono: true) }
        if friend.blocked { row("Blocked", "Yes") }
        if friend.gone { row("Account", "Deleted") }
      }
      if friend.user == nil {
        Section {
          Text("No chat yet. The first Yo starts one.").foregroundStyle(.secondary)
        }
      } else {
        Section {
          row("Sealed Sender", "On")
          row("Verified", info?.verified == true ? "Yes" : "No")
          if let number = info?.safetyNumber { row("Safety Number", grouped(number), mono: true) }
        } header: {
          Text("Encryption")
        } footer: {
          Text("The server delivers your Yos without learning they are from you.")
        }
        ForEach(Array((info?.chats ?? []).enumerated()), id: \.offset) { index, chat in
          Section(info?.chats.count == 1 ? "MLS Chat" : "MLS Chat \(index + 1)") {
            row("State", chat.lost ? "Restarting" : chat.restarted ? "Restarted" : "Active")
            row("Epoch", "\(chat.epoch)")
            row("Joined at Epoch", "\(chat.joinedEpoch)")
            row("Devices", "\(chat.members)")
            row(
              "Last Key Update",
              chat.lastUpdateMs == 0
                ? "Not yet"
                : Date(timeIntervalSince1970: Double(chat.lastUpdateMs) / 1000).formatted(.relative(presentation: .named))
            )
            row("Key Updates", chat.updates ? "On" : "Off")
            row("Cipher Suite", chat.ciphersuite, mono: true)
            row("Group ID", chat.group, mono: true)
          }
        }
      }
    }
    .navigationTitle(friend.name)
    .navigationBarTitleDisplayMode(.inline)
    .task { await load() }
  }

  private func row(_ title: String, _ value: String, mono: Bool = false) -> some View {
    LabeledContent(title) {
      Text(value)
        .font(mono ? .system(.footnote, design: .monospaced) : .body)
        .multilineTextAlignment(.trailing)
        .textSelection(.enabled)
    }
  }

  /// "12345 67890 …": the safety number in fives, as it is read aloud.
  private func grouped(_ digits: String) -> String {
    stride(from: 0, to: digits.count, by: 5).map { start in
      let from = digits.index(digits.startIndex, offsetBy: start)
      return String(digits[from..<(digits.index(from, offsetBy: 5, limitedBy: digits.endIndex) ?? digits.endIndex)])
    }.joined(separator: " ")
  }

  private func load() async {
    guard let client = accounts.client, let user = friend.user else { return }
    info = try? await Hub.run {
      Info(
        accountKey: client.accountKeyOf(user: user),
        safetyNumber: try? client.safetyNumber(who: user),
        verified: client.contacts().first { $0.user == user }?.verified ?? false,
        chats: client.chatsWith(user: user))
    }
  }
}
