// "Account Recovery" (spec 7), one screen each, built from the same step cards; only the cards change state.
// The recovery key (from the red badge and the account page's Recovery Key row): save it through the share sheet,
// show it. The passkey (from its row): create it, then try it once for its PRF output, as not every password manager
// gives one and asking up front isn't reliable.
import LocalAuthentication
import SwiftUI
import YoCore

/// The two recovery screens open at half height, one detent, never growing.
private struct RecoveryScreen<Content: View>: View {
  let symbol: String
  let title: String
  let line: String
  let busy: Bool
  var close: (() -> Void)?
  @ViewBuilder let content: Content

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      ScrollView {
        VStack(spacing: 0) {
          Image(systemName: symbol)
            .font(.system(size: 36))
            .accessibilityHidden(true)
          Text(title)
            .font(.yo(24))
            .multilineTextAlignment(.center)
            .lineLimit(2)
            .minimumScaleFactor(0.6)
            .accessibilityAddTraits(.isHeader)
            .padding(.top, 8)
          Text(line)
            .font(.footnote.weight(.semibold))
            .multilineTextAlignment(.center)
            .opacity(0.85)
            .padding(.top, 4)
          content
        }
        .padding(.horizontal, 24)
        .padding(.top, 28)
        .padding(.bottom, 16)
      }
      .scrollBounceBehavior(.basedOnSize)
      .scrollIndicators(.hidden)
    }
    .overlay(alignment: .topTrailing) { SheetClose(action: close).disabled(busy).padding(.top, 12) }
    .foregroundStyle(.white)
    .presentationDetents([.medium])
    .presentationDragIndicator(.hidden)
    .preferredColorScheme(.dark)
    .interactiveDismissDisabled(busy)
  }
}

struct RecoveryKeySheet: View {
  var close: (() -> Void)? = nil

  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.scenePhase) private var phase
  @State private var initialized = false
  @State private var words = ""
  @State private var busy = false
  @State private var showingKey = false
  @State private var copiedKey = false
  @State private var problem: String?

  private var secured: Bool { accounts.account?.secured == true }
  private var saved: Bool { secured && accounts.wordsSaved }

  var body: some View {
    RecoveryScreen(
      symbol: "key.fill", title: ProductText.recoveryKeyTitle.english, line: ProductText.recoveryKeyLine.english,
      busy: busy, close: close
    ) {
      VStack(spacing: 10) {
        StepCard(
          number: 1, title: ProductText.recoveryKey.english,
          status: (saved ? ProductText.saved : .notSavedYet).english, done: saved
        ) {
          StepButton(
            title: (showingKey ? ProductText.hide : .show).english, symbol: showingKey ? "eye.slash" : "eye",
            prominent: false
          ) {
            if showingKey { showingKey = false } else { Task { await showKey() } }
          }
          .accessibilityLabel(showingKey ? "Hide Recovery Key" : "Show Recovery Key")
          StepButton(
            title: (saved ? ProductText.saveAgain : .save).english, symbol: "square.and.arrow.up", busy: busy
          ) { Task { await share() } }
          .accessibilityLabel(saved ? "Save Recovery Key Again" : "Save Recovery Key")
        }
        if showingKey {
          KeyCard(key: words, copied: copiedKey) {
            UIPasteboard.general.string = RecoveryCode.shown(words)
            copiedKey = true
          }
          .transition(.opacity.combined(with: .move(edge: .top)))
        }
      }
      .disabled(busy)
      .padding(.top, 18)
      Note(problem)
        .padding(.top, 8)
      PrimaryButton(ProductText.done.english) { if let close { close() } else { dismiss() } }
        .disabled(busy)
        .padding(.top, 18)
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

  /// A key already saved shows or goes out again only after Face ID.
  private func allowed(_ reason: String) async -> Bool {
    guard saved else { return true }
    return (try? await LAContext().evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)) == true
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
      problem = error.localizedDescription
      return false
    }
  }

  private func showKey() async {
    problem = nil
    guard await allowed("Show your recovery key."), await ensureWords() else { return }
    showingKey = true
  }

  private func share() async {
    problem = nil
    guard await allowed("Save your recovery key."), await ensureWords() else { return }
    Share.recoveryKey(name: accounts.account?.username ?? "", key: words) { completed in
      if completed { Task { await markSaved() } }
    }
  }

  /// Saved (shared or copied): the backup opens with the words.
  private func markSaved() async {
    guard !saved, words.count == 64 else { return }
    busy = true
    defer { busy = false }
    do {
      if !secured { try await accounts.keepWords() }
      accounts.markWordsSaved(true)
    } catch { problem = error.localizedDescription }
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
      line: "Sign in with Face ID on a new phone. Works with iCloud Keychain and some password managers.",
      busy: busy, close: close
    ) {
      VStack(spacing: 10) {
        StepCard(
          number: 1, title: ProductText.createPasskey.english,
          status: on || created ? ProductText.created.english : nil, done: on || created
        ) {
          if !on && !created {
            StepButton(title: ProductText.create.english, busy: busy) { Task { await createPasskey() } }
              .accessibilityLabel(ProductText.createPasskey.english)
          }
        }
        StepCard(
          number: 2, title: ProductText.tryItOnce.english, status: ProductText.tryItOnceLine.english, done: on
        ) {
          if !on {
            StepButton(title: "Try with Face ID", symbol: "faceid", busy: busy && created) {
              Task { await checkPasskey() }
            }
            .disabled(!created)
          }
        }
      }
      .disabled(busy)
      .padding(.top, 18)
      Note(problem)
        .padding(.top, 8)
      PrimaryButton(ProductText.done.english) { if let close { close() } else { dismiss() } }
        .disabled(busy)
        .padding(.top, 18)
    }
    .animation(reduceMotion ? nil : .smooth(duration: 0.3), value: on)
    .animation(reduceMotion ? nil : .smooth(duration: 0.3), value: created)
  }

  private static func detail(_ error: Error) -> String {
    guard Hub.isDevApp else { return "" }
    let e = error as NSError
    return " (\(e.domain) \(e.code): \(e.localizedDescription))"
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
      if !Passkeys.isCancellation(error) {
        problem = "Couldn’t create passkey. Try again." + Self.detail(error)
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
        problem = "That didn’t work. Try again." + Self.detail(error)
      }
    }
  }
}

