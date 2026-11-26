// The way in: Sign Up with a name (secured later from the profile), or Sign In by iCloud Keychain account, passkey or
// recovery key. A single non-signed-out iCloud Keychain account comes back silently before anything.
import PhotosUI
import _PhotosUI_SwiftUI
import SwiftUI
import YoCore

struct Onboarding: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(Inbox.self) private var inbox

  private enum Step: Equatable {
    case welcome, logIn
  }

  /// Set by Sign Out for this run only: the welcome then shows the Sign In card instead of signing in again.
  @MainActor static var signedOutThisRun = false

  @State private var step = Step.welcome
  @State private var name = ""
  @State private var busy = false
  @State private var problem: String?
  /// The silent look for an iCloud Keychain account happens once, not each time the welcome comes back.
  @State private var looked = false
  @State private var nameField: YoNameField?
  @State private var nameState: YoNameState?
  @State private var sequence: UInt64?
  @State private var registrationRefusal: YoNamePhase?
  @State private var shakes = 0
  @State private var focusRequests = 0
  @State private var signingIn: Data?
  @State private var signingInCard = false
  /// Which of the card's other ways is signing in now: its spinner.
  @State private var signingInBy: SignInCard.Way?
  /// The carousel's height, and the room it gets above the form (less with the keyboard up).
  @State private var introHeight = 0.0
  @State private var introRoom = 0.0

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      welcome
    }
    .foregroundStyle(.white)
    .animation(.smooth(duration: 0.35), value: step)
    .preferredColorScheme(.dark)
    .sheet(isPresented: $signingInCard) { signInCard.appObjects() }
  }

  /// "By continuing you agree to the Terms.", Terms a white underlined link (no purple on purple).
  private static let termsLine: AttributedString = {
    var link = AttributedString("Terms")
    link.link = URL(string: "https://justyo.co/p/terms")
    link.underlineStyle = .single
    link.foregroundColor = .white
    return AttributedString("By continuing you agree to the ") + link + AttributedString(".")
  }()

  // MARK: Welcome

  /// The logo and the 2014 tagline as the first intro page (Intro.swift), then Sign Up and Sign In.
  private var welcome: some View {
    VStack(spacing: 0) {
      Intro {
        VStack(spacing: 0) {
          Text("Yo")
            .font(.yo(150))
          Text(ProductText.thatSimple.english)
            .font(.yo(20))
            .opacity(0.85)
        }
      }
      .onGeometryChange(for: Double.self) { $0.size.height } action: { introHeight = $0 }
      // With the keyboard up the carousel shrinks into the room left, so the field and Sign Up stay above it.
      .scaleEffect(introHeight > 0 ? min(1, introRoom / introHeight) : 1)
      .frame(minHeight: 0, maxHeight: .infinity)
      .onGeometryChange(for: Double.self) { $0.size.height } action: { introRoom = $0 }
      VStack(spacing: 0) {
        if let invited = Invite.shared.pending {
          Text(productInvitedYou(name: invited, lang: .english))
            .font(.yo(17))
            .opacity(0.85)
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .padding(.bottom, 16)
            .transition(.opacity)
        }
        VStack(spacing: 10) {
          NameField(name: Binding(get: { name }, set: typed), phase: phase,
            shakes: shakes, focusRequests: focusRequests, onSubmit: signUp)
          Text(hint)
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(.white.opacity(0.75))
            .multilineTextAlignment(.center)
            .frame(minHeight: 22)
          if phase == .taken {
            Button {
              go(.logIn)
            } label: {
              (Text(ProductText.nameTaken.english + " ").foregroundStyle(.white.opacity(0.8))
                + Text(ProductText.signIn.english).foregroundStyle(.white).underline())
                .font(.subheadline.weight(.semibold))
            }
            .buttonStyle(.plain)
            .accessibilityLabel(ProductText.nameTakenLabel.english)
          }
          PrimaryButton(ProductText.signUp.english, busy: busy || pending, action: signUp)
        }

        Note(problem)
        // "Got Yo? Sign In" centred as one line.
        Button { go(.logIn) } label: {
          HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(ProductText.gotYo.english).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white.opacity(0.85))
            Text(ProductText.signIn.english).font(.yo(18))
          }
          .lineLimit(1)
          .fixedSize()
        }
        .accessibilityLabel("Got Yo? Sign In")
        .padding(.top, 14)

        // Signing up agrees to the terms (App Review 1.2), said under the buttons; the link opens them in Safari.
        Text(Self.termsLine)
          .font(.footnote)
          .foregroundStyle(.white.opacity(0.7))
          .tint(.white)
          .multilineTextAlignment(.center)
          .padding(.top, 10)
          .accessibilityIdentifier("onboarding.terms")
      }
      .padding(.horizontal, 28)
    }
    .padding(.bottom, 16)
    .disabled(busy || pending)
    .task {
      guard nameField == nil else { return }
      let field = YoNameField(transport: Hub.transport)
      nameField = field
      await Attestation.forSignUp(field)
    }
    .task(id: sequence) {
      guard let seq = sequence, let field = nameField else { return }
      let checked = name
      guard (try? await Task.sleep(for: .milliseconds(300))) != nil, sequence == seq, name == checked else { return }
      let result = await Task.detached { field.check(seq: seq) }.value
      guard !Task.isCancelled, sequence == seq, name == checked else { return }
      nameState = result
      if let decision = result.decided { finish(decision) }
    }
    .task {
      guard !looked else { return }
      looked = true
      // Another phone took the account: said on the welcome and on the Sign In card over it.
      let moved = accounts.replacedHere ? AccountError.hub(.Replaced).errorDescription : nil
      accounts.replacedHere = false
      problem = moved
      // A cold start signs in from the iCloud Keychain silently (also a fresh install after a Sign Out on the old one);
      // right after a Sign Out in this run the card comes instead.
      let fresh = !Self.signedOutThisRun
      if fresh {
        busy = true
        let back = await accounts.restoreFromKeychain(fresh: true)
        busy = false
        if back { return }
      }
      // Gone meanwhile (Sign Up or Sign In tapped): nothing over it.
      guard (try? await Task.sleep(for: .seconds(0.4))) != nil, step == .welcome else { return }
      inbox.playYo()
      // Otherwise any iCloud Keychain accounts bring up the Sign In card. No passkey is looked for unasked.
      if !SyncedAccount.all().isEmpty {
        withAnimation(.smooth) { go(.logIn) }
        problem = moved
      }
    }
  }

  /// Sign In as a small card over the welcome: iCloud Keychain accounts to tap; under "Use a Key" the passkey and
  /// the recovery key.
  private var signInCard: some View {
    SignInCard(
      busy: busy, signingIn: signingIn, signingInBy: signingInBy, problem: problem,
      logIn: { item in Task { await logIn(item) } },
      passkey: { Task { await logInWithPasskey() } },
      recoveryKey: { code in Task { await logIn(code: code) } })
  }

  // MARK: Doing

  private var phase: YoNamePhase { registrationRefusal ?? nameState?.phase ?? .empty }
  private var pending: Bool { nameState?.signupPending == true }

  private var hint: String {
    switch phase {
    case .ok, .tooShort, .taken: nameState?.min.map { "Minimum \($0) characters" } ?? ""
    case .invalid:
      if let bad = nameState?.bad, !bad.isEmpty {
        bad.map { $0 == " " ? "No spaces" : "No \($0)" }.joined(separator: " · ")
      } else {
        Username.problem(name) ?? "Choose another name."
      }
    case .offline: "Yo can’t reach its server. Check your connection and try again."
    case .empty, .checking: ""
    }
  }

  private func typed(_ text: String) {
    let value = String(Username.latin(text).precomposedStringWithCanonicalMapping.prefix(16))
    guard value != name, !pending, !busy, let nameField else { return }
    name = value
    registrationRefusal = nil
    problem = nil
    sequence = nameField.typed(text: value)
    nameState = nameField.state()
  }

  private func signUp() {
    guard !busy, !pending, let nameField else { return }
    finish(registrationRefusal == nil ? nameField.signUp() : .shake)
  }

  private func finish(_ decision: YoSignUp) {
    switch decision {
    case .go:
      busy = true
      Task { await register() }
    case .wait: nameState?.signupPending = true
    case .shake:
      shakes += 1
      if name.isEmpty { focusRequests += 1 }
      UINotificationFeedbackGenerator().notificationOccurred(name.isEmpty ? .warning : .error)
    }
  }

  private func go(_ next: Step) {
    problem = nil
    if next == .logIn {
      // The card a moment later: in the same update the sheet kept drawing the stale list.
      step = .welcome
      Task { @MainActor in
        await Task.yield()
        signingInCard = true
      }
      return
    }
    step = next
  }

  private func logIn(_ item: SyncedAccount.Item) async {
    busy = true
    signingIn = item.userId
    defer {
      busy = false
      signingIn = nil
    }
    problem = nil
    do {
      try await accounts.restore(synced: item)
    } catch {
      problem = Problems.words(for: error)
    }
  }

  private func register() async {
    defer { busy = false }
    problem = nil
    do {
      try await accounts.register(name, attest: nameField?.attestation())
    } catch AccountError.taken {
      registrationRefusal = .taken
      UINotificationFeedbackGenerator().notificationOccurred(.error)
    } catch AccountError.hub(.Refused(status: 400)) {
      // The hub's refusal of the name (its least length, spec 2.2) reads as too short. `register` throws the
      // hub's errors wrapped in AccountError: a bare YoError never comes here.
      registrationRefusal = .tooShort
      UINotificationFeedbackGenerator().notificationOccurred(.error)
    } catch {
      // Its own words: too many tries (429), the keychain, no server, the phone's clock; else one plain line.
      problem = (error as? AccountError)?.errorDescription ?? "Couldn’t sign up. Check your connection and try again."
      UINotificationFeedbackGenerator().notificationOccurred(.error)
    }
  }

  /// The recovery key, already parsed by the card (`RecoveryCode.parse`).
  private func logIn(code: String) async {
    guard !busy else { return }
    busy = true
    signingInBy = .recoveryKey
    defer {
      busy = false
      signingInBy = nil
    }
    problem = nil
    do {
      try await accounts.restore(code: code)
    } catch {
      problem = Problems.words(for: error)
    }
  }

  private func logInWithPasskey() async {
    guard !busy else { return }
    busy = true
    signingInBy = .passkey
    defer {
      busy = false
      signingInBy = nil
    }
    problem = nil
    do {
      try await accounts.restoreWithPasskey()
    } catch {
      problem = (error as? AccountError)?.errorDescription ?? "We couldn’t sign you in with a passkey. Try again or use your recovery key."
    }
  }
}

