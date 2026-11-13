// The way in. Start → a name → a passkey saves it (one tap; if it cannot, 12 words, best saved to Photos) → the
// list. "Log In" brings an account from before: a passkey, or the name and its 12 words; a passkey on this phone is
// offered by the system at once.
//
// Every step after the welcome is the same page, as Apple's setup screens are: a title and one line at the top, the
// content in the middle, the button at the bottom over the full width, a text button under it if there is a second
// way, back at the top left.
import Photos
import SwiftUI

struct Onboarding: View {
  @Environment(AccountStore.self) private var accounts

  private enum Step: Equatable {
    case welcome, name, code([String]), confirm([String]), logIn
  }

  @State private var step = Step.welcome
  @State private var name = ""
  @State private var busy = false
  @State private var problem: String?
  /// The quiet look for a passkey on this phone happens once, not each time the welcome comes back.
  @State private var looked = false

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch step {
        case .welcome: welcome
        case .name: naming
        case .code(let words): code(words)
        case .confirm(let words): ConfirmCode(words: words, back: { go(.code(words)) }) { accounts.confirm() }
        case .logIn: LogIn(name: name) { go(.welcome) }
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

  /// The logo with the 2014 tagline, then the usual pair at the bottom: "Start" for a new name, "Log In" under it.
  private var welcome: some View {
    VStack(spacing: 0) {
      Spacer()
      Text("Yo")
        .font(.yo(150))
      Text("It's that simple.")
        .font(.yo(20))
        .opacity(0.85)
      Spacer()
      PrimaryButton("Start") { go(.name) }
      Button("Log In") { Task { await logIn() } }
        .font(.yo(18))
        .padding(.top, 18)
    }
    .padding(.horizontal, 28)
    .padding(.bottom, 16)
    .disabled(busy)
    .task {
      guard !looked else { return }
      looked = true
      try? await accounts.restoreWithPasskey(quietly: true)  // a Yo passkey here: the system offers it
    }
  }

  // MARK: Name

  private var naming: some View {
    let trouble = Username.problem(name) ?? (MockHub.isTaken(name) ? "Taken" : nil)
    let mine = MockHub.isTaken(name) && !Username.reserved.contains(name)
    return Page(title: "Choose a Name", back: { go(.welcome) }) {
      NameField(name: $name, onSubmit: { if trouble == nil { Task { await register() } } })
      Text(name.isEmpty ? " " : (trouble.map { $0 } ?? "Available"))
        .font(.system(size: 15, weight: .semibold))
        .opacity(trouble == nil ? 1 : 0.75)
      if mine {
        Button("This is my name") { Task { await logIn() } }.font(.yo(17))
      }
    } bottom: {
      PrimaryButton("Continue", busy: busy) { Task { await register() } }
        .disabled(trouble != nil || busy)
      Note(problem)
    }
  }

  // MARK: Recovery words

  private func code(_ words: [String]) -> some View {
    Page(title: "Save Your Words", message: "No passkey could keep your Yo. These 12 words bring it back on a new phone.") {
      WordGrid(words: words)
    } bottom: {
      PrimaryButton("Save to Photos", busy: busy) { Task { await saveToPhotos(words) } }
        .disabled(busy)
      Button("I Wrote Them Down") { go(.confirm(words)) }
        .font(.yo(17))
      Note(problem)
    }
  }

  // MARK: Doing

  private func go(_ next: Step) {
    problem = nil
    step = next
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

  /// A passkey first (any of this domain, also on another device); without one, or cancelled, the name and words.
  private func logIn() async {
    busy = true
    defer { busy = false }
    do {
      try await accounts.restoreWithPasskey()
    } catch {
      go(.logIn)
    }
  }

  /// The 12 words as a picture in the photo library (asked once, add only); saved, the account is in use.
  private func saveToPhotos(_ words: [String]) async {
    busy = true
    defer { busy = false }
    let renderer = ImageRenderer(content: RecoveryCard(name: name, words: words))
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
}

// MARK: - The page every step shares

/// Title and message at the top, the content in the middle, the buttons at the bottom; back at the top left.
private struct Page<Content: View, Bottom: View>: View {
  let title: String
  var message: String?
  var back: (() -> Void)?
  @ViewBuilder let content: Content
  @ViewBuilder let bottom: Bottom

  var body: some View {
    VStack(spacing: 0) {
      HStack {
        if let back {
          Button("Back", systemImage: "chevron.backward", action: back)
            .labelStyle(.iconOnly)
            .font(.title3.weight(.semibold))
            .frame(width: 44, height: 44, alignment: .leading)
        }
        Spacer()
      }
      .frame(height: 44)
      VStack(spacing: 10) {
        Text(title).font(.yo(30)).multilineTextAlignment(.center)
        if let message {
          Text(message).font(.system(size: 17)).multilineTextAlignment(.center).opacity(0.85)
        }
      }
      .padding(.top, 24)
      VStack(spacing: 12) { content }
        .padding(.top, 32)
      Spacer(minLength: 16)
      VStack(spacing: 18) { bottom }
    }
    .padding(.horizontal, 24)
    .padding(.bottom, 16)
  }
}

/// The one button at the bottom: white glass, the full width.
private struct PrimaryButton: View {
  let title: String
  var busy = false
  let action: () -> Void

  init(_ title: String, busy: Bool = false, action: @escaping () -> Void) {
    self.title = title
    self.busy = busy
    self.action = action
  }

  var body: some View {
    Button(action: action) {
      Group {
        if busy { ProgressView().tint(Color.amethyst) } else { Text(title).font(.yo(21)) }
      }
      .frame(maxWidth: .infinity, minHeight: 40)
    }
    .buttonStyle(.glassProminent)
    .tint(.white)
    .foregroundStyle(Color.amethyst)
  }
}

/// A line for what went wrong, under the buttons; nothing when nothing did.
private struct Note: View {
  let text: String?

  init(_ text: String?) { self.text = text }

  var body: some View {
    if let text {
      Text(text).font(.system(size: 14, weight: .semibold)).multilineTextAlignment(.center)
    }
  }
}

/// A name as Yo writes them: large, in upper case, only what a username may hold.
private struct NameField: View {
  @Binding var name: String
  var onSubmit: () -> Void = {}
  @FocusState private var focused: Bool

  var body: some View {
    TextField("", text: $name, prompt: Text("NAME").foregroundStyle(.white.opacity(0.3)))
      .font(.yo(34))
      .multilineTextAlignment(.center)
      .textInputAutocapitalization(.characters)
      .autocorrectionDisabled()
      .submitLabel(.continue)
      .focused($focused)
      .padding(.vertical, 14)
      .background(.white.opacity(0.14), in: .rect(cornerRadius: 16))
      .onChange(of: name) { _, typed in
        let clean = Username.clean(typed)
        if clean != typed { name = clean }
      }
      .onSubmit(onSubmit)
      .onAppear { focused = true }
  }
}

/// The 12 words, numbered, in two columns.
private struct WordGrid: View {
  let words: [String]

  var body: some View {
    LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 10) {
      ForEach(Array(words.enumerated()), id: \.offset) { index, word in
        HStack(spacing: 8) {
          Text("\(index + 1)").font(.system(size: 13, weight: .semibold).monospacedDigit()).opacity(0.6)
            .frame(width: 20, alignment: .trailing)
          Text(word).font(.yo(18))
          Spacer(minLength: 0)
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 12)
        .background(.white.opacity(0.14), in: .rect(cornerRadius: 12))
      }
    }
  }
}

/// Two of the words typed again, so they were really written down.
private struct ConfirmCode: View {
  let words: [String]
  let back: () -> Void
  let done: () -> Void
  @State private var asked = Array((0..<12).shuffled().prefix(2)).sorted()
  @State private var typed = ["", ""]
  @State private var wrong = false
  @FocusState private var field: Int?

  var body: some View {
    Page(title: "Check Your Words", message: "Type word \(asked[0] + 1) and word \(asked[1] + 1).", back: back) {
      ForEach(0..<2, id: \.self) { i in
        TextField("", text: $typed[i], prompt: Text("Word \(asked[i] + 1)").foregroundStyle(.white.opacity(0.35)))
          .font(.yo(22))
          .multilineTextAlignment(.center)
          .textInputAutocapitalization(.never)
          .autocorrectionDisabled()
          .focused($field, equals: i)
          .submitLabel(i == 0 ? .next : .done)
          .onSubmit { if i == 0 { field = 1 } else { check() } }
          .padding(.vertical, 14)
          .background(.white.opacity(0.14), in: .rect(cornerRadius: 16))
      }
    } bottom: {
      PrimaryButton("Done", action: check)
      Note(wrong ? "Not quite: look at your words again" : nil)
    }
    .onAppear { field = 0 }
    .sensoryFeedback(.error, trigger: wrong) { _, now in now }
  }

  private func check() {
    let right = (0..<2).allSatisfy { typed[$0].trimmingCharacters(in: .whitespaces).lowercased() == words[asked[$0]] }
    if right { done() } else { wrong = true }
  }
}

/// Logging in without a passkey: the name and its 12 words.
private struct LogIn: View {
  @State var name: String
  let back: () -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var words = ""
  @State private var problem: String?

  var body: some View {
    Page(title: "Log In", message: "Your name and your 12 words.", back: back) {
      NameField(name: $name)
      TextField("", text: $words, prompt: Text("12 words").foregroundStyle(.white.opacity(0.35)), axis: .vertical)
        .font(.yo(18))
        .lineLimit(3...5)
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .padding(14)
        .background(.white.opacity(0.14), in: .rect(cornerRadius: 16))
    } bottom: {
      PrimaryButton("Log In", action: logIn)
        .disabled(name.isEmpty || words.isEmpty)
      Note(problem)
    }
  }

  private func logIn() {
    guard let code = RecoveryCode.parse(words) else { return problem = "12 words from your list, please" }
    do {
      try accounts.restore(username: name, code: code)
    } catch {
      problem = error.localizedDescription
    }
  }
}

/// The recovery words as a picture for Photos: the brand's purple, the name, the 12 words, what they are for.
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
