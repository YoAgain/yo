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

  var body: some View {
    NavigationStack {
      Form {
        if let account = accounts.account {
          Section {
            NavigationLink {
              SignInRecoveryView()
            } label: {
              VStack(alignment: .leading, spacing: 4) {
                Text(account.username)
                  .font(.yo(24, relativeTo: .title2))
                  .foregroundStyle(.primary)
                  .fixedSize(horizontal: false, vertical: true)

                Text("Sign-In & Recovery")
                  .font(.subheadline)
                  .foregroundStyle(.secondary)
              }
              .padding(.vertical, 4)
            }
          } footer: {
            Text(
              account.secured
                ? "Your name is secured. You can sign in on a new iPhone."
                : "If your name isn’t secured, it becomes available to others "
                  + "after 30 days without opening Yo."
            )
          }
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
          Text(
            "Automatically save photos and videos you send "
              + "from the camera to Photos."
          )
        }

        Section {
          Button("Reset Yo", role: .destructive) {
            resetting = true
          }
        } footer: {
          Text("Removes Yo data from this iPhone.")
        }
      }
      .navigationTitle("Profile")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Close") { dismiss() }
        }
      }
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
          "Allow Yo to add photos in Settings to save "
            + "photos and videos you send."
        )
      }
      .onAppear { checkPhotosPermission() }
      .onChange(of: scenePhase) { _, phase in
        if phase == .active { checkPhotosPermission() }
      }
    }
    .tint(Color.amethyst)
  }

  private var resetMessage: String {
    let explanation =
      "This removes your account, friends, settings, and recovery key "
      + "from this iPhone. Photos saved in Photos are kept."

    guard let account = accounts.account, account.secured else {
      return explanation
        + "\n\nYour name isn’t secured. You won’t be able to sign in to it again."
    }

    if account.byPasskey {
      return explanation
        + "\n\nKeep your passkey or a copy of your recovery key to sign in again."
    }

    return explanation
      + "\n\nYou’ll need a copy of your recovery key to sign in again."
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

@MainActor
private struct SignInRecoveryView: View {
  @Environment(AccountStore.self) private var accounts
  @State private var addingPasskey = false
  @State private var problem: String?

  var body: some View {
    Form {
      if let account = accounts.account {
        Section {
          LabeledContent {
            Text(account.username)
              .font(.yo(17, relativeTo: .body))
              .fixedSize(horizontal: false, vertical: true)
          } label: {
            Text("Username")
          }

          LabeledContent(
            "Status",
            value: account.secured ? "Secured" : "Not Secured"
          )
        } header: {
          Text("Your Name")
        } footer: {
          Text(statusExplanation(account))
        }

        if account.secured {
          passkeySection(account)
          recoverySection
        } else {
          Section {
            NavigationLink("Secure Your Name") {
              SecureNamePage()
            }
          } footer: {
            Text(
              "Set up a passkey and a recovery key. "
                + "Either one can restore your name."
            )
          }
        }
      }
    }
    .navigationTitle("Sign-In & Recovery")
    .navigationBarTitleDisplayMode(.inline)
    .alert(
      "Couldn’t Set Up Passkey",
      isPresented: Binding(
        get: { problem != nil },
        set: { if !$0 { problem = nil } }
      )
    ) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(problem ?? "")
    }
  }

  private func statusExplanation(_ account: Account) -> String {
    guard account.secured else {
      return "Your name becomes available to others after 30 days without "
        + "opening Yo. Secure it to keep your name and sign in on a new iPhone."
    }

    let method = account.byPasskey
      ? "your passkey or recovery key"
      : "your recovery key"

    return "Your name stays yours, even if you stop opening Yo. "
      + "Use \(method) to sign in on a new iPhone."
  }

  private func passkeySection(_ account: Account) -> some View {
    Section {
      if account.byPasskey {
        LabeledContent("Passkey", value: "Set Up")
      } else {
        Button {
          Task { await addPasskey() }
        } label: {
          HStack {
            Text(
              addingPasskey
                ? "Setting Up Passkey…"
                : "Set Up Passkey"
            )
            Spacer()
            if addingPasskey {
              ProgressView()
                .accessibilityLabel("Setting Up Passkey")
            }
          }
        }
        .disabled(addingPasskey || RecoveryWords.load() == nil)
      }
    } header: {
      Text("Passkey")
    } footer: {
      VStack(alignment: .leading, spacing: 8) {
        Text(
          "A passkey lets you sign in with Face ID or your device passcode. "
            + "It’s stored in iCloud Keychain or your chosen password manager."
        )

        if !account.byPasskey && RecoveryWords.load() == nil {
          Text(
            "Your recovery key must be on this iPhone "
              + "before you can add a passkey."
          )
        }
      }
    }
  }

  private var recoverySection: some View {
    Section {
      if let words = RecoveryWords.load() {
        NavigationLink("Recovery Key") {
          RecoveryKeyView(
            name: accounts.account?.username ?? "",
            words: words,
            explanation:
              "Use these 12 words to sign in on a new iPhone, "
              + "even without your passkey."
          )
        }
      } else {
        LabeledContent("Recovery Key", value: "Not on This iPhone")
      }
    } header: {
      Text("Recovery Key")
    } footer: {
      if RecoveryWords.load() != nil {
        Text(
          "Your recovery key is 12 words. Use it to sign in without a passkey. "
            + "Anyone with these words can sign in to your account."
        )
      } else {
        Text(
          "Your recovery key isn’t stored on this iPhone. "
            + "Keep any copy you already have."
        )
      }
    }
  }

  private func addPasskey() async {
    guard !addingPasskey else { return }
    addingPasskey = true
    defer { addingPasskey = false }

    do {
      try await accounts.addPasskey()
    } catch {
      if SecurityPresentation.isCancellation(error) { return }
      problem = SecurityPresentation.passkeyMessage(
        error,
        fallback: "Try again. Your recovery key still works."
      )
    }
  }
}