/// The shared onboarding page: title and message on top, content in the middle, buttons at the bottom.
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
      // Scrolls when large text needs more room; the buttons stay.
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

/// Close for a guide sheet, in the pages' back-button row: says how the sheet goes away, not only by swiping down.
struct SheetClose: View {
  @Environment(\.dismiss) private var dismiss
  /// For when the environment's dismiss would not reach the sheet.
  var action: (() -> Void)? = nil

  var body: some View {
    Button { if let action { action() } else { dismiss() } } label: {
      Image(systemName: "xmark")
        .font(.system(size: 17, weight: .semibold))
        .frame(width: 44, height: 44)
        .glassEffect(.regular.interactive(), in: .circle)
        .contentShape(.circle)
    }
    .accessibilityLabel("Close")
    .buttonStyle(.plain)
    .padding(.trailing, 16)
    // Below the grabber strip, which takes touches for dragging.
    .padding(.top, 20)
  }
}

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

struct Note: View {
  let text: String?

  init(_ text: String?) { self.text = text }

  var body: some View {
    if let text {
      Text(text).font(.footnote.weight(.semibold)).multilineTextAlignment(.center)
    }
  }
}

private struct NameField: View {
  @Binding var name: String
  let phase: YoNamePhase
  var shakes = 0
  /// Counted up to bring the keyboard up from outside (Sign Up tapped with no name yet).
  var focusRequests = 0
  var onSubmit: () -> Void = {}
  @FocusState private var focused: Bool
  /// The name's width at full size and the field's room: a long name shrinks until it fits.
  @State private var needed = 0.0
  @State private var room = 0.0