/// One step: its number (a green check once done), name and status line; beneath, its buttons side by side.
private struct StepCard<Buttons: View>: View {
  let number: Int
  let title: String
  let status: String?
  let done: Bool
  @ViewBuilder let buttons: Buttons

  @ScaledMetric(relativeTo: .headline) private var circle: CGFloat = 28

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 12) {
        ZStack {
          Circle().fill(done ? Color.green : .white)
          Group {
            if done {
              Image(systemName: "checkmark").fontWeight(.heavy).foregroundStyle(.white)
            } else {
              Text("\(number)").font(.yo(14, relativeTo: .headline)).foregroundStyle(Color.amethyst)
            }
          }
          .transition(.scale.combined(with: .opacity))
        }
        .frame(width: circle, height: circle)
        .accessibilityHidden(true)
        VStack(alignment: .leading, spacing: 1) {
          Text(title).font(.headline)
          if let status { Text(status).font(.footnote.weight(.medium)).opacity(0.85) }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Step \(number), \(title)\(done ? ", done" : "")\(status.map { ", \($0)" } ?? "")")
        .frame(maxWidth: .infinity, alignment: .leading)
      }
      HStack(spacing: 8) { buttons }
    }
    .padding(12)
    .background(.white.opacity(0.14), in: .rect(cornerRadius: 20))
    .sensoryFeedback(.success, trigger: done) { _, now in now }
  }
}

/// A card's button, each as wide as the others: white (Save, Create) or glass on the purple (Show).
private struct StepButton: View {
  let title: String
  var symbol: String? = nil
  var prominent = true
  var busy = false
  let action: () -> Void

  var body: some View {
    let label = Group {
      if busy {
        ProgressView().tint(prominent ? .black : .white)
      } else if let symbol {
        Label(title, systemImage: symbol)
      } else {
        Text(title)
      }
    }
    .font(.subheadline.weight(.bold))
    .lineLimit(1)
    .minimumScaleFactor(0.7)
    .frame(maxWidth: .infinity, minHeight: 30)
    if prominent {
      Button(action: action) { label }
        .buttonStyle(.glassProminent)
        .tint(.white)
        .foregroundStyle(.black)
    } else {
      Button(action: action) { label }
        .buttonStyle(.glass)
        .foregroundStyle(.white)
    }
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
