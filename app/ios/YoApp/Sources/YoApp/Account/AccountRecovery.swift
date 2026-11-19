import SwiftUI
import UIKit
import YoCore

struct RecoveryBadge: View {
  var body: some View {
    Text("1")
      .font(.system(size: 13, weight: .regular))
      .foregroundStyle(.white)
      .frame(minWidth: 20, minHeight: 20)
      .background(.red, in: .capsule)
      .accessibilityLabel("Account recovery needs attention")
  }
}

/// "How Account Recovery Works".
struct RecoveryHelpSheet: View {
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 24) {
          Text("Your account is protected by an account key that only your devices hold.")
            .foregroundStyle(.secondary)
          item(
            "key.icloud.fill", "iCloud Keychain",
            "Your account key is saved in iCloud Keychain, so your other Apple devices can sign in to Yo.")
          item(
            "faceid", "Passkey",
            "Optionally, back up your account key with a passkey. Face ID or your password manager unlocks it on a new phone.")
          item(
            "key.fill", "Recovery Key",
            "Optionally, keep a recovery key. Enter it on any phone to get your account back.")
          item(
            "arrow.triangle.2.circlepath.icloud.fill", "Account Backup",
            "Your contacts, groups and settings are kept on Yo’s server, encrypted with a key made from your account key, so a new phone finds them again. Yos, photos and videos aren’t backed up.")
          Text(
            "Account Backup is end-to-end encrypted. Yo’s server stores them but can’t read them."
          )
          .font(.footnote)
          .foregroundStyle(.secondary)
        }
        .padding(20)
      }
      .background(Color(.systemGroupedBackground))
      .navigationTitle("Account Security")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .confirmationAction) {
          Button("Done", systemImage: "checkmark") { dismiss() }
        }
      }
    }
    .presentationDetents([.large])
    .presentationDragIndicator(.hidden)
  }

  private func item(_ symbol: String, _ title: String, _ text: String) -> some View {
    HStack(alignment: .top, spacing: 14) {
      Image(systemName: symbol)
        .font(.title2)
        .foregroundStyle(Color.amethyst)
        .frame(width: 32)
        .accessibilityHidden(true)
      VStack(alignment: .leading, spacing: 4) {
        Text(title).font(.headline)
        Text(text).foregroundStyle(.secondary)
      }
    }
    .accessibilityElement(children: .combine)
  }
}
