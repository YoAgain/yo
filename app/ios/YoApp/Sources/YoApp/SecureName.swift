// "Secure Your Account" (spec 7): a guide of two full-colour pages like the onboarding, one task each, as iOS guides a
// setup. Opened at a step (from its row) or at the first one left; after each step it goes on to the next one left,
// and closes when none is. 1, the passkey: made, then used once for its PRF output (iCloud Keychain gives it at once;
// 1Password only when used, so the same page asks once more); skippable. 2, the recovery key: the 12 words, saved
// through the system's share sheet (Save Image, Notes, Files, a password manager, AirDrop).
import SwiftUI

enum SecureStep { case passkey, recoveryKey }

struct SecureNameSheet: View {
  /// Where to begin; nil, the first step left.
  var start: SecureStep?
  /// nil keeps the combined first-run guide.
  var only: SecureStep? = nil
  /// Closes the sheet through the presenter's binding.
  var close: (() -> Void)? = nil

  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var page = Stage.passkey
  /// The passkey is saved in the password manager; signing in with it once is left (the check).
  @State private var initialized = false
  @State private var created = false
  @State private var skipped = false
  @State private var words = ""
  @State private var busy = false
  @State private var problem: String?

  private enum Stage { case passkey, wordsIntro, words, done }

  private var secured: Bool { accounts.account?.secured == true }
  private var passkeyDone: Bool {
    accounts.account?.byPasskey == true && accounts.account?.passkeyAuth == true
  }
  private var wordsDone: Bool { secured && accounts.wordsSaved }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch page {
        case .passkey:
          if created { checkPage } else { passkeyPage }
        case .wordsIntro: wordsIntroPage
        case .words: wordsPage
        case .done: donePage
        }
      }
      .transition(.asymmetric(insertion: .move(edge: .trailing), removal: .move(edge: .leading)))
    }
    .overlay(alignment: .topTrailing) { SheetClose(action: close).disabled(busy) }
    .animation(.smooth(duration: 0.35), value: page)
    .animation(.smooth(duration: 0.35), value: created)
    .foregroundStyle(.white)
    .presentationDragIndicator(.visible)
    .preferredColorScheme(.dark)
    .interactiveDismissDisabled(busy)
    .onAppear {
      guard !initialized else { return }
      initialized = true
      words = secured ? RecoveryWords.load() ?? "" : accounts.secureWords()
      skipped = accounts.passkeyTurnedOff && (only ?? start) != .passkey  // declined on purpose: not asked again
      switch only ?? start {
      case .passkey: page = .passkey
      case .recoveryKey: page = .wordsIntro
      case nil: page = passkeyDone || skipped ? .wordsIntro : .passkey
      }
    }
  }

  // MARK: Pages

  private var passkeyPage: some View {
    Page(title: "Secure Your Account",
      message: "In case you leave Apple or lose your Apple Account.") {
      BigSymbol(name: "faceid")
    } bottom: {
      PrimaryButton("Continue with Face ID", busy: busy) { Task { await createPasskey() } }
        .disabled(busy)
      Button(only == nil ? "Skip" : "Cancel") {
        if only != nil { dismiss(); return }
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
      // A password manager that cannot do it (no PRF): on without the passkey, to the recovery key.
      Button(only == nil ? "Skip" : "Cancel") {
        if only != nil { dismiss(); return }
        skipped = true
        created = false
        accounts.declinePasskey()  // as the first page's Skip: not asked again
        next()
      }
      .font(.yo(17))
      .disabled(busy)
      Note(problem)
    }
  }

  private var wordsIntroPage: some View {
    Page(title: "Recovery Key", message: "Save your recovery key to sign back in.") {
      BigSymbol(name: "key.fill")
    } bottom: {
      PrimaryButton("Continue", busy: busy) {
        Task {
          busy = true
          defer { busy = false }
          do {
            if secured && words.isEmpty { words = try await accounts.newRecoveryKey() }
            page = .words
          } catch { problem = error.localizedDescription }
        }
      }
      .disabled(busy)
      Note(problem)
    }
  }

  private var wordsPage: some View {
    Page(title: "Recovery Key",
      message: only == nil && passkeyDone ? "Your passkey works. Save your recovery key too." : "Save your recovery key to sign back in.") {
      FrostedKey(key: words, qrSize: 80)
    } bottom: {
      PrimaryButton("Save…") { save() }
        .disabled(busy)
      Button("Copy") { UIPasteboard.general.string = RecoveryCode.shown(words) }
        .font(.yo(17))
        .disabled(busy)
      Button("I Saved My Recovery Key") { Task { await confirmWords() } }
        .font(.yo(17))
        .disabled(busy)
      Note(problem)
    }
  }

  /// The end: it worked, and what is set up, each with a clean green checkmark.
  private var donePage: some View {
    Page(title: "All Set", message: "“\(accounts.account?.username ?? "")” is yours forever.") {
      VStack(spacing: 12) {
        if only != .recoveryKey && passkeyDone { DoneRow(title: "Passkey and Face ID") }
        if only != .passkey && wordsDone { DoneRow(title: "Recovery Key") }
      }
      .padding(.top, 20)
    } bottom: {
      PrimaryButton("Done") { dismiss() }
    }
  }

  // MARK: Doing

  /// The system's reason after the words, in Yo Dev only (for development); never in Yo itself.
  private static func detail(_ error: Error) -> String {
    guard Hub.isDevApp else { return "" }
    let e = error as NSError
    return " (\(e.domain) \(e.code): \(e.localizedDescription))"
  }

  /// On to the next step left (a skipped passkey is not asked again), or the end.
  private func next() {
    problem = nil
    if only != nil {
      page = .done
    } else if !passkeyDone && !skipped && page != .passkey {
      page = .passkey
    } else if !wordsDone && page != .words {
      page = .wordsIntro
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
      if !SecurityPresentation.isCancellation(error) {
        problem = (only == nil ? "Couldn’t create passkey. Try again or skip." : "Couldn’t create passkey. Try again.") + Self.detail(error)
      }
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
    } catch AccountError.tooMany {
      problem = "Too many tries. Try again later."
    } catch AccountError.noPRF {
      problem = "This password manager can’t be used. Try iCloud Keychain."
    } catch {
      if !SecurityPresentation.isCancellation(error) {
        problem = "That didn’t work. Try again." + Self.detail(error)
      }
    }
  }

  private func confirmWords() async {
    guard words.count == 64 else { return problem = "Your recovery key isn’t on this iPhone." }
    busy = true
    defer { busy = false }
    do {
      if !secured { try await accounts.keepWords() }
      accounts.markWordsSaved(true)
      next()
    } catch { problem = error.localizedDescription }
  }

  /// Share the recovery card and text before confirming that the key is saved.
  private func save() {
    // The key missing here (a secured account brought back without it): nothing to draw.
    guard words.count == 64 else {
      return problem = "Your recovery key isn’t on this iPhone."
    }
    let name = accounts.account?.username ?? ""
    let renderer = ImageRenderer(content: RecoveryCard(name: name, key: words))
    renderer.scale = 3
    var items: [Any] = ["Yo recovery key for \(name): " + RecoveryCode.shown(words)]
    if let image = renderer.uiImage { items.insert(image, at: 0) }
    Share.present(items) { completed in
      guard completed else { return }
      // Sharing is followed by the explicit saved confirmation on the key page.
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
      Text(title).font(.body.weight(.semibold))
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

/// The recovery key from Security, after Face ID: the key and its QR code on the full-colour card, Save…
/// to the share sheet, Done to close. No second "show" step.
struct RecoveryShowSheet: View {
  let name: String
  let key: String
  @Environment(\.dismiss) private var dismiss
  @State private var saved = false

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Page(title: "Recovery Key", message: "Anyone with this key can sign in as you.") {
        FrostedKey(key: key, qrSize: 80)  // milky until tapped, as everywhere the key shows
      } bottom: {
        PrimaryButton(saved ? "Done" : "Save…") {
          if saved { return dismiss() }
          let renderer = ImageRenderer(content: RecoveryCard(name: name, key: key))
          renderer.scale = 3
          var items: [Any] = ["Yo recovery key for \(name): " + RecoveryCode.shown(key)]
          if let image = renderer.uiImage { items.insert(image, at: 0) }
          Share.present(items) { completed in if completed { saved = true } }
        }
        Button("Done") { dismiss() }
          .font(.yo(17))
          .opacity(saved ? 0 : 1)
      }
    }
    .overlay(alignment: .topTrailing) { SheetClose() }
    .foregroundStyle(.white)
    .preferredColorScheme(.dark)
    .presentationDragIndicator(.visible)
  }
}
