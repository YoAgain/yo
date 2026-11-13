// "Secure Your Account", from the profile (spec 7): one full-colour screen like the onboarding, two steps, each with a
// circle that turns into a green checkmark once done. 1, Passkey (optional): made, then used once for its PRF output
// (iCloud Keychain gives it at once; 1Password only when used, so the step then asks to confirm). 2, Recovery Key: the
// 12 words, saved through the system's share sheet (Save Image, Notes, Files, a password manager, AirDrop); the eye
// shows them in place. Done once they are saved.
import SwiftUI

struct SecureNameSheet: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var passkey = PasskeyStep.ready
  @State private var words: [String] = []
  @State private var saved = false
  @State private var revealed = false
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
        VStack(spacing: 12) {
          StepCard(
            number: 1, title: "Passkey",
            detail: passkey == .confirm ? "Confirm once more to finish." : "Sign in with Face ID.",
            done: passkey == .done
          ) {
            passkeyControl
          }
          StepCard(number: 2, title: "Recovery Key", detail: "12 words to sign back in.", done: saved) {
            HStack(spacing: 6) {
              Button { withAnimation(.snappy) { revealed.toggle() } } label: {
                Image(systemName: revealed ? "eye.slash" : "eye")
                  .font(.system(size: 17, weight: .semibold))
                  .frame(width: 36, height: 36)
              }
              .accessibilityLabel(revealed ? "Hide Recovery Key" : "Show Recovery Key")
              if !saved {
                StepButton(title: "Save", busy: false) { save() }
              }
            }
          }
          if revealed {
            WordGrid(words: words)
              .privacySensitive()
              .transition(.opacity.combined(with: .move(edge: .top)))
          }
        }
        .padding(.top, 8)
      } bottom: {
        PrimaryButton("Done") { finish() }
          .disabled(!saved || busy)
        Note(problem)
      }
    }
    .foregroundStyle(.white)
    .preferredColorScheme(.dark)
    .interactiveDismissDisabled(busy)
    .onAppear { if words.isEmpty { words = accounts.secureWords() } }
  }

  /// Step 1's control: Set Up (or Try Again), Confirm for 1Password; nothing once done (the checkmark says it).
  @ViewBuilder private var passkeyControl: some View {
    switch passkey {
    case .done:
      EmptyView()
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
      passkey = .failed
      if !SecurityPresentation.isCancellation(error) { problem = "Couldn’t create passkey. Try again." }
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
    } catch {
      if !SecurityPresentation.isCancellation(error) { problem = "Couldn’t confirm passkey. Try again." }
    }
  }

  /// The words as the card (an image) and as text, to the share sheet; saved once something was done with them.
  private func save() {
    let name = accounts.account?.username ?? ""
    let renderer = ImageRenderer(content: RecoveryCard(name: name, words: words))
    renderer.scale = 3
    var items: [Any] = ["Yo recovery key for \(name): " + words.joined(separator: " ")]
    if let image = renderer.uiImage { items.insert(image, at: 0) }
    Share.present(items) { completed in
      if completed { withAnimation(.snappy) { saved = true } }
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

/// One step on glass: a circle with its number that becomes a green checkmark when done, the title and one line,
/// its control at the end.
private struct StepCard<Control: View>: View {
  let number: Int
  let title: String
  let detail: String
  let done: Bool
  @ViewBuilder let control: Control

  var body: some View {
    HStack(spacing: 14) {
      ZStack {
        if done {
          Image(systemName: "checkmark.circle.fill")
            .font(.system(size: 30))
            .foregroundStyle(.white, .green)
            .transition(.scale.combined(with: .opacity))
        } else {
          Text("\(number)")
            .font(.system(size: 16, weight: .bold))
            .frame(width: 30, height: 30)
            .overlay(Circle().stroke(.white.opacity(0.6), lineWidth: 2))
        }
      }
      .frame(width: 32)
      .animation(.snappy, value: done)
      VStack(alignment: .leading, spacing: 2) {
        Text(title).font(.system(size: 18, weight: .semibold))
        Text(done ? "Done" : detail).font(.system(size: 14)).opacity(0.75)
      }
      .layoutPriority(1)
      Spacer(minLength: 8)
      control
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 14)
    .background(.white.opacity(0.14), in: .rect(cornerRadius: 18))
    .sensoryFeedback(.success, trigger: done) { _, now in now }
    .accessibilityElement(children: .contain)
  }
}

/// The small white glass button at the end of a step.
private struct StepButton: View {
  let title: String
  let busy: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Group {
        if busy { ProgressView().tint(Color.amethyst) } else { Text(title).font(.system(size: 15, weight: .semibold)) }
      }
      .frame(minWidth: 64, minHeight: 30)
    }
    .buttonStyle(.glassProminent)
    .tint(.white)
    .foregroundStyle(Color.amethyst)
    .disabled(busy)
  }
}
