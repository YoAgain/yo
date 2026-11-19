import LocalAuthentication
import SwiftUI
import UIKit
import YoCore

@MainActor
struct AccountPage: View {
  let account: Account
  let hasRecoveryKey: Bool
  var securityOnly = false
  var dismissSettings: () -> Void
  /// Pushes Account Recovery through the settings path.
  var openSecurity: (() -> Void)? = nil
  @Environment(AccountStore.self) private var accounts
  @State private var signingOut: Account?
  @State private var addingPasskey = false
  @State private var passkeyProblem: String?
  @State private var securing = false
  @State private var showingWords = false
  @State private var recoveryKey = ""
  @State private var turningOffPasskey = false
  @State private var renewingWords = false
  @State private var deleteTyped = ""
  @State private var deletingAccount = false
  @Binding var showingDelete: Bool
  @State private var deleteProblem: String?
  @State private var syncFailed = false
  @State private var showingSecurity = false
  /// The guide finished with Done here: Security opens once its sheet is gone.
  @State private var securityAfterGuide = false
  @State private var showingRecoveryHelp = false

  /// Privacy, security and backup, then signing out and deleting (App Review 5.1.1(v)).
  var body: some View {
    Form {
      if securityOnly {
        securitySection(account)
      } else {
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
          Button {
            if accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey) {
              securing = true
            } else {
              showSecurity()
            }
          } label: {
            HStack {
              Label("Account Recovery", systemImage: "lock.fill").labelStyle(Tile(.orange))
              Spacer(minLength: 8)
              if accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey) { RecoveryBadge() }
            }
          }
          .tint(Color(.label))
          .accessibilityIdentifier("account.security")
          // App Review 5.1.1(v): deleting is easy to find, no page between.
          Button {
            signingOut = account
          } label: {
            Label("Sign Out", systemImage: "rectangle.portrait.and.arrow.right").labelStyle(Tile(.gray))
          }
          .tint(Color(.label))
          .disabled(addingPasskey || deletingAccount)
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
          .disabled(addingPasskey || deletingAccount)
          .accessibilityLabel("Delete Your Account")
          .accessibilityIdentifier("account.delete")
        }
      }
    }
    .accessibilityIdentifier(securityOnly ? "security.page" : "account.page")
    .navigationTitle(securityOnly ? "Account Recovery" : account.username)
    .sheetFit()
    .navigationBarTitleDisplayMode(.inline)
    .toolbar {
      ToolbarItem(placement: .principal) {
        if securityOnly {
          Text("Account Recovery").font(.headline)
        } else {
          RoundTripTitle(name: account.username, client: accounts.client)
        }
      }
    }
    .problemAlert($passkeyProblem)
    .sheet(isPresented: $showingWords) {
      if let account = accounts.account {
        RecoveryShowSheet(name: account.username, key: recoveryKey)
      }
    }
    .alert("Turn Off Passkey?", isPresented: $turningOffPasskey) {
      Button("Turn Off", role: .destructive) { removePasskey() }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        accounts.wordsSaved
          ? "Delete the passkey in your password manager too."
          : "Only iCloud Keychain will remain. If you lose access to it, you may lose your account. Delete the passkey in your password manager too."
      )
    }
    .alert("Turn Off Recovery Key?", isPresented: $renewingWords) {
      Button("Turn Off", role: .destructive) {
        Task {
          do { _ = try await accounts.newRecoveryKey() } catch { show(error) }
        }
      }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        account.byPasskey && account.passkeyAuth == true
          ? "Your saved recovery key will stop working."
          : "Your saved recovery key will stop working. Only iCloud Keychain will remain. If you lose access to it, you may lose your account."
      )
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
    .navigationDestination(isPresented: $showingSecurity) {
      AccountPage(
        account: account, hasRecoveryKey: hasRecoveryKey, securityOnly: true,
        dismissSettings: dismissSettings, showingDelete: $showingDelete)
    }
    .confirmDestructive(
      $signingOut, title: { _ in "Sign Out?" },
      message: { $0.secured ? "" : "Your account isn’t secured. Set up recovery methods to access your account." },
      action: { _ in "Sign Out" }, run: { _ in reset() })
    .interactiveDismissDisabled(addingPasskey)
    .sheet(isPresented: $showingRecoveryHelp) { RecoveryHelpSheet() }
    .sheet(isPresented: $securing, onDismiss: {
      if securityAfterGuide {
        securityAfterGuide = false
        showSecurity()
      }
    }) {
      // Done lands in Security, where its switches are; opened from Security, it just closes back to it.
      SecureNameSheet(close: { securing = false }, done: { securityAfterGuide = !securityOnly })
    }
  }

  private func showSecurity() {
    if let openSecurity { openSecurity() } else { showingSecurity = true }
  }

  @ViewBuilder private func securitySection(_ account: Account) -> some View {
    let wordsDone = account.secured && accounts.wordsSaved && hasRecoveryKey
    let passkeyDone = account.byPasskey && account.passkeyAuth == true
    Section {
      AccountSyncRow(client: accounts.client, failed: $syncFailed)
        .accessibilityIdentifier("security.accountSync")
    }
    Section {
      Toggle(
        isOn: Binding(
          get: { passkeyDone },
          set: { if $0 { securing = true } else { turningOffPasskey = true } })
      ) {
        Label {
          Text("Passkey")
        } icon: { Image(systemName: "faceid") }
        .labelStyle(Tile(.amethyst))
      }
      .accessibilityIdentifier("security.passkey")
      Toggle(
        isOn: Binding(
          get: { wordsDone },
          set: { if $0 { securing = true } else { renewingWords = true } })
      ) {
        Button {
          if wordsDone { Task { await showRecoveryKey() } }
        } label: {
          Label {
            Text("Recovery Key")
          } icon: { Image(systemName: "key.fill") }
          .labelStyle(Tile(.amethyst))
            .frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
      }
      .accessibilityIdentifier("security.recoveryKey")
      SecurityRow(
        icon: "icloud.fill", color: .amethyst, title: "iCloud Keychain",
        value: accounts.keychainRecoveryActive ? "On" : "Off",
        done: accounts.keychainRecoveryActive)
        .accessibilityIdentifier("security.keychainRecovery")
    } footer: {
      Button("How Account Recovery Works") { showingRecoveryHelp = true }
        .font(.footnote)
        .foregroundStyle(.tint)
        .accessibilityIdentifier("security.howRecoveryWorks")
    }
    .disabled(addingPasskey)
  }

  private func showRecoveryKey() async {
    guard await authenticated("Show your recovery key."), let key = RecoveryWords.load() else { return }
    recoveryKey = key
    showingWords = true
  }

  /// One backup change at a time (`addingPasskey` disables the security rows meanwhile).
  private func removePasskey() {
    guard !addingPasskey else { return }
    addingPasskey = true
    Task {
      defer { addingPasskey = false }
      do { try await accounts.removePasskey() } catch { show(error) }
    }
  }

  /// A security change failed (not a cancellation, nor a sign-out meanwhile).
  private func show(_ error: Error) {
    guard !Passkeys.isCancellation(error) else { return }
    passkeyProblem =
      if case .noPRF? = error as? AccountError {
        "Your password manager can’t do this. Try iCloud Keychain."
      } else {
        error.localizedDescription
      }
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
    .sheetFit()
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
  /// Neither the passkey nor a saved recovery key can bring the account back.
  func recoveryNeedsAttention(hasRecoveryKey: Bool) -> Bool {
    let passkeyOn = account?.byPasskey == true && account?.passkeyAuth == true
    let keyOn = account?.secured == true && wordsSaved && hasRecoveryKey
    return !passkeyOn && !keyOn
  }
}
