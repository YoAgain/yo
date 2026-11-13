import AuthenticationServices
import Foundation
import LocalAuthentication
import Photos
import SwiftUI
import UIKit

/// The iPhone's own appearance, whatever the app's window was set to (the list forces dark): read from the screen.
@MainActor enum SystemAppearance {
  static var scheme: ColorScheme {
    let screen = (UIApplication.shared.connectedScenes.first as? UIWindowScene)?.screen
    return screen?.traitCollection.userInterfaceStyle == .dark ? .dark : .light
  }
}

enum SettingKey {
  static let saveSent = "settings.saveSent"
}

@MainActor
struct SettingsView: View {
  @AppStorage(SettingKey.saveSent) private var saveSent = false
  @Environment(\.dismiss) private var dismiss
  @Environment(\.openURL) private var openURL
  @Environment(\.scenePhase) private var scenePhase
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends

  @State private var resetting = false
  @State private var requestingPhotos = false
  @State private var photosDenied = false
  @State private var addingPasskey = false
  @State private var passkeyProblem: String?
  /// No pages under the profile (as Signal keeps it flat): securing and the words open as sheets.
  @State private var securing = false
  @State private var securingStart: SecureStep?
  @State private var showingWords = false
  @State private var turningOffPasskey = false
  @State private var renewingWords = false
  @State private var confirmingDelete = false
  @State private var deletingAccount = false
  @State private var deleteProblem: String?

