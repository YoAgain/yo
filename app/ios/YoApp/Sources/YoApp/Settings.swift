import AuthenticationServices
import Foundation
import LocalAuthentication
import Photos
import SwiftUI
import UIKit
import YoCore

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
  /// Securing and the words open as sheets.
  @State private var securing = false
  @State private var securingStart: SecureStep?
  @State private var showingWords = false
  @State private var showingCode = false
  @State private var turningOffPasskey = false
  @State private var renewingWords = false
  @State private var deleteTyped = ""
  @State private var deletingAccount = false
  @State private var deleteProblem: String?
  @State private var sendingFeedback = false

  var body: some View {
    NavigationStack {
      Form {
        if let account = accounts.account {
          // You, at the top: a picture (your initial, as Signal makes one), your name, and your code to show.
          Section {
            HStack(spacing: 14) {
              Avatar(name: account.username, size: 56)
              Text(account.username)
                .font(.yo(26, relativeTo: .title2))
                .lineLimit(1)
                .minimumScaleFactor(0.6)
              Spacer()
              Button { showingCode = true } label: {
                Image(systemName: "qrcode")
                  .font(.title2)
                  .frame(width: 44, height: 44)
              }
              .buttonStyle(.borderless)
              .accessibilityLabel("Show Your Code")
            }
            .padding(.vertical, 4)
          }
          // While something is left to secure: the guide, right here, with the badge.
          if accounts.toDo > 0 {
            Section {
              Button { secure(nil) } label: {
                HStack {
                  Label {
                    Text("Secure Your Account").foregroundStyle(Color(.label))
                  } icon: {
                    Image(systemName: "lock.fill")
                  }
                  .labelStyle(Tile(.orange))
                  Spacer()
                  CountBadge(count: accounts.toDo)
                  Chevron()
                }
              }
            } footer: {
              // Kept in the iCloud Keychain already (SyncedAccount): the passkey and words are for life without it.
              Text("Secure your account to keep your name “\(account.username)” forever.")
            }
          }
          // Two groups: Account (security, signing out, deleting) and Blocked; then Statistics and Appearance.
          Section {
            NavigationLink {
              accountPage(account)
            } label: {
              HStack {
                Label("Account", systemImage: "person.fill")
                  .labelStyle(Tile(accounts.toDo > 0 ? .orange : .green))
                Spacer()
                if accounts.toDo > 0 {
                  Text("Not Secured").foregroundStyle(Color(.secondaryLabel))
                } else {
                  Text("Secured").foregroundStyle(Color(.secondaryLabel))
                  Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).accessibilityHidden(true)
                }
              }
            }
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
          Section {
            NavigationLink {
              mediaPage
            } label: {
              Label("Media", systemImage: "photo.on.rectangle").labelStyle(Tile(.green))
            }
            NavigationLink {
              AppearancePage()
            } label: {
              Label("Appearance", systemImage: "paintbrush.fill").labelStyle(Tile(.blue))
            }
            NavigationLink {
              StatsPage()
            } label: {
              HStack {
                Label("Statistics", systemImage: "chart.bar.fill").labelStyle(Tile(.amethyst))
                Spacer()
                Text("\(Stats.sent + Stats.received)").foregroundStyle(Color(.secondaryLabel))
              }
            }
          }
          // The very last row: a word to the Yo team.
          Section {
            Button { sendingFeedback = true } label: {
              Label {
                Text("Send Feedback").foregroundStyle(Color(.label))
              } icon: {
                Image(systemName: "envelope.fill")
              }
              .labelStyle(Tile(.gray))
            }
          }
        }
      }
      .sheet(isPresented: $sendingFeedback) { FeedbackSheet() }
      .sheet(isPresented: $showingCode) {
        if let account = accounts.account { NameCodeSheet(name: account.username) }
      }
      .navigationTitle("Profile")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        // As Signal's settings: the system's close button, top right.
        ToolbarItem(placement: .topBarTrailing) {
          Button(role: .close) { dismiss() }
            .disabled(addingPasskey)
        }
      }
      .interactiveDismissDisabled(addingPasskey)
      .sheet(isPresented: $securing) { SecureNameSheet(start: securingStart) }
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

  /// Media: what happens to the photos and videos you send.
  private var mediaPage: some View {
    Form {
      Section {
        Toggle("Keep Sent Photos", isOn: Binding(get: { saveSent }, set: { setSaveSent($0) }))
          .disabled(requestingPhotos)
      } footer: {
        Text("Saves photos and videos you send to your camera roll.")
      }
    }
    .navigationTitle("Media")
    .navigationBarTitleDisplayMode(.inline)
  }

  /// Your account: its security (passkey and recovery key), then signing out and, below, deleting it (App Review
  /// 5.1.1(v)).
  private func accountPage(_ account: Account) -> some View {
    Form {
      securitySection(account)
    }
    .navigationTitle("Account")
    .navigationBarTitleDisplayMode(.inline)
    // Signing out and deleting: at the very bottom of the screen, however short the list.
    .safeAreaInset(edge: .bottom) {
      VStack(spacing: 10) {
        Button(role: .destructive) { resetting = true } label: {
          Text("Sign Out").frame(maxWidth: .infinity)
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
        NavigationLink {
          deletePage(account)
        } label: {
          Text("Delete Account").foregroundStyle(.red).frame(maxWidth: .infinity)
        }
      }
      .buttonStyle(.glass)
      .controlSize(.large)
      .disabled(addingPasskey || deletingAccount)
      .padding(.horizontal, 20)
      .padding(.bottom, 8)
    }
  }

  /// Security: Passkey and Face ID, and Recovery Key, one row each with its state. A step not done yet opens the
  /// guide at it; one done opens its own page with what can be done with it (as Settings' Face ID & Passcode).
  @ViewBuilder private func securitySection(_ account: Account) -> some View {
    let wordsDone = account.secured && accounts.wordsSaved && RecoveryWords.load() != nil
    Section("Security") {
      if account.byPasskey {
        NavigationLink {
          securityPage("Passkey and Face ID") {
            Button("Replace Passkey") { Task { await replacePasskey() } }
            if wordsDone {
              Button("Turn Off Passkey", role: .destructive) { turningOffPasskey = true }
            }
          }
        } label: {
          SecurityRow(
            icon: "faceid", color: .green, title: "Passkey and Face ID", value: "On", done: true, chevron: false)
        }
      } else {
        Button { secure(.passkey) } label: {
          SecurityRow(icon: "faceid", color: .orange, title: "Passkey and Face ID", value: "Off")
        }
      }
      if wordsDone {
        NavigationLink {
          securityPage("Recovery Key") {
            Button("Show Recovery Key") { Task { await showRecoveryKey() } }
            Button("Create New Recovery Key") { renewingWords = true }
          }
        } label: {
          SecurityRow(icon: "key.fill", color: .green, title: "Recovery Key", value: "Saved", done: true, chevron: false)
        }
      } else {
        Button { secure(.words) } label: {
          SecurityRow(icon: "key.fill", color: .orange, title: "Recovery Key", value: "Not Saved")
        }
      }
    }
    .disabled(addingPasskey)
  }

  /// The page under Passkey and Face ID or Recovery Key: what can be done with it, and the dialogs those ask.
  private func securityPage(_ title: String, @ViewBuilder actions: () -> some View) -> some View {
    Form {
      Section { actions() }
        .disabled(addingPasskey)
    }
    .navigationTitle(title)
    .navigationBarTitleDisplayMode(.inline)
    // The words show from here, the page their button is on (a sheet from the list two pages down came up late).
    .sheet(isPresented: $showingWords) {
      if let account = accounts.account, let words = RecoveryWords.load() {
        RecoveryShowSheet(name: account.username, words: words)
      }
    }
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

  /// Deleting, on a page of its own: what it means, your name typed to confirm, then Face ID (or the passcode).
  private func deletePage(_ account: Account) -> some View {
    let confirmed = Username.skeleton(deleteTyped) == Username.skeleton(account.username)
    return Form {
      Section {
        Label("“\(account.username)” becomes free for anyone.", systemImage: "person.crop.circle.badge.xmark")
        Label("Your friends can’t reach you anymore.", systemImage: "person.2.slash")
        Label("This can’t be undone.", systemImage: "exclamationmark.triangle")
        if account.byPasskey {
          Label("Delete the passkey in your password manager too.", systemImage: "key")
        }
      }
      Section {
        TextField("Your Name", text: $deleteTyped)
          .textInputAutocapitalization(.never)
          .autocorrectionDisabled()
      } footer: {
        Text("Type “\(account.username)” to confirm.")
      }
      Section {
        Button(role: .destructive) {
          Task { await deleteAccount() }
        } label: {
          if deletingAccount { ProgressView().frame(maxWidth: .infinity) } else {
            Text("Delete Account").frame(maxWidth: .infinity)
          }
        }
        .disabled(!confirmed || deletingAccount)
      }
    }
    .navigationTitle("Delete Account")
    .navigationBarTitleDisplayMode(.inline)
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
    guard (try? await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "Delete your account."))
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
    dismiss()
    accounts.signOut()  // the friends go with the account (YoApp)
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
        ToolbarItem(placement: .topBarTrailing) {
          Button(role: .close) { dismiss() }
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
      // Done: a green check; not yet: an empty circle, the step still to take.
      Image(systemName: done ? "checkmark.circle.fill" : "circle")
        .foregroundStyle(done ? Color.green : Color(.tertiaryLabel))
        .accessibilityHidden(true)
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

/// A picture for someone without one, as Signal makes it: the first letter of the name on a colour of the list's,
/// the same for the same name on every phone.
struct Avatar: View {
  let name: String
  var size: CGFloat = 44

  var body: some View {
    let seed = Username.skeleton(name).unicodeScalars.reduce(0) { $0 &+ Int($1.value) }
    Circle()
      .fill(Color.row(seed))
      .frame(width: size, height: size)
      .overlay {
        Text(name.first.map { String($0).uppercased() } ?? "?")
          .font(.yo(size * 0.45))
          .foregroundStyle(.white)
      }
      .accessibilityHidden(true)
  }
}

/// Your code, big: a friend's camera reads it to add you. It holds your link (justyo.co/NAME).
struct NameCodeSheet: View {
  let name: String
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    NavigationStack {
      VStack(spacing: 20) {
        Spacer()
        if let image = RecoveryQR.image(text: "https://justyo.co/\(name)") {
          Image(uiImage: image)
            .interpolation(.none)
            .resizable()
            .scaledToFit()
            .frame(width: 240, height: 240)
            .padding(16)
            .background(.white, in: .rect(cornerRadius: 22))
            .accessibilityLabel("QR code for \(name)")
        }
        Text(name).font(.yo(30, relativeTo: .title))
        Spacer()
      }
      .frame(maxWidth: .infinity)
      .navigationTitle("Your Code")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .topBarTrailing) { Button(role: .close) { dismiss() } }
      }
    }
    .presentationDetents([.medium, .large])
  }
}

/// Send Feedback: a few words to the Yo team, with the app's version. Plain to the hub (spec 1), and the footer says
/// so. Sent through the core off the main actor; a short "Thanks!" and the sheet closes.
struct FeedbackSheet: View {
  @Environment(\.dismiss) private var dismiss
  @Environment(AccountStore.self) private var accounts

  @State private var text = ""
  @State private var sending = false
  @State private var thanked = false
  @State private var problem: String?
  @FocusState private var focused: Bool

  /// The hub takes up to this many bytes of UTF-8 (spec 6.3).
  private static let maxBytes = 2000

  private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

  /// "1.2 (34)": CFBundleShortVersionString and CFBundleVersion.
  static var appVersion: String {
    let info = Bundle.main.infoDictionary
    let short = info?["CFBundleShortVersionString"] as? String ?? "?"
    let build = info?["CFBundleVersion"] as? String ?? "?"
    return String(bytes: "\(short) (\(build))".utf8.prefix(64), encoding: .utf8) ?? "?"
  }

  var body: some View {
    NavigationStack {
      Form {
        Section {
          TextEditor(text: $text)
            .focused($focused)
            .frame(minHeight: 180)
            .overlay(alignment: .topLeading) {
              if text.isEmpty {
                Text("What should Yo do better?")
                  .foregroundStyle(Color(.placeholderText))
                  .padding(.top, 8)
                  .padding(.leading, 5)
                  .allowsHitTesting(false)
              }
            }
            .disabled(sending || thanked)
        } footer: {
          Text("Goes to the Yo team, not end-to-end encrypted.")
        }
      }
      .overlay {
        if thanked {
          Label("Thanks!", systemImage: "checkmark.circle.fill")
            .font(.yo(22, relativeTo: .title3))
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
            .glassEffect()
            .transition(.scale.combined(with: .opacity))
        }
      }
      .navigationTitle("Send Feedback")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Cancel", role: .cancel) { dismiss() }
            .disabled(sending)
        }
        ToolbarItem(placement: .confirmationAction) {
          if sending {
            ProgressView()
          } else {
            Button("Send") { Task { await send() } }
              .disabled(trimmed.isEmpty || thanked)
          }
        }
      }
      .onChange(of: text) { _, new in
        // Within the hub's length, cut at a whole character.
        if new.utf8.count > Self.maxBytes {
          var cut = new
          while cut.utf8.count > Self.maxBytes { cut.removeLast() }
          text = cut
        }
      }
      .onAppear { focused = true }
      .alert(
        "Unable to Send Feedback",
        isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })
      ) {
        Button("OK", role: .cancel) {}
      } message: {
        Text(problem ?? "")
      }
    }
    .interactiveDismissDisabled(sending || !trimmed.isEmpty)
    .sensoryFeedback(.success, trigger: thanked) { _, now in now }
  }

  private func send() async {
    guard let client = accounts.client, !trimmed.isEmpty, !sending else { return }
    let words = trimmed
    let version = Self.appVersion
    sending = true
    defer { sending = false }
    do {
      try await Hub.run { try client.sendFeedback(text: words, appVersion: version) }
    } catch YoError.Refused(let status) where status == 429 {
      problem = "You’ve sent a lot of feedback today. Try again tomorrow."
      return
    } catch let error as YoError {
      problem = AccountError.hub(error).localizedDescription
      return
    } catch {
      problem = error.localizedDescription
      return
    }
    focused = false
    withAnimation(.snappy) { thanked = true }
    try? await Task.sleep(for: .seconds(1.2))
    dismiss()
  }
}

