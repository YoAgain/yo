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

  /// Laid out like iOS Settings › Blocked: whether strangers may ask, block one, then the blocked contacts.
  var body: some View {
    SettingsForm { sections }
      .navigationTitle("Blocked")
      .navigationBarTitleDisplayMode(.inline)
      .confirmCard(
        $unblocking, title: { "Unblock \($0.name)?" }, action: { _ in "Unblock" },
        run: { unblock($0) })
      .task(id: accounts.client.map(ObjectIdentifier.init)) {
        guard let client = accounts.client else { return }
        requestsOpen = (try? await Hub.run { client.requestsOpen() }) ?? true
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
        SettingsNavigationLabel { Text("Block Contact…") }
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
            HStack(spacing: 12) {
              Avatar(name: friend.name, size: 36)
              Text(friend.name).lineLimit(1)
              Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .contentShape(Rectangle())
          }
          .foregroundStyle(Color(.label))
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

  /// Contacts that can be blocked; a tap asks, then blocks and goes back.
  private var pickToBlock: some View {
    SettingsForm {
      Section {
        let candidates = friends.contacts.filter { !$0.blocked && $0.group == nil }
        if candidates.isEmpty {
          Text("No Contacts").foregroundStyle(.secondary)
        } else {
          ForEach(candidates) { friend in
            Button {
              blockingNew = friend
            } label: {
              HStack(spacing: 12) {
                Avatar(name: friend.name, size: 36)
                Text(friend.name).lineLimit(1)
                Spacer(minLength: 0)
              }
              .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
              .contentShape(Rectangle())
            }
            .foregroundStyle(Color(.label))
          }
        }
      }
    }
    .navigationTitle("Block Contact")
    .navigationBarTitleDisplayMode(.inline)
    .confirmCard(
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
