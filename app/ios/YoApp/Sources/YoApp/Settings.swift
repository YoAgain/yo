import AuthenticationServices
import Foundation
import LocalAuthentication
import Photos
import SwiftUI
import UIKit

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

  var body: some View {
    NavigationStack {
      Form {
        if let account = accounts.account {
          profileHeader(account)
          accountSection(account)
        }

        Section {
          Toggle(
            "Save Sent Photos and Videos",
            isOn: Binding(
              get: { saveSent },
              set: { setSaveSent($0) }
            )
          )
          .disabled(requestingPhotos)
        } header: {
          Text("Photos and Videos")
        } footer: {
          Text("Save photos and videos you take and send in Yo to Photos.")
        }

        Section {
          Button("Reset Yo", role: .destructive) {
            resetting = true
          }
          .disabled(addingPasskey)
        } footer: {
          Text("Remove your account, friends, and settings from this iPhone.")
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
      .confirmationDialog(
        "Reset Yo?",
        isPresented: $resetting,
        titleVisibility: .visible
      ) {
        Button("Reset Yo", role: .destructive) { reset() }
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
        "Unable to Set Up Passkey",
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

  private func profileHeader(_ account: Account) -> some View {
    Section {
      VStack(spacing: 12) {
        Image(systemName: "person.crop.circle.fill")
          .font(.system(size: 72))
          .foregroundStyle(.secondary)
          .accessibilityLabel("Profile Picture")

        Text(account.username)
          .font(.yo(28, relativeTo: .title))
          .foregroundStyle(.primary)
          .multilineTextAlignment(.center)
          .fixedSize(horizontal: false, vertical: true)
          .textSelection(.enabled)
      }
      .frame(maxWidth: .infinity)
      .padding(.vertical, 12)
      .listRowBackground(Color.clear)
    }
  }

  /// One section, "Account": whether the name is secured, and what secures it (passkey, recovery key) or the way to.
  private func accountSection(_ account: Account) -> some View {
    Section {
      if account.secured {
        Label("Name Secured", systemImage: "checkmark.shield")
        LabeledContent("Passkey", value: account.byPasskey ? "Set Up" : "Not Set Up")
        if !account.byPasskey {
          Button {
            Task { await addPasskey() }
          } label: {
            HStack {
              Text(addingPasskey ? "Setting Up…" : "Set Up Passkey")
              Spacer()
              if addingPasskey {
                ProgressView().accessibilityLabel("Setting Up Passkey")
              }
            }
          }
          .disabled(addingPasskey || RecoveryWords.load() == nil)
        }
        if let words = RecoveryWords.load() {
          NavigationLink("Recovery Key") {
            RecoveryKeyView(
              name: account.username, words: words, explanation: SecurityPresentation.recoveryExplanation)
          }
          .disabled(addingPasskey)
        } else {
          LabeledContent("Recovery Key", value: "Not on This iPhone")
        }
      } else {
        Label("Name Not Secured", systemImage: "exclamationmark.circle")
        NavigationLink("Secure Your Name") { SecureNamePage() }
      }
    } header: {
      Text("Account")
    } footer: {
      if !account.secured {
        Text("Your name becomes available to others after 30 days without opening Yo.")
      } else if account.byPasskey {
        Text("Your name stays reserved. Use your passkey or recovery key to sign in on a new iPhone.")
      } else {
        Text("Your name stays reserved. Use your recovery key to sign in on a new iPhone.")
      }
    }
  }

  private var resetMessage: String {
    let explanation =
      "Your account, friends, settings, and saved recovery key will be "
      + "removed from this iPhone. Photos and videos in Photos won’t be deleted."

    guard let account = accounts.account else { return explanation }

    guard account.secured else {
      return explanation
        + "\n\nYour name isn’t secured. You won’t be able to sign in "
        + "to this account again."
    }

    if account.byPasskey {
      return explanation
        + "\n\nYou’ll need your passkey or a saved copy of your "
        + "recovery key to sign in again."
    }

    return explanation
      + "\n\nYou’ll need a saved copy of your recovery key to sign in again."
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
    "Sign in with Face ID or your iPhone passcode. Your passkey is "
    + "saved in iCloud Keychain or another password manager."

  static let recoveryExplanation =
    "Use these 12 words to sign in if you can’t use your passkey."

  static func isCancellation(_ error: Error) -> Bool {
    if error is CancellationError { return true }

    let error = error as NSError
    return error.domain == ASAuthorizationError.errorDomain
      && error.code == ASAuthorizationError.Code.canceled.rawValue
  }

  static func passkeyMessage(_ error: Error, fallback: String) -> String {
    if let error = error as? AccountError, case .noPRF = error {
      return "This password manager doesn’t support the passkey features "
        + "Yo needs. Try another password manager."
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
          "Keep a copy in a safe place. Anyone with your recovery key "
            + "can sign in to your account."
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
            Text("Save a copy you can access without this iPhone.")
          }
        }
      }

      if let onTryPasskey {
        Section {
          Button("Try Passkey Again", action: onTryPasskey)
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
