// The way in, as the original's: a name, and you are in. Start → your name → a passkey saves it (one tap; if it
// cannot, 12 words to write down) → the list. "I have Yo" brings an account back on a new phone: the passkey, or the
// name and its 12 words.
import Photos
import SwiftUI

struct Onboarding: View {
  @Environment(AccountStore.self) private var accounts

  private enum Step: Equatable {
    case welcome, name, code([String]), confirm([String]), restore
  }

  @State private var step = Step.welcome
  @State private var name = ""
  @State private var busy = false
  @State private var problem: String?
  @FocusState private var typing: Bool

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch step {
        case .welcome: welcome
        case .name: naming
        case .code(let words): code(words)
        case .confirm(let words): ConfirmCode(words: words) { accounts.confirm() }
        case .restore: RestoreWithCode { go(.welcome) }
        }
      }
      .transition(.asymmetric(insertion: .move(edge: .trailing).combined(with: .opacity),
        removal: .move(edge: .leading).combined(with: .opacity)))
    }
    .foregroundStyle(.white)
    .animation(.smooth(duration: 0.35), value: step)
    .preferredColorScheme(.dark)
  }

  // MARK: Welcome

  /// The logo and the line, then the two ways in, one under the other: a new account (the button), or bringing an
  /// existing one to this phone (the line under it), as Apple's own apps put "Sign In" under "Create".
  private var welcome: some View {
    VStack(spacing: 0) {
      Spacer()
      Text("Yo")
        .font(.yo(150))
        .padding(.bottom, 4)
      Text("One tap, one Yo.")
        .font(.yo(19))
        .opacity(0.8)
      Button { go(.name) } label: {
        Text("Make my Yo").font(.yo(22)).frame(width: 230, height: 34)
      }
      .buttonStyle(.glassProminent)
      .tint(.white)
      .foregroundStyle(Color.amethyst)
      .padding(.top, 44)
      Button("I already have Yo") { Task { await restoreWithPasskey() } }
        .font(.yo(17))
        .opacity(0.9)
        .padding(.top, 20)
      note
      Spacer()
      Spacer()
    }
    .padding(.horizontal, 28)
    .padding(.bottom, 12)
    .disabled(busy)
  }

  // MARK: Name

  private var naming: some View {
    let trouble = Username.problem(name) ?? (MockHub.isTaken(name) ? "Taken" : nil)
    return VStack(spacing: 14) {
      HStack {
        Button("Back", systemImage: "chevron.backward") { go(.welcome) }
          .labelStyle(.iconOnly)
          .font(.title3.weight(.semibold))
          .disabled(busy)
        Spacer()
      }
      Spacer()
      Text("YOUR NAME")
        .font(.yo(14))
        .tracking(3)
        .opacity(0.7)
      TextField("", text: $name, prompt: Text("LEO").foregroundStyle(.white.opacity(0.3)))
        .font(.yo(52))
        .multilineTextAlignment(.center)
        .textInputAutocapitalization(.characters)
        .autocorrectionDisabled()
        .submitLabel(.next)
        .focused($typing)
        .onChange(of: name) { _, typed in
          let clean = Username.clean(typed)
          if clean != typed { name = clean }
          problem = nil
        }
        .onSubmit { if trouble == nil { Task { await register() } } }
      Text(name.isEmpty ? "How your friends find you" : (trouble ?? "✓ Yours"))
        .font(.system(size: 15, weight: .semibold))
        .opacity(trouble == nil && !name.isEmpty ? 1 : 0.7)
        .contentTransition(.opacity)
      Spacer()
      Button { Task { await register() } } label: {
        Group {
          if busy { ProgressView().tint(Color.amethyst) } else { Text("Next").font(.yo(22)) }
        }
        .frame(maxWidth: .infinity, minHeight: 34)
      }
      .buttonStyle(.glassProminent)
      .tint(.white)
      .foregroundStyle(Color.amethyst)
      .disabled(trouble != nil || busy)
      note
    }
    .padding(.horizontal, 28)
    .padding(.bottom, 12)
    .onAppear { typing = true }
  }

  // MARK: Recovery code

  private func code(_ words: [String]) -> some View {
    VStack(spacing: 22) {
      Spacer(minLength: 12)
      Text("Write these down")
        .font(.yo(28))
      Text("No passkey could keep your Yo. These 12 words bring it back on a new phone, and nothing else can.")
        .font(.system(size: 16, weight: .medium))
        .multilineTextAlignment(.center)
        .opacity(0.85)
      LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
        ForEach(Array(words.enumerated()), id: \.offset) { index, word in
          HStack(spacing: 8) {
            Text("\(index + 1)").font(.system(size: 13, weight: .semibold).monospacedDigit()).opacity(0.6)
              .frame(width: 20, alignment: .trailing)
            Text(word).font(.yo(19))
            Spacer(minLength: 0)
          }
          .padding(.vertical, 10)
          .padding(.horizontal, 12)
          .background(.white.opacity(0.14), in: .rect(cornerRadius: 12))
        }
      }
      Spacer()
      // The easy way: the words as a picture in Photos, and in. Or written down, and two of them typed again.
      Button { Task { await saveToPhotos(words) } } label: {
        Label("Save to Photos", systemImage: "photo.badge.arrow.down").font(.yo(20))
          .frame(maxWidth: .infinity, minHeight: 34)
      }
      .buttonStyle(.glassProminent)
      .tint(.white)
      .foregroundStyle(Color.amethyst)
      .disabled(busy)
      Button("I wrote them down") { go(.confirm(words)) }
        .font(.yo(17))
        .opacity(0.9)
      note
    }
    .padding(.horizontal, 24)
    .padding(.bottom, 12)
  }

  // MARK: Shared

  @ViewBuilder private var note: some View {
    if let problem {
      Text(problem).font(.system(size: 14, weight: .semibold)).padding(.top, 10).transition(.opacity)
    }
  }

  private func go(_ next: Step) {
    problem = nil
    step = next
  }

  /// The 12 words as a picture in the photo library (asked once, add only); saved, the account is in use.
  private func saveToPhotos(_ words: [String]) async {
    busy = true
    defer { busy = false }
    let card = RecoveryCard(name: name, words: words)
    let renderer = ImageRenderer(content: card)
    renderer.scale = 3
    guard let image = renderer.uiImage else { return problem = "Could not make the picture" }
    let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
    guard status == .authorized || status == .limited else {
      return problem = "Photos is not allowed: write them down instead"
    }
    do {
      try await PHPhotoLibrary.shared().performChanges { PHAssetChangeRequest.creationRequestForAsset(from: image) }
      accounts.confirm()
    } catch {
      problem = "Not saved: \(error.localizedDescription)"
    }
  }

  private func register() async {
    guard Username.problem(name) == nil, !busy else { return }
    busy = true
    defer { busy = false }
    do {
      switch try await accounts.register(name) {
      case .done: break  // the account is there: the list takes over
      case .code(let words): go(.code(words))
      }
    } catch {
      problem = error.localizedDescription
    }
  }

  /// "I have Yo": a passkey first; without one (or cancelled), the name and the 12 words.
  private func restoreWithPasskey() async {
    busy = true
    defer { busy = false }
    do {
      try await accounts.restoreWithPasskey()
    } catch {
      go(.restore)
    }
  }
}

