// Profile and settings, from your picture in the bar: the system's form in a sheet (Liquid Glass from iOS 26 on its
// own), as the App Store and Music open their account.
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
                accounts.account?.byPasskey == true ? "Saved with a passkey" : "Saved with 12 words",
                systemImage: accounts.account?.byPasskey == true ? "person.badge.key.fill" : "list.number"
              )
              .font(.subheadline)
              .foregroundStyle(.secondary)
            }
          }
          .padding(.vertical, 4)
        }
        Section {
          Toggle("Save Sent Photos & Videos", isOn: $saveSent)
        } header: {
          Text("Photos & Videos")
        } footer: {
          Text("What you send from the camera is saved to your photo library. Nothing you receive is saved.")
        }
        PasskeyTestSection()
        Section {
          Button("Reset Yo", systemImage: "arrow.counterclockwise", role: .destructive) { resetting = true }
        } footer: {
          Text("This phone forgets your Yo, as a new phone would. Your passkey or your 12 words bring it back.")
        }
      }
      .confirmationDialog("Reset Yo?", isPresented: $resetting, titleVisibility: .visible) {
        Button("Reset Yo", role: .destructive) { reset() }
      } message: {
        Text("Everything on this phone goes. Your passkey or your 12 words bring your name back.")
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