/// Appearance: the app icon, chosen by its picture alone. The default, its negative, and the original Yo's plain
/// purple square (Info.plist CFBundleAlternateIcons).
struct AppearancePage: View {
  @State private var current = UIApplication.shared.alternateIconName

  private let icons: [(name: String?, image: String)] = [
    (nil, "IconStandard"), ("Negative", "IconNegative"), ("Original", "IconOriginal"),
  ]

  var body: some View {
    Form {
      Section("App Icon") {
        HStack(spacing: 22) {
          ForEach(icons, id: \.image) { icon in
            Button {
              guard current != icon.name else { return }
              Task {
                try? await UIApplication.shared.setAlternateIconName(icon.name)
                current = UIApplication.shared.alternateIconName
              }
            } label: {
              VStack(spacing: 8) {
                Image(uiImage: UIImage(named: icon.image) ?? UIImage())
                  .resizable()
                  .frame(width: 64, height: 64)
                  .clipShape(.rect(cornerRadius: 15, style: .continuous))
                  .overlay(
                    RoundedRectangle(cornerRadius: 15, style: .continuous).strokeBorder(Color(.separator), lineWidth: 0.5))
                Image(systemName: current == icon.name ? "checkmark.circle.fill" : "circle")
                  .foregroundStyle(current == icon.name ? Color.amethyst : Color(.tertiaryLabel))
              }
            }
            .buttonStyle(.plain)
            .accessibilityLabel(icon.name ?? "Default")
            .accessibilityAddTraits(current == icon.name ? .isSelected : [])
          }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
      }
    }
    .navigationTitle("Appearance")
    .navigationBarTitleDisplayMode(.inline)
  }
}
