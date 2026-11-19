// "Set Up Account Recovery" (spec 7): 1, save the recovery key through the share sheet (Copy on its card counts too);
// 2, set up a passkey, then confirm it once for its PRF output. Done once either step is done.
import SwiftUI

enum SecureStep { case passkey, recoveryKey }

struct SecureNameSheet: View {
  var close: (() -> Void)? = nil
  /// Finished with Done (not closed): the presenter may go on to the Security page.
  var done: (() -> Void)? = nil

  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var initialized = false
  /// Saved in the password manager; signing in with it once is left (Confirm).
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
  private var canFinish: Bool { wordsDone || passkeyDone }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      GuidePage(symbol: "key.fill", title: "Set Up\nAccount Recovery") {
        StepCaption(number: 1)
        StepButton(
          "Save Recovery Key", systemImage: "square.and.arrow.up", doneTitle: "Recovery Key Saved",
          done: wordsDone, busy: busy == .recoveryKey
        ) { Task { await share() } }
        .disabled(busy != nil)
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

  private static func detail(_ error: Error) -> String {
    guard Hub.isDevApp else { return "" }
    let e = error as NSError
    return " (\(e.domain) \(e.code): \(e.localizedDescription))"
  }

  /// The passkey not set up is a choice, so it is not asked for again.
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

  private func share() async {
    problem = nil
    guard await ensureWords() else { return }
    Share.recoveryKey(name: accounts.account?.username ?? "", key: words) { completed in
      if completed { Task { await markSaved() } }
    }
  }

  /// Saved (shared or copied): the backup opens with the words.
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
      if !Passkeys.isCancellation(error) {
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
      if !Passkeys.isCancellation(error) {
        problem = "That didn’t work. Try again." + Self.detail(error)
      }
    }
  }
}

/// A large symbol above the title, the steps scrolling when large text needs the room, the buttons at the bottom.
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
        .buttonStyle(.glassProminent)  // matches Done below
        .tint(.white)
        .foregroundStyle(.black)
        .transition(.opacity)
      }
    }
    .sensoryFeedback(.success, trigger: done) { _, now in now }
  }
}

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

/// The share sheet from whatever is on top, telling whether something was done with the items.
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

  static func recoveryKey(name: String, key: String, completion: @escaping @MainActor (Bool) -> Void) {
    let renderer = ImageRenderer(content: RecoveryCard(name: name, key: key))
    renderer.scale = 3
    var items: [Any] = ["Yo recovery key for \(name): " + RecoveryCode.shown(key)]
    if let image = renderer.uiImage { items.insert(image, at: 0) }
    present(items, completion: completion)
  }
}

struct RecoveryShowSheet: View {
  let name: String
  let key: String
  @Environment(\.dismiss) private var dismiss
  @State private var saved = false

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Page(title: "Recovery Key", message: "Anyone with this key can sign in as you.") {
        FrostedKey(key: key, qrSize: 80)  // milky until tapped
      } bottom: {
        PrimaryButton(saved ? "Done" : "Save…") {
          if saved { return dismiss() }
          Share.recoveryKey(name: name, key: key) { completed in if completed { saved = true } }
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
