import SwiftUI
import UIKit
import YoCore

/// Backup status with a separate, explicit system button to write now.
struct AccountSyncRow: View {
  let client: YoClient?
  @Environment(Inbox.self) private var inbox
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @State private var savedAt: Date?
  @State private var unavailable = true
  @State private var saving = false
  @Binding var failed: Bool
  @State private var savedNow = false
  @State private var justSaved = false
  @State private var savedTime = "Not Saved"

  var body: some View {
    Group {
      LabeledContent {
        HStack(spacing: 8) {
          if failed {
            Text("Failed").foregroundStyle(.orange)
          } else {
            Text(unavailable ? "Unavailable" : justSaved ? "Just now" : savedTime)
              .foregroundStyle(.secondary)
          }
          Image(
            systemName: (unavailable || savedAt == nil || failed ? Check.waiting : Check.ok).symbol
          )
          .foregroundStyle(
            (unavailable || savedAt == nil || failed ? Check.waiting : Check.ok).color
          )
          .accessibilityHidden(true)
          if saving {
            ProgressView().controlSize(.small)
          } else {
            Button {
              Task { await save() }
            } label: {
              Image(
                systemName: failed
                  ? "exclamationmark.icloud" : savedNow ? "checkmark.icloud" : "icloud.and.arrow.up"
              )
              .foregroundStyle(failed ? Color.orange : savedNow ? Color.green : Color.secondary)
            }
            .buttonStyle(.borderless)
            .tint(.secondary)
            .accessibilityLabel("Account Sync Now")
            .disabled(client == nil || savedNow)
          }
        }
      } label: {
        Label {
          Text("Account Sync")
        } icon: {
          Image(systemName: "icloud.fill")
        }
        .labelStyle(Tile(.blue))
      }
    }
    .sensoryFeedback(.success, trigger: savedNow) { _, now in now }
    .sensoryFeedback(.error, trigger: failed) { _, now in now }
    .task(id: justSaved) {
      guard justSaved else { return }
      do { try await Task.sleep(for: .seconds(60)) } catch { return }
      justSaved = false
    }
    // A write from elsewhere (the inbox's sync) shows up.
    .task(id: client.map(ObjectIdentifier.init)) {
      while !Task.isCancelled {
        await load()
        do { try await Task.sleep(for: .seconds(5)) } catch { return }
      }
    }
  }

  private func load() async {
    guard let client else {
      unavailable = true
      return
    }
    do {
      let ms = try await Hub.run { try client.storageSavedAt() }
      savedAt = ms.map { Date(timeIntervalSince1970: Double($0) / 1000) }
      savedTime =
        savedAt.map { date in
          if Calendar.current.isDateInToday(date) {
            return date.formatted(date: .omitted, time: .shortened)
          }
          if Calendar.current.isDateInYesterday(date) { return "Yesterday" }
          return date.formatted(.dateTime.day().month(.abbreviated))
        } ?? "Pending"
      unavailable = false
    } catch { unavailable = true }
  }

  private func save() async {
    guard client != nil, !saving else { return }
    saving = true
    failed = false
    savedNow = false
    let before = savedAt
    let succeeded = await inbox.maintain(accounts: accounts, friends: friends)
    await load()
    failed = !succeeded || unavailable || savedAt == nil
    savedNow = !failed
    // "Just now" only when a newer backup really landed; nothing changed, the time it has stays (it is current).
    justSaved = !failed && savedAt != before
    saving = false
    if savedNow {
      do { try await Task.sleep(for: .seconds(1.5)) } catch { return }
      savedNow = false
    }
  }
}
