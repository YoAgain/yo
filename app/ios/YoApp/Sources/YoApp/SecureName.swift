import SwiftUI

// Retained for callers that present setup as a sheet.
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
            ? "Face ID is on. Keep these words too."
            : "Keep these words somewhere safe.",
          requiresAuthentication: false,
          finishTitle: hasPasskey ? "Done" : "Secure Your Account",
          onFinish: { finishRecovery(hasPasskey: hasPasskey) },
          onTryPasskey: hasPasskey ? nil : { tryPasskeyAgain() }
        )
      }
    }
    .navigationBarBackButtonHidden(busy)
    .interactiveDismissDisabled(busy)
    .toolbar(busy ? .hidden : .visible, for: .navigationBar)
    .alert("Unable to Secure Your Account", isPresented: $setupFailed) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(
        "Try again."
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
      } footer: {
        Text("Keep your name forever, on any iPhone.")
      }

      Section {
        Text(SecurityPresentation.passkeyExplanation)
      } header: {
        Text("Face ID")
      }

      Section {
        Text("12 words, for when you can’t use Face ID.")
      } header: {
        Text("Recovery Key")
      }

      Section {
        Button {
          Task { await createPasskey() }
        } label: {
          HStack {
            Text(busy ? "Setting Up…" : "Continue with Face ID")
            Spacer()
            if busy {
              ProgressView()
                .accessibilityLabel("Setting Up Face ID")
            }
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
    .navigationTitle("Secure Your Account")
    .navigationBarTitleDisplayMode(.inline)
  }

  private var confirmation: some View {
    Form {
      Section {
        Text("One more Face ID to finish.")
      }

      Section {
        Button {
          Task { await confirmPasskey() }
        } label: {
          HStack {
            Text(busy ? "Confirming…" : "Continue with Face ID")
            Spacer()
            if busy {
              ProgressView()
                .accessibilityLabel("Confirming Face ID")
            }
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
    .navigationTitle("Continue with Face ID")
    .navigationBarTitleDisplayMode(.inline)
  }

  private func createPasskey() async {
    guard !busy else { return }
    busy = true
    passkeyProblem = nil
    defer { busy = false }

    // A successful call clears the store's temporary setup state.
    let words = accounts.secureWords()

    do {
      let confirmed = try await accounts.securePasskey()
      if confirmed {
        stage = .recovery(words: words, hasPasskey: true)
      } else {
        stage = .confirmation
      }
    } catch {
      recoveryAvailable = true

      // Cancellation offers the fallback without an error message.
      if SecurityPresentation.isCancellation(error) { return }

      passkeyProblem = SecurityPresentation.passkeyMessage(
        error,
        fallback:
          "Face ID isn’t on yet. Try again, or use the words."
      )
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
          "Face ID isn’t on yet. Try again, or use the words."
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
