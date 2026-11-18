// "Set Up Account Recovery" (spec 7): one full-colour page, a large plain key symbol above the title, two steps,
// each its caption and one full-width white button, the action itself (a translucent row with a green checkmark
// once done). 1, Save Recovery Key through the system's share sheet (Save Image, Notes, Files, a password manager,
// AirDrop); Show Recovery Key under it opens the key in a card, Copy there counts as saved. 2, Set Up Passkey
// (Face ID): made, then Confirm Passkey uses it once for its PRF output (iCloud Keychain gives it at once;
// 1Password only when used). Done once either step is done.
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
  private var canFinish: Bool { wordsDone || passkeyDone }  // either way back in is enough

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      GuidePage(symbol: "key.fill", title: "Set Up\nAccount Recovery") {
        // Two steps, each its caption and one full-width button, the step's action itself; once done, a
        // translucent row with a green checkmark.
        StepCaption(number: 1)
        StepButton(
          "Save Recovery Key", systemImage: "square.and.arrow.up", doneTitle: "Recovery Key Saved",
          done: wordsDone, busy: busy == .recoveryKey
        ) { Task { await share() } }
        .disabled(busy != nil)
        // Under the step: Show Recovery Key, the key itself in its own card, the QR code, the words, Copy.
        Button {
          if showingKey { showingKey = false } else { Task { await showKey() } }
        } label: {
          Label(showingKey ? "Hide Recovery Key" : "Show Recovery Key", systemImage: showingKey ? "eye.slash" : "eye")
            .font(.subheadline.weight(.semibold))
            .contentTransition(.symbolEffect(.replace))
            .frame(minHeight: 36)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .opacity(0.85)
        .disabled(busy != nil)
        if showingKey {
          KeyCard(key: words, copied: copiedKey) {
            UIPasteboard.general.string = RecoveryCode.shown(words)
            copiedKey = true
            Task { await markSaved() }
          }
          .transition(.opacity.combined(with: .move(edge: .top)))
        }
        StepCaption(number: 2)
          .padding(.top, 12)
        StepButton(
          created ? "Confirm Passkey" : "Set Up Passkey", systemImage: "faceid", doneTitle: "Passkey Set Up",
          done: passkeyDone, busy: busy == .passkey
        ) { Task { created ? await checkPasskey() : await createPasskey() } }
        .disabled(busy != nil)
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

/// The guide's page, as Apple's Settings: a large plain symbol above the title, the steps under it scrolling when
/// large text needs the room, the buttons at the bottom.
private struct GuidePage<Content: View, Bottom: View>: View {
  let symbol: String
  let title: String
  @ViewBuilder let content: Content
  @ViewBuilder let bottom: Bottom

  var body: some View {
    VStack(spacing: 0) {
      Color.clear.frame(height: 44)
      ScrollView {
        VStack(spacing: 20) {
          Image(systemName: symbol)
            .font(.system(size: 96))
            .accessibilityHidden(true)
          Text(title).font(.yo(30)).multilineTextAlignment(.center)
        }
        .padding(.top, 8)
        VStack(spacing: 8) { content }
          .padding(.top, 28)
          .padding(.bottom, 16)
      }
      .scrollBounceBehavior(.basedOnSize)
      .scrollIndicators(.hidden)
      VStack(spacing: 18) { bottom }
    }
    .padding(.horizontal, 24)
    .padding(.bottom, 16)
  }
}

/// A step's caption above its card: "STEP 1", small and uppercase.
private struct StepCaption: View {
  let number: Int

  var body: some View {
    Text("Step \(number)")
      .textCase(.uppercase)
      .font(.caption.weight(.semibold))
      .opacity(0.75)
      .frame(maxWidth: .infinity, alignment: .center)
  }
}

/// One step's action: a full-width white button with black label (a progress view while busy); once done, a
/// translucent row with a green checkmark and what was done, no longer a button.
private struct StepButton: View {
  let title: String
  let systemImage: String
  let doneTitle: String
  let done: Bool
  let busy: Bool
  let action: () -> Void

  init(
    _ title: String, systemImage: String, doneTitle: String, done: Bool, busy: Bool,
    action: @escaping () -> Void
  ) {
    self.title = title
    self.systemImage = systemImage
    self.doneTitle = doneTitle
    self.done = done
    self.busy = busy
    self.action = action
  }

  var body: some View {
    Group {
      if done {
        Label {
          Text(doneTitle)
        } icon: {
          Image(systemName: "checkmark.circle.fill")
            .symbolRenderingMode(.palette)
            .foregroundStyle(.white, .green)
        }
        .font(.system(.title3, weight: .semibold))
        .frame(maxWidth: .infinity, minHeight: 48)
        .background(.white.opacity(0.14), in: .capsule)
        .accessibilityElement(children: .combine)
        .transition(.opacity)
      } else {
        Button(action: action) {
          Group {
            if busy {
              ProgressView().tint(.black)
            } else {
              Label(title, systemImage: systemImage)
                .contentTransition(.interpolate)
            }
          }
          .font(.system(.title3, weight: .semibold))
          .frame(maxWidth: .infinity, minHeight: 40)
        }
        .buttonStyle(.glassProminent)  // the same glass and height as Done below
        .tint(.white)
        .foregroundStyle(.black)
        .transition(.opacity)
      }
    }
    .sensoryFeedback(.success, trigger: done) { _, now in now }
  }
}

/// The recovery key shown in its own card under its step: QR code, the words, Copy (counted as saved).
private struct KeyCard: View {
  let key: String
  let copied: Bool
  let copy: () -> Void

  var body: some View {
    VStack(spacing: 12) {
      RecoveryQRView(key: key, size: 120)
      KeyGrid(key: key)
      Button(copied ? "Copied" : "Copy", action: copy)
        .buttonStyle(.plain)
        .fontWeight(.semibold)
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
