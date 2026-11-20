// "Account Recovery" (spec 7), one full-height screen each, without cards: the symbol, title and line, then at the
// bottom the step as a small line over one big white button and quiet links beneath.
// The recovery key (from the red badge and the account page's Recovery Key row): save it through the share sheet,
// or show it; either keeps it, and only then the button becomes Done. The passkey (from its row): create it, then try
// it once for its PRF output, as not every password manager gives one and asking up front isn't reliable.
import LocalAuthentication
import SwiftUI
import YoCore

/// The two recovery screens: full-height Amethyst assistants, the symbol, title and line centred in the screen; at
/// the bottom the step (a green check once done), the problem if any, the one big white button and its quiet links.
/// The content scrolls only when it does not fit; the close button leaves at any time.
private struct RecoveryScreen<Content: View, Links: View>: View {
  let symbol: String
  let title: String
  let line: String
  let step: String
  let stepDone: Bool
  let problem: String?
  let busy: Bool
  let button: String
  var buttonSymbol: String? = nil
  var close: (() -> Void)?
  let action: () -> Void
  @ViewBuilder let content: Content
  @ViewBuilder let links: Links

  @ScaledMetric(relativeTo: .largeTitle) private var symbolSize: CGFloat = 56
  @ScaledMetric(relativeTo: .footnote) private var check: CGFloat = 16

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      GeometryReader { geometry in
        ScrollView {
          VStack(spacing: 0) {
            Spacer(minLength: 56)
            Image(systemName: symbol)
              .font(.system(size: symbolSize))
              .accessibilityHidden(true)
            Text(title)
              .font(.yo(34))  // a setup screen's large title
              .multilineTextAlignment(.center)
              .lineLimit(2)
              .minimumScaleFactor(0.7)
              .accessibilityAddTraits(.isHeader)
              .padding(.top, 20)
            Text(line)
              .font(.subheadline.weight(.medium))  // 15 pt, with Dynamic Type
              .multilineTextAlignment(.center)
              .opacity(0.85)
              .padding(.top, 8)
            content
              .padding(.top, 24)
            Spacer(minLength: 32)
            HStack(spacing: 6) {
              if stepDone {
                Image(systemName: "checkmark")
                  .font(.system(size: check * 0.6, weight: .heavy))
                  .frame(width: check, height: check)
                  .background(Color.green, in: .circle)
                  .accessibilityHidden(true)
                  .transition(.scale.combined(with: .opacity))
              }
              Text(step.isEmpty ? " " : step)  // nothing to say yet: the line keeps its place, the button stays put
                .font(.footnote.weight(.semibold))
                .opacity(stepDone ? 1 : 0.85)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel(stepDone ? "\(step), done" : step)
            .accessibilityHidden(step.isEmpty)
            .sensoryFeedback(.success, trigger: stepDone) { _, now in now }
            Note(problem)
              .padding(.top, 8)
            Button(action: action) {
              Group {
                if busy {
                  ProgressView().tint(.black)
                } else if let buttonSymbol {
                  Label(button, systemImage: buttonSymbol)
                } else {
                  Text(button)
                }
              }
              .font(.system(.title3, weight: .semibold))
              .lineLimit(1)
              .minimumScaleFactor(0.7)
              .frame(maxWidth: .infinity, minHeight: 40)
            }
            .buttonStyle(.glassProminent)
            .tint(.white)
            .foregroundStyle(.black)
            .disabled(busy)
            .padding(.top, 10)
            HStack(spacing: 10) { links }
              .font(.subheadline.weight(.semibold))
              .buttonStyle(QuietLink())
              .disabled(busy)
              .padding(.top, 14)
          }
          .padding(.horizontal, 24)
          .padding(.bottom, 16)
          .frame(maxWidth: .infinity, minHeight: geometry.size.height)
        }
        .scrollBounceBehavior(.basedOnSize)
        .scrollIndicators(.hidden)
      }
    }
    .overlay(alignment: .topTrailing) { SheetClose(action: close).disabled(busy) }
    .foregroundStyle(.white)
    // Its own full-height sheet over the settings, as the first assistant was.
    .presentationDetents([.large])
    .presentationDragIndicator(.hidden)
    .preferredColorScheme(.dark)
    .interactiveDismissDisabled(busy)
  }
}