  var body: some View {
    NavigationStack {
      Form {
        if let account = accounts.account {
          // You, at the top: the picture and the name.
          Section {
            profileRow(account)
          }
          // Security: one row, with the badge while something is left. Until the passkey or the recovery key is
          // there, it opens the guide; after, the page with both.
          Section {
            let started = account.byPasskey || (account.secured && accounts.wordsSaved)
            if started {
              NavigationLink {
                Form { securitySection(account) }
                  .navigationTitle("Account Security")
                  .navigationBarTitleDisplayMode(.inline)
              } label: {
                securityRowLabel
              }
            } else {
              Button { secure(nil) } label: {
                HStack {
                  securityRowLabel
                  Chevron()
                }
              }
            }
          } footer: {
            if accounts.toDo > 0 {
              Text("Secure your account to keep your name “\(account.username)” forever.")
            }
          }
          Section {
            NavigationLink {
              BlockedList()
            } label: {
              HStack {
                Label("Blocked", systemImage: "hand.raised.fill").labelStyle(Tile(.red))
                Spacer()
                Text("\(friends.blocked.count)").foregroundStyle(Color(.secondaryLabel))
              }
            }
          }
        }

        // "Keep Sent Photos" comes back with the camera mode, once the core carries photos and videos (spec 5).

        // As Settings ends the Apple Account: Sign Out, red, on its own; the dialog says what happens.
        Section {
          Button("Sign Out", role: .destructive) { resetting = true }
            .frame(maxWidth: .infinity)
            .disabled(addingPasskey || deletingAccount)
        }
        // Below it, as Apple asks for (App Review 5.1.1(v)): gone for good, with everything the hub holds.
        if accounts.account != nil {
          Section {
            Button("Delete Account", role: .destructive) { confirmingDelete = true }
              .frame(maxWidth: .infinity)
              .disabled(addingPasskey || deletingAccount)
          }
        }
      }
      .confirmationDialog("Delete Account?", isPresented: $confirmingDelete, titleVisibility: .visible) {
        Button("Delete Account", role: .destructive) { Task { await deleteAccount() } }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text(deleteMessage)
      }
      .alert(
        "Unable to Delete Account",
        isPresented: Binding(get: { deleteProblem != nil }, set: { if !$0 { deleteProblem = nil } })
      ) {
        Button("OK", role: .cancel) {}
      } message: {
        Text(deleteProblem ?? "")
      }
      // Not secured all the way: the guide first, signing out anyway second.
      .confirmationDialog("Sign Out?", isPresented: $resetting, titleVisibility: .visible) {
        if accounts.toDo > 0 {
          Button("Secure Your Account") { secure(nil) }
          Button("Sign Out Anyway", role: .destructive) { reset() }
        } else {
          Button("Sign Out", role: .destructive) { reset() }
        }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text(resetMessage)
      }
      .navigationTitle("Profile")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Close") { dismiss() }
            .disabled(addingPasskey)
        }
      }
      .interactiveDismissDisabled(addingPasskey)
      .sheet(isPresented: $securing) { SecureNameSheet(start: securingStart) }
      .sheet(isPresented: $showingWords) {
        if let account = accounts.account, let words = RecoveryWords.load() {
          RecoveryShowSheet(name: account.username, words: words)
        }
      }
      .alert("Allow Access to Photos", isPresented: $photosDenied) {
        Button("Open Settings") { openSystemSettings() }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text(
          "To save photos and videos, allow Yo to add to your "
            + "photo library in Settings."
        )
      }
      .alert(
        "Unable to Turn On Face ID",
        isPresented: Binding(
          get: { passkeyProblem != nil },
          set: { if !$0 { passkeyProblem = nil } }
        )
      ) {
        Button("OK", role: .cancel) {}
      } message: {
        Text(passkeyProblem ?? "")
      }
      .onAppear { checkPhotosPermission() }
      .onChange(of: scenePhase) { _, phase in
        if phase == .active { checkPhotosPermission() }
      }
    }
    .tint(Color.amethyst)
  }

  /// The Account Security row: its tile (orange while something is left), the badge or "On" with a checkmark.
  private var securityRowLabel: some View {
    HStack {
      Label {
        Text("Account Security").foregroundStyle(Color(.label))
      } icon: {
        Image(systemName: "lock.fill")
      }
      .labelStyle(Tile(accounts.toDo > 0 ? .orange : .green))
      Spacer()
      if accounts.toDo > 0 {
        CountBadge(count: accounts.toDo)
      } else {
        Text("On").foregroundStyle(Color(.secondaryLabel))
        Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).accessibilityHidden(true)
      }
    }
  }

  /// A small picture and the name, with whether the account is secured.
  private func profileRow(_ account: Account) -> some View {
    HStack(spacing: 14) {
      Image(systemName: "person.crop.circle.fill")
        .font(.system(size: 44))
        .foregroundStyle(.white, Color.amethyst)
        .accessibilityHidden(true)
      VStack(alignment: .leading, spacing: 2) {
        Text(account.username)
          .font(.yo(24, relativeTo: .title3))
        Text(account.secured ? "Account Secured" : "Account Not Secured")
          .font(.subheadline)
          .foregroundStyle(.secondary)
      }
    }
    .padding(.vertical, 2)
  }

  /// Account Security: two groups, Passkey and Recovery Key, each one row with its state. A step not done yet opens
  /// the guide at it; one done opens its own page with what can be done with it (as Settings' Face ID & Passcode).
  @ViewBuilder private func securitySection(_ account: Account) -> some View {
    let wordsDone = account.secured && accounts.wordsSaved && RecoveryWords.load() != nil
    Section {
      if account.byPasskey {
        NavigationLink {
          securityPage("Passkey", footer: "Sign back in with Face ID on a new iPhone.") {
            Button("Replace Passkey") { Task { await replacePasskey() } }
            if wordsDone {
              Button("Turn Off Passkey", role: .destructive) { turningOffPasskey = true }
            }
          }
        } label: {
          SecurityRow(icon: "faceid", color: .green, title: "Passkey", value: "On", done: true, chevron: false)
        }
      } else {
        Button { secure(.passkey) } label: {
          SecurityRow(icon: "faceid", color: .orange, title: "Passkey", value: "Set Up")
        }
      }
    } footer: {
      Text("Sign back in with Face ID on a new iPhone.")
    }
    .disabled(addingPasskey)
    Section {
      if wordsDone {
        NavigationLink {
          securityPage("Recovery Key", footer: "12 words to sign back in without Face ID.") {
            Button("Show Recovery Key") { Task { await showRecoveryKey() } }
            Button("Create New Recovery Key") { renewingWords = true }
          }
        } label: {
          SecurityRow(icon: "key.fill", color: .green, title: "Recovery Key", value: "Saved", done: true, chevron: false)
        }
      } else {
        Button { secure(.words) } label: {
          SecurityRow(icon: "key.fill", color: .orange, title: "Recovery Key", value: "Save")
        }
      }
    } footer: {
      Text("12 words to sign back in without Face ID.")
    }
    .disabled(addingPasskey)
  }

  /// The page under Passkey or Recovery Key: what can be done with it, and the dialogs those ask.
  private func securityPage(
    _ title: String, footer: String, @ViewBuilder actions: () -> some View
  ) -> some View {
    Form {
      Section {
        actions()
      } footer: {
        Text(footer)
      }
      .disabled(addingPasskey)
    }
    .navigationTitle(title)
    .navigationBarTitleDisplayMode(.inline)
    .modifier(SecurityDialogs(
      turningOffPasskey: $turningOffPasskey, renewingWords: $renewingWords, turnOff: removePasskey, renew: newWords))
  }

  /// The guide, at a step or (nil) at the first one left.
  private func secure(_ step: SecureStep?) {
    securingStart = step
    securing = true
  }

  /// The words, after Face ID (or the passcode), straight on screen.
  private func showRecoveryKey() async {
    let context = LAContext()
    guard (try? await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "Show your recovery key."))
      == true
    else { return }
    showingWords = true
  }


  private func replacePasskey() async {
    addingPasskey = true
    defer { addingPasskey = false }
    do { try await accounts.replacePasskey() } catch {
      if !SecurityPresentation.isCancellation(error) {
        passkeyProblem = SecurityPresentation.passkeyMessage(error, fallback: "Try again.")
      }
    }
  }

  private func removePasskey() {
    Task {
      do { try await accounts.removePasskey() } catch { passkeyProblem = error.localizedDescription }
    }
  }

  private func newWords() {
    Task {
      do {
        _ = try await accounts.newRecoveryKey()
        secure(.words)  // the new words, to save in the guide
      } catch {
        passkeyProblem = error.localizedDescription
      }
    }
  }


  /// What signing out costs, in one line: whether, and how, the name comes back.
  private var resetMessage: String {
    guard let account = accounts.account, account.secured else {
      return "Your account isn’t secured. You won’t be able to sign in as "
        + "\(accounts.account?.username ?? "this name") again."
    }
    if !accounts.wordsSaved && !account.byPasskey {
      return "Your recovery key isn’t saved. You may not be able to sign back in."
    }
    return account.byPasskey
      ? "Sign back in with your passkey or recovery key. Friends on this iPhone are removed."
      : "Sign back in with your recovery key. Friends on this iPhone are removed."
  }



  private func addPasskey() async {
    guard !addingPasskey else { return }
    addingPasskey = true
    defer { addingPasskey = false }

    do {
      try await accounts.addPasskey()
    } catch {
      if SecurityPresentation.isCancellation(error) { return }
      passkeyProblem = SecurityPresentation.passkeyMessage(
        error,
        fallback: "Try again or sign in with your recovery key."
      )
    }
  }

  private func setSaveSent(_ enabled: Bool) {
    guard enabled else {
      saveSent = false
      return
    }
    guard !requestingPhotos else { return }

    requestingPhotos = true
    Task { @MainActor in
      defer { requestingPhotos = false }

      let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
      let allowed = status == .authorized || status == .limited
      saveSent = allowed
      photosDenied = !allowed
    }
  }

  private func checkPhotosPermission() {
    let status = PHPhotoLibrary.authorizationStatus(for: .addOnly)
    if saveSent && (status == .denied || status == .restricted) {
      saveSent = false
    }
  }

  private func openSystemSettings() {
    guard let url = URL(string: UIApplication.openSettingsURLString) else {
      return
    }
    openURL(url)
  }

  /// What deleting costs, in one line: everything, and the name for anyone.
  private var deleteMessage: String {
    let name = accounts.account?.username ?? "Your name"
    let passkey = accounts.account?.byPasskey == true ? " Delete the passkey in your password manager too." : ""
    return "This can’t be undone. “\(name)” becomes free for anyone.\(passkey)"
  }

  /// Deleted at the hub, then forgotten here as signing out forgets.
  private func deleteAccount() async {
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
    friends.reset()
    dismiss()
    accounts.reset()
  }
}

