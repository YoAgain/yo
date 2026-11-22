import LocalAuthentication
import SwiftUI
import UIKit
import YoCore

@MainActor
struct AccountPage: View {
  let account: Account
  let hasRecoveryKey: Bool
  var dismissSettings: () -> Void
  /// Opens a full page as its own sheet over the half sheet.
  var openSheet: (SettingsView.Page) -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var signingOut: Account?
  @State private var showingRecoveryKey = false
  /// The key kept there with Done: its sheet gone, Account Security opens, where it now shows on.
  @State private var keptKey = false
  @State private var showingSecurity = false
  @State private var deleteTyped = ""
  @State private var deletingAccount = false
  @Binding var showingDelete: Bool
  @State private var deleteProblem: String?

  /// Privacy, then Account Security, then signing out and deleting (App Review 5.1.1(v)).
  var body: some View {
    Form {
      Section {
        // A full page: only its own sheet, no push into the half sheet.
        Button {
          openSheet(.contacts)
        } label: {
          ChevronRow {
            HStack(spacing: 8) {
              ContactSecurityPage(summaryOnly: true)
              DisclosureChevron()
            }
          } label: {
            Label("Contacts", systemImage: "person.2.fill").labelStyle(Tile(.green))
          }
        }
        .tint(Color(.label))
        .accessibilityIdentifier("account.contacts")
        NavigationLink {
          BlockedPage()
        } label: {
          Label("Blocked", systemImage: "hand.raised.fill").labelStyle(Tile(.red))
        }
        .accessibilityIdentifier("account.blocked")
      }
      Section {
        // Backup and the ways back on a new phone, on a page of their own; the row says whether they are in place.
        // With the red badge it opens the recovery key's screen itself.
        if accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey) {
          Button { showingRecoveryKey = true } label: {
            ChevronRow {
              HStack(spacing: 8) {
                RecoveryBadge()
                DisclosureChevron()
              }
            } label: {
              Label("Account Security", systemImage: "shield.fill").labelStyle(Tile(.amethyst))
            }
          }
          .tint(Color(.label))
          .accessibilityIdentifier("account.security")
        } else {
          NavigationLink(value: SettingsView.Page.security) {
            LabeledContent {
              Image(systemName: Check.ok.symbol).foregroundStyle(Check.ok.color).accessibilityLabel("Set Up")
            } label: {
              Label("Account Security", systemImage: "shield.fill").labelStyle(Tile(.amethyst))
            }
          }
          .accessibilityIdentifier("account.security")
        }
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
        // On the row, so its popover points at Sign Out (on the page it pointed at the title).
        .confirmDestructive(
          $signingOut, title: { _ in "Sign Out?" },
          message: { $0.secured ? "" : "Your account isn’t secured. Set up recovery methods to access your account." },
          action: { _ in "Sign Out" }, run: { _ in reset() })
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
      .systemAppearance().appObjects()
    }
    .sheet(
      isPresented: $showingRecoveryKey,
      onDismiss: {
        if keptKey {
          keptKey = false
          showingSecurity = true
        }
      }
    ) {
      RecoveryKeySheet(
        close: { showingRecoveryKey = false },
        done: {
          keptKey = true
          showingRecoveryKey = false
        }
      ).systemAppearance().appObjects()
    }
    .navigationDestination(isPresented: $showingSecurity) {
      AccountSecurityPage(account: account, hasRecoveryKey: hasRecoveryKey)
    }
  }

