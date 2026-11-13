// "Secure Your Name", from the profile (spec 7): a passkey first, made and then used once (its PRF output comes only
// when a passkey is used); if that does not work, the 12 words. One sheet that changes in place, no pages.
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
    /// No passkey: the words.
    case words([String])
  }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch stage {
        case .passkey: passkey
        case .confirm: confirm
        case .words(let words): wordsPage(words)
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

  private func wordsPage(_ words: [String]) -> some View {
    Page(title: "Recovery Key", message: "Keep it somewhere safe. It's how you get your name back on a new iPhone.") {
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
      PrimaryButton("Done") { keepWords() }
      Button("Try Passkey Again") { stage = .passkey; Task { await makePasskey() } }
        .font(.yo(17))
      Note(problem)
    }
  }

  // MARK: Doing

  private func makePasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      if try await accounts.securePasskey() { return done() }
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
      try await accounts.confirmPasskey()
      done()
    } catch let error as AccountError {
      problem = error.errorDescription
    } catch {
      problem = "Passkey not confirmed. Try again."
    }
  }

  private func showWords() {
    problem = nil
    stage = .words(accounts.secureWords())
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