struct RecoveryKeySheet: View {
  var close: (() -> Void)? = nil
  /// Done (the key kept), when it leads somewhere else than `close`.
  var done: (() -> Void)? = nil

  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.scenePhase) private var phase
  @State private var initialized = false
  @State private var words = ""
  @State private var busy = false
  @State private var showingKey = false
  @State private var copiedKey = false
  /// Kept this time by showing it, not through the share sheet: the step says "Shown".
  @State private var keptByShowing = false
  /// Face ID passed once on this screen; showing then saving does not ask again.
  @State private var authorized = false
  @State private var problem: String?

  private var secured: Bool { accounts.account?.secured == true }
  private var saved: Bool { secured && accounts.wordsSaved }

  var body: some View {
    RecoveryScreen(
      symbol: "key.fill", title: ProductText.recoveryKeyTitle.english, line: ProductText.recoveryKeyLine.english,
      step: saved ? (keptByShowing ? ProductText.shown : .saved).english : "", stepDone: saved,
      problem: problem, busy: busy,
      button: (saved ? ProductText.done : .saveRecoveryKey).english, buttonSymbol: saved ? nil : "square.and.arrow.up",
      close: close, action: { if saved { finish() } else { Task { await share() } } }
    ) {
      if showingKey {
        KeyCard(key: words, copied: copiedKey) {
          UIPasteboard.general.string = RecoveryCode.shown(words)
          copiedKey = true
        }
        .transition(.opacity.combined(with: .move(edge: .top)))
      }
    } links: {
      if saved {
        Button(ProductText.saveAgain.english, systemImage: "square.and.arrow.up") { Task { await share() } }
          .accessibilityLabel("Save Recovery Key Again")
        Text("·").accessibilityHidden(true)
      }
      Button(
        (showingKey ? ProductText.hideKey : .showKey).english, systemImage: showingKey ? "eye.slash" : "eye"
      ) {
        if showingKey { showingKey = false } else { Task { await showKey() } }
      }
      .accessibilityLabel(showingKey ? "Hide Recovery Key" : "Show Recovery Key")
    }
    .animation(reduceMotion ? nil : .smooth(duration: 0.3), value: saved)
    .animation(reduceMotion ? nil : .smooth(duration: 0.25), value: showingKey)
    // Hidden again when Yo leaves the screen, so the app switcher's snapshot does not show it.
    .onChange(of: phase) { _, now in
      if now != .active { showingKey = false }
    }
    .onAppear {
      guard !initialized else { return }
      initialized = true
      words = secured ? RecoveryWords.load() ?? "" : accounts.secureWords()
    }
  }

  private func finish() {
    if let done { done() } else if let close { close() } else { dismiss() }
  }

  /// A key already saved shows or goes out again only after Face ID, once per screen.
  private func allowed(_ reason: String) async -> Bool {
    guard saved, !authorized else { return true }
    authorized = (try? await LAContext().evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)) == true
    return authorized
  }

  /// The words to show or save; a secured account brought back without them gets new ones.
  private func ensureWords() async -> Bool {
    if words.count == 64 { return true }
    guard secured else {
      problem = "Your recovery key isn’t on this iPhone."
      return false
    }
    busy = true
    defer { busy = false }
    do {
      words = try await accounts.newRecoveryKey()
      return true
    } catch {
      problem = Problems.words(for: error)
      return false
    }
  }

  private func showKey() async {
    problem = nil
    guard await allowed("Show your recovery key."), await ensureWords() else { return }
    showingKey = true
    // Seen counts as kept, as saved does: the step is ticked and Done opens.
    if !saved {
      authorized = true
      keptByShowing = true
      await markSaved()
    }
  }

  private func share() async {
    problem = nil
    guard await allowed("Save your recovery key."), await ensureWords() else { return }
    Share.recoveryKey(name: accounts.account?.username ?? "", key: words) { completed in
      if completed {
        keptByShowing = false
        authorized = true
        Task { await markSaved() }
      }
    }
  }

  /// Kept (shown or shared): the backup opens with the words.
  private func markSaved() async {
    guard !saved, words.count == 64 else { return }
    busy = true
    defer { busy = false }
    do {
      if !secured { try await accounts.keepWords() }
      accounts.markWordsSaved(true)
    } catch { problem = Problems.words(for: error) }
  }
}

struct PasskeySheet: View {
  var close: (() -> Void)? = nil

  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  /// Saved in the password manager; trying it once is left.
  @State private var created = false
  @State private var busy = false
  @State private var problem: String?

  private var on: Bool { accounts.account?.byPasskey == true && accounts.account?.passkeyAuth == true }

  var body: some View {
    RecoveryScreen(
      symbol: "person.badge.key.fill", title: ProductText.passkey.english,
      line: "Sign in with Face ID on a new phone.",
      step: (on ? ProductText.passkeyOn : created ? .passkeyStepTry : .passkeyStepCreate).english, stepDone: on,
      problem: problem, busy: busy,
      button: on ? ProductText.done.english : created ? "Try with Face ID" : ProductText.createPasskey.english,
      buttonSymbol: on ? nil : created ? "faceid" : nil, close: close,
      action: {
        if on { finish() } else if created { Task { await checkPasskey() } } else { Task { await createPasskey() } }
      }
    ) {
      EmptyView()
    } links: {
      EmptyView()
    }
    .animation(reduceMotion ? nil : .smooth(duration: 0.3), value: on)
    .animation(reduceMotion ? nil : .smooth(duration: 0.3), value: created)
  }

  private func finish() { if let close { close() } else { dismiss() } }

  private func createPasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      try await accounts.createPasskey()
      UINotificationFeedbackGenerator().notificationOccurred(.success)
      created = true
    } catch {
      if !Passkeys.isCancellation(error) {
        problem = "Couldn’t create passkey. Try again."
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
    } catch AccountError.tooMany {
      problem = "Too many tries. Try again later."
    } catch AccountError.noPRF {
      problem = "Your password manager can’t do this. Try iCloud Keychain."
    } catch {
      if !Passkeys.isCancellation(error) {
        problem = "That didn’t work. Try again."
      }
    }
  }
}

/// A quiet link under the big button, plain as text but 44 pt tall to touch (its text alone was about 20).
private struct QuietLink: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .frame(minHeight: 44)
      .contentShape(.rect)
      .opacity(configuration.isPressed ? 0.5 : 1)
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
