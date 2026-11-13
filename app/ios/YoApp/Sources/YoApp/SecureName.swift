import SwiftUI

// Preserves the existing standalone API.
// Settings pushes SecureNamePage within its existing NavigationStack.
@MainActor
struct SecureNameSheet: View {
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    NavigationStack {
      SecureNamePage(onFinish: { dismiss() })
        .toolbar {
          ToolbarItem(placement: .cancellationAction) {
            Button("Close") { dismiss() }
          }
        }
    }
    .tint(Color.amethyst)
  }
}

@MainActor
struct SecureNamePage: View {
  var onFinish: (() -> Void)? = nil

  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss

  @State private var stage = Stage.introduction
  @State private var busy = false
  @State private var passkeyProblem: String?
  @State private var recoveryAvailable = false
  @State private var setupFailed = false

  private enum Stage {
    case introduction
    case confirmation
    case recovery(words: [String], hasPasskey: Bool)
  }

  var body: some View {
    Group {
      switch stage {
      case .introduction:
        introduction

      case .confirmation:
        confirmation

      case .recovery(let words, let hasPasskey):
        RecoveryKeyView(
          name: accounts.account?.username ?? "",
          words: words,
          explanation: hasPasskey
            ? "Your passkey is set up and your name is secured. "
              + "Save these 12 words so you can also sign in without your passkey."
            : "Save these 12 words outside Yo. They’ll be your only way "
              + "to sign in on a new iPhone.",
          requiresAuthentication: false,
          finishTitle: hasPasskey ? "Done" : "Secure My Name",
          onFinish: { finishRecovery(hasPasskey: hasPasskey) },
          onTryPasskey: hasPasskey ? nil : { tryPasskeyAgain() }
        )
      }
    }
    .navigationBarBackButtonHidden(busy)
    .interactiveDismissDisabled(busy)
    .alert("Couldn’t Secure Your Name", isPresented: $setupFailed) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(
        "Try again. Keep this recovery key until setup is complete."
      )
    }
  }

  private var introduction: some View {
    Form {
      Section {
        LabeledContent {
          Text(accounts.account?.username ?? "")
            .font(.yo(17, relativeTo: .body))
            .fixedSize(horizontal: false, vertical: true)
        } label: {
          Text("Username")
        }

        Text(
          "Keep your name, even if you stop opening Yo, "
            + "and sign in on a new iPhone."
        )
      } header: {
        Text("Your Name")
      }

      Section {
        Text(
          "Sign in with Face ID or your device passcode. "
            + "Your passkey is stored in iCloud Keychain "
            + "or your chosen password manager."
        )
      } header: {
        Text("Passkey")
      }

      Section {
        Text(
          "You’ll also get 12 recovery words. "
            + "They let you sign in if you can’t use your passkey."
        )
      } header: {
        Text("Recovery Key")
      }

      Section {
        Button {
          Task { await createPasskey() }
        } label: {
          HStack {
            Text(busy ? "Creating Passkey…" : "Create Passkey")
            Spacer()
            if busy { ProgressView() }
          }
        }
        .disabled(busy)

        if recoveryAvailable {
          Button("Use Recovery Key Only") { showRecoveryOnly() }
            .disabled(busy)
        }
      } footer: {
        if let passkeyProblem {
          Text(passkeyProblem)
        }
      }
    }
    .navigationTitle("Secure Your Name")
    .navigationBarTitleDisplayMode(.inline)
  }

  private var confirmation: some View {
    Form {
      Section {
        Text(
          "Your passkey was created. "
            + "Use it once to finish setting it up for Yo."
        )
      }

      Section {
        Button {
          Task { await confirmPasskey() }
        } label: {
          HStack {
            Text(busy ? "Confirming Passkey…" : "Confirm Passkey")
            Spacer()
            if busy { ProgressView() }
          }
        }
        .disabled(busy)

        Button("Use Recovery Key Only") { showRecoveryOnly() }
          .disabled(busy)
      } footer: {
        if let passkeyProblem {
          Text(passkeyProblem)
        }
      }
    }
    .navigationTitle("Confirm Passkey")
    .navigationBarTitleDisplayMode(.inline)
  }

  private func createPasskey() async {
    guard !busy else { return }
    busy = true
    passkeyProblem = nil
    defer { busy = false }

    // Keep these words: a successful call clears AccountStore's
    // temporary setup state after saving the backup.
    let words = accounts.secureWords()

    do {
      let confirmed = try await accounts.securePasskey()

      if confirmed {
        stage = .recovery(words: words, hasPasskey: true)
      } else {
        stage = .confirmation
      }
    } catch {
      // Cancellation is also a valid reason to offer the fallback.
      // Keep it inline rather than presenting an error alert.
      recoveryAvailable = true
      passkeyProblem =
        "A passkey wasn’t set up. Try again or use a recovery key on its own."
    }
  }

  private func confirmPasskey() async {
    guard !busy else { return }
    busy = true
    passkeyProblem = nil
    defer { busy = false }

    let words = accounts.secureWords()

    do {
      try await accounts.confirmPasskey()
      stage = .recovery(words: words, hasPasskey: true)
    } catch {
      if SecurityPresentation.isCancellation(error) { return }

      passkeyProblem = SecurityPresentation.passkeyMessage(
        error,
        fallback:
          "A passkey wasn’t set up. Try again or use a recovery key on its own."
      )
    }
  }

  private func showRecoveryOnly() {
    guard !busy else { return }
    passkeyProblem = nil
    stage = .recovery(
      words: accounts.secureWords(),
      hasPasskey: false
    )
  }

  private func tryPasskeyAgain() {
    guard !busy else { return }
    passkeyProblem = nil
    stage = .introduction
  }

  private func finishRecovery(hasPasskey: Bool) {
    if !hasPasskey {
      do {
        try accounts.keepWords()
      } catch {
        setupFailed = true
        return
      }
    }

    if let onFinish {
      onFinish()
    } else {
      dismiss()
    }
  }
}
