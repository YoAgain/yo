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
  @State private var turningOffPasskey = false
  @State private var renewingWords = false

  var body: some View {
    NavigationStack {
      Form {
        if let account = accounts.account {
          // You, as Settings shows the Apple Account and Signal its profile: one row, tapped for the details.
          Section {
            NavigationLink {
              Form {
                Section {
                  LabeledContent("Name") { Text(account.username).font(.yo(17, relativeTo: .body)) }
                }
                securitySection(account)
                // As the Apple Account page ends: Sign Out, red, on its own; the dialog says what happens.
                Section {
                  Button("Sign Out", role: .destructive) { resetting = true }
                    .frame(maxWidth: .infinity)
                    .disabled(addingPasskey)
                }
              }
              .navigationTitle("Account")
              .navigationBarTitleDisplayMode(.inline)
              .confirmationDialog("Sign Out?", isPresented: $resetting, titleVisibility: .visible) {
                Button("Sign Out", role: .destructive) { reset() }
                Button("Cancel", role: .cancel) {}
              } message: {
                Text(resetMessage)
              }
              .modifier(SecurityDialogs(
                turningOffPasskey: $turningOffPasskey, renewingWords: $renewingWords,
                turnOff: removePasskey, renew: newWords))
            } label: {
              profileRow(account)
            }
            // The one open task, in the same group as you (as Settings puts suggestions under the Apple Account),
            // with the badge the profile button has too.
            if account.secured {
              NavigationLink {
                recoveryPage(account)
              } label: {
                Label("Account Secured", systemImage: "checkmark").labelStyle(Tile(.green))
              }
            } else {
              Button { securing = true } label: {
                HStack {
                  Label("Secure Your Account", systemImage: "lock.fill").labelStyle(Tile(.orange))
                  Spacer()
                  CountBadge(count: 1)
                  Chevron()
                }
              }
              .tint(.primary)
            }
          } footer: {
            if !account.secured {
              Text("Set up account recovery to keep your name “\(account.username)” forever.")
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
            Label("Keep Sent Photos", systemImage: "photo.on.rectangle").labelStyle(Tile(.blue))
          }
          .disabled(requestingPhotos)
        } footer: {
          Text("Saves photos and videos you send to your camera roll.")
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

  /// "Account Security": the passkey and the recovery key, each a row with its state and what can be done with it.
  private func securitySection(_ account: Account) -> some View {
    let hasWords = account.secured && RecoveryWords.load() != nil
    return Section {
      if account.secured {
        Menu {
          if account.byPasskey {
            Button("Replace Passkey", systemImage: "arrow.triangle.2.circlepath") { Task { await replacePasskey() } }
            Button("Turn Off Passkey", systemImage: "xmark.circle", role: .destructive) { turningOffPasskey = true }
          } else {
            Button("Turn On Passkey", systemImage: "faceid") { Task { await addPasskey() } }
          }
        } label: {
          SecurityRow(icon: "faceid", color: .green, title: "Passkey", value: account.byPasskey ? "On" : "Off")
        }
        .disabled(addingPasskey || !hasWords)
        Menu {
          Button("Show Recovery Key", systemImage: "eye") { showingWords = true }
          Button("Create New Recovery Key", systemImage: "arrow.triangle.2.circlepath") { renewingWords = true }
        } label: {
          SecurityRow(icon: "key.fill", color: .gray, title: "Recovery Key",
            value: hasWords ? "Created" : "Not on This iPhone")
        }
        .disabled(addingPasskey || !hasWords)
      } else {
        Button { securing = true } label: {
          SecurityRow(icon: "faceid", color: .orange, title: "Passkey", value: "Off")
        }
        Button { securing = true } label: {
          SecurityRow(icon: "key.fill", color: .orange, title: "Recovery Key", value: "Not Set Up")
        }
      }
    } header: {
      Text("Account Security")
    } footer: {
      Text(account.secured
        ? "Either one signs you back in on a new iPhone."
        : "Secure your account to keep your name “\(account.username)” forever.")
    }
  }

  /// What secures the account, each with what can be done to it: the passkey (Face ID) and the recovery key.
  private func recoveryPage(_ account: Account) -> some View {
    let hasWords = RecoveryWords.load() != nil
    return Form {
      Section {
        LabeledContent {
          Text(account.byPasskey ? "On" : "Off")
        } label: {
          Label("Passkey", systemImage: "faceid").labelStyle(Tile(.green))
        }
        if account.byPasskey {
          Button("Replace Passkey") { Task { await replacePasskey() } }
            .disabled(addingPasskey || !hasWords)
          Button("Turn Off Passkey", role: .destructive) { turningOffPasskey = true }
            .disabled(addingPasskey || !hasWords)
        } else {
          Button(addingPasskey ? "Turning On…" : "Turn On Passkey") { Task { await addPasskey() } }
            .disabled(addingPasskey || !hasWords)
        }
      } header: {
        Text("Passkey (Face ID)")
      } footer: {
        Text(account.byPasskey ? "Sign back in with Face ID on any iPhone." : "Turn on to sign back in with Face ID.")
      }
      Section {
        LabeledContent {
          Text(hasWords ? "Created" : "Not on This iPhone")
        } label: {
          Label("Recovery Key", systemImage: "key.fill").labelStyle(Tile(.gray))
        }
        if hasWords {
          Button("Show Recovery Key") { showingWords = true }
          Button("Create New Recovery Key") { renewingWords = true }
        }
      } header: {
        Text("Recovery Key")
      } footer: {
        Text("12 words to sign back in without Face ID.")
      }
    }
    .navigationTitle("Account Recovery")
    .navigationBarTitleDisplayMode(.inline)
    .disabled(addingPasskey)
    .confirmationDialog("Turn Off Passkey?", isPresented: $turningOffPasskey, titleVisibility: .visible) {
      Button("Turn Off Passkey", role: .destructive) { removePasskey() }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text("You’ll sign back in with your recovery key only. Delete the passkey in your password manager too.")
    }
    .confirmationDialog("Create New Recovery Key?", isPresented: $renewingWords, titleVisibility: .visible) {
      Button("Create New Recovery Key", role: .destructive) { newWords() }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text("Your current 12 words will stop working.")
    }
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
    do { try accounts.removePasskey() } catch { passkeyProblem = "Try again." }
  }

  private func newWords() {
    do {
      _ = try accounts.newRecoveryKey()
      showingWords = true  // the new words, to keep
    } catch {
      passkeyProblem = "Try again."
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
      .foregroundStyle(.tertiary)
      .accessibilityHidden(true)
  }
}

/// A security row: its tile, its title, its state at the end, as Settings lists a setting.
private struct SecurityRow: View {
  let icon: String
  let color: Color
  let title: String
  let value: String

  var body: some View {
    HStack {
      Label(title, systemImage: icon).labelStyle(Tile(color)).foregroundStyle(.primary)
      Spacer()
      Text(value).foregroundStyle(.secondary)
      Chevron()
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
