// "Account Recovery" (spec 7): two step cards, 1, save the recovery key through the share sheet (then Show reveals
// it); 2, set up a passkey, then confirm it once for its PRF output. Done finishes once either step is done.
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
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  private var secured: Bool { accounts.account?.secured == true }
  private var passkeyDone: Bool {
    accounts.account?.byPasskey == true && accounts.account?.passkeyAuth == true
  }
  private var wordsDone: Bool { secured && accounts.wordsSaved }
  private var canFinish: Bool { wordsDone || passkeyDone }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      ScrollView {
        VStack(spacing: 0) {
          Image(systemName: "key.fill")
            .font(.system(size: 64))
            .accessibilityHidden(true)
          Text("Set Up\nAccount Recovery")
            .font(.yo(30))
            .multilineTextAlignment(.center)
            .lineLimit(2)
            .minimumScaleFactor(0.6)
            .accessibilityAddTraits(.isHeader)
            .padding(.top, 14)
          Text("Either one brings your account back on a new phone.")
            .font(.subheadline.weight(.semibold))
            .multilineTextAlignment(.center)
            .opacity(0.85)
            .padding(.top, 6)
          VStack(spacing: 10) {
            StepCard(
              number: 1, title: "Recovery Key", done: wordsDone, busy: busy == .recoveryKey,
              actionTitle: wordsDone ? nil : "Save", doneTitle: "Saved",
              action: { Task { await share() } }
            ) {
              if wordsDone {
                HStack(spacing: 4) {
                  Text("Saved ·")
                  Button(showingKey ? "Hide" : "Show") {
                    if showingKey { showingKey = false } else { Task { await showKey() } }
                  }
                  .buttonStyle(.plain)
                  .underline()
                  .accessibilityLabel(showingKey ? "Hide Recovery Key" : "Show Recovery Key")
                }
              }
            }
            if showingKey {
              KeyCard(key: words, copied: copiedKey) {
                UIPasteboard.general.string = RecoveryCode.shown(words)
                copiedKey = true
              }
              .transition(.opacity.combined(with: .move(edge: .top)))
            }
            StepCard(
              number: 2, title: "Passkey", done: passkeyDone, busy: busy == .passkey,
              actionTitle: passkeyDone ? nil : created ? "Confirm" : "Set Up", doneTitle: "Done",
              action: { Task { created ? await checkPasskey() : await createPasskey() } }
            ) {
              if created && !passkeyDone { Text("Confirm it once with Face ID") }
            }
          }
          .disabled(busy != nil)
          .padding(.top, 28)
          Note(problem)
            .padding(.top, 8)
          PrimaryButton("Done") { finish() }
            .disabled(busy != nil)
            .padding(.top, 28)
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 56)
      }
      .scrollBounceBehavior(.basedOnSize)
      .scrollIndicators(.hidden)
      .defaultScrollAnchor(.center, for: .alignment)
    }
    .overlay(alignment: .topTrailing) { SheetClose(action: close).disabled(busy != nil) }
    .animation(reduceMotion ? nil : .smooth(duration: 0.3), value: wordsDone)
    .animation(reduceMotion ? nil : .smooth(duration: 0.3), value: passkeyDone)
    .animation(reduceMotion ? nil : .smooth(duration: 0.3), value: created)
    .animation(reduceMotion ? nil : .smooth(duration: 0.25), value: showingKey)
    .foregroundStyle(.white)
    // Its own full-height sheet over the settings, whatever height they keep.
    .presentationDetents([.large])
    .presentationDragIndicator(.hidden)
    .preferredColorScheme(.dark)
    .interactiveDismissDisabled(busy != nil)
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

  /// The passkey not set up is a choice, so it is not asked for again. Nothing done yet: Done only closes.
  private func finish() {
    guard canFinish else {
      if let close { close() } else { dismiss() }
      return
    }
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

/// One step: its number (a green check once done), name and a short line, its button on the right.
private struct StepCard<Detail: View>: View {
  let number: Int
  let title: String
  let done: Bool
  let busy: Bool
  /// The button's title while the step is open, nil once done.
  let actionTitle: String?
  let doneTitle: String
  let action: () -> Void
  @ViewBuilder let detail: Detail

  @Environment(\.dynamicTypeSize) private var typeSize
  @ScaledMetric(relativeTo: .headline) private var circle: CGFloat = 30

  var body: some View {
    let layout = typeSize.isAccessibilitySize
      ? AnyLayout(VStackLayout(alignment: .leading, spacing: 12))
      : AnyLayout(HStackLayout(spacing: 12))
    layout {
      HStack(spacing: 12) {
        ZStack {
          Circle().fill(done ? Color.green : .white)
          Group {
            if done {
              Image(systemName: "checkmark").fontWeight(.heavy).foregroundStyle(.white)
            } else {
              Text("\(number)").font(.yo(15, relativeTo: .headline)).foregroundStyle(Color.amethyst)
            }
          }
          .transition(.scale.combined(with: .opacity))
        }
        .frame(width: circle, height: circle)
        .accessibilityHidden(true)
        VStack(alignment: .leading, spacing: 2) {
          Text(title).font(.headline)
            .accessibilityLabel("Step \(number), \(title)\(done ? ", done" : "")")
          detail.font(.footnote.weight(.medium)).opacity(0.85)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
      }
      Group {
        if let actionTitle {
          Button(action: action) {
            Group {
              if busy { ProgressView().tint(.black) } else { Text(actionTitle) }
            }
            .font(.subheadline.weight(.bold))
            .padding(.horizontal, 4)
            .frame(minHeight: 28)
          }
          .buttonStyle(.glassProminent)
          .tint(.white)
          .foregroundStyle(.black)
          .accessibilityLabel("\(actionTitle) \(title)")
        } else {
          Text(doneTitle)
            .font(.subheadline.weight(.bold))
            .padding(.horizontal, 16)
            .frame(minHeight: 34)
            .overlay { Capsule().strokeBorder(.white.opacity(0.6), lineWidth: 1.5) }
            .accessibilityHidden(true)
        }
      }
      .fixedSize()
      .transition(.opacity)
    }
    .padding(14)
    .background(.white.opacity(0.14), in: .rect(cornerRadius: 20))
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
    .presentationDetents([.large])
    .presentationDragIndicator(.hidden)
  }
}
