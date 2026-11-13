// "Secure Your Name", from the profile (spec 7): a passkey first, made and then used once (its PRF output comes only
// when a passkey is used), then the 12 words to keep as well; if the passkey does not work, the words alone. One
// sheet that changes in place.
import SwiftUI

struct SecureNameSheet: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var stage = Stage.passkey
  @State private var busy = false
  @State private var problem: String?
  @State private var kept: Set<String> = []
  @State private var started = false

  private enum Stage: Equatable {
    /// Making the passkey (the system's sheet is up).
    case passkey
    /// Made, but its PRF output not given yet (1Password): used once more, said why.
    case confirm
    /// The words to keep: with the passkey already saved (`saved`), or alone.
    case words([String], saved: Bool)
  }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch stage {
        case .passkey: passkey
        case .confirm: confirm
        case .words(let words, let saved): wordsPage(words, saved: saved)
        }
      }
      .transition(.opacity)
    }
    .animation(.smooth(duration: 0.3), value: stage)
    .foregroundStyle(.white)
    .preferredColorScheme(.dark)
    .task {
      guard !started else { return }
      started = true
      await makePasskey()
    }
  }

  // MARK: Stages

  private var passkey: some View {
    Page(title: "Secure Your Name", message: "So you can get it back on a new iPhone.") {
      KeyIcon()
    } bottom: {
      PrimaryButton("Use Passkey", busy: busy) { Task { await makePasskey() } }
        .disabled(busy)
      Button("Use Recovery Key Instead") { showWords() }
        .font(.yo(17))
      Note(problem)
    }
  }

  private var confirm: some View {
    Page(title: "Secure Your Name", message: "Use your new passkey once to finish.") {
      KeyIcon()
    } bottom: {
      PrimaryButton("Confirm Passkey", busy: busy) { Task { await confirmPasskey() } }
        .disabled(busy)
      Button("Use Recovery Key Instead") { showWords() }
        .font(.yo(17))
      Note(problem)
    }
  }

  private func wordsPage(_ words: [String], saved: Bool) -> some View {
    Page(title: "Recovery Key",
      message: saved
        ? "Your passkey is set up. Keep these words too: they get your name back without it."
        : "Keep it somewhere safe. It's how you get your name back on a new iPhone.") {
      FrostedKey(words: words, qrSize: 96)
      HStack(spacing: 12) {
        KeepButton(title: "Save to Photos", icon: "photo.badge.arrow.down", done: kept.contains("photos")) {
          Task { await saveToPhotos(words) }
        }
        KeepButton(title: "Copy", icon: "doc.on.doc", done: kept.contains("copied")) {
          RecoveryWords.copy(words, name: accounts.account?.username ?? "")
          kept.insert("copied")
        }
      }
      .padding(.top, 6)
      .disabled(busy)
    } bottom: {
      PrimaryButton("Done") { saved ? done() : keepWords() }
      if !saved {
        Button("Try Passkey Again") { stage = .passkey; Task { await makePasskey() } }
          .font(.yo(17))
      }
      Note(problem)
    }
  }

  // MARK: Doing

  private func makePasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    let words = accounts.secureWords()  // made now: shown after the passkey, kept with it
    do {
      if try await accounts.securePasskey() { return stage = .words(words, saved: true) }
      stage = .confirm
    } catch {
      showWords()  // cancelled, or the password manager could not make it
    }
  }

  private func confirmPasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      let words = accounts.secureWords()
      try await accounts.confirmPasskey()
      stage = .words(words, saved: true)
    } catch let error as AccountError {
      problem = error.errorDescription
    } catch {
      problem = "Passkey not confirmed. Try again."
    }
  }

  private func showWords() {
    problem = nil
    stage = .words(accounts.secureWords(), saved: false)
  }

  private func keepWords() {
    do {
      try accounts.keepWords()
      done()
    } catch {
      problem = "Couldn’t save your recovery key. Try again."
    }
  }

  private func done() {
    UINotificationFeedbackGenerator().notificationOccurred(.success)
    dismiss()
  }

  private func saveToPhotos(_ words: [String]) async {
    busy = true
    defer { busy = false }
    let renderer = ImageRenderer(content: RecoveryCard(name: accounts.account?.username ?? "", words: words))
    renderer.scale = 3
    guard let image = renderer.uiImage else { return problem = "Couldn’t create your recovery key image. Try again." }
    do {
      try await RecoveryWords.saveToPhotos(image)
      kept.insert("photos")
    } catch {
      problem = error.localizedDescription
    }
  }
}

private struct KeyIcon: View {
  var body: some View {
    Image(systemName: "person.badge.key.fill")
      .font(.system(size: 64))
      .padding(.top, 20)
  }
}
