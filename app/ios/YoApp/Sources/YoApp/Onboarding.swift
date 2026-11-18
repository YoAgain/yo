// The way in. Sign Up → a name → the list (the name is secured later, from the profile: Secure Your Account). The one
// account in the iCloud Keychain not signed out (the app installed again, a new iPhone) comes back before anything,
// without a word. "Sign In" brings an account from before: one of the iCloud Keychain's, tapped; a passkey; or the recovery
// key, which says whose it is. With an account in the iCloud Keychain, Sign In comes up by itself at the first start.
//
// Every step after the welcome is the same page, as Apple's setup screens are: a title and one line at the top, the
// content in the middle, the button at the bottom over the full width, a text button under it if there is a second
// way, back at the top left.
import PhotosUI
import _PhotosUI_SwiftUI
import SwiftUI
import YoCore

struct Onboarding: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(Inbox.self) private var inbox

  private enum Step: Equatable {
    case welcome, name, logIn, logInWithWords
  }

  /// Set by Sign Out, for this run of the app only: the welcome then shows the Sign In card instead of signing in
  /// again. The next cold start (the app closed, or installed anew) signs in from the iCloud Keychain by itself.
  @MainActor static var signedOutThisRun = false

  @State private var step = Step.welcome
  @State private var name = ""
  @State private var busy = false
  @State private var problem: String?
  /// The quiet look for a passkey on this phone happens once, not each time the welcome comes back.
  @State private var looked = false
  /// Continue tapped with a name too short: the minimum shows, the field shakes (once per tap).
  @State private var tooShort = false
  /// Names the hub refused as shorter than it allows now (its `YO_NAME_MIN`), by skeleton.
  @State private var short: Set<String> = []
  @State private var shakes = 0
  @State private var focusRequests = 0
  /// The name once the typing paused on it: only then its taken or free mark.
  @State private var settled = ""
  /// What the hub said about names typed, by skeleton: free or taken (the checkmark or the red cross).
  @State private var checked: [String: Bool] = [:]
  /// The accounts in the iCloud Keychain, for Sign In; the one being signed in with.
  @State private var synced: [SyncedAccount.Item] = []
  @State private var signingIn: Data?
  /// The Sign In card over the welcome.
  @State private var signingInCard = false
  /// The account the card signs in by itself after a moment (a cold start with exactly one in the keychain).
  @State private var autoSignIn: Data?
  /// The carousel's own height on the welcome, and the room it gets above the form (less with the keyboard up).
  @State private var introHeight = 0.0
  @State private var introRoom = 0.0

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
    .sheet(isPresented: $signingInCard) { signInCard }
  }

  // MARK: Welcome

  /// The logo with the 2014 tagline, then the usual pair at the bottom: "Sign Up" for a new name, "Sign In" under it.
  /// The logo is the first of the pages swiped through above the pair (Intro.swift), as in the original.
  private var welcome: some View {
    VStack(spacing: 0) {
      // The pages and their dots as one block, a little higher; the room left goes between them and the name.
      Intro {
        VStack(spacing: 0) {
          Text("Yo")
            .font(.yo(150))
          Text("It’s that simple.")
            .font(.yo(20))
            .opacity(0.85)
        }
      }
      .onGeometryChange(for: Double.self) { $0.size.height } action: { introHeight = $0 }
      // With the keyboard up the room above the form gets smaller than the cards: the carousel shrinks into it, so
      // the name field and the whole Sign Up button stay above the keyboard on every iPhone (the form keeps its size).
      .scaleEffect(introHeight > 0 ? min(1, introRoom / introHeight) : 1)
      // The carousel where it was (centred in the room above the form); only its dots sit close under the cards.
      .frame(minHeight: 0, maxHeight: .infinity)
      .onGeometryChange(for: Double.self) { $0.size.height } action: { introRoom = $0 }
      VStack(spacing: 0) {
        // An invite link opened before the account: its name comes on the list once there is one (Invite.swift).
        if let invited = Invite.shared.pending {
          Text("\(invited) invited you")
            .font(.yo(17))
            .opacity(0.85)
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .padding(.bottom, 16)
            .transition(.opacity)
        }
        // Sign Up right here: the name, its free/taken mark, the button. No page of its own.
        // Taken or free shows once the typing has paused (500 ms), never while passing a name on the way ("Peter5").
        let free = name == settled ? checked[Username.skeleton(name)] : nil
        let taken = !name.isEmpty && free == false
        ZStack {
          Text("Choose a longer name.")
            .font(.subheadline.weight(.semibold))
            .opacity(tooShort ? 1 : 0)
            .accessibilityHidden(!tooShort)
          Button {
            go(.logIn)
          } label: {
            (Text("Name taken. Yours? ").foregroundStyle(.white.opacity(0.8))
              + Text("Sign In").foregroundStyle(.white).underline())
              .font(.subheadline.weight(.semibold))
          }
          .buttonStyle(.plain)
          .opacity(taken ? 1 : 0)
          .disabled(!taken)
          .accessibilityHidden(!taken)
          .accessibilityLabel("Name taken. Sign In if it’s yours")
        }
        .frame(minHeight: 22)
        .padding(.bottom, 8)
        .animation(.snappy(duration: 0.2), value: taken)
        .animation(.snappy(duration: 0.2), value: tooShort)
        .onChange(of: name) { tooShort = false }
        .task(id: name) { await check(name) }
        .task(id: name) {
          guard (try? await Task.sleep(for: .milliseconds(500))) != nil else { return }
          settled = name
        }
        .task(id: name) {
          // Too short shows (line and red mark) only once the typing has paused, never on the first letter.
          guard (try? await Task.sleep(for: .seconds(1.5))) != nil else { return }
          if (1...2).contains(name.count) || short.contains(Username.skeleton(name)) { tooShort = true }
        }
        // The line about the name, then field, Sign Up and Sign In close together as one block.
        VStack(spacing: 10) {
          NameField(name: $name, mark: name.isEmpty ? nil : Username.problem(name) == nil && free == true ? .available
            : taken || tooShort ? .taken : nil,
            shakes: shakes, autofocus: false, focusRequests: focusRequests, onSubmit: { Task { await register() } })
          // Always tappable: with no name yet it shakes the field and brings the keyboard up.
          PrimaryButton("Sign Up", busy: busy) {
            if name.isEmpty {
              shakes += 1
              focusRequests += 1
              UINotificationFeedbackGenerator().notificationOccurred(.warning)
            } else {
              Task { await register() }
            }
          }
          .disabled(taken || busy)
        }

        Note(problem)
        // One line, all of it the link: the question small, "Sign In" what stands out, exactly in the middle of the
        // screen; the question hangs to its left, the same room left unseen on the right keeps it there.
        Button { go(.logIn) } label: {
          let question = Text("Got Yo?").font(.system(size: 15, weight: .semibold)).foregroundStyle(.white.opacity(0.85))
          HStack(alignment: .firstTextBaseline, spacing: 6) {
            question
            Text("Sign In").font(.yo(18))
            question.hidden()
          }
          .lineLimit(1)
          .fixedSize()
        }
        .accessibilityLabel("Got Yo? Sign In")
        .padding(.top, 14)
      }
      .padding(.horizontal, 28)
    }
    .padding(.bottom, 16)
    .disabled(busy)
    .task {
      guard !looked else { return }
      looked = true
      // Exactly one account in the iCloud Keychain, not signed out on purpose (a new iPhone, the app installed again):
      // straight back to the list, without a question.
      // Installed anew (no defaults of this app yet): a fresh start, even after a Sign Out on the old install.
      // A cold start signs in from the iCloud Keychain by itself; right after a Sign Out (same run) the card comes.
      let fresh = !Self.signedOutThisRun
      if fresh {
        busy = true
        let back = await accounts.restoreFromKeychain(fresh: true)
        busy = false
        if back { return }
      }
      // The welcome on screen first; gone meanwhile (Sign Up or Sign In tapped): nothing over it.
      guard (try? await Task.sleep(for: .seconds(0.4))) != nil, step == .welcome else { return }
      inbox.playYo()  // the app says what it is: "Yo"
      // Otherwise accounts in the iCloud Keychain (signed out before, or several): the Sign In card comes up by itself,
      // listing them to tap, with the passkey and the recovery key as the other ways. No passkey is looked for unasked.
      if !SyncedAccount.all().isEmpty { withAnimation(.smooth) { go(.logIn) } }
    }
  }

  // MARK: Name

  private var naming: some View {
    // No words while typing: the field keeps the rules, a checkmark says free, a red cross says taken. Too short is
    // said only after a pause in typing, or when Continue is tapped (the field shakes too), until the next letter.
    let free = checked[Username.skeleton(name)]  // the hub's answer: true free, false taken, nil not yet
    let taken = !name.isEmpty && free == false
    let trouble = Username.problem(name) ?? (taken ? "taken" : nil)
    let mine = taken  // or reserved: only the hub knows, and signing in with a name not one's own just fails
    return Page(title: "Choose a Name", back: { go(.welcome) }) {
      NameField(name: $name, mark: name.isEmpty ? nil : trouble == nil && free == true ? .available
        : trouble != nil ? .taken : nil,
        shakes: shakes, onSubmit: { Task { await register() } })
      // Under the field, one place: the minimum after a Continue too soon, or "This Is My Name" when it is taken.
      ZStack {
        Text("Choose a longer name.")
          .font(.subheadline.weight(.semibold))
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
  private var logInChoice: some View { welcome }

  /// Sign In as a small card over the welcome: the iCloud Keychain's accounts to tap, then the passkey and the
  /// recovery key. Comes up by itself at the first start when the keychain holds an account.
  private var signInCard: some View {
    SignInCard(
      busy: busy, signingIn: signingIn, problem: problem, autoSignIn: $autoSignIn,
      logIn: { item in Task { await logIn(item) } },
      passkey: { Task { await logInWithPasskey() } },
      recoveryKey: {
        signingInCard = false
        go(.logInWithWords)
      })
  }

  // MARK: Doing

  /// Asks the hub whether a name is free, a moment after the typing stops (one question per pause, not per letter).
  private func check(_ typed: String) async {
    let skeleton = Username.skeleton(typed)
    guard Username.problem(typed) == nil, checked[skeleton] == nil,
      (try? await Task.sleep(for: .milliseconds(500))) != nil
    else { return }
    let transport = Hub.transport
    do {
      let free = try await Hub.run({ try usernameFree(transport: transport, username: typed) })
      // Taken by this phone's own try whose answer was lost (its key is kept): Continue takes it (`NewAccountKey`).
      checked[skeleton] = free || NewAccountKey.has(typed)
    } catch YoError.Refused(status: 400) {
      // A name of the right form the hub finds too short: how short a name may be is the hub's switch (spec 3.3).
      short.insert(skeleton)  // shown after the pause (the 1.5 s task), not at once
    } catch {}
  }

  private func go(_ next: Step) {
    problem = nil
    if next == .logIn {
      // The list first, the card a moment later: shown in the same update, the sheet drew the empty list from before
      // and refreshed only on the next change (a passkey try cancelled).
      synced = SyncedAccount.all()
      problem = nil
      step = .welcome
      Task { @MainActor in
        await Task.yield()
        signingInCard = true
      }
      return
    }
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
    guard !name.isEmpty, !short.contains(Username.skeleton(name)) else {
      tooShort = true
      shakes += 1
      UINotificationFeedbackGenerator().notificationOccurred(.error)
      return
    }
    guard Username.problem(name) == nil else { return }
    busy = true
    defer { busy = false }
    problem = nil
    let asked = name  // the field may change while the hub answers: "taken" is about this one
    do {
      try await accounts.register(asked)  // in: the list takes over, YoGPT on it (YoApp); secured later, from the profile
    } catch YoError.Refused(status: 400) {
      short.insert(Username.skeleton(asked))
      tooShort = true
      UINotificationFeedbackGenerator().notificationOccurred(.error)
    } catch AccountError.taken {
      checked[Username.skeleton(asked)] = false
      UINotificationFeedbackGenerator().notificationOccurred(.error)
    } catch {
      // The hub's refusal of a name can arrive wrapped (no raw codes on screen): a refusal reads as too short, anything
      // else as one plain line.
      if case YoError.Refused = error {
        short.insert(Username.skeleton(asked))
        tooShort = true
      } else {
        problem = "Couldn’t sign up. Check your connection and try again."
      }
      UINotificationFeedbackGenerator().notificationOccurred(.error)
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
          // The whole 44 points take the tap, not only the chevron.
          Button(action: back) {
            Image(systemName: "chevron.backward")
              .font(.title3.weight(.semibold))
              .frame(width: 44, height: 44, alignment: .leading)
              .contentShape(.rect)
          }
          .accessibilityLabel("Back")
        }
        Spacer()
      }
      .frame(height: 44)
      // Title, message and content scroll when large text needs more room than there is; the buttons stay.
      ScrollView {
        VStack(spacing: 10) {
          Text(title).font(.yo(30)).multilineTextAlignment(.center)
          if let message {
            Text(message).font(.body).multilineTextAlignment(.center).opacity(0.85)
          }
        }
        .padding(.top, 24)
        VStack(spacing: 12) { content }
          .padding(.top, 32)
          .padding(.bottom, 16)
      }
      .scrollBounceBehavior(.basedOnSize)
      .scrollIndicators(.hidden)
      VStack(spacing: 18) { bottom }
    }
    .padding(.horizontal, 24)
    .padding(.bottom, 16)
  }
}

