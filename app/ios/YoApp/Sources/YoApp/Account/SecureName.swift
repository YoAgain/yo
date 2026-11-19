// "Account Recovery" (spec 7): save the recovery key through the share sheet (Copy on its card counts too), or set up
// a passkey and confirm it once for its PRF output. Done once either one is done.
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
    NavigationStack {
      Form {
        Section {
          Text("Save a recovery key or set up a passkey. Either one brings your account back on a new iPhone.")
            .foregroundStyle(.secondary)
        }
        Section {
          SecureRow(
            "Save Recovery Key", systemImage: "square.and.arrow.up", doneTitle: "Recovery Key Saved",
            done: wordsDone, busy: busy == .recoveryKey
          ) { Task { await share() } }
          Button {
            if showingKey { showingKey = false } else { Task { await showKey() } }
          } label: {
            Label(showingKey ? "Hide Recovery Key" : "Show Recovery Key", systemImage: showingKey ? "eye.slash" : "eye")
              .labelStyle(Tile(.gray))
          }
          .tint(Color(.label))
          if showingKey {
            KeyCard(key: words, copied: copiedKey) {
              UIPasteboard.general.string = RecoveryCode.shown(words)
              copiedKey = true
              Task { await markSaved() }
            }
            .listRowBackground(Color.amethyst)
          }
        } footer: {
          Text("Keep it somewhere safe, like your password manager.")
        }
        Section {
          SecureRow(
            created ? "Confirm Passkey" : "Set Up Passkey", systemImage: "faceid", doneTitle: "Passkey Set Up",
            done: passkeyDone, busy: busy == .passkey
          ) { Task { created ? await checkPasskey() : await createPasskey() } }
        } footer: {
          if let problem { Text(problem).foregroundStyle(.red) }
        }
      }
      .disabled(busy != nil)
      .navigationTitle("Account Recovery")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Close", systemImage: "xmark") { if let close { close() } else { dismiss() } }
            .disabled(busy != nil)
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("Done", systemImage: "checkmark") { finish() }
            .disabled(!canFinish || busy != nil)
        }
      }
    }
    .sensoryFeedback(.success, trigger: wordsDone) { _, now in now }
    .sensoryFeedback(.success, trigger: passkeyDone) { _, now in now }
    .presentationDragIndicator(.hidden)
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

/// A step as a Form row: its button until done, then what was done with a green check.
private struct SecureRow: View {
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
    if done {
      LabeledContent {
        Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
      } label: {
        Label(doneTitle, systemImage: systemImage).labelStyle(Tile(.amethyst))
      }
      .accessibilityElement(children: .combine)
    } else {
      Button(action: action) {
        LabeledContent {
          if busy { ProgressView() }
        } label: {
          Label(title, systemImage: systemImage).labelStyle(Tile(.amethyst))
        }
      }
      .tint(Color(.label))
    }
  }
}

/// The key itself, as the recovery card shows it: QR code and blocks, white on amethyst.
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
    .foregroundStyle(.white)
    .frame(maxWidth: .infinity)
    .padding(.vertical, 8)
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
    .presentationDragIndicator(.hidden)
  }
}
