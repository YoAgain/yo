// "Secure Your Account", from the profile (spec 7): a native setup checklist in a sheet, each row saying what is done.
// 1, Passkey (optional): made, then used once for its PRF output (iCloud Keychain gives it at once; 1Password only
// when used, so the row then asks to confirm). 2, Recovery Key: the 12 words, saved through the system's share sheet
// (Save Image, Notes, Files, a password manager, AirDrop); an eye shows them in place. Done once they are saved.
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
    NavigationStack {
      Form {
        Section {
          SetupRow(
            icon: "key.fill", color: .blue, title: "Passkey",
            status: passkey == .done ? "Set up" : passkey == .confirm ? "Confirm to finish" : "Not set up",
            done: passkey == .done
          ) {
            passkeyControl
          }
          SetupRow(
            icon: "list.number", color: .gray, title: "Recovery Key",
            status: saved ? "Saved" : "Not saved", done: saved
          ) {
            HStack(spacing: 8) {
              Button { revealed.toggle() } label: {
                Image(systemName: revealed ? "eye.slash" : "eye")
                  .frame(minWidth: 30, minHeight: 30)
              }
              .buttonStyle(.borderless)
              .accessibilityLabel(revealed ? "Hide Recovery Key" : "Show Recovery Key")
              if saved {
                Checkmark(label: "Recovery key saved")
              } else {
                Button("Save") { save() }
                  .buttonStyle(.bordered)
                  .disabled(busy)
              }
            }
          }
          if revealed {
            Words(words: words)
          }
        } footer: {
          Text(problem ?? "Keep your name “\(accounts.account?.username ?? "")” forever.")
        }
      }
      .navigationTitle("Secure Your Account")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Close") { dismiss() }
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("Done") { finish() }
            .disabled(!saved || busy)
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
        .accessibilityLabel(passkey == .confirm ? "Confirming passkey" : "Setting up passkey")
    } else {
      switch passkey {
      case .done:
        Checkmark(label: "Passkey set up")
      case .ready, .failed:
        Button(passkey == .failed ? "Try Again" : "Set Up") { Task { await makePasskey() } }
          .buttonStyle(.bordered)
      case .confirm:
        Button("Confirm") { Task { await confirmPasskey() } }
          .buttonStyle(.borderedProminent)
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
    Share.present(items) { completed in if completed { saved = true } }
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

/// One step: a Settings-style icon tile, the title with its state under it, and its control at the end.
private struct SetupRow<Control: View>: View {
  let icon: String
  let color: Color
  let title: String
  let status: String
  let done: Bool
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
        Text(status).font(.footnote).foregroundStyle(done ? .green : .secondary)
      }
      .layoutPriority(1)
      Spacer(minLength: 8)
      control
    }
    .padding(.vertical, 2)
  }
}

/// The 12 words in place, numbered, in two columns.
private struct Words: View {
  let words: [String]

  var body: some View {
    Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 8) {
      ForEach(0..<(words.count + 1) / 2, id: \.self) { row in
        GridRow {
          word(row)
          word(row + (words.count + 1) / 2)
        }
      }
    }
    .padding(.vertical, 6)
    .privacySensitive()
  }

  @ViewBuilder private func word(_ index: Int) -> some View {
    if index < words.count {
      HStack(spacing: 8) {
        Text("\(index + 1)").font(.footnote.monospacedDigit()).foregroundStyle(.secondary).frame(width: 20, alignment: .trailing)
        Text(words[index]).font(.body.monospaced())
      }
    }
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
