// "Secure Your Account", from the profile (spec 7): two full-colour pages like the onboarding, one task each, as iOS
// guides a setup (Face ID, Apple Pay). 1, the passkey: made, then used once for its PRF output (iCloud Keychain gives
// it at once; 1Password only when used, so the same page asks once more); skippable. 2, the recovery key: the 12
// words, saved through the system's share sheet (Save Image, Notes, Files, a password manager, AirDrop). Done once
// they are saved.
import SwiftUI

struct SecureNameSheet: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var page = Stage.passkey
  @State private var confirming = false
  @State private var passkeyDone = false
  @State private var words: [String] = []
  @State private var saved = false
  @State private var busy = false
  @State private var problem: String?

  private enum Stage { case passkey, words }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch page {
        case .passkey: passkeyPage
        case .words: wordsPage
        }
      }
      .transition(.asymmetric(insertion: .move(edge: .trailing), removal: .move(edge: .leading)))
    }
    .animation(.smooth(duration: 0.35), value: page)
    .foregroundStyle(.white)
    .preferredColorScheme(.dark)
    .interactiveDismissDisabled(busy)
    .onAppear { if words.isEmpty { words = accounts.secureWords() } }
  }

  // MARK: Pages

  private var passkeyPage: some View {
    Page(title: "Secure Your Account",
      message: confirming
        ? "One more Face ID to finish."
        : "Keep your name “\(accounts.account?.username ?? "")” forever.") {
      Image(systemName: "faceid")
        .font(.system(size: 80, weight: .light))
        .padding(.top, 28)
        .accessibilityHidden(true)
    } bottom: {
      PrimaryButton(confirming ? "Continue" : "Continue with Face ID", busy: busy) {
        Task { confirming ? await confirmPasskey() : await makePasskey() }
      }
      .disabled(busy)
      Button("Skip") { go(.words) }
        .font(.yo(17))
        .disabled(busy)
      Note(problem)
    }
  }

  private var wordsPage: some View {
    Page(title: "Recovery Key",
      message: passkeyDone ? "Face ID is on. Save these 12 words too." : "Save these 12 words to sign back in.",
      back: passkeyDone ? nil : { go(.passkey) }) {
      FrostedKey(words: words, qrSize: 80)
    } bottom: {
      if saved {
        PrimaryButton("Done") { finish() }
      } else {
        PrimaryButton("Save…") { save() }
      }
      Note(problem)
    }
  }

  // MARK: Doing

  private func go(_ next: Stage) {
    problem = nil
    page = next
  }

  private func makePasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      if try await accounts.securePasskey() {
        passkeyDone = true
        go(.words)
      } else {
        confirming = true  // 1Password: its PRF output only when the passkey is used
      }
    } catch {
      if !SecurityPresentation.isCancellation(error) { problem = "Couldn’t create passkey. Try again or skip." }
    }
  }

  private func confirmPasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      try await accounts.confirmPasskey()
      passkeyDone = true
      go(.words)
    } catch {
      if !SecurityPresentation.isCancellation(error) { problem = "Couldn’t confirm passkey. Try again or skip." }
    }
  }

  /// The words as the card (an image) and as text, to the share sheet; saved once something was done with them.
  private func save() {
    let name = accounts.account?.username ?? ""
    let renderer = ImageRenderer(content: RecoveryCard(name: name, words: words))
    renderer.scale = 3
    var items: [Any] = ["Yo recovery key for \(name): " + words.joined(separator: " ")]
    if let image = renderer.uiImage { items.insert(image, at: 0) }
    Share.present(items) { completed in
      if completed {
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        withAnimation(.snappy) { saved = true }
      }
    }
  }

  /// With the passkey the backup is saved already; without, the words save it now.
  private func finish() {
    if !passkeyDone {
      do { try accounts.keepWords() } catch { return problem = "Couldn’t save. Try again." }
    }
    dismiss()
  }
}

/// The system's share sheet, from whatever is on top, telling whether something was done with the items.
@MainActor enum Share {
  static func present(_ items: [Any], completion: @escaping @MainActor (Bool) -> Void) {
    let controller = UIActivityViewController(activityItems: items, applicationActivities: nil)
    controller.completionWithItemsHandler = { _, completed, _, _ in
      Task { @MainActor in completion(completed) }
    }
    let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
    var top = scene?.keyWindow?.rootViewController
    while let presented = top?.presentedViewController { top = presented }
    top?.present(controller, animated: true)
  }
}
