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

  var body: some View {
    NavigationStack {
      Form {
        Section {
          // You: a placeholder until accounts come with the hub.
          HStack(spacing: 14) {
            Image(systemName: "person.crop.circle.fill")
              .font(.system(size: 52))
              .foregroundStyle(.white, Color.amethyst)
            VStack(alignment: .leading, spacing: 2) {
              Text("You").font(.title3.weight(.semibold))
              Text("No account yet").foregroundStyle(.secondary)
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
