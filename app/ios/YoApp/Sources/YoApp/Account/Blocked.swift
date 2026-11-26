import SwiftUI
import UIKit
import YoCore

/// Blocking: block a contact, refuse strangers, the blocked contacts (spec 3.3); its own page under Yodentity › the account.
@MainActor
struct BlockedPage: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @Environment(MediaOutbox.self) private var outbox
  @State private var requestsOpen: Bool?

  /// Laid out like iOS Settings › Blocked.
  var body: some View {
    Form { sections }
      .navigationTitle("Blocked")
      .navigationBarTitleDisplayMode(.inline)
      .task(id: accounts.client.map(ObjectIdentifier.init)) {
        guard let client = accounts.client else { return }
        requestsOpen = (try? await Hub.run { client.requestsOpen() }) ?? true
      }
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
          ContactRow(friend: friend, action: "Unblock", run: unblock)
        }
      }
    }
  }

  private func setRequestsOpen(_ open: Bool) {
    guard let client = accounts.client else { return }
    requestsOpen = open
    Task { @MainActor in
      do {
        try await Hub.run { try client.setRequestsOpen(open: open) }
      } catch {
        Problems.shared.report(error)
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
            ContactRow(friend: friend, action: "Block", message: "You won’t get their Yos.", run: block)
          }
        }
      }
    }
    .navigationTitle("Block Contact")
    .navigationBarTitleDisplayMode(.inline)
  }

  private func block(_ friend: Friend) {
    Task {
      do {
        try await accounts.setBlocked(friend.user ?? friend.name, true)
        withAnimation { friends.setBlocked(friend, true) }
      } catch {
        Problems.shared.report(error)  // the one banner; an alert as well said it twice
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
        Problems.shared.report(error)  // the one banner; an alert as well said it twice
      }
    }
  }
}

private struct ContactRow: View {
  let friend: Friend
  let action: String
  var message: String? = nil
  let run: (Friend) -> Void
  @State private var confirming: Friend?

  var body: some View {
    Button {
      confirming = friend
    } label: {
      HStack(spacing: 12) {
        Avatar(name: friend.name, size: 36)
        Text(friend.name).lineLimit(1)
      }
    }
    .tint(Color(.label))
    .confirmDestructive(
      $confirming, title: { "\(action) \($0.name)?" }, message: { _ in message ?? "" },
      action: { _ in action }, run: run)
  }
}
