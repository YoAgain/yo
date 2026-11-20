import LocalAuthentication
import SwiftUI
import UIKit
import YoCore

@MainActor
struct AccountPage: View {
  let account: Account
  let hasRecoveryKey: Bool
  var dismissSettings: () -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var signingOut: Account?
  @State private var deleteTyped = ""
  @State private var deletingAccount = false
  @Binding var showingDelete: Bool
  @State private var deleteProblem: String?
  @State private var syncFailed = false
  @State private var showingRecoveryKey = false
  @State private var showingPasskey = false
  @State private var showingRecoveryHelp = false

  /// Privacy, backup and recovery, then signing out and deleting (App Review 5.1.1(v)).
  var body: some View {
    let keySaved = account.secured && accounts.wordsSaved && hasRecoveryKey
    let passkeyOn = account.byPasskey && account.passkeyAuth == true
    Form {
      Section {
        NavigationLink(value: SettingsView.Page.contacts) {
          LabeledContent {
            ContactSecurityPage(summaryOnly: true)
          } label: {
            Label("Contacts", systemImage: "person.2.fill").labelStyle(Tile(.green))
          }
        }
        .accessibilityIdentifier("account.contacts")
        NavigationLink {
          BlockedPage()
        } label: {
          Label("Blocked", systemImage: "hand.raised.fill").labelStyle(Tile(.red))
        }
        .accessibilityIdentifier("account.blocked")
      }
      Section {
        AccountSyncRow(client: accounts.client, failed: $syncFailed)
          .accessibilityIdentifier("security.accountSync")
      }
      Section {
        RecoveryRow(
          icon: "key.fill", title: ProductText.recoveryKey.english,
          value: (keySaved ? ProductText.saved : .notSaved).english,
          badge: accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey)
        ) { showingRecoveryKey = true }
        .accessibilityIdentifier("security.recoveryKey")
        RecoveryRow(
          icon: "faceid", title: ProductText.passkey.english, value: (passkeyOn ? ProductText.on : .off).english
        ) { showingPasskey = true }
        .accessibilityIdentifier("security.passkey")
        SecurityRow(
          icon: "icloud.fill", color: .amethyst, title: "iCloud Keychain",
          value: (accounts.keychainRecoveryActive ? ProductText.on : .off).english,
          done: accounts.keychainRecoveryActive)
          .accessibilityIdentifier("security.keychainRecovery")
      } header: {
        HStack {
          Text(ProductText.accountRecovery.english)
          Spacer()
          Button("How Account Recovery Works", systemImage: "info.circle") { showingRecoveryHelp = true }
            .labelStyle(.iconOnly)
            .accessibilityIdentifier("security.howRecoveryWorks")
        }
      } footer: {
        Text(ProductText.recoveryFooter.english)
      }
      Section {
        // App Review 5.1.1(v): deleting is easy to find, no page between.
        Button {
          signingOut = account
        } label: {
          Label("Sign Out", systemImage: "rectangle.portrait.and.arrow.right").labelStyle(Tile(.gray))
        }
        .tint(Color(.label))
        .disabled(deletingAccount)
        .accessibilityIdentifier("account.signOut")
      } footer: {
        // A footer link as iOS Settings writes them, still easy to find (App Review 5.1.1(v)).
        Button {
          showingDelete = true
        } label: {
          (Text("Leaving Yo for good? ").foregroundStyle(.secondary)
            + Text("Delete Your Account").foregroundStyle(Color.amethyst))
            .font(.footnote)
        }
        .buttonStyle(.plain)
        .disabled(deletingAccount)
        .accessibilityLabel("Delete Your Account")
        .accessibilityIdentifier("account.delete")
      }
    }
    .accessibilityIdentifier("account.page")
    .navigationTitle(account.username)
    .navigationBarTitleDisplayMode(.inline)
    .toolbar {
      ToolbarItem(placement: .principal) {
        RoundTripTitle(name: account.username, client: accounts.client)
      }
    }
    // Deleting gets the whole height, its own sheet over the settings.
    .sheet(isPresented: $showingDelete) {
      NavigationStack {
        deletePage(account)
          .toolbar {
            ToolbarItem(placement: .cancellationAction) {
              Button("Cancel", systemImage: "xmark") { showingDelete = false }
            }
          }
      }
      .presentationDetents([.large])
      .presentationDragIndicator(.hidden)
    }
    .confirmDestructive(
      $signingOut, title: { _ in "Sign Out?" },
      message: { $0.secured ? "" : "Your account isn’t secured. Set up recovery methods to access your account." },
      action: { _ in "Sign Out" }, run: { _ in reset() })
    .sheet(isPresented: $showingRecoveryHelp) { RecoveryHelpSheet() }
    .sheet(isPresented: $showingRecoveryKey) { RecoveryKeySheet(close: { showingRecoveryKey = false }) }
    .sheet(isPresented: $showingPasskey) { PasskeySheet(close: { showingPasskey = false }) }
  }

  private func deletePage(_ account: Account) -> some View {
    let confirmed = Username.skeleton(deleteTyped) == Username.skeleton(account.username)
    return Form {
      Section {
        VStack(spacing: 16) {
          Image(systemName: "exclamationmark.triangle.fill")
            .font(.system(size: 56))
            .foregroundStyle(Color.red)
            .accessibilityHidden(true)
          Text("Delete Your Account")
            .font(.largeTitle.bold())
            .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 16)
        .listRowBackground(Color.clear)
        .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
      }
      Section {
        Label("Username “\(account.username)” is free again.", systemImage: "person.crop.circle.badge.xmark")
        Label("Friends can’t reach you anymore.", systemImage: "person.2.slash")
        Label("Can’t be undone.", systemImage: "exclamationmark.triangle")
        if account.byPasskey {
          Label("Delete the passkey yourself.", systemImage: "key")
        }
      }
      Section {
        TextField("Type \(account.username) to confirm.", text: $deleteTyped)
          .textInputAutocapitalization(.never)
          .autocorrectionDisabled()
      }
      Section {
        Button(role: .destructive) {
          Task { await deleteAccount() }
        } label: {
          if deletingAccount {
            ProgressView().frame(maxWidth: .infinity)
          } else {
            Text("Delete Account").frame(maxWidth: .infinity)
          }
        }
        .tint(Color.red)
        .disabled(!confirmed || deletingAccount)
      }
    }
    .navigationTitle("Delete Account")
    .navigationBarTitleDisplayMode(.inline)
    .onDisappear { deleteTyped = "" }
    .problemAlert("Unable to Delete Account", $deleteProblem)
  }

  private func deleteAccount() async {
    guard await authenticated("Delete your account.") else { return }
    deletingAccount = true
    defer { deletingAccount = false }
    do {
      try await accounts.deleteAccount()
      reset()
    } catch {
      deleteProblem = error.localizedDescription
    }
  }

  private func authenticated(_ reason: String) async -> Bool {
    (try? await LAContext().evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)) == true
  }

  private func reset() {
    if let domain = Bundle.main.bundleIdentifier {
      UserDefaults.standard.removePersistentDomain(forName: domain)
    }
    Onboarding.signedOutThisRun = true  // no silent sign-in until the next cold start
    dismissSettings()
    accounts.signOut()
  }
}

extension AccountStore {
  /// Neither a saved recovery key nor the passkey can bring the account back: the badge asks for the key, which
  /// alone is enough.
  func recoveryNeedsAttention(hasRecoveryKey: Bool) -> Bool {
    let passkeyOn = account?.byPasskey == true && account?.passkeyAuth == true
    let keyOn = account?.secured == true && wordsSaved && hasRecoveryKey
    return !passkeyOn && !keyOn
  }
}

/// A recovery method and its state; a tap opens its screen.
private struct RecoveryRow: View {
  let icon: String
  let title: String
  let value: String
  var badge = false
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      LabeledContent {
        HStack(spacing: 8) {
          if badge { RecoveryBadge() }
          Text(value).foregroundStyle(.secondary).lineLimit(1)
          Image(systemName: "chevron.forward")
            .font(.footnote.weight(.semibold))
            .foregroundStyle(.tertiary)
            .accessibilityHidden(true)
        }
      } label: {
        Label(title, systemImage: icon).labelStyle(Tile(.amethyst))
      }
    }
    .tint(Color(.label))
  }
}
