// "Secure Your Account", from the profile (spec 7): one screen, two steps. 1, a passkey: made, then used once for its
// PRF output (iCloud Keychain gives it at once; 1Password only when used, so it asks once more). 2, the 12 words, to
// keep. Done once the words are kept; the passkey is optional, the words alone also secure the account.
import SwiftUI

struct SecureNameSheet: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var passkey = PasskeyStep.ready
  @State private var words: [String] = []
  @State private var kept: Set<String> = []
  @State private var busy = false
  @State private var problem: String?

  private enum PasskeyStep {
    case ready, confirm, done, failed
  }

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Page(title: "Secure Your Account",
        message: "Keep your name “\(accounts.account?.username ?? "")” forever.") {
        VStack(alignment: .leading, spacing: 24) {
          StepRow(number: 1, title: "Passkey") {
            passkeyControl
          }
          StepRow(number: 2, title: "12 Words") {
            EmptyView()
          }
          FrostedKey(words: words, qrSize: 80)
          HStack(spacing: 12) {
            KeepButton(title: "Save to Photos", icon: "photo.badge.arrow.down", done: kept.contains("photos")) {
              Task { await saveToPhotos() }
            }
            KeepButton(title: "Copy", icon: "doc.on.doc", done: kept.contains("copied")) {
              RecoveryWords.copy(words, name: accounts.account?.username ?? "")
              kept.insert("copied")
            }
          }
        }
      } bottom: {
        PrimaryButton("Done") { finish() }
          .disabled(kept.isEmpty || busy)
        Note(problem)
      }
    }
    .foregroundStyle(.white)
    .preferredColorScheme(.dark)
    .onAppear { if words.isEmpty { words = accounts.secureWords() } }
  }

  /// Step 1's control: a button until the passkey is there, then a checkmark.
  @ViewBuilder private var passkeyControl: some View {
    switch passkey {
    case .done:
      Image(systemName: "checkmark.circle.fill")
        .font(.system(size: 26))
        .foregroundStyle(.white, .green)
        .accessibilityLabel("Passkey set up")
    case .ready, .failed:
      StepButton(title: passkey == .failed ? "Try Again" : "Set Up", busy: busy) { Task { await makePasskey() } }
    case .confirm:
      StepButton(title: "Confirm", busy: busy) { Task { await confirmPasskey() } }
    }
  }

  // MARK: Doing

  private func makePasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      passkey = try await accounts.securePasskey() ? .done : .confirm
    } catch {
      passkey = .failed  // cancelled, or the password manager could not make it: the words still secure it
    }
  }

  /// 1Password: the passkey used once more, for its PRF output.
  private func confirmPasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      try await accounts.confirmPasskey()
      passkey = .done
    } catch let error as AccountError {
      problem = error.errorDescription
    } catch {
      problem = "Passkey not confirmed. Try again."
    }
  }

  /// With the passkey the backup is saved already; without, the words save it now.
  private func finish() {
    if passkey != .done {
      do { try accounts.keepWords() } catch { return problem = "Couldn’t save. Try again." }
    }
    UINotificationFeedbackGenerator().notificationOccurred(.success)
    dismiss()
  }

  private func saveToPhotos() async {
    busy = true
    defer { busy = false }
    let renderer = ImageRenderer(content: RecoveryCard(name: accounts.account?.username ?? "", words: words))
    renderer.scale = 3
    guard let image = renderer.uiImage else { return problem = "Couldn’t create the image. Try again." }
    do {
      try await RecoveryWords.saveToPhotos(image)
      kept.insert("photos")
    } catch {
      problem = error.localizedDescription
    }
  }
}

/// A numbered step: the number in a circle, the title, its control at the end.
private struct StepRow<Control: View>: View {
  let number: Int
  let title: String
  @ViewBuilder let control: Control

  var body: some View {
    HStack(spacing: 14) {
      Text("\(number)")
        .font(.system(size: 17, weight: .bold))
        .frame(width: 30, height: 30)
        .background(.white.opacity(0.2), in: .circle)
      Text(title).font(.system(size: 20, weight: .semibold))
      Spacer()
      control
    }
  }
}

/// A small glass button at the end of a step.
private struct StepButton: View {
  let title: String
  let busy: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Group {
        if busy { ProgressView().tint(Color.amethyst) } else { Text(title).font(.system(size: 15, weight: .semibold)) }
      }
      .frame(minWidth: 70, minHeight: 28)
    }
    .buttonStyle(.glassProminent)
    .tint(.white)
    .foregroundStyle(Color.amethyst)
    .disabled(busy)
  }
}