  private static let full = 28.0

  var body: some View {
    TextField("", text: $name)
      // The placeholder is drawn over it, so VoiceOver gets the field's name here.
      .accessibilityLabel(ProductText.username.english)
      .font(.yo(fontSize))
      .tint(.white)
      .multilineTextAlignment(.center)
      .textInputAutocapitalization(.words)
      .autocorrectionDisabled()
      .submitLabel(.continue)
      .focused($focused)
      .onGeometryChange(for: Double.self) { $0.size.width } action: { room = $0 }
      .background {
        Text(name.isEmpty ? " " : name).font(.yo(Self.full)).fixedSize().hidden()
          .onGeometryChange(for: Double.self) { $0.size.width } action: { needed = $0 }
      }
      .padding(.vertical, 14)
      .padding(.horizontal, 44)  // room for the checkmark
      .background(.white.opacity(0.16), in: .capsule)
      .overlay(alignment: .trailing) {
        if phase == .checking {
          ProgressView().controlSize(.small).tint(.white).padding(.trailing, 14)
        } else if phase != .empty {
          Image(systemName: phase == .ok ? "checkmark.circle.fill" : "xmark.circle.fill")
            .font(.system(size: 24))
            .foregroundStyle(.white, phase == .ok ? .green : .red)
            .padding(.trailing, 14)
            .transition(.scale.combined(with: .opacity))
            .accessibilityLabel(phase == .ok ? "Available" : "Not available")
        }
      }
      .keyframeAnimator(initialValue: 0.0, trigger: shakes) { field, x in field.offset(x: x) } keyframes: { _ in
        for x in [-12.0, 10, -7, 4, 0] { SpringKeyframe(x, duration: 0.07) }
      }
      .onSubmit(onSubmit)
      .onChange(of: focusRequests) { focused = true }
      .overlay {
        if name.isEmpty {
          Text(ProductText.username.english)
            .font(.yo(Self.full))
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .foregroundStyle(.white.opacity(0.45))
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
  }

  /// Smaller as a long name needs, so all 16 characters fit beside the checkmark (and the cursor).
  private var fontSize: Double {
    let fits = room - 6
    guard needed > fits, fits > 0 else { return Self.full }
    return max(8, (Self.full * fits / needed).rounded(.down))
  }
}

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
              Text(blocks[i]).font(.system(.title2, design: .monospaced, weight: .semibold))
                .frame(maxWidth: .infinity)
            }
          }
        }
      }
      Text("Use this recovery key to sign in to Yo on a new device. Keep it safe and don’t share it.")
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

