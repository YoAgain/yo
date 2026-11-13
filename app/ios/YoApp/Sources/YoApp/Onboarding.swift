// The way in. Start → a name → a passkey saves it (one tap; if it cannot, 12 words, best saved to Photos) → the
// list. "Sign In" brings an account from before: a passkey, or the name and its 12 words; a passkey on this phone is
// offered by the system at once.
//
// Every step after the welcome is the same page, as Apple's setup screens are: a title and one line at the top, the
// content in the middle, the button at the bottom over the full width, a text button under it if there is a second
// way, back at the top left.
import PhotosUI
import SwiftUI

struct Onboarding: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(Inbox.self) private var inbox

  private enum Step: Equatable {
    case welcome, name, passkey([String]), code([String]), logIn, logInWithWords
  }

  @State private var step = Step.welcome
  @State private var name = ""
  @State private var busy = false
  @State private var problem: String?
  /// The quiet look for a passkey on this phone happens once, not each time the welcome comes back.
  @State private var looked = false
  /// Continue tapped with a name too short: the minimum shows, the field shakes (once per tap).
  @State private var tooShort = false
  @State private var shakes = 0
  /// Where the recovery words went: "photos", "copied". Kept somewhere, Continue goes straight in.
  @State private var kept: Set<String> = []

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch step {
        case .welcome: welcome
        case .name: naming
        case .passkey(let words): confirmPasskeyPage(then: words)
        case .code(let words): code(words)
        case .logIn: logInChoice
        case .logInWithWords: LogIn { go(.logIn) }
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

  /// The logo with the 2014 tagline, then the usual pair at the bottom: "Start" for a new name, "Sign In" under it.
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
      Button("Sign In") { go(.logIn) }
        .font(.yo(18))
        .padding(.top, 18)
    }
    .padding(.horizontal, 28)
    .padding(.bottom, 16)
    .disabled(busy)
    .task {
      guard !looked else { return }
      looked = true
      try? await Task.sleep(for: .seconds(0.4))  // the welcome on screen first
      inbox.playYo()  // the app says what it is: "Yo"
      try? await accounts.restoreWithPasskey(quietly: true)  // a Yo passkey here: the system offers it
    }
  }

  // MARK: Name

  private var naming: some View {
    // No words while typing: the field keeps the rules, a checkmark says free, a red cross says taken. Too short is
    // said only after a pause in typing, or when Continue is tapped (the field shakes too), until the next letter.
    let taken = name.count >= 3 && MockHub.isTaken(name)
    let trouble = Username.problem(name) ?? (taken ? "taken" : nil)
    let mine = taken && !Username.reserved.contains(name)
    return Page(title: "Choose a Name", back: { go(.welcome) }) {
      NameField(name: $name, mark: trouble == nil ? .available : taken ? .taken : nil,
        shakes: shakes, onSubmit: { Task { await register() } })
      // Under the field, one place: the minimum after a Continue too soon, or "This Is My Name" when it is taken.
      ZStack {
        Text("Use at least 3 characters.")
          .font(.system(size: 15, weight: .semibold))
          .opacity(tooShort ? 1 : 0)
        Button("This Is My Name") { go(.logIn) }
          .font(.yo(17))
          .opacity(mine ? 1 : 0)
          .disabled(!mine)
      }
      .animation(.snappy(duration: 0.2), value: mine)
      .animation(.snappy(duration: 0.2), value: tooShort)
      .onChange(of: name) { tooShort = false }
      .task(id: name) {
        // A pause after one or two letters: the minimum shows (someone typing on never sees it).
        guard (1...2).contains(name.count), (try? await Task.sleep(for: .seconds(1.5))) != nil else { return }
        tooShort = true
      }
    } bottom: {
      PrimaryButton("Continue", busy: busy) { Task { await register() } }
        .disabled(taken || busy)
      Note(problem)
    }
  }

  // MARK: Log in

  /// Asked for on purpose: the two ways, the passkey first.
  private var logInChoice: some View {
    Page(title: "Sign In", message: "Use your passkey or recovery key.", back: { go(.welcome) }) {
      EmptyView()
    } bottom: {
      PrimaryButton("Use Passkey", busy: busy) { Task { await logInWithPasskey() } }
        .disabled(busy)
      Button("Use Recovery Key") { go(.logInWithWords) }
        .font(.yo(17))
      Note(problem)
    }
  }

  // MARK: Passkey, once more

  /// Some password managers (1Password) give what the backup needs only when the passkey is used, not when it is
  /// made: so it is used once, here, said beforehand. Cancelled, the page stays; skipped, Settings can do it later.
  private func confirmPasskeyPage(then words: [String]) -> some View {
    Page(title: "Confirm Your Passkey",
      message: "Use your new passkey once to finish setting it up.", back: backToName) {
      Image(systemName: "person.badge.key.fill")
        .font(.system(size: 64))
        .padding(.top, 20)
    } bottom: {
      PrimaryButton("Use Passkey", busy: busy) { Task { await confirmPasskey(then: words) } }
        .disabled(busy)
      Button("Not Now") { go(.code(words)) }
        .font(.yo(17))
      Note(problem)
    }
  }

  private func confirmPasskey(then words: [String]) async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      try await accounts.confirmPasskey()
      go(.code(words))
    } catch let error as AccountError {
      problem = error.errorDescription
    } catch {
      problem = "Passkey not confirmed. Try again."
    }
  }

  // MARK: Recovery words

  /// The words, and two ways to keep them right under them (Photos, the clipboard); Next at the bottom. They stay on
  /// this phone too, in the profile.
  private func code(_ words: [String]) -> some View {
    Page(title: "Recovery Key", message: "Keep it somewhere safe. It's how you get back into Yo on a new iPhone.",
      back: backToName) {
      FrostedKey(words: words, qrSize: 96)
      HStack(spacing: 12) {
        KeepButton(title: "Save to Photos", icon: "photo.badge.arrow.down", done: kept.contains("photos")) {
          Task { await saveToPhotos(words) }
        }
        KeepButton(title: "Copy", icon: "doc.on.doc", done: kept.contains("copied")) { copy(words) }
      }
      .padding(.top, 6)
      .disabled(busy)
    } bottom: {
      PrimaryButton("Continue") { accounts.confirm() }
      Note(problem)
    }
  }

  // MARK: Doing

  /// Back from the passkey or the words: the registration is dropped, the name stays in the field.
  private func backToName() {
    accounts.cancelRegistration()
    go(.name)
  }

  private func go(_ next: Step) {
    problem = nil
    step = next
  }

  private func register() async {
    guard !busy else { return }
    guard name.count >= 3 else {
      tooShort = true
      shakes += 1
      UINotificationFeedbackGenerator().notificationOccurred(.error)
      return
    }
    guard Username.problem(name) == nil, !MockHub.isTaken(name) else { return }
    busy = true
    defer { busy = false }
    do {
      let words = try await accounts.register(name)
      go(accounts.passkeyToConfirm ? .passkey(words) : .code(words))
    } catch {
      problem = error.localizedDescription
    }
  }

  /// Any Yo passkey, on this phone or another device (the system's sheet offers both).
  private func logInWithPasskey() async {
    busy = true
    defer { busy = false }
    problem = nil
    do {
      try await accounts.restoreWithPasskey()
    } catch {
      problem = (error as? AccountError)?.errorDescription ?? "We couldn’t sign you in with a passkey. Try again or use your recovery key."
    }
  }

  private func copy(_ words: [String]) {
    RecoveryWords.copy(words, name: name)
    kept.insert("copied")
  }

  private func saveToPhotos(_ words: [String]) async {
    busy = true
    defer { busy = false }
    let renderer = ImageRenderer(content: RecoveryCard(name: name, words: words))
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

/// A way to keep the words: glass, the width of half the page; a checkmark once done.
struct KeepButton: View {
  let title: String
  let icon: String
  let done: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Label(title, systemImage: done ? "checkmark" : icon)
        .font(.system(size: 15, weight: .semibold))
        .frame(maxWidth: .infinity, minHeight: 30)
        .contentTransition(.symbolEffect(.replace))
    }
    .buttonStyle(.glass)
    .sensoryFeedback(.success, trigger: done) { _, now in now }
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
  /// At the end of the field: a green checkmark when the name is free, a red cross when it is taken.
  var mark: Mark?
  enum Mark { case available, taken }
  /// Counted up to shake the field (Continue with a name too short).
  var shakes = 0
  var onSubmit: () -> Void = {}
  @FocusState private var focused: Bool

  var body: some View {
    TextField("", text: $name, prompt: Text("Name").foregroundStyle(.white.opacity(0.3)))
      .font(.yo(34))
      .multilineTextAlignment(.center)
      .textInputAutocapitalization(.characters)
      .autocorrectionDisabled()
      .submitLabel(.continue)
      .focused($focused)
      .padding(.vertical, 14)
      .padding(.horizontal, 44)  // room for the checkmark, the name staying in the middle
      .background(.white.opacity(0.14), in: .rect(cornerRadius: 16))
      .overlay(alignment: .trailing) {
        if let mark {
          Image(systemName: mark == .available ? "checkmark.circle.fill" : "xmark.circle.fill")
            .font(.system(size: 24))
            .foregroundStyle(.white, mark == .available ? .green : .red)
            .padding(.trailing, 14)
            .transition(.scale.combined(with: .opacity))
            .id(mark)
            .accessibilityLabel(mark == .available ? "Available" : "Taken")
        }
      }
      .animation(.snappy(duration: 0.2), value: mark)
      .keyframeAnimator(initialValue: 0.0, trigger: shakes) { field, x in field.offset(x: x) } keyframes: { _ in
        for x in [-12.0, 10, -7, 4, 0] { SpringKeyframe(x, duration: 0.07) }
      }
      .onChange(of: name) { _, typed in
        let clean = Username.clean(typed)
        if clean != typed { name = clean }
      }
      .onSubmit(onSubmit)
      .onAppear { focused = true }
  }
}