enum SecurityPresentation {
  static let passkeyExplanation =
    "Sign in on a new iPhone with Face ID."

  static let recoveryExplanation =
    "12 words to sign in without Face ID."

  static func isCancellation(_ error: Error) -> Bool {
    if error is CancellationError { return true }

    let error = error as NSError
    return error.domain == ASAuthorizationError.errorDomain
      && error.code == ASAuthorizationError.Code.canceled.rawValue
  }

  static func passkeyMessage(_ error: Error, fallback: String) -> String {
    if let error = error as? AccountError, case .noPRF = error {
      return "Your password manager can’t do this. Try iCloud Keychain."
    }
    return fallback
  }
}

// Retained for callers that present recovery words as a sheet.
@MainActor
struct RecoveryWordsSheet: View {
  let name: String
  let words: [String]

  @Environment(\.dismiss) private var dismiss

  var body: some View {
    NavigationStack {
      RecoveryKeyView(
        name: name,
        words: words,
        explanation: SecurityPresentation.recoveryExplanation
      )
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Close") { dismiss() }
        }
      }
    }
    .tint(Color.amethyst)
  }
}

@MainActor
struct RecoveryKeyView: View {
  let name: String
  let words: [String]
  let explanation: String

  var requiresAuthentication = true
  var finishTitle: String? = nil
  var onFinish: (() -> Void)? = nil
  var onTryPasskey: (() -> Void)? = nil