// Shared presentation helpers; no account or credential state is changed here.
enum SecurityPresentation {
  static func isCancellation(_ error: Error) -> Bool {
    let error = error as NSError

    return error.domain == ASAuthorizationError.errorDomain
      && error.code == ASAuthorizationError.Code.canceled.rawValue
  }

  static func passkeyMessage(_ error: Error, fallback: String) -> String {
    if let error = error as? AccountError, case .noPRF = error {
      return "This passkey can’t be used with Yo. "
        + "Try a different password manager."
    }
    return fallback
  }
}

// Preserves the existing standalone API. Profile uses navigation instead.
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
        explanation:
          "Use these 12 words to sign in on a new iPhone, "
          + "even without your passkey."
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

// Shared by existing-key viewing and new-name setup.
// Existing keys require authentication each time they are revealed.
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

        Text(explanation)
      }

      Section {
        Button {
          Task { await toggleVisibility() }
        } label: {
          HStack {
            Text(revealed ? "Hide Recovery Key" : "Show Recovery Key")
            Spacer()
            if authenticating { ProgressView() }
          }
        }
        .disabled(authenticating || saving)

        if revealed {
          RecoveryQRView(words: words)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .accessibilityLabel("QR code containing your recovery key")
            .privacySensitive()
            .opacity(scenePhase == .active ? 1 : 0)

          ForEach(Array(words.enumerated()), id: \.offset) { index, word in
            LabeledContent {
              Text(word)
                .font(.yo(17, relativeTo: .body))
            } label: {
              Text("\(index + 1)")
                .monospacedDigit()
                .foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Word \(index + 1), \(word)")
            .privacySensitive()
            .opacity(scenePhase == .active ? 1 : 0)
          }
        }
      } header: {
        Text("Recovery Key")
      } footer: {
        Text(
          "Keep a copy outside Yo. Anyone with these words "
            + "can sign in to your account."
        )
      }

      if revealed {
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
        // Asked only when the words are the one way back (no passkey): with a passkey, Done is enough.
        let wordsOnly = onTryPasskey != nil
        Section {
          if wordsOnly {
            Toggle("I Saved My Recovery Key", isOn: $acknowledged)
              .disabled(!revealed || saving)
          }

          Button(finishTitle, action: onFinish)
            .disabled((wordsOnly && !acknowledged) || saving || authenticating)
        } footer: {
          if wordsOnly { Text("Confirm after saving a copy outside Yo.") }
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
    .onChange(of: scenePhase) { _, phase in
      if phase != .active { revealed = false }
    }
    .alert(
      "Couldn’t Show Recovery Key",
      isPresented: $authenticationFailed
    ) {
      Button("OK", role: .cancel) {}
    } message: {
      Text("Try again and verify with Face ID or your device passcode.")
    }
    .alert(
      "Couldn’t Save Recovery Key",
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
      revealed = true
      return
    }

    authenticating = true
    defer { authenticating = false }

    let context = LAContext()
    do {
      let allowed = try await context.evaluatePolicy(
        .deviceOwnerAuthentication,
        localizedReason: "Show your Yo recovery key."
      )
      revealed = allowed
      if !allowed { authenticationFailed = true }
    } catch {
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
    guard !saving, revealed else { return }
    saving = true
    needsPhotosPermission = false
    defer { saving = false }

    let renderer = ImageRenderer(
      content: RecoveryCard(name: name, words: words)
    )
    renderer.scale = 3

    guard let image = renderer.uiImage else {
      saveProblem = "Try again, or copy your recovery key instead."
      return
    }

    do {
      try await RecoveryWords.saveToPhotos(image)
      savedToPhotos = true
    } catch {
      let status = PHPhotoLibrary.authorizationStatus(for: .addOnly)
      needsPhotosPermission = status == .denied || status == .restricted
      saveProblem = needsPhotosPermission
        ? "Allow Yo to add photos in Settings, or copy your recovery key instead."
        : "Try again, or copy your recovery key instead."
    }
  }
}

// Existing sent-media behavior is preserved.
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