  private func deletePage(_ account: Account) -> some View {
    let confirmed = Username.skeleton(deleteTyped) == Username.skeleton(account.username)
    // The button stays in sight above the keyboard while the name is typed, and Return deletes.
    return ScrollViewReader { proxy in
      Form {
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
            .submitLabel(.go)
            .onSubmit {
              if confirmed && !deletingAccount { Task { await deleteAccount() } }
            }
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
          .id(Self.deleteButton)
        }
      }
      // Once the keyboard is up (and the form's bottom inset with it), not on a timer.
      .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardDidShowNotification)) { _ in
        withAnimation { proxy.scrollTo(Self.deleteButton, anchor: .bottom) }
      }
    }
    .navigationTitle("Delete Account")
    .navigationBarTitleDisplayMode(.inline)
    .onDisappear { deleteTyped = "" }
    .problemAlert("Unable to Delete Account", $deleteProblem)
  }

  private static let deleteButton = "account.deleteButton"

  private func deleteAccount() async {
    guard await authenticated("Delete your account.") else { return }
    deletingAccount = true
    defer { deletingAccount = false }
    do {
      try await accounts.deleteAccount()
      reset()
    } catch {
      deleteProblem = Problems.words(for: error)
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

/// Account Security: Yo's own backup, then the ways back on a new phone, each with its switch.
@MainActor
struct AccountSecurityPage: View {
  let account: Account
  let hasRecoveryKey: Bool
  @Environment(AccountStore.self) private var accounts
  @State private var syncFailed = false
  @State private var showingRecoveryKey = false
  @State private var showingPasskey = false
  @State private var showingRecoveryHelp = false
  @State private var turningOffKey = false
  @State private var turningOffPasskey = false
  @State private var busy = false
  @State private var problem: String?

  var body: some View {
    let keySaved = account.secured && accounts.wordsSaved && hasRecoveryKey
    let passkeyOn = account.byPasskey && account.passkeyAuth == true
    Form {
      Section {
        AccountSyncRow(client: accounts.client, failed: $syncFailed)
          .accessibilityIdentifier("security.accountSync")
      }
      Section {
        // On opens the key's screen (saved there through the share sheet); off makes the saved key stop working.
        Toggle(
          isOn: Binding(
            get: { keySaved },
            set: { if $0 { showingRecoveryKey = true } else { turningOffKey = true } })
        ) {
          HStack(spacing: 8) {
            Label(ProductText.recoveryKey.english, systemImage: "key.fill").labelStyle(Tile(.amethyst))
            if accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey) { RecoveryBadge() }
            // While on, the eye left of the switch opens the key's screen again, to show or save it.
            if keySaved {
              Spacer()
              Button { showingRecoveryKey = true } label: {
                // The eye alone was a 25 × 15 pt target beside the switch.
                Image(systemName: "eye").frame(minWidth: 44, minHeight: 44).contentShape(.rect)
              }
                .buttonStyle(.borderless)
                .tint(.amethyst)
                .accessibilityLabel(ProductText.showOrSaveKey.english)
                .accessibilityIdentifier("security.showKey")
            }
          }
        }
        .accessibilityIdentifier("security.recoveryKey")
        .accessibilityActions {
          if keySaved { Button(ProductText.showOrSaveKey.english) { showingRecoveryKey = true } }
        }
        // On opens the passkey's screen; it is on once tried there.
        Toggle(
          isOn: Binding(
            get: { passkeyOn },
            set: { if $0 { showingPasskey = true } else { turningOffPasskey = true } })
        ) {
          Label(ProductText.passkey.english, systemImage: "faceid").labelStyle(Tile(.amethyst))
        }
        .accessibilityIdentifier("security.passkey")
        // Follows the iCloud Keychain in iOS Settings; Yo has no switch of its own for it.
        Toggle(isOn: .constant(accounts.keychainRecoveryActive)) {
          Label("iCloud Keychain", systemImage: "icloud.fill").labelStyle(Tile(.amethyst))
        }
        .disabled(true)
        .accessibilityIdentifier("security.keychainRecovery")
      } header: {
        HStack {
          Text(ProductText.accountRecovery.english)
          Spacer()
          Button("How Account Recovery Works", systemImage: "info.circle") { showingRecoveryHelp = true }
            .labelStyle(.iconOnly)
            .contentShape(.rect.inset(by: -13))  // 18 pt drawn, 44 pt to touch, without a taller header
            .accessibilityIdentifier("security.howRecoveryWorks")
        }
      }
      .disabled(busy)
    }
    // Fits the half-height sheet without scrolling: compact sections from a small top margin.
    .listSectionSpacing(.compact)
    .contentMargins(.top, 8, for: .scrollContent)
    .scrollBounceBehavior(.basedOnSize)
    .accessibilityIdentifier("security.page")
    .navigationTitle("Account Security")
    .sheet(isPresented: $showingRecoveryHelp) { RecoveryHelpSheet().systemAppearance().appObjects() }
    .sheet(isPresented: $showingRecoveryKey) { RecoveryKeySheet(close: { showingRecoveryKey = false }).systemAppearance().appObjects() }
    .sheet(isPresented: $showingPasskey) { PasskeySheet(close: { showingPasskey = false }).systemAppearance().appObjects() }
    .alert("Turn Off Recovery Key?", isPresented: $turningOffKey) {
      Button("Turn Off", role: .destructive) { change { _ = try await accounts.newRecoveryKey() } }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        passkeyOn
          ? "Your saved recovery key will stop working."
          : "Your saved recovery key will stop working. Only iCloud Keychain will remain. If you lose access to it, you may lose your account."
      )
    }
    .alert("Turn Off Passkey?", isPresented: $turningOffPasskey) {
      Button("Turn Off", role: .destructive) { change { try await accounts.removePasskey() } }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        accounts.wordsSaved
          ? "Delete the passkey in your password manager too."
          : "Only iCloud Keychain will remain. If you lose access to it, you may lose your account. Delete the passkey in your password manager too."
      )
    }
    .problemAlert($problem)
  }

  /// One change at a time; the switches wait meanwhile.
  private func change(_ work: @escaping @MainActor () async throws -> Void) {
    guard !busy else { return }
    busy = true
    Task {
      defer { busy = false }
      do { try await work() } catch {
        if !Passkeys.isCancellation(error) { problem = Problems.words(for: error) }
      }
    }
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