/// A sign-in option's sign where an account's avatar sits, at the avatar's size.
private struct OptionIcon: View {
  let systemName: String

  var body: some View {
    Image(systemName: systemName)
      .font(.system(size: 17, weight: .semibold))
      .frame(width: 36, height: 36)
      .background(.white.opacity(0.18), in: .circle)
  }
}

/// One card (design J): the iCloud Keychain accounts as white capsules, or a notice that there is none; under "Not
/// listed? Use a Key" the passkey and a recovery key field, which signs in once it holds a whole key. Its own view with
/// its own list: a sheet does not reliably redraw from its presenter's state.
private struct SignInCard: View {
  enum Way { case passkey, recoveryKey }

  let busy: Bool
  let signingIn: Data?
  let signingInBy: Way?
  let problem: String?
  let logIn: (SyncedAccount.Item) -> Void
  let passkey: () -> Void
  /// A parsed recovery key (`RecoveryCode.parse`).
  let recoveryKey: (String) -> Void
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var synced: [SyncedAccount.Item] = SyncedAccount.all()
  @State private var removing: SyncedAccount.Item?
  @State private var height: CGFloat = 300
  /// Opened or closed by hand; until then open only without an account.
  @State private var opened: Bool?
  @State private var words = ""
  /// The last key tried, so a refused one is not tried again on each change.
  @State private var tried: String?
  @State private var keyProblem: String?
  @State private var scanning = false
  @State private var picked: _PhotosUI_SwiftUI.PhotosPickerItem?

  private var open: Bool { opened ?? synced.isEmpty }
  private var motion: Animation? { reduceMotion ? nil : .smooth(duration: 0.35) }