/// Close for a guide shown as a sheet (Secure Your Account, the recovery key): at the top right, in the row of the
/// pages' back button and as large, so the sheet says how it goes away, not only by a swipe down.
struct SheetClose: View {
  @Environment(\.dismiss) private var dismiss
  /// The presenter's own close (its binding), when the environment's dismiss would not reach the sheet.
  var action: (() -> Void)? = nil

  var body: some View {
    Button { if let action { action() } else { dismiss() } } label: {
      // The system's close: a Liquid Glass circle, as the navigation bar's buttons.
      Image(systemName: "xmark")
        .font(.system(size: 17, weight: .semibold))
        .frame(width: 44, height: 44)
        .glassEffect(.regular.interactive(), in: .circle)
        .contentShape(.circle)
    }
    .accessibilityLabel("Close")
    .buttonStyle(.plain)
    .padding(.trailing, 16)
    // Below the sheet's grabber strip: the system takes touches there for dragging, not for this button.
    .padding(.top, 20)
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
        if busy { ProgressView() } else { Text(title).font(.system(.title3, weight: .semibold)) }
      }
      .frame(maxWidth: .infinity, minHeight: 40)
    }
    .buttonStyle(.glassProminent)
    .tint(.white)
    .foregroundStyle(.black)
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
        .font(.subheadline.weight(.semibold))
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
      Text(text).font(.footnote.weight(.semibold)).multilineTextAlignment(.center)
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
  /// The keyboard comes up at once (its own page); on the welcome it waits for a tap.
  var autofocus = true
  /// Counted up to bring the keyboard up from outside (Sign Up tapped with no name yet).
  var focusRequests = 0
  var onSubmit: () -> Void = {}
  @FocusState private var focused: Bool
  /// The name's width at full size, and the field's room for it: a long name gets smaller until all of it fits.
  @State private var needed = 0.0
  @State private var room = 0.0

  private static let full = 28.0

  var body: some View {
    TextField("", text: $name)
      .font(.yo(fontSize))
      .tint(.white)
      .multilineTextAlignment(.center)
      .textInputAutocapitalization(.words)
      .autocorrectionDisabled()
      .submitLabel(.continue)
      .focused($focused)
      .onGeometryChange(for: Double.self) { $0.size.width } action: { room = $0 }
      // The name as it would be drawn at full size, unseen: how wide it is in this font at this text size.
      .background {
        Text(name.isEmpty ? " " : name).font(.yo(Self.full)).fixedSize().hidden()
          .onGeometryChange(for: Double.self) { $0.size.width } action: { needed = $0 }
      }
      .padding(.vertical, 14)
      .padding(.horizontal, 44)  // room for the checkmark, the name staying in the middle
      // A pill like the Sign Up button under it, same width and shape; tinted, not white, so the two read as field
      // and button. Its prompt says what goes in.
      .background(.white.opacity(0.16), in: .capsule)
      .overlay(alignment: .trailing) {
        if let mark {
          Image(systemName: mark == .available ? "checkmark.circle.fill" : "xmark.circle.fill")
            .font(.system(size: 24))
            .foregroundStyle(.white, mark == .available ? .green : .red)
            .padding(.trailing, 14)
            .transition(.scale.combined(with: .opacity))
            .id(mark)
            .accessibilityLabel(mark == .available ? "Available" : "Not available")
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
      .onAppear { if autofocus { focused = true } }
      .onChange(of: focusRequests) { focused = true }
      // The prompt in the name's own font and size, one short word.
      .overlay {
        if name.isEmpty {
          Text("Username")
            .font(.yo(Self.full))
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .foregroundStyle(.white.opacity(0.45))
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
  }

  /// Full size, and smaller as a long name needs, so all 16 characters stay in sight beside the checkmark (the cursor
  /// needs a few points too).
  private var fontSize: Double {
    let fits = room - 6
    guard needed > fits, fits > 0 else { return Self.full }
    return max(8, (Self.full * fits / needed).rounded(.down))
  }
}

/// The recovery key in 16 blocks of 4, four to a row, monospaced so the blocks line up.
struct KeyGrid: View {
  let key: String

  var body: some View {
    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 4), spacing: 8) {
      ForEach(Array(RecoveryCode.blocks(key).enumerated()), id: \.offset) { _, block in
        Text(block)
          .font(.system(size: 18, weight: .semibold, design: .monospaced))
          .lineLimit(1)
          .minimumScaleFactor(0.6)
          .frame(maxWidth: .infinity)
          .padding(.vertical, 10)
          .background(.white.opacity(0.14), in: .rect(cornerRadius: 12))
      }
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(RecoveryCode.blocks(key).map { $0.map(String.init).joined(separator: " ") }.joined(separator: ", "))
  }
}

/// Logging in without a passkey: the recovery key, nothing else (it says whose it is): pasted or typed, or read from
/// the card's QR code by the camera or from Photos.
private struct LogIn: View {
  let back: () -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var words = ""
  @State private var problem: String?
  @State private var scanning = false
  @State private var picked: _PhotosUI_SwiftUI.PhotosPickerItem?
  /// Signing in: one at a time (a second registration would replace the first).
  @State private var busy = false
  @FocusState private var focused: Bool

  var body: some View {
    Page(title: "Recovery Key", message: "Paste, type, or scan your recovery key.", back: back) {
      TextField("", text: $words, prompt: Text("64 characters").foregroundStyle(.white.opacity(0.35)), axis: .vertical)
        .font(.system(size: 18, weight: .semibold, design: .monospaced))
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
          Label("Photos", systemImage: "photo").font(.subheadline.weight(.semibold))
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
        else { return problem = "We couldn’t read a recovery key from this photo. Try another photo or enter the key." }
        words = text
        logIn()
      }
    }
  }

  private func logIn() {
    guard let code = RecoveryCode.parse(words) else {
      return problem = "That doesn’t match a recovery key. Check it and try again."
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

/// The recovery key as a picture for Photos: the brand's purple, the name, the key in blocks of 4, what it is for.
struct RecoveryCard: View {
  let name: String
  let key: String

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
        RecoveryQRView(key: key, size: 110)
      }
      let blocks = RecoveryCode.blocks(key)
      Grid(horizontalSpacing: 18, verticalSpacing: 10) {
        ForEach(0..<(blocks.count + 3) / 4, id: \.self) { row in
          GridRow {
            ForEach(row * 4..<min(row * 4 + 4, blocks.count), id: \.self) { i in
              Text(blocks[i]).font(.system(size: 22, weight: .semibold, design: .monospaced))
                .frame(maxWidth: .infinity)
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

/// Sign In as a small card: its own view with its own list, read from the iCloud Keychain as it appears (a sheet does
/// not redraw from its presenter's state reliably: the list showed only after the next tap).
private struct SignInCard: View {
  let busy: Bool
  let signingIn: Data?
  let problem: String?
  @Binding var autoSignIn: Data?
  let logIn: (SyncedAccount.Item) -> Void
  let passkey: () -> Void
  let recoveryKey: () -> Void
  @State private var synced: [SyncedAccount.Item] = SyncedAccount.all()
  @State private var removing: SyncedAccount.Item?
  /// The card's own height, measured: as tall as its accounts and buttons, no taller.
  @State private var height: CGFloat = 300

  var body: some View {
    VStack(spacing: 14) {
      Text("Sign In")
        .font(.yo(28))
        .padding(.top, 28)
      if autoSignIn != nil {
        Button("Cancel") { withAnimation(.smooth) { autoSignIn = nil } }
          .font(.yo(17))
          .accessibilityLabel("Cancel signing in")
      }
      if !synced.isEmpty {
        VStack(spacing: 10) {
          ForEach(synced) { item in
            Button { logIn(item) } label: {
              HStack(spacing: 12) {
                Avatar(name: item.username, size: 36)
                Text(item.username).font(.yo(20)).lineLimit(1).foregroundStyle(.white)
                Spacer(minLength: 0)
                if signingIn == item.userID {
                  ProgressView().tint(.white)
                } else if autoSignIn == item.userID {
                  // Two seconds to cancel, as a ring filling up; tap Cancel (or anything) to stay signed out.
                  AutoSignInRing()
                }
              }
              .padding(.horizontal, 14)
              .frame(maxWidth: .infinity, minHeight: 56)
              .background(.white.opacity(0.16), in: .capsule)
              .contentShape(.capsule)
            }
            .buttonStyle(.plain)
            .disabled(busy)
            .accessibilityLabel("Sign In as \(item.username)")
            // Clean up: an old account taken out of the iCloud Keychain (on all devices), after a question.
            .contextMenu {
              Button("Remove from iCloud Keychain", systemImage: "trash", role: .destructive) { removing = item }
            }
            .accessibilityAction(named: "Remove from iCloud Keychain") { removing = item }
          }
        }
        Text("or").font(.yo(17)).opacity(0.75)
      } else {
        Color.clear.frame(height: 18)  // some air between the title and the buttons when there is no account to show
      }
      PrimaryButton("Use Passkey", busy: busy) { passkey() }
        .disabled(busy)
      Button("Use Recovery Key") { recoveryKey() }
        .font(.yo(17))
      Note(problem)
    }
    .padding(.horizontal, 24)
    .padding(.bottom, 12)
    .fixedSize(horizontal: false, vertical: true)
    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { new in
      if abs(new - height) > 1 { height = new }
    }
    .frame(maxHeight: .infinity, alignment: .top)
    .foregroundStyle(.white)
    .task(id: autoSignIn) {
      guard let id = autoSignIn, (try? await Task.sleep(for: .seconds(2))) != nil, autoSignIn == id,
        let item = synced.first(where: { $0.userID == id })
      else { return }
      autoSignIn = nil
      logIn(item)
    }
    .onDisappear { autoSignIn = nil }
    .task {
      for _ in 0..<5 where synced.isEmpty {
        try? await Task.sleep(for: .milliseconds(300))
        synced = SyncedAccount.all()
      }
    }
    .alert(
      "Remove \(removing?.username ?? "")?",
      isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })
    ) {
      Button("Remove", role: .destructive) {
        if let item = removing {
          SyncedAccount.delete(item.userID)
          withAnimation(.smooth) { synced = SyncedAccount.all() }
        }
        removing = nil
      }
      Button("Cancel", role: .cancel) { removing = nil }
    } message: {
      Text("It’s taken out of your iCloud Keychain on all your devices. Without a passkey or recovery key, you can’t sign in to it again.")
    }
    .presentationDetents([.height(height)])
    .presentationBackground(Color.amethyst)
    .presentationDragIndicator(.hidden)  // one detent: nothing to pull
    .preferredColorScheme(.dark)
  }
}

/// Fills once in two seconds: the time left to cancel signing in by itself.
private struct AutoSignInRing: View {
  @State private var done = false
  var body: some View {
    Circle()
      .trim(from: 0, to: done ? 1 : 0)
      .stroke(.white, style: StrokeStyle(lineWidth: 3, lineCap: .round))
      .rotationEffect(.degrees(-90))
      .frame(width: 22, height: 22)
      .onAppear { withAnimation(.linear(duration: 2)) { done = true } }
      .accessibilityHidden(true)
  }
}
