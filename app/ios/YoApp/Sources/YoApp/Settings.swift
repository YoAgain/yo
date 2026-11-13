// Profile and settings, from your picture in the bar: the system's form in a sheet (Liquid Glass from iOS 26 on its
// own), as the App Store and Music open their account.
import LocalAuthentication
import Photos
import SwiftUI

enum SettingKey {
  /// Photos and videos you send go to your photo library as well (only the ones sent, not the cancelled).
  static let saveSent = "settings.saveSent"
}

struct SettingsView: View {
  @AppStorage(SettingKey.saveSent) private var saveSent = false
  @Environment(\.dismiss) private var dismiss
  @Environment(AccountStore.self) private var accounts
  @Environment(Friends.self) private var friends
  @State private var resetting = false
  /// The recovery words on screen, after Face ID.
  @State private var words: [String]?
  @State private var addingPasskey = false
  @State private var passkeyProblem: String?
  @State private var securing = false

  var body: some View {
    NavigationStack {
      Form {
        Section {
          // You: your name, and how your Yo comes back on a new phone.
          HStack(spacing: 14) {
            Image(systemName: "person.crop.circle.fill")
              .font(.system(size: 52))
              .foregroundStyle(.white, Color.amethyst)
            VStack(alignment: .leading, spacing: 2) {
              Text(accounts.account?.username ?? "You").font(.yo(24, relativeTo: .title3))
              Label(
                accounts.account?.secured != true
                  ? "Not secured yet"
                  : accounts.account?.byPasskey == true ? "Saved with a passkey" : "Saved with a recovery key",
                systemImage: accounts.account?.secured != true
                  ? "exclamationmark.circle"
                  : accounts.account?.byPasskey == true ? "person.badge.key.fill" : "list.number"
              )
              .font(.subheadline)
              .foregroundStyle(.secondary)
            }
          }
          .padding(.vertical, 4)
        }
        if accounts.account?.secured == false {
          // As Settings shows "Finish Setting Up": the one thing left, with the red dot the profile button has too.
          Section {
            Button { securing = true } label: {
              HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                  Text("Secure Your Name").foregroundStyle(.primary)
                  Text("So you can get it back on a new iPhone. Names not secured become available after 30 days "
                    + "without opening Yo.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                }
                Spacer()
                Circle().fill(.red).frame(width: 10, height: 10)
              }
            }
          }
        }
        Section {
          Toggle("Save Sent Photos and Videos", isOn: $saveSent)
        } header: {
          Text("Photos and Videos")
        } footer: {
          Text("What you send from the camera is saved to your photo library. Nothing you receive is saved.")
        }
        Section {
          if RecoveryWords.load() != nil {
            Button("Recovery Key", systemImage: "key") { Task { await showWords() } }
          }
          if accounts.account?.secured == true, accounts.account?.byPasskey == false {
            Button("Set Up Passkey", systemImage: "person.badge.key") { Task { await addPasskey() } }
              .disabled(addingPasskey)
          }
        } footer: {
          if let passkeyProblem {
            Text(passkeyProblem).foregroundStyle(.red)
          } else if accounts.account?.secured != true {
            Text("Use your passkey or recovery key to sign in to Yo on a new iPhone.")
          } else if accounts.account?.byPasskey == false {
            Text("Use your recovery key to sign in to Yo on a new iPhone. Set up a passkey to sign in without it.")
          } else {
            Text("Use your passkey or recovery key to sign in to Yo on a new iPhone.")
          }
        }
        PasskeyTestSection()
        Section {
          Button("Reset Yo", systemImage: "arrow.counterclockwise", role: .destructive) { resetting = true }
        } footer: {
          Text("Resets Yo on this iPhone. Keep your passkey or a copy of your recovery key so you can sign in again.")
        }
      }
      .sheet(isPresented: $securing) { SecureNameSheet() }
      .sheet(isPresented: Binding(get: { words != nil }, set: { if !$0 { words = nil } })) {
        if let words { RecoveryWordsSheet(name: accounts.account?.username ?? "", words: words) }
      }
      .confirmationDialog("Reset Yo?", isPresented: $resetting, titleVisibility: .visible) {
        Button("Reset Yo", role: .destructive) { reset() }
      } message: {
        Text("This removes your Yo account, friends, settings, and saved recovery key from this iPhone. You’ll need your passkey or a copy of your recovery key to sign in again.")
      }
      .navigationTitle("Profile")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .confirmationAction) {
          Button("Done", systemImage: "checkmark") { dismiss() }
        }
      }
      .onChange(of: saveSent) { _, on in
        guard on else { return }
        Task {
          // Asked once, when switched on; refused, the switch goes back off.
          let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
          if status != .authorized && status != .limited { saveSent = false }
        }
      }
    }
  }
}