/// Two of the words typed again, so they were really written down.
private struct ConfirmCode: View {
  let words: [String]
  let done: () -> Void
  @State private var asked = Array((0..<12).shuffled().prefix(2)).sorted()
  @State private var typed = ["", ""]
  @State private var wrong = false
  @FocusState private var field: Int?

  var body: some View {
    VStack(spacing: 18) {
      Spacer()
      Text("Check").font(.yo(28))
      Text("Type two of your words.").font(.system(size: 16, weight: .medium)).opacity(0.85)
      ForEach(0..<2, id: \.self) { i in
        HStack {
          Text("Word \(asked[i] + 1)").font(.system(size: 15, weight: .semibold)).opacity(0.7).frame(width: 74, alignment: .leading)
          TextField("", text: $typed[i])
            .font(.yo(24))
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .focused($field, equals: i)
            .submitLabel(i == 0 ? .next : .done)
            .onSubmit { if i == 0 { field = 1 } else { check() } }
        }
        .padding(14)
        .background(.white.opacity(0.14), in: .rect(cornerRadius: 14))
      }
      Text(wrong ? "Not quite: look at your words again" : " ").font(.system(size: 14, weight: .semibold))
      Spacer()
      Button(action: check) {
        Text("Done").font(.yo(22)).frame(maxWidth: .infinity, minHeight: 34)
      }
      .buttonStyle(.glassProminent)
      .tint(.white)
      .foregroundStyle(Color.amethyst)
    }
    .padding(.horizontal, 28)
    .padding(.bottom, 12)
    .onAppear { field = 0 }
    .sensoryFeedback(.error, trigger: wrong) { _, now in now }
  }

