// "Secure Your Account", from the profile (spec 7), built the way iOS shows a short setup checklist (Settings'
// "Finish Setting Up"): a system sheet with a header, one row per step, a Set Up button or a checkmark at the end.
// 1, Passkey: made, then used once for its PRF output (iCloud Keychain gives it at once; 1Password only when used,
// so the row then asks to confirm). 2, Recovery Key: the 12 words, shown and kept. Done once the words are kept;
// the passkey is optional, the words alone also secure the account.
import SwiftUI

struct SecureNameSheet: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var passkey = PasskeyStep.ready
  @State private var words: [String] = []
  @State private var kept = false
  @State private var showingWords = false
  @State private var busy = false
  @State private var problem: String?

  private enum PasskeyStep {
    case ready, confirm, done, failed
  }

  var body: some View {
    NavigationStack {
      Form {
        Section {
          VStack(spacing: 10) {
            Image(systemName: "lock.shield.fill")
              .font(.system(size: 56))
              .foregroundStyle(Color.amethyst)
            Text("Secure Your Account")
              .font(.title2.bold())
            Text("Keep your name “\(accounts.account?.username ?? "")” forever.")
              .foregroundStyle(.secondary)
          }
          .multilineTextAlignment(.center)
          .frame(maxWidth: .infinity)
          .listRowBackground(Color.clear)
        }

        Section {
          SetupRow(icon: "faceid", color: .green, title: "Passkey", detail: "Sign in with Face ID.") {
            passkeyControl
          }
          SetupRow(icon: "key.fill", color: .gray, title: "Recovery Key", detail: "12 words, for when you can’t use Face ID.") {
            if kept {
              Checkmark(label: "Recovery key saved")
            } else {
              Button("Show") { showingWords = true }
                .buttonStyle(.bordered)
                .buttonBorderShape(.capsule)
                .controlSize(.small)
            }
          }
        } footer: {
          if let problem { Text(problem) }
        }
      }
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Cancel") { dismiss() }
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("Done") { finish() }
            .disabled(!kept || busy)
        }
      }
      .sheet(isPresented: $showingWords) {
        NavigationStack {
          RecoveryKeyView(
            name: accounts.account?.username ?? "",
            words: words,
            explanation: SecurityPresentation.recoveryExplanation,
            requiresAuthentication: false,
            finishTitle: "Done",
            onFinish: {
              kept = true
              showingWords = false
            }
          )
          .toolbar {
            ToolbarItem(placement: .cancellationAction) {
              Button("Cancel") { showingWords = false }
            }
          }
        }
      }
    }
    .tint(Color.amethyst)
    .interactiveDismissDisabled(busy)
    .onAppear { if words.isEmpty { words = accounts.secureWords() } }
  }

  /// The passkey row's end: Set Up (or Try Again), Confirm for 1Password, a checkmark once it is there.
  @ViewBuilder private var passkeyControl: some View {
    if busy {
      ProgressView()
    } else {
      switch passkey {
      case .done:
        Checkmark(label: "Passkey set up")
      case .ready, .failed:
        Button(passkey == .failed ? "Try Again" : "Set Up") { Task { await makePasskey() } }
          .buttonStyle(.bordered)
          .buttonBorderShape(.capsule)
          .controlSize(.small)
      case .confirm:
        Button("Confirm") { Task { await confirmPasskey() } }
          .buttonStyle(.borderedProminent)
          .buttonBorderShape(.capsule)
          .controlSize(.small)
      }
    }
  }

  // MARK: Doing

  private func makePasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      passkey = try await accounts.securePasskey() ? .done : .confirm
      if passkey == .confirm { problem = "Confirm with Face ID once more to finish." }
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
}

/// One step: a Settings-style icon tile, a title with a short line under it, and its control at the end.
private struct SetupRow<Control: View>: View {
  let icon: String
  let color: Color
  let title: String
  let detail: String
  @ViewBuilder let control: Control

  var body: some View {
    HStack(spacing: 14) {
      Image(systemName: icon)
        .font(.system(size: 15, weight: .semibold))
        .foregroundStyle(.white)
        .frame(width: 29, height: 29)
        .background(color.gradient, in: .rect(cornerRadius: 7))
      VStack(alignment: .leading, spacing: 2) {
        Text(title)
        Text(detail).font(.footnote).foregroundStyle(.secondary)
      }
      Spacer(minLength: 8)
      control
    }
    .padding(.vertical, 2)
  }
}

private struct Checkmark: View {
  let label: String

  var body: some View {
    Image(systemName: "checkmark.circle.fill")
      .font(.title3)
      .foregroundStyle(.green)
      .accessibilityLabel(label)
  }
}