  @Environment(\.scenePhase) private var scenePhase
  @Environment(\.openURL) private var openURL

  @State private var revealed = false
  @State private var authenticating = false
  @State private var saving = false
  @State private var copied = false
  @State private var savedToPhotos = false
  @State private var acknowledged = false
  @State private var visible = false

  @State private var authenticationFailed = false
  @State private var saveProblem: String?
  @State private var needsPhotosPermission = false

  var body: some View {
    Form {
      Section {
        LabeledContent {
          Text(name)
            .font(.yo(17, relativeTo: .body))
            .fixedSize(horizontal: false, vertical: true)
        } label: {
          Text("Username")
        }
      } footer: {
        Text(explanation)
      }

      Section {
        Button {
          Task { await toggleVisibility() }
        } label: {
          HStack {
            Text(revealed ? "Hide Recovery Key" : "Show Recovery Key")
            Spacer()
            if authenticating {
              ProgressView()
                .accessibilityLabel("Showing Recovery Key")
            }
          }
        }
        .disabled(authenticating || saving)

        if revealed && scenePhase == .active {
          RecoveryQRView(words: words)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .accessibilityLabel("Recovery key QR code")
            .privacySensitive()

          ForEach(Array(words.enumerated()), id: \.offset) { index, word in
            LabeledContent {
              Text(word)
                .font(.system(.body, design: .monospaced))
            } label: {
              Text("\(index + 1)")
                .monospacedDigit()
                .foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Word \(index + 1), \(word)")
            .privacySensitive()
          }
        }
      } header: {
        Text("Recovery Key")
      } footer: {
        Text(
          "Anyone with these words can sign in as you."
        )
      }

      if revealed && scenePhase == .active {
        Section {
          Button {
            RecoveryWords.copy(words, name: name)
            copied = true
          } label: {
            Label(
              copied ? "Copied" : "Copy Recovery Key",
              systemImage: copied ? "checkmark" : "doc.on.doc"
            )
          }

          Button {
            Task { await save() }
          } label: {
            HStack {
              Label(
                saving
                  ? "Saving…"
                  : savedToPhotos ? "Saved to Photos" : "Save to Photos",
                systemImage:
                  savedToPhotos ? "checkmark" : "photo.badge.arrow.down"
              )
              Spacer()
              if saving { ProgressView() }
            }
          }
        }
        .disabled(saving)
      }

      if let finishTitle, let onFinish {
        let wordsOnly = onTryPasskey != nil

        Section {
          if wordsOnly {
            Toggle("I Saved My Recovery Key", isOn: $acknowledged)
              .disabled(!revealed || saving)
          }

          Button(finishTitle, action: onFinish)
            .disabled(
              (wordsOnly && !acknowledged) || saving || authenticating
            )
        } footer: {
          if wordsOnly {
            Text("Keep a copy outside this iPhone.")
          }
        }
      }

      if let onTryPasskey {
        Section {
          Button("Try Face ID Again", action: onTryPasskey)
            .disabled(saving || authenticating)
        }
      }
    }
    .navigationTitle("Recovery Key")
    .navigationBarTitleDisplayMode(.inline)
    .onAppear {
      visible = true
      if !requiresAuthentication && scenePhase == .active {
        revealed = true
      }
    }
    .onDisappear {
      visible = false
      revealed = false
    }
    .onChange(of: scenePhase) { _, phase in
      if phase != .active { revealed = false }
    }
    .alert(
      "Unable to Show Recovery Key",
      isPresented: $authenticationFailed
    ) {
      Button("OK", role: .cancel) {}
    } message: {
      Text("Try again using Face ID or your iPhone passcode.")
    }
    .alert(
      "Unable to Save Recovery Key",
      isPresented: Binding(
        get: { saveProblem != nil },
        set: { if !$0 { saveProblem = nil } }
      )
    ) {
      if needsPhotosPermission {
        Button("Open Settings") {
          guard let url = URL(
            string: UIApplication.openSettingsURLString
          ) else { return }
          openURL(url)
        }
      }
      Button("OK", role: .cancel) {}
    } message: {
      Text(saveProblem ?? "")
    }
  }

  private func toggleVisibility() async {
    guard !authenticating, !saving else { return }

    if revealed {
      revealed = false
      return
    }

    guard requiresAuthentication else {
      revealed = scenePhase == .active
      return
    }

    authenticating = true
    defer { authenticating = false }

    let context = LAContext()
    do {
      let allowed = try await context.evaluatePolicy(
        .deviceOwnerAuthentication,
        localizedReason: "Show your recovery key."
      )
      guard visible else { return }

      revealed = allowed && scenePhase == .active
      if !allowed { authenticationFailed = true }
    } catch {
      guard visible else { return }

      if let error = error as? LAError {
        switch error.code {
        case .userCancel, .appCancel, .systemCancel:
          return
        default:
          break
        }
      }
      authenticationFailed = true
    }
  }

  private func save() async {
    guard !saving, revealed, scenePhase == .active else { return }
    saving = true
    needsPhotosPermission = false
    defer { saving = false }

    let renderer = ImageRenderer(
      content: RecoveryCard(name: name, words: words)
    )
    renderer.scale = 3

    guard let image = renderer.uiImage else {
      saveProblem = "Try again or copy your recovery key."
      return
    }

    do {
      try await RecoveryWords.saveToPhotos(image)
      savedToPhotos = true
    } catch {
      let status = PHPhotoLibrary.authorizationStatus(for: .addOnly)
      needsPhotosPermission = status == .denied || status == .restricted
      saveProblem = needsPhotosPermission
        ? "Allow Yo to add to your photo library in Settings, "
          + "or copy your recovery key."
        : "Try again or copy your recovery key."
    }
  }
}

// Existing sent-media behavior.
enum Library {
  static func saveIfWanted(photo: Data? = nil, video: URL? = nil) {
    guard UserDefaults.standard.bool(forKey: SettingKey.saveSent) else {
      return
    }

    var file: URL?
    if let video {
      let destination = FileManager.default.temporaryDirectory
        .appending(path: "library-\(UUID().uuidString).mov")

      guard
        (try? FileManager.default.copyItem(
          at: video,
          to: destination
        )) != nil
      else { return }

      file = destination
    }

    let copy = file
    PHPhotoLibrary.shared().performChanges {
      let request = PHAssetCreationRequest.forAsset()

      if let photo {
        request.addResource(with: .photo, data: photo, options: nil)
      } else if let copy {
        let options = PHAssetResourceCreationOptions()
        options.shouldMoveFile = true
        request.addResource(with: .video, fileURL: copy, options: options)
      }
    } completionHandler: { saved, _ in
      if !saved, let copy {
        try? FileManager.default.removeItem(at: copy)
      }
    }
  }
}

/// The friends blocked on this phone (spec 4.8), as Settings lists blocked contacts: swipe to unblock, and they are
/// back on the list.
@MainActor
private struct BlockedList: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @State private var problem: String?