  var body: some View {
    // Many accounts, or the key options opened, can outgrow the screen: the sheet grows to full height at most and
    // the card scrolls inside it.
    ScrollViewReader { proxy in
      ScrollView {
        content
      }
      .scrollBounceBehavior(.basedOnSize)
      .onChange(of: open) { _, now in
        guard now else { return }
        // Opened: the passkey and the key field come into view once the sheet has grown.
        Task {
          try? await Task.sleep(for: .milliseconds(400))
          withAnimation(motion) { proxy.scrollTo(Self.options, anchor: .bottom) }
        }
      }
    }
    .foregroundStyle(.white)
    .task {
      for _ in 0..<5 where synced.isEmpty {
        try? await Task.sleep(for: .milliseconds(300))
        let found = SyncedAccount.all()
        if !found.isEmpty { withAnimation(motion) { synced = found } }
      }
    }
    .onChange(of: words) { _, typed in
      keyProblem = nil
      // A whole key signs in by itself; one that only looks whole says so.
      if let code = RecoveryCode.parse(typed) {
        if code != tried { submit(code) }
      } else if typed.filter({ $0.isASCII && ($0.isLetter || $0.isNumber) }).count >= 64 {
        keyProblem = Self.mismatch
      }
    }
    .sheet(isPresented: $scanning) {
      QRScanner { text in
        scanning = false
        words = text
        submitTyped()
      }
      .ignoresSafeArea()
      .appObjects()
    }
    .onChange(of: picked) { _, item in
      guard let item else { return }
      picked = nil
      Task {
        guard let data = try? await item.loadTransferable(type: Data.self), let image = UIImage(data: data),
          let text = RecoveryQR.read(image)
        else { return keyProblem = "We couldn’t read a recovery key from this photo. Try another photo or enter the key." }
        words = text
        submitTyped()
      }
    }
    .alert(
      "Remove \(removing?.username ?? "")?",
      isPresented: $removing.isPresent
    ) {
      Button("Remove", role: .destructive) {
        if let item = removing {
          SyncedAccount.delete(item.userId)
          withAnimation(motion) { synced = SyncedAccount.all() }
        }
        removing = nil
      }
      Button("Cancel", role: .cancel) { removing = nil }
    } message: {
      Text("It’s taken out of your iCloud Keychain on all your devices. Without a passkey or recovery key, you can’t sign in to it again.")
    }
    .presentationDetents([.height(height)])
    .presentationBackground(Color.amethyst)
    .presentationDragIndicator(.visible)
    .preferredColorScheme(.dark)
  }

  private static let options = "options"

  /// The card itself; its height sets the sheet's (a detent above the screen is held at full height).
  private var content: some View {
    VStack(spacing: 12) {
      Text(ProductText.signIn.english)
        .font(.yo(28))
        .padding(.top, 28)
        .padding(.bottom, 2)
      if synced.isEmpty {
        Text(ProductText.noAccountInICloud.english)
          .font(.subheadline.weight(.semibold))
          .multilineTextAlignment(.center)
          .opacity(0.85)
          .padding(.horizontal, 16)
          .frame(maxWidth: .infinity, minHeight: 44)
          .background(.white.opacity(0.16), in: .capsule)
      } else {
        ForEach(synced) { account($0) }
      }
      Button {
        withAnimation(motion) { opened = !open }
      } label: {
        // As the welcome's "Got Yo? Sign In": the quiet question, then the action.
        HStack(alignment: .firstTextBaseline, spacing: 6) {
          Text(ProductText.notListed.english).font(.system(size: 15, weight: .semibold))
            .foregroundStyle(.white.opacity(0.85))
          Text(ProductText.useAKey.english).font(.yo(18))
        }
        .lineLimit(1)
        .fixedSize()
        .frame(minHeight: 36)
        .contentShape(.rect)
      }
      .buttonStyle(.plain)
      .accessibilityElement(children: .ignore)
      .accessibilityLabel("Not listed? Use a key: passkey or recovery key")
      .accessibilityAddTraits(.isButton)
      .accessibilityValue(open ? "Expanded" : "Collapsed")
      .accessibilityHint(open ? "Collapses" : "Expands")
      if open {
        VStack(spacing: 12) {
          passkeyButton
          HStack(spacing: 10) {
            line
            Text(ProductText.or.english).font(.subheadline.weight(.semibold)).opacity(0.7)
            line
          }
          .accessibilityHidden(true)
          keyField
        }
        .id(Self.options)
        .transition(reduceMotion ? .opacity : .opacity.combined(with: .move(edge: .top)))
      }
      Note(keyProblem ?? problem)
    }
    .padding(.horizontal, 24)
    .padding(.bottom, 12)
    .fixedSize(horizontal: false, vertical: true)
    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { new in
      if abs(new - height) > 1 { height = new }
    }
  }

  private static let mismatch = "That doesn’t match a recovery key. Check it and try again."