/// The 12 words, numbered, in two columns.
struct WordGrid: View {
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

/// Logging in without a passkey: the 12 words, nothing else (they say whose they are): pasted or typed, or read from
/// the card's QR code by the camera or from Photos.
private struct LogIn: View {
  let back: () -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var words = ""
  @State private var problem: String?
  @State private var scanning = false
  @State private var picked: PhotosPickerItem?

  var body: some View {
    Page(title: "Recovery Key", message: "Paste, type, or scan your recovery key.", back: back) {
      TextField("", text: $words, prompt: Text("12 words").foregroundStyle(.white.opacity(0.35)), axis: .vertical)
        .font(.yo(18))
        .lineLimit(3...5)
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .padding(14)
        .background(.white.opacity(0.14), in: .rect(cornerRadius: 16))
      HStack(spacing: 12) {
        if QRScanner.isAvailable {
          KeepButton(title: "Scan", icon: "qrcode.viewfinder", done: false) { scanning = true }
        }
        PhotosPicker(selection: $picked, matching: .images) {
          Label("Photos", systemImage: "photo").font(.system(size: 15, weight: .semibold))
            .frame(maxWidth: .infinity, minHeight: 30)
        }
        .buttonStyle(.glass)
      }
    } bottom: {
      PrimaryButton("Sign In", action: logIn)
        .disabled(words.isEmpty)
      Note(problem)
    }
    .sheet(isPresented: $scanning) {
      QRScanner { text in
        scanning = false
        words = text
        logIn()
      }
      .ignoresSafeArea()
    }
    .onChange(of: picked) { _, item in
      guard let item else { return }
      Task {
        guard let data = try? await item.loadTransferable(type: Data.self), let image = UIImage(data: data),
          let text = RecoveryQR.read(image)
        else { return problem = "We couldn’t read a recovery key from this photo. Try another photo or enter the 12 words." }
        words = text
        logIn()
      }
    }
  }

  private func logIn() {
    guard let code = RecoveryCode.parse(words) else {
      return problem = "That doesn't match a recovery key. Check the words and try again."
    }
    do {
      try accounts.restore(code: code)
    } catch {
      problem = error.localizedDescription
    }
  }
}

/// The recovery words as a picture for Photos: the brand's purple, the name, the 12 words, what they are for.
struct RecoveryCard: View {
  let name: String
  let words: [String]

  var body: some View {
    VStack(alignment: .leading, spacing: 18) {
      HStack(alignment: .firstTextBaseline) {
        Text("Yo").font(.yo(44))
        Spacer()
        Text(name).font(.yo(24))
      }
      HStack(alignment: .bottom) {
        Text("Recovery Key").font(.yo(22))
        Spacer()
        RecoveryQRView(words: words, size: 110)
      }
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
      Text("Use this recovery key to sign in to Yo on a new iPhone. Keep it safe and don’t share it.")
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
