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
  @State private var page = Stage.passkey
  /// The passkey is saved in the password manager; signing in with it once is left (the check).
  @State private var created = false
  @State private var skipped = false
  @State private var words: [String] = []
  @State private var busy = false
  @State private var problem: String?

  private enum Stage { case passkey, words, done }

  private var secured: Bool { accounts.account?.secured == true }
  private var passkeyDone: Bool { accounts.account?.byPasskey == true }
  private var wordsDone: Bool { secured && accounts.wordsSaved }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch page {
        case .passkey:
          if created { checkPage } else { passkeyPage }
        case .words: wordsPage
        case .done: donePage
        }
      }
      .transition(.asymmetric(insertion: .move(edge: .trailing), removal: .move(edge: .leading)))
    }
    .animation(.smooth(duration: 0.35), value: page)
    .animation(.smooth(duration: 0.35), value: created)
    .foregroundStyle(.white)
    .preferredColorScheme(.dark)
    .interactiveDismissDisabled(busy)
    .onAppear {
      guard words.isEmpty else { return }
      words = secured ? RecoveryWords.load() ?? [] : accounts.secureWords()
      skipped = accounts.passkeyTurnedOff && start != .passkey  // declined on purpose: not asked again
      switch start {
      case .passkey: page = .passkey
      case .words: page = .words
      case nil: page = passkeyDone || skipped ? .words : .passkey
      }
    }
  }

  // MARK: Pages

  private var passkeyPage: some View {
    Page(title: "Secure Your Account",
      message: "Keep your name “\(accounts.account?.username ?? "")” forever.") {
      BigSymbol(name: "faceid")
    } bottom: {
      PrimaryButton("Continue with Face ID", busy: busy) { Task { await createPasskey() } }
        .disabled(busy)
      Button("Skip") {
        skipped = true
        accounts.declinePasskey()  // a choice, as turning it off: not asked again
        next()
      }
      .font(.yo(17))
      .disabled(busy)
      Note(problem)
    }
  }

  /// Saved: a green checkmark, then one sign-in to see that it works.
  private var checkPage: some View {
    Page(title: "Passkey Saved", message: "Now sign in once to check it works.") {
      BigSymbol(name: "checkmark.circle.fill", done: true)
    } bottom: {
      PrimaryButton("Sign In", busy: busy) { Task { await checkPasskey() } }
        .disabled(busy)
      Note(problem)
    }
  }

  private var wordsPage: some View {
    Page(title: "Recovery Key",
      message: passkeyDone ? "Your passkey works. Save these 12 words too." : "Save these 12 words to sign back in.") {
      FrostedKey(words: words, qrSize: 80)
    } bottom: {
      PrimaryButton("Save…") { save() }
      Note(problem)
    }
  }

  /// The end: it worked, and what is set up, each with a clean green checkmark.
  private var donePage: some View {
    Page(title: "All Set", message: "“\(accounts.account?.username ?? "")” is yours forever.") {
      VStack(spacing: 12) {
        if passkeyDone { DoneRow(title: "Passkey") }
        if wordsDone { DoneRow(title: "Recovery Key") }
      }
      .padding(.top, 20)
    } bottom: {
      PrimaryButton("Done") { dismiss() }
    }
  }

  // MARK: Doing

  /// On to the next step left (a skipped passkey is not asked again), or the end.
  private func next() {
    problem = nil
    if !passkeyDone && !skipped && page != .passkey {
      page = .passkey
    } else if !wordsDone && page != .words {
      page = .words
    } else {
      UINotificationFeedbackGenerator().notificationOccurred(.success)
      page = .done
    }
  }

  private func createPasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      try await accounts.createPasskey()
      UINotificationFeedbackGenerator().notificationOccurred(.success)
      created = true
    } catch {
      if !SecurityPresentation.isCancellation(error) { problem = "Couldn’t create passkey. Try again or skip." }
    }
  }

  private func checkPasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      try await accounts.checkPasskey()
      created = false
      next()
    } catch AccountError.noPRF {
      problem = "This password manager can’t be used. Try iCloud Keychain."
    } catch {
      if !SecurityPresentation.isCancellation(error) { problem = "That didn’t work. Try again." }
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
      Task {
        if !secured {
          busy = true
          defer { busy = false }
          do { try await accounts.keepWords() } catch { return problem = error.localizedDescription }
        }
        accounts.markWordsSaved(true)
        next()
      }
    }
  }
}

/// One thing set up: a green checkmark in a circle and its name, on glass.
private struct DoneRow: View {
  let title: String

  var body: some View {
    HStack(spacing: 14) {
      Image(systemName: "checkmark.circle.fill")
        .font(.system(size: 30))
        .foregroundStyle(.white, .green)
      Text(title).font(.system(size: 18, weight: .semibold))
      Spacer()
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 14)
    .background(.white.opacity(0.14), in: .rect(cornerRadius: 18))
    .accessibilityElement(children: .combine)
    .accessibilityLabel("\(title), set up")
  }
}

/// A large symbol for a guide page; green and white once something is done.
private struct BigSymbol: View {
  let name: String
  var done = false

  var body: some View {
    Image(systemName: name)
      .font(.system(size: 80, weight: done ? .regular : .light))
      .foregroundStyle(.white, done ? Color.green : .white)
      .symbolEffect(.bounce, value: done)
      .padding(.top, 28)
      .accessibilityHidden(true)
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