extension SettingsView {
  /// The words, after the owner of the phone proved it (Face ID, else the passcode).
  fileprivate func showWords() async {
    let context = LAContext()
    guard (try? await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "Verify your identity to view your recovery key."))
      == true
    else { return }
    words = RecoveryWords.load()
  }

  /// A passkey added to the backup. The system may ask twice: once to create it, once to use it.
  fileprivate func addPasskey() async {
    addingPasskey = true
    defer { addingPasskey = false }
    passkeyProblem = nil
    do {
      try await accounts.addPasskey()
    } catch let error as AccountError {
      passkeyProblem = error.errorDescription
    } catch {
      passkeyProblem = "Passkey not set up. Try again."
    }
  }

  /// Everything of this app on this phone goes (settings, friends, the account), not the simulated hub's backups.
  fileprivate func reset() {
    if let domain = Bundle.main.bundleIdentifier { UserDefaults.standard.removePersistentDomain(forName: domain) }
    friends.reset()
    dismiss()
    accounts.reset()
  }
}

/// Saving a sent photo or video to the library, when the setting is on.
enum Library {
  static func saveIfWanted(photo: Data? = nil, video: URL? = nil) {
    guard UserDefaults.standard.bool(forKey: SettingKey.saveSent) else { return }
    // A video goes in as its own copy, which Photos moves in when it is ready: the original may be deleted (seen)
    // before Photos has read it.
    var file: URL?
    if let video {
      let to = FileManager.default.temporaryDirectory.appending(path: "library-\(UUID().uuidString).mov")
      guard (try? FileManager.default.copyItem(at: video, to: to)) != nil else { return }
      file = to
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
      if !saved, let copy { try? FileManager.default.removeItem(at: copy) }
    }
  }
}

/// The recovery words from the profile, in the brand's purple: the words, Save to Photos and Copy.
struct RecoveryWordsSheet: View {
  let name: String
  let words: [String]
  @Environment(\.dismiss) private var dismiss
  @State private var kept: Set<String> = []
  @State private var problem: String?

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(spacing: 16) {
          FrostedKey(words: words)
          HStack(spacing: 12) {
            KeepButton(title: "Save to Photos", icon: "photo.badge.arrow.down", done: kept.contains("photos")) {
              Task { await save() }
            }
            KeepButton(title: "Copy", icon: "doc.on.doc", done: kept.contains("copied")) {
              RecoveryWords.copy(words, name: name)
              kept.insert("copied")
            }
          }
          if let problem { Text(problem).font(.footnote.weight(.semibold)) }
        }
        .padding(20)
      }
      .foregroundStyle(.white)
      .background(Color.amethyst.ignoresSafeArea())
      .navigationTitle("Recovery Key")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .confirmationAction) {
          Button("Done", systemImage: "checkmark") { dismiss() }
        }
      }
    }
    .preferredColorScheme(.dark)
  }

  private func save() async {
    let renderer = ImageRenderer(content: RecoveryCard(name: name, words: words))
    renderer.scale = 3
    guard let image = renderer.uiImage else { return problem = "Couldn’t create your recovery key image. Try again." }
    do {
      try await RecoveryWords.saveToPhotos(image)
      kept.insert("photos")
    } catch {
      problem = error.localizedDescription
    }
  }
}
