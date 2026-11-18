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
  /// Pushes Account Recovery through the settings path, so the sheet can grow before the push.
  var openSecurity: (() -> Void)? = nil
  @Environment(AccountStore.self) private var accounts
  @State private var resetting = false
  /// Sign Out asks on a card from the bottom (the account it is about).
  @State private var signingOut: Account?
  @State private var addingPasskey = false
  @State private var passkeyProblem: String?
  /// Securing and the words open as sheets.
  @State private var securing = false
  @State private var securingStart: SecureStep? = .recoveryKey
  @State private var showingWords = false
  @State private var recoveryKey = ""
  @State private var turningOffPasskey = false
  @State private var renewingWords = false
  @State private var deleteTyped = ""
  @State private var deletingAccount = false
  @Binding var showingDelete: Bool
  @Binding var showingManage: Bool
  @State private var deleteProblem: String?
  @State private var syncFailed = false
  @State private var showingSecurity = false
  /// The guide finished with Done from the account page: Security opens once its sheet is gone.
  @State private var securityAfterGuide = false
  @State private var showingRecoveryHelp = false
  @State private var showingBlocked = false

  /// Your account: privacy, security and backup, then signing out and deleting it (App Review
  /// 5.1.1(v)).
  var body: some View {
    SettingsForm {
      if securityOnly {
        securitySection(account)
      } else {
        Section {
          NavigationLink(value: SettingsView.Page.contacts) {
            SettingsNavigationLabel {
              LabeledContent {
                ContactSecurityPage(summaryOnly: true)
              } label: {
                Label("Contacts", systemImage: "person.2.fill").labelStyle(Tile(.green))
              }
            }
          }
          .accessibilityIdentifier("account.contacts")
          Button {
            showingBlocked = true
          } label: {
            SettingsNavigationLabel {
              Label("Blocked", systemImage: "hand.raised.fill").labelStyle(Tile(.red))
            }
          }
          .accessibilityIdentifier("account.blocked")
        }
        Section {
          Button {
            if recoveryNeedsAttention {
              securingStart = nil
              securing = true
            } else {
              showSecurity()
            }
          } label: {
            SettingsNavigationLabel {
              HStack {
                Label("Account Recovery", systemImage: "lock.fill").labelStyle(Tile(.orange))
                Spacer(minLength: 8)
                if recoveryNeedsAttention { RecoveryBadge() }
              }
            }
          }
          .accessibilityIdentifier("account.security")
          // Sign Out and Delete Account right here (App Review 5.1.1(v): deleting is easy to find), no page between.
          Button {
            signingOut = account
          } label: {
            Label("Sign Out", systemImage: "rectangle.portrait.and.arrow.right")
              .labelStyle(Tile(.gray))
              .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
              .contentShape(Rectangle())
          }
          .foregroundStyle(Color(.label))
          .disabled(addingPasskey || deletingAccount)
          .accessibilityIdentifier("account.signOut")
        } footer: {
          // Deleting stays easy to find (App Review 5.1.1(v)) without a row of its own: a footer link, as iOS
          // Settings writes them — grey words, the link in the tint colour.
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
    .navigationBarTitleDisplayMode(.inline)
    .toolbar(.visible, for: .navigationBar)
    .toolbar {
      ToolbarItem(placement: .principal) {
        if securityOnly {
          Text("Account Recovery").font(.headline)
        } else {
          RoundTripTitle(name: account.username, client: accounts.client)
        }
      }
    }
    .alert(
      "Something Went Wrong",
      isPresented: Binding(get: { passkeyProblem != nil }, set: { if !$0 { passkeyProblem = nil } })
    ) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(passkeyProblem ?? "")
    }
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
    // Deleting gets the whole height: its own sheet over the settings (App Review 5.1.1(v): in Security, easy to find).
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
    }
    .navigationDestination(isPresented: $showingSecurity) {
      AccountPage(
        account: account, hasRecoveryKey: hasRecoveryKey, securityOnly: true,
        dismissSettings: dismissSettings,
        showingDelete: $showingDelete, showingManage: $showingManage)
    }
    .confirmCard(
      $signingOut, title: { _ in "Sign Out?" },
      message: { $0.secured ? "" : "Your account isn’t secured. Set up recovery methods to access your account." },
      action: { _ in "Sign Out" }, run: { _ in reset() })
    .interactiveDismissDisabled(addingPasskey)
    .sheet(isPresented: $showingRecoveryHelp) { RecoveryHelpSheet() }
    .navigationDestination(isPresented: $showingBlocked) {
      BlockedPage()
    }
    .sheet(isPresented: $securing, onDismiss: {
      if securityAfterGuide {
        securityAfterGuide = false
        showSecurity()
      }
    }) {
      // Done lands in Security, where its switches are; opened from Security, it just closes back to it.
      SecureNameSheet(only: securingStart, close: { securing = false },
        done: { securityAfterGuide = !securityOnly })
    }
  }

  private func showSecurity() {
    if let openSecurity { openSecurity() } else { showingSecurity = true }
  }

  /// Account actions together at the end of the page.
  @ViewBuilder private func leaving(_ account: Account) -> some View {
    Section {
      Button {
        resetting = true
      } label: {
        Label("Sign Out", systemImage: "rectangle.portrait.and.arrow.right")
          .labelStyle(Tile(.gray))
          .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
          .contentShape(Rectangle())
      }
      .foregroundStyle(Color(.label))
      .alert("Sign Out?", isPresented: $resetting) {
        if !account.secured {
          Button("Set Up Recovery Key") { secure(.recoveryKey) }
        }
        Button("Sign Out", role: .destructive) { reset() }
        Button("Cancel", role: .cancel) {}
      } message: {
        if !account.secured {
          Text("Your account isn’t secured. Set up recovery methods to access your account.")
        }
      }
    }
    .disabled(addingPasskey || deletingAccount)
    Section {
      Button(role: .destructive) {
        showingDelete = true
      } label: {
        Label("Delete Account", systemImage: "trash.fill")
          .labelStyle(Tile(.red))
          .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
          .contentShape(Rectangle())
      }
      .foregroundStyle(Color.red)
    }
    .disabled(addingPasskey || deletingAccount)
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
          set: { on in
            if on {
              secure(.passkey)
            } else {
              turningOffPasskey = true
            }
          })
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
          set: { on in
            if on {
              secure(.recoveryKey)
            } else {
              renewingWords = true
            }
          })
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
      // As Settings links its explanations: blue, under the section (the link names the section; no header).
      Button("How Account Recovery Works") { showingRecoveryHelp = true }
        .font(.footnote)
        .foregroundStyle(.tint)
        .accessibilityIdentifier("security.howRecoveryWorks")
    }
    .disabled(addingPasskey)
  }

  private var recoveryNeedsAttention: Bool {
    let passkeyOn = accounts.account?.byPasskey == true && accounts.account?.passkeyAuth == true
    let keyOn = accounts.account?.secured == true && accounts.wordsSaved && hasRecoveryKey
    return !passkeyOn && !keyOn
  }

  private func secure(_ step: SecureStep) {
    securingStart = step
    securing = true
  }

  /// The words, after Face ID (or the passcode), straight on screen.
  private func showRecoveryKey() async {
    let context = LAContext()
    guard
      (try? await context.evaluatePolicy(
        .deviceOwnerAuthentication, localizedReason: "Show your recovery key."))
        == true
    else { return }
    guard let key = RecoveryWords.load() else { return }
    recoveryKey = key
    showingWords = true
  }

  /// One change of the backup at a time (`addingPasskey` disables the security rows and pages meanwhile).
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
    if !SecurityPresentation.isCancellation(error) {
      passkeyProblem = SecurityPresentation.passkeyMessage(
        error, fallback: error.localizedDescription)
    }
  }

  /// Deleting, on a page of its own: what it means, your name typed to confirm, then Face ID (or the passcode).
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
    .toolbar(.visible, for: .navigationBar)
    .onDisappear { deleteTyped = "" }
    .alert(
      "Unable to Delete Account",
      isPresented: Binding(get: { deleteProblem != nil }, set: { if !$0 { deleteProblem = nil } })
    ) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(deleteProblem ?? "")
    }
  }

  /// Deleted at the hub, then forgotten here as signing out forgets.
  private func deleteAccount() async {
    let context = LAContext()
    guard
      (try? await context.evaluatePolicy(
        .deviceOwnerAuthentication, localizedReason: "Delete your account."))
        == true
    else { return }
    deletingAccount = true
    defer { deletingAccount = false }
    do {
      try await accounts.deleteAccount()
      reset()
    } catch {
      deleteProblem = error.localizedDescription
    }
  }

  private func reset() {
    if let domain = Bundle.main.bundleIdentifier {
      UserDefaults.standard.removePersistentDomain(forName: domain)
    }
    Onboarding.signedOutThisRun = true  // no sign-in by itself until the app starts cold again
    dismissSettings()
    accounts.signOut()  // the friends go with the account (YoApp)
  }
}
