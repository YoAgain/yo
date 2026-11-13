// "Secure Your Account" (spec 7): a guide of two full-colour pages like the onboarding, one task each, as iOS guides a
// setup. Opened at a step (from its row) or at the first one left; after each step it goes on to the next one left,
// and closes when none is. 1, the passkey: made, then used once for its PRF output (iCloud Keychain gives it at once;
// 1Password only when used, so the same page asks once more); skippable. 2, the recovery key: the 12 words, saved
// through the system's share sheet (Save Image, Notes, Files, a password manager, AirDrop).
import SwiftUI

enum SecureStep { case passkey, words }

struct SecureNameSheet: View {
  /// Where to begin; nil, the first step left.
  var start: SecureStep?

  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var page = SecureStep.passkey
  @State private var confirming = false
  @State private var skipped = false
  @State private var words: [String] = []
  @State private var busy = false
  @State private var problem: String?

  private var secured: Bool { accounts.account?.secured == true }
  private var passkeyDone: Bool { accounts.account?.byPasskey == true }
  private var wordsDone: Bool { secured && accounts.wordsSaved }

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
    .onAppear {
      guard words.isEmpty else { return }
      words = secured ? RecoveryWords.load() ?? [] : accounts.secureWords()
      skipped = accounts.passkeyTurnedOff && start != .passkey  // turned off on purpose: not asked again
      page = start ?? (passkeyDone || skipped ? .words : .passkey)
    }
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
      Button("Skip") {
        skipped = true
        next()
      }
      .font(.yo(17))
      .disabled(busy)
      Note(problem)
    }
  }

  private var wordsPage: some View {
    Page(title: "Recovery Key",
      message: passkeyDone ? "Face ID is on. Save these 12 words too." : "Save these 12 words to sign back in.") {
      FrostedKey(words: words, qrSize: 80)
    } bottom: {
      PrimaryButton("Save…") { save() }
      Note(problem)
    }
  }

  // MARK: Doing

  /// On to the next step left (a skipped passkey is not asked again), or closed when none is.
  private func next() {
    problem = nil
    confirming = false
    if !passkeyDone && !skipped && page != .passkey {
      page = .passkey
    } else if !wordsDone && page != .words {
      page = .words
    } else {
      UINotificationFeedbackGenerator().notificationOccurred(.success)
      dismiss()
    }
  }

  /// Not secured yet: the passkey starts the backup (and asks once more for 1Password). Secured by the words already:
  /// the passkey is added to it.
  private func makePasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      if secured {
        try await accounts.addPasskey()
        next()
      } else if try await accounts.securePasskey() {
        next()
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
      next()
    } catch {
      if !SecurityPresentation.isCancellation(error) { problem = "Couldn’t confirm passkey. Try again or skip." }
    }
  }

  /// The words as the card (an image) and as text, to the share sheet. Once saved somewhere, they also secure the
  /// account if nothing did yet, and the guide goes on.
  private func save() {
    let name = accounts.account?.username ?? ""
    let renderer = ImageRenderer(content: RecoveryCard(name: name, words: words))
    renderer.scale = 3
    var items: [Any] = ["Yo recovery key for \(name): " + words.joined(separator: " ")]
    if let image = renderer.uiImage { items.insert(image, at: 0) }
    Share.present(items) { completed in
      guard completed else { return }
      if !secured {
        do { try accounts.keepWords() } catch { return problem = "Couldn’t save. Try again." }
      }
      accounts.markWordsSaved(true)
      next()
    }
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

/// The recovery key from Account Security, after Face ID: the words and their QR code on the full-colour card, Save…
/// to the share sheet, Done to close. No second "show" step.
struct RecoveryShowSheet: View {
  let name: String
  let words: [String]
  @Environment(\.dismiss) private var dismiss
  @State private var saved = false

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Page(title: "Recovery Key", message: "Anyone with these words can sign in as you.") {
        FrostedKey(words: words, qrSize: 80)  // milky until tapped, as everywhere the words show
      } bottom: {
        PrimaryButton(saved ? "Done" : "Save…") {
          if saved { return dismiss() }
          let renderer = ImageRenderer(content: RecoveryCard(name: name, words: words))
          renderer.scale = 3
          var items: [Any] = ["Yo recovery key for \(name): " + words.joined(separator: " ")]
          if let image = renderer.uiImage { items.insert(image, at: 0) }
          Share.present(items) { completed in if completed { saved = true } }
        }
        Button("Done") { dismiss() }
          .font(.yo(17))
          .opacity(saved ? 0 : 1)
      }
    }
    .foregroundStyle(.white)
    .preferredColorScheme(.dark)
  }
}
