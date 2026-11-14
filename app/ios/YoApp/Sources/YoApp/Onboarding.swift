// The way in. Start → a name → a passkey saves it (one tap; if it cannot, 12 words, best saved to Photos) → the
// list. An account in the iCloud Keychain (the app installed again, a new iPhone) comes back before anything, without a
// word. "Sign In" brings an account from before: a passkey, or the name and its 12 words; a passkey on this phone is
// offered by the system at once.
//
// Every step after the welcome is the same page, as Apple's setup screens are: a title and one line at the top, the
// content in the middle, the button at the bottom over the full width, a text button under it if there is a second
// way, back at the top left.
import PhotosUI
import SwiftUI
import YoCore

struct Onboarding: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(Inbox.self) private var inbox

  private enum Step: Equatable {
    case welcome, name, logIn, logInWithWords
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
  /// What the hub said about names typed, by skeleton: free or taken (the checkmark or the red cross).
  @State private var checked: [String: Bool] = [:]
  /// The accounts in the iCloud Keychain, for Sign In; the one being signed in with.
  @State private var synced: [SyncedAccount.Item] = []
  @State private var signingIn: Data?

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch step {
        case .welcome: welcome
        case .name: naming
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

  /// The logo with the 2014 tagline, then the usual pair at the bottom: "Sign Up" for a new name, "Sign In" under it.
  /// The logo is the first of the pages swiped through above the pair (Intro.swift), as in the original.
  private var welcome: some View {
    VStack(spacing: 0) {
      Intro {
        VStack(spacing: 0) {
          Text("Yo")
            .font(.yo(150))
          Text("It’s that simple.")
            .font(.yo(20))
            .opacity(0.85)
        }
      }
      VStack(spacing: 0) {
        PrimaryButton("Sign Up") { go(.name) }
        Button("Sign In") { go(.logIn) }
          .font(.yo(18))
          .padding(.top, 18)
      }
      .padding(.horizontal, 28)
    }
    .padding(.bottom, 16)
    .disabled(busy)
    .task {
      guard !looked else { return }
      looked = true
      // The account in the iCloud Keychain (the app installed again, a new iPhone): straight back to the list.
      busy = true
      let back = await accounts.restoreFromKeychain()
      busy = false
      if back { return }
      // The welcome on screen first; gone meanwhile (Sign Up or Sign In tapped): no quiet passkey look over it.
      guard (try? await Task.sleep(for: .seconds(0.4))) != nil else { return }
      inbox.playYo()  // the app says what it is: "Yo"
      try? await accounts.restoreWithPasskey(quietly: true)  // a Yo passkey here: the system offers it
    }
  }

  // MARK: Name

  private var naming: some View {
    // No words while typing: the field keeps the rules, a checkmark says free, a red cross says taken. Too short is
    // said only after a pause in typing, or when Continue is tapped (the field shakes too), until the next letter.
    let free = checked[Username.skeleton(name)]  // the hub's answer: true free, false taken, nil not yet
    let taken = name.count >= 3 && free == false
    let trouble = Username.problem(name) ?? (taken ? "taken" : nil)
    let mine = taken  // or reserved: only the hub knows, and signing in with a name not one's own just fails
    return Page(title: "Choose a Name", back: { go(.welcome) }) {
      NameField(name: $name, mark: trouble == nil && free == true ? .available : taken ? .taken : nil,
        shakes: shakes, onSubmit: { Task { await register() } })
      // Under the field, one place: the minimum after a Continue too soon, or "This Is My Name" when it is taken.
      ZStack {
        Text("Use at least 3 characters.")
          .font(.system(size: 15, weight: .semibold))
          .opacity(tooShort ? 1 : 0)
          .accessibilityHidden(!tooShort)
        Button("This Is My Name") { go(.logIn) }
          .font(.yo(17))
          .opacity(mine ? 1 : 0)
          .disabled(!mine)
          .accessibilityHidden(!mine)
      }
      .animation(.snappy(duration: 0.2), value: mine)
      .animation(.snappy(duration: 0.2), value: tooShort)
      .onChange(of: name) { tooShort = false }
      .task(id: name) { await check(name) }
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

  /// Asked for on purpose: the accounts in the iCloud Keychain to tap, if any; then the two ways, the passkey first.
  private var logInChoice: some View {
    Page(title: "Sign In", message: "Use your passkey or recovery key.", back: { go(.welcome) }) {
      if !synced.isEmpty {
        ScrollView {
          VStack(spacing: 10) {
            ForEach(synced) { item in
              Button { Task { await logIn(item) } } label: {
                HStack(spacing: 12) {
                  Avatar(name: item.username, size: 36)
                  Text(item.username).font(.yo(20)).lineLimit(1)
                  Spacer(minLength: 0)
                  if signingIn == item.userID { ProgressView().tint(.white) }
                }
                .frame(maxWidth: .infinity, minHeight: 44)
              }
              .buttonStyle(.glass)
              .disabled(busy)
              .accessibilityLabel("Sign In as \(item.username)")
            }
            Text("Saved in your iCloud Keychain.").font(.footnote).opacity(0.75)
          }
        }
        .scrollBounceBehavior(.basedOnSize)
      }
    } bottom: {
      PrimaryButton("Use Passkey", busy: busy) { Task { await logInWithPasskey() } }
        .disabled(busy)
      Button("Use Recovery Key") { go(.logInWithWords) }
        .font(.yo(17))
      Note(problem)
    }
  }

  // MARK: Doing

  /// Asks the hub whether a name is free, a moment after the typing stops (one question per pause, not per letter).
  private func check(_ typed: String) async {
    let skeleton = Username.skeleton(typed)
    guard Username.problem(typed) == nil, checked[skeleton] == nil,
      (try? await Task.sleep(for: .milliseconds(350))) != nil
    else { return }
    let transport = Hub.transport
    if let free = try? await Hub.run({ try usernameFree(transport: transport, username: typed) }) {
      checked[skeleton] = free
    }
  }

  private func go(_ next: Step) {
    problem = nil
    if next == .logIn { synced = SyncedAccount.all() }
    step = next
  }

  /// An account from the iCloud Keychain, tapped.
  private func logIn(_ item: SyncedAccount.Item) async {
    busy = true
    signingIn = item.userID
    defer {
      busy = false
      signingIn = nil
    }
    problem = nil
    do {
      try await accounts.restore(synced: item)
    } catch {
      synced = SyncedAccount.all()  // one whose name has another key now is gone
      problem = error.localizedDescription
    }
  }

  private func register() async {
    guard !busy else { return }
    guard name.count >= 3 else {
      tooShort = true
      shakes += 1
      UINotificationFeedbackGenerator().notificationOccurred(.error)
      return
    }
    guard Username.problem(name) == nil else { return }
    busy = true
    defer { busy = false }
    problem = nil
    do {
      try await accounts.register(name)  // in: the list takes over, YoGPT on it (YoApp); secured later, from the profile
    } catch AccountError.taken {
      checked[Username.skeleton(name)] = false
      UINotificationFeedbackGenerator().notificationOccurred(.error)
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

}

// MARK: - The page every step shares

/// Title and message at the top, the content in the middle, the buttons at the bottom; back at the top left.
struct Page<Content: View, Bottom: View>: View {
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
struct PrimaryButton: View {
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
struct Note: View {
  let text: String?

  init(_ text: String?) { self.text = text }

  var body: some View {
    if let text {
      Text(text).font(.system(size: 14, weight: .semibold)).multilineTextAlignment(.center)
    }
  }
}

/// A name as its owner writes it: large, only what a username may hold.
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
      .font(.yo(Self.fontSize(name)))
      .tint(.white)
      .multilineTextAlignment(.center)
      .textInputAutocapitalization(.words)
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

  /// Large, and smaller as a long name needs, so all 16 characters stay in sight beside the checkmark.
  private static func fontSize(_ name: String) -> CGFloat {
    let width = 260.0  // the field's room for text at the narrowest iPhone's width, less the checkmark's
    guard let font = UIFont(name: "Montserrat-Bold", size: 34) else { return 34 }
    let needed = (name as NSString).size(withAttributes: [.font: font]).width
    return needed > width ? max(18, (34 * width / needed).rounded(.down)) : 34
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
  /// Signing in: one at a time (a second registration would replace the first).
  @State private var busy = false
  @FocusState private var focused: Bool

  var body: some View {
    Page(title: "Recovery Key", message: "Paste, type, or scan your recovery key.", back: back) {
      TextField("", text: $words, prompt: Text("12 words").foregroundStyle(.white.opacity(0.35)), axis: .vertical)
        .font(.yo(18))
        .lineLimit(3...5)
        .tint(.white)
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .padding(14)
        .background(.white.opacity(0.14), in: .rect(cornerRadius: 16))
        .focused($focused)
        .onAppear { focused = true }
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
      PrimaryButton("Sign In", busy: busy, action: logIn)
        .disabled(words.isEmpty || busy)
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
      return problem = "That doesn’t match a recovery key. Check the words and try again."
    }
    guard !busy else { return }
    busy = true
    problem = nil
    Task {
      defer { busy = false }
      do {
        try await accounts.restore(code: code)
      } catch {
        problem = error.localizedDescription
      }
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
