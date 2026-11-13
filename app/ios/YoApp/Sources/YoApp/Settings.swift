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
  @State private var showingWords = false

  var body: some View {
    NavigationStack {
      Form {
        if let account = accounts.account {
          // You, as Settings shows the Apple Account and Signal its profile: one row, tapped for the details.
          Section {
            NavigationLink {
              Form { accountSection(account) }
                .navigationTitle("Account")
                .navigationBarTitleDisplayMode(.inline)
            } label: {
              profileRow(account)
            }
          }
          // The one open task, right under you, with the badge the profile button has too.
          if !account.secured {
            Section {
              Button { securing = true } label: {
                Label("Secure Your Account", systemImage: "lock.shield")
              }
              .tint(.primary)
              .badge(1)
            } footer: {
              Text("When your account is secured, you keep your name forever.")
            }
          }
        }

        Section {
          Toggle(
            isOn: Binding(
              get: { saveSent },
              set: { setSaveSent($0) }
            )
          ) {
            Label("Save Sent Photos and Videos", systemImage: "photo.on.rectangle")
          }
          .disabled(requestingPhotos)
        } footer: {
          Text("Photos and videos you send are also saved to your library.")
        }

        Section {
          // As Settings signs out of an Apple Account: centred, red, no footer; the dialog says what happens.
          Button("Sign Out", role: .destructive) {
            resetting = true
          }
          .frame(maxWidth: .infinity)
          .disabled(addingPasskey)
        }
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
      .sheet(isPresented: $securing) { SecureNameSheet() }
      .sheet(isPresented: $showingWords) {
        if let account = accounts.account, let words = RecoveryWords.load() {
          RecoveryWordsSheet(name: account.username, words: words)
        }
      }
      .confirmationDialog(
        "Sign Out?",
        isPresented: $resetting,
        titleVisibility: .visible
      ) {
        Button("Sign Out", role: .destructive) { reset() }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text(resetMessage)
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

  /// One section, "Account": whether the name is secured, and what secures it (passkey, recovery key) or the way to.
  private func accountSection(_ account: Account) -> some View {
    Section {
      if account.secured {
        Label("Account Secured", systemImage: "checkmark.shield.fill")
        LabeledContent {
          Text(account.byPasskey ? "On" : "Off")
        } label: {
          Label("Face ID", systemImage: "faceid")
        }
        if !account.byPasskey {
          Button {
            Task { await addPasskey() }
          } label: {
            HStack {
              Text(addingPasskey ? "Setting Up…" : "Turn On Face ID")
              Spacer()
              if addingPasskey {
                ProgressView().accessibilityLabel("Turning On Face ID")
              }
            }
          }
          .disabled(addingPasskey || RecoveryWords.load() == nil)
        }
        if RecoveryWords.load() != nil {
          Button { showingWords = true } label: {
            Label("Recovery Key", systemImage: "key")
          }
          .tint(.primary)
          .disabled(addingPasskey)
        } else {
          LabeledContent {
            Text("Not on This iPhone")
          } label: {
            Label("Recovery Key", systemImage: "key")
          }
        }
      } else {
        Button { securing = true } label: {
          Label("Secure Your Account", systemImage: "lock.shield")
        }
        .tint(.primary)
        .badge(1)
      }
    } header: {
      Text("Account")
    } footer: {
      if !account.secured {
        Text("When your account is secured, you keep your name forever.")
      } else if account.byPasskey {
        Text("You keep your name forever, on any iPhone.")
      } else {
        Text("You keep your name forever. Turn on Face ID to sign in without your recovery key.")
      }
    }
  }

  /// What signing out costs, in one or two lines: friends go; the name comes back only if the account is secured.
  private var resetMessage: String {
    guard let account = accounts.account, account.secured else {
      return "Your account isn’t secured. You won’t be able to sign in as "
        + "\(accounts.account?.username ?? "this name") again."
    }
    return account.byPasskey
      ? "Sign back in with Face ID or your recovery key. Friends on this iPhone are removed."
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