  var body: some View {
    Form {
      if friends.blocked.isEmpty {
        ContentUnavailableView("No Blocked Friends", systemImage: "hand.raised")
      } else {
        Section {
          ForEach(friends.blocked) { friend in
            Text(friend.name)
              .font(.yo(17, relativeTo: .body))
              .swipeActions {
                Button("Unblock") { unblock(friend) }
              }
          }
        } footer: {
          Text("Blocked friends can’t send you Yos. Swipe to unblock.")
        }
      }
    }
    .navigationTitle("Blocked")
    .navigationBarTitleDisplayMode(.inline)
    .alert("Unable to Unblock", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(problem ?? "")
    }
  }

  private func unblock(_ friend: Friend) {
    Task {
      do {
        try await accounts.setBlocked(friend.user ?? friend.name, false)
        withAnimation { friends.setBlocked(friend, false) }
      } catch {
        problem = error.localizedDescription
      }
    }
  }
}

/// A row's symbol as Settings draws it: white on a small rounded square of colour.
struct Tile: LabelStyle {
  let color: Color
  init(_ color: Color) { self.color = color }

  func makeBody(configuration: Configuration) -> some View {
    HStack(spacing: 14) {
      configuration.icon
        .font(.system(size: 15, weight: .semibold))
        .foregroundStyle(.white)
        .frame(width: 29, height: 29)
        .background(color.gradient, in: .rect(cornerRadius: 7))
      configuration.title
    }
  }
}