  private func check() {
    let right = (0..<2).allSatisfy { typed[$0].trimmingCharacters(in: .whitespaces).lowercased() == words[asked[$0]] }
    if right { done() } else { wrong = true }
  }
}

/// A new phone without a passkey: the name and the 12 words.
private struct RestoreWithCode: View {
  let back: () -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var name = ""
  @State private var words = ""
  @State private var problem: String?

  var body: some View {
    VStack(spacing: 16) {
      HStack {
        Button("Back", systemImage: "chevron.backward", action: back).labelStyle(.iconOnly).font(.title3.weight(.semibold))
        Spacer()
      }
      Spacer()
      Text("Welcome back").font(.yo(30))
      TextField("", text: $name, prompt: Text("YOUR NAME").foregroundStyle(.white.opacity(0.35)))
        .font(.yo(30))
        .multilineTextAlignment(.center)
        .textInputAutocapitalization(.characters)
        .autocorrectionDisabled()
        .onChange(of: name) { _, typed in
          let clean = Username.clean(typed)
          if clean != typed { name = clean }
        }
      TextField("", text: $words, prompt: Text("your 12 words").foregroundStyle(.white.opacity(0.35)), axis: .vertical)
        .font(.yo(19))
        .lineLimit(3...5)
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .padding(14)
        .background(.white.opacity(0.14), in: .rect(cornerRadius: 14))
      Text(problem ?? " ").font(.system(size: 14, weight: .semibold))
      Spacer()
      Button(action: restore) {
        Text("Bring my Yo back").font(.yo(20)).frame(maxWidth: .infinity, minHeight: 34)
      }
      .buttonStyle(.glassProminent)
      .tint(.white)
      .foregroundStyle(Color.amethyst)
      .disabled(name.isEmpty || words.isEmpty)
    }
    .padding(.horizontal, 28)
    .padding(.bottom, 12)
  }

  private func restore() {
    guard let code = RecoveryCode.parse(words) else { return problem = "12 words from your list, please" }
    do {
      try accounts.restore(username: name, code: code)
    } catch {
      problem = error.localizedDescription
    }
  }
}

/// The recovery code as a picture for Photos: the brand's purple, the name, the 12 words, what they are for.
private struct RecoveryCard: View {
  let name: String
  let words: [String]

  var body: some View {
    VStack(alignment: .leading, spacing: 18) {
      HStack(alignment: .firstTextBaseline) {
        Text("Yo").font(.yo(44))
        Spacer()
        Text(name).font(.yo(24))
      }
      Text("Your 12 words").font(.yo(22))
      Grid(horizontalSpacing: 26, verticalSpacing: 10) {
        ForEach(0..<6, id: \.self) { row in
          GridRow {
            ForEach([row, row + 6], id: \.self) { i in
              HStack(spacing: 8) {
                Text("\(i + 1)").font(.system(size: 13, weight: .semibold).monospacedDigit()).opacity(0.6)
                  .frame(width: 20, alignment: .trailing)
                Text(words[i]).font(.yo(20))
              }
              .frame(maxWidth: .infinity, alignment: .leading)
            }
          }
        }
      }
      Text("They bring your Yo back on a new phone, and nothing else can. Keep them to yourself.")
        .font(.system(size: 13, weight: .medium))
        .opacity(0.85)
      Text(Date.now.formatted(date: .long, time: .omitted)).font(.system(size: 12, weight: .medium)).opacity(0.6)
    }
    .foregroundStyle(.white)
    .padding(30)
    .frame(width: 390)
    .background(Color.amethyst)
  }
}
