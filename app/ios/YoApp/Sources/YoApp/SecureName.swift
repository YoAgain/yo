// "Set Up Account Recovery" (spec 7): one full-colour page with two steps, each a card with its number (a green
// checkmark once done) and its buttons. 1, the recovery key: shown (frosted until tapped) and saved through the
// system's share sheet (Save Image, Notes, Files, a password manager, AirDrop) or copied. 2, the passkey: made, then
// used once for its PRF output (iCloud Keychain gives it at once; 1Password only when used). The passkey is optional;
// Done waits for the recovery key.
import SwiftUI

enum SecureStep { case passkey, recoveryKey }

struct SecureNameSheet: View {
  /// Kept for the callers; the page shows both steps.
  var start: SecureStep?
  /// The step the sheet was opened for; nil, the combined first-run guide.
  var only: SecureStep? = nil
  /// Closes the sheet through the presenter's binding.
  var close: (() -> Void)? = nil
  /// Finished with Done (not closed): the presenter may go on to the Security page.
  var done: (() -> Void)? = nil

  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var initialized = false
  /// The passkey is saved in the password manager; signing in with it once is left (Confirm).
  @State private var created = false
  @State private var words = ""
  @State private var busy: SecureStep?
  @State private var showingKey = false
  @State private var copiedKey = false
  @State private var problem: String?