/// The red count Settings shows on a row with something to do ("1" beside Finish Setting Up).
struct CountBadge: View {
  let count: Int

  var body: some View {
    Text("\(count)")
      .font(.subheadline.weight(.semibold))
      .foregroundStyle(.white)
      .frame(minWidth: 23, minHeight: 23)
      .background(.red, in: .capsule)
      .accessibilityLabel("\(count) thing to do")
  }
}

/// The disclosure arrow Settings draws after a row's count, so the row reads as tappable.
struct Chevron: View {
  var body: some View {
    Image(systemName: "chevron.right")
      .font(.footnote.weight(.semibold))
      .foregroundStyle(Color(.tertiaryLabel))
      .accessibilityHidden(true)
  }
}

/// A security row: its tile, its title, its state at the end, as Settings lists a setting.
private struct SecurityRow: View {
  let icon: String
  let color: Color
  let title: String
  let value: String
  var done = false
  var chevron = true

  var body: some View {
    // Plain label colours, not the tint a button or menu would give them; the checkmark after the state.
    HStack {
      Label {
        Text(title).foregroundStyle(Color(.label))
      } icon: {
        Image(systemName: icon)
      }
      .labelStyle(Tile(color))
      Spacer()
      Text(value).foregroundStyle(Color(.secondaryLabel))
      if done {
        Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).accessibilityHidden(true)
      }
      if chevron { Chevron() }
    }
  }
}

/// The two confirmations of the security rows, wherever they are shown.
private struct SecurityDialogs: ViewModifier {
  @Binding var turningOffPasskey: Bool
  @Binding var renewingWords: Bool
  let turnOff: () -> Void
  let renew: () -> Void

  func body(content: Content) -> some View {
    content
      .confirmationDialog("Turn Off Passkey?", isPresented: $turningOffPasskey, titleVisibility: .visible) {
        Button("Turn Off Passkey", role: .destructive) { turnOff() }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text("You’ll sign back in with your recovery key only. Delete the passkey in your password manager too.")
      }
      .confirmationDialog("Create New Recovery Key?", isPresented: $renewingWords, titleVisibility: .visible) {
        Button("Create New Recovery Key", role: .destructive) { renew() }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text("Your current 12 words will stop working.")
      }
  }
}