  private func account(_ item: SyncedAccount.Item) -> some View {
    Button { logIn(item) } label: {
      HStack(spacing: 12) {
        Avatar(name: item.username, size: 36)
        Text(item.username).font(.yo(22)).lineLimit(1).foregroundStyle(.black)
        Spacer(minLength: 0)
        if signingIn == item.userId {
          ProgressView().tint(.black)
        }
      }
      .padding(.horizontal, 12)
      .frame(maxWidth: .infinity, minHeight: 56)
      .background(.white, in: .capsule)
      .contentShape(.capsule)
    }
    .buttonStyle(.plain)
    .disabled(busy)
    .accessibilityLabel("Sign in as \(item.username)")
    .contextMenu {
      Button("Remove from iCloud Keychain", systemImage: "trash", role: .destructive) { removing = item }
    }
    .accessibilityAction(named: "Remove from iCloud Keychain") { removing = item }
  }

  private var passkeyButton: some View {
    Button {
      keyProblem = nil
      passkey()
    } label: {
      // The accounts' capsule, in glass: icon where the avatar sits, the name's size, the same height.
      HStack(spacing: 12) {
        OptionIcon(systemName: "person.badge.key.fill")
        Text(ProductText.passkey.english).font(.yo(22)).lineLimit(1)
        Spacer(minLength: 0)
        if signingInBy == .passkey { ProgressView().tint(.white) }
      }
      .foregroundStyle(.white)
      .padding(.horizontal, 12)
      .frame(maxWidth: .infinity, minHeight: 56)
      .background(.white.opacity(0.16), in: .capsule)
      .contentShape(.capsule)
    }
    // As the key field beside it: the passkey and the recovery key are two equal options.
    .buttonStyle(.plain)
    .disabled(busy)
  }

  private var line: some View {
    Rectangle().fill(.white.opacity(0.4)).frame(height: 1)
  }

  /// One line: paste or type, scan, or a photo of the card; with text in it, a button to sign in.
  private var keyField: some View {
    HStack(spacing: 6) {
      OptionIcon(systemName: "key.fill").padding(.trailing, 6).accessibilityHidden(true)
      TextField(
        "", text: $words,
        prompt: Text(ProductText.recoveryKey.english).font(.yo(22))
          .foregroundStyle(.white.opacity(0.45)))
        .font(.system(.title2, design: .monospaced, weight: .semibold))
        .lineLimit(1)
        .tint(.white)
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .keyboardType(.asciiCapable)
        .submitLabel(.go)
        .onSubmit(submitTyped)
        .accessibilityLabel(ProductText.recoveryKey.english)
      if signingInBy == .recoveryKey {
        ProgressView().tint(.white).frame(width: 36, height: 36)
      } else if !words.isEmpty {
        Button(action: submitTyped) {
          Image(systemName: "arrow.right")
            .font(.body.weight(.bold))
            .foregroundStyle(.black)
            .frame(width: 36, height: 36)
            .background(.white, in: .circle)
            .contentShape(.circle)
        }
        .buttonStyle(.plain)
        .disabled(busy)
        .accessibilityLabel(ProductText.signIn.english)
      } else {
        if QRScanner.isAvailable {
          Button { scanning = true } label: {
            Image(systemName: "camera").font(.body.weight(.semibold)).frame(width: 36, height: 36)
          }
          .buttonStyle(.plain)
          .glassEffect(.regular.interactive(), in: .circle)
          .accessibilityLabel("Scan recovery key")
        }
        PhotosPicker(selection: $picked, matching: .images) {
          Image(systemName: "photo").font(.body.weight(.semibold)).frame(width: 36, height: 36)
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .circle)
        .accessibilityLabel("Choose recovery key photo")
      }
    }
    .padding(.leading, 12)
    .padding(.trailing, 10)
    .frame(minHeight: 56)
    .background(.white.opacity(0.16), in: .capsule)
    .disabled(busy)
  }

  /// The field's text as it is: a key signs in, anything else says it does not match.
  private func submitTyped() {
    guard let code = RecoveryCode.parse(words) else { return keyProblem = Self.mismatch }
    submit(code)
  }

  private func submit(_ code: String) {
    guard !busy else { return }
    tried = code
    keyProblem = nil
    recoveryKey(code)
  }
}