  private var secured: Bool { accounts.account?.secured == true }
  private var passkeyDone: Bool {
    accounts.account?.byPasskey == true && accounts.account?.passkeyAuth == true
  }
  private var wordsDone: Bool { secured && accounts.wordsSaved }
  /// Done needs the recovery key; opened for the passkey alone, the passkey is enough.
  private var canFinish: Bool { wordsDone || (only == .passkey && passkeyDone) }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Page(title: "Set Up\nAccount Recovery") {
        StepCard(number: 1, title: "Recovery Key", done: wordsDone) {
          Button(showingKey ? "Hide" : "Show") {
            if showingKey { showingKey = false } else { Task { await showKey() } }
          }
            .buttonStyle(.glass)
          if !wordsDone {
            Button { Task { await share() } } label: {
              Image(systemName: "square.and.arrow.up")
                .accessibilityLabel("Save Recovery Key")
            }
            .buttonStyle(.glassProminent)
            .tint(.white)
            .foregroundStyle(.black)
          }
        }
        .disabled(busy != nil)
        // Shown: the key itself in its own card right under its step, the QR code and the words, and Copy.
        if showingKey {
          KeyCard(key: words, copied: $copiedKey) {
            UIPasteboard.general.string = RecoveryCode.shown(words)
            copiedKey = true
            Task { await markSaved() }
          }
          .transition(.opacity.combined(with: .move(edge: .top)))
        }
        StepCard(number: 2, title: "Passkey", done: passkeyDone) {
          if busy == .passkey {
            ProgressView()
          } else if !passkeyDone {
            Button(created ? "Confirm" : "Set Up") {
              Task { created ? await checkPasskey() : await createPasskey() }
            }
            .buttonStyle(.glassProminent)
            .tint(.white)
            .foregroundStyle(.black)
            .disabled(busy != nil)
          }
        }
        Note(problem)
          .padding(.top, 4)
      } bottom: {
        PrimaryButton("Done") { finish() }
          .disabled(!canFinish || busy != nil)
      }
    }
    .overlay(alignment: .topTrailing) { SheetClose(action: close).disabled(busy != nil) }
    .animation(.smooth(duration: 0.3), value: wordsDone)
    .animation(.smooth(duration: 0.3), value: passkeyDone)
    .animation(.smooth(duration: 0.3), value: created)
    .foregroundStyle(.white)
    .presentationDragIndicator(.visible)
    .preferredColorScheme(.dark)
    .interactiveDismissDisabled(busy != nil)
    .animation(.smooth(duration: 0.25), value: showingKey)
    .onAppear {
      guard !initialized else { return }
      initialized = true
      words = secured ? RecoveryWords.load() ?? "" : accounts.secureWords()
    }
  }

  // MARK: Doing

  /// The system's reason after the words, in Yo Dev only (for development); never in Yo itself.
  private static func detail(_ error: Error) -> String {
    guard Hub.isDevApp else { return "" }
    let e = error as NSError
    return " (\(e.domain) \(e.code): \(e.localizedDescription))"
  }

  /// Done: the passkey not set up is a choice (as Skip was), so it is not asked for again.
  private func finish() {
    if !passkeyDone { accounts.declinePasskey() }
    done?()
    if let close { close() } else { dismiss() }
  }

  /// The words to show or save; a secured account brought back without them gets new ones.
  private func ensureWords() async -> Bool {
    if words.count == 64 { return true }
    guard secured else {
      problem = "Your recovery key isn’t on this iPhone."
      return false
    }
    busy = .recoveryKey
    defer { busy = nil }
    do {
      words = try await accounts.newRecoveryKey()
      return true
    } catch {
      problem = error.localizedDescription
      return false
    }
  }

  private func showKey() async {
    problem = nil
    if await ensureWords() { showingKey = true }
  }

  /// The recovery card and text to the share sheet; done once something was done with them.
  private func share() async {
    problem = nil
    guard await ensureWords() else { return }
    let name = accounts.account?.username ?? ""
    let renderer = ImageRenderer(content: RecoveryCard(name: name, key: words))
    renderer.scale = 3
    var items: [Any] = ["Yo recovery key for \(name): " + RecoveryCode.shown(words)]
    if let image = renderer.uiImage { items.insert(image, at: 0) }
    Share.present(items) { completed in
      if completed { Task { await markSaved() } }
    }
  }

  /// Saved (shared or copied): the backup opens with the words, and the step is done.
  private func markSaved() async {
    guard !wordsDone, words.count == 64 else { return }
    busy = .recoveryKey
    defer { busy = nil }
    do {
      if !secured { try await accounts.keepWords() }
      accounts.markWordsSaved(true)
    } catch { problem = error.localizedDescription }
  }

  private func createPasskey() async {
    busy = .passkey
    defer { busy = nil }
    problem = nil
    do {
      try await accounts.createPasskey()
      UINotificationFeedbackGenerator().notificationOccurred(.success)
      created = true
    } catch {
      if !SecurityPresentation.isCancellation(error) {
        problem = "Couldn’t create passkey. Try again." + Self.detail(error)
      }
    }
  }

  private func checkPasskey() async {
    busy = .passkey
    defer { busy = nil }
    problem = nil
    do {
      try await accounts.checkPasskey()
      created = false
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
}

/// One step: its number in a white circle (a green checkmark once done), its name, its buttons, on glass.
private struct StepCard<Buttons: View>: View {
  let number: Int
  let title: String
  let done: Bool
  @ViewBuilder let buttons: Buttons

  var body: some View {
    HStack(spacing: 12) {
      Group {
        if done {
          Image(systemName: "checkmark.circle.fill")
            .foregroundStyle(.white, .green)
        } else {
          Image(systemName: "\(number).circle.fill")
            .symbolRenderingMode(.palette)
            .foregroundStyle(Color.amethyst, .white)
        }
      }
      .font(.system(size: 30))
      .contentTransition(.symbolEffect(.replace))
      .accessibilityHidden(true)
      Text(title)
        .font(.body.weight(.semibold))
        .accessibilityLabel(done ? "\(title), set up" : "Step \(number), \(title)")
      Spacer(minLength: 8)
      HStack(spacing: 8) { buttons }
        .font(.subheadline.weight(.semibold))
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 12)
    .frame(minHeight: 64)
    .background(.white.opacity(0.14), in: .rect(cornerRadius: 18))
    .sensoryFeedback(.success, trigger: done) { _, now in now }
  }
}

/// The recovery key shown in the guide, as a card under its step: QR code, the words, Copy (counted as saved).
private struct KeyCard: View {
  let key: String
  @Binding var copied: Bool
  let copy: () -> Void

  var body: some View {
    VStack(spacing: 14) {
      RecoveryQRView(key: key, size: 120)
      KeyGrid(key: key)
      Button(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc", action: copy)
        .buttonStyle(.glass)
    }
    .frame(maxWidth: .infinity)
    .padding(16)
    .background(.white.opacity(0.14), in: .rect(cornerRadius: 18))
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
