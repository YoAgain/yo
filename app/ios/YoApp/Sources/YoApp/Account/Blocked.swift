import SwiftUI
import UIKit
import YoCore

/// Blocking: block a contact, refuse strangers, the blocked contacts (spec 4.8); its own page under Yo ID.
@MainActor
struct BlockedPage: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @Environment(MediaOutbox.self) private var outbox
  @State private var problem: String?
  @State private var requestsOpen: Bool?

  @State private var unblocking: Friend?
  @State private var blockingNew: Friend?

  /// Laid out like iOS Settings › Blocked.
  var body: some View {
    Form { sections }
      .navigationTitle("Blocked")
      .sheetFit()
      .navigationBarTitleDisplayMode(.inline)
      .confirmDestructive(
        $unblocking, title: { "Unblock \($0.name)?" }, action: { _ in "Unblock" },
        run: { unblock($0) })
      .task(id: accounts.client.map(ObjectIdentifier.init)) {
        guard let client = accounts.client else { return }
        requestsOpen = (try? await Hub.run { client.requestsOpen() }) ?? true
      }
      .problemAlert($problem)
  }

  @ViewBuilder private var sections: some View {
    Section {
      Toggle(
        "Allow Yo Requests",
        isOn: Binding(get: { requestsOpen ?? true }, set: { setRequestsOpen($0) })
      )
      .disabled(requestsOpen == nil)
      NavigationLink {
        pickToBlock
      } label: {
        Text("Block Contact…")
      }
      .accessibilityIdentifier("blocked.add")
    }
    Section("Blocked Contacts") {
      if friends.blocked.isEmpty {
        Text("No Blocked Contacts").foregroundStyle(.secondary)
      } else {
        ForEach(friends.blocked) { friend in
          Button {
            unblocking = friend
          } label: {
            ContactRow(name: friend.name)
          }
          .tint(Color(.label))
        }
      }
    }
  }

  private func setRequestsOpen(_ open: Bool) {
    guard let client = accounts.client else { return }
    requestsOpen = open
    Task { @MainActor in
      if (try? await Hub.run { try client.setRequestsOpen(open: open) }) == nil {
        requestsOpen = (try? await Hub.run { client.requestsOpen() }) ?? true
      }
    }
  }

  private var pickToBlock: some View {
    Form {
      Section {
        let candidates = friends.contacts.filter { !$0.blocked && $0.group == nil }
        if candidates.isEmpty {
          Text("No Contacts").foregroundStyle(.secondary)
        } else {
          ForEach(candidates) { friend in
            Button {
              blockingNew = friend
            } label: {
              ContactRow(name: friend.name)
            }
            .tint(Color(.label))
          }
        }
      }
    }
    .navigationTitle("Block Contact")
    .sheetFit()
    .navigationBarTitleDisplayMode(.inline)
    .confirmDestructive(
      $blockingNew, title: { "Block \($0.name)?" }, message: { _ in "You won’t get their Yos." },
      action: { _ in "Block" }, run: { block($0) })
  }

  private func block(_ friend: Friend) {
    Task {
      do {
        try await accounts.setBlocked(friend.user ?? friend.name, true)
        withAnimation { friends.setBlocked(friend, true) }
      } catch {
        problem = error.localizedDescription
      }
    }
  }

  private func unblock(_ friend: Friend) {
    Task {
      do {
        try await accounts.setBlocked(friend.user ?? friend.name, false)
        withAnimation { friends.setBlocked(friend, false) }
        outbox.retryNow()  // photos taken for them while blocked go now
      } catch {
        problem = error.localizedDescription
      }
    }
  }
}

private struct ContactRow: View {
  let name: String

  var body: some View {
    HStack(spacing: 12) {
      Avatar(name: name, size: 36)
      Text(name).lineLimit(1)
    }
  }
}
