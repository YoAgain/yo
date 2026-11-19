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
    case welcome, logIn, logInWithWords
  }

  /// Set by Sign Out for this run only: the welcome then shows the Sign In card instead of signing in again.
  @MainActor static var signedOutThisRun = false

  @State private var step = Step.welcome
  @State private var name = ""
  @State private var busy = false
  @State private var problem: String?
  /// The silent look for an iCloud Keychain account happens once, not each time the welcome comes back.
  @State private var looked = false
  @State private var tooShort = false
  /// Names the hub refused as too short (its `YO_NAME_MIN`), by skeleton.
  @State private var short: Set<String> = []
  @State private var shakes = 0
  @State private var focusRequests = 0
  /// The name once typing paused on it: only then its taken or free mark.
  @State private var settled = ""
  @State private var checked: [String: Bool] = [:]
  @State private var signingIn: Data?
  @State private var signingInCard = false
  /// The carousel's height, and the room it gets above the form (less with the keyboard up).
  @State private var introHeight = 0.0
  @State private var introRoom = 0.0

  var body: some View {
    ZStack {
      Color.amethyst.ignoresSafeArea()
      Group {
        switch step {
        case .welcome, .logIn: welcome
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
        // Taken or free shows once typing paused (500 ms), never for a name passed on the way ("Peter5").
        let free = name == settled ? checked[Username.skeleton(name)] : nil
        let taken = !name.isEmpty && free == false
        ZStack {
          Text(ProductText.longerName.english)
            .font(.subheadline.weight(.semibold))
            .opacity(tooShort ? 1 : 0)
            .accessibilityHidden(!tooShort)
          Button {
            go(.logIn)
          } label: {
            (Text(ProductText.nameTaken.english + " ").foregroundStyle(.white.opacity(0.8))
              + Text(ProductText.signIn.english).foregroundStyle(.white).underline())
              .font(.subheadline.weight(.semibold))
          }
          .buttonStyle(.plain)
          .opacity(taken ? 1 : 0)
          .disabled(!taken)
          .accessibilityHidden(!taken)
          .accessibilityLabel(ProductText.nameTakenLabel.english)
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
          // Too short shows only once typing paused, never on the first letter.
          guard (try? await Task.sleep(for: .seconds(1.5))) != nil else { return }
          if short.contains(Username.skeleton(name)) { tooShort = true }
        }
        VStack(spacing: 10) {
          NameField(name: $name, mark: name.isEmpty ? nil : Username.problem(name) == nil && free == true ? .available
            : taken || tooShort ? .taken : nil,
            shakes: shakes, focusRequests: focusRequests, onSubmit: { Task { await register() } })
          // Always tappable: with no name yet it shakes the field and brings the keyboard up.
          PrimaryButton(ProductText.signUp.english, busy: busy) {
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
      }
      .padding(.horizontal, 28)
    }
    .padding(.bottom, 16)
    .disabled(busy)
    .task {
      guard !looked else { return }
      looked = true
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
      if !SyncedAccount.all().isEmpty { withAnimation(.smooth) { go(.logIn) } }
    }
  }

  /// Sign In as a small card over the welcome: iCloud Keychain accounts to tap, then passkey and recovery key.
  private var signInCard: some View {
    SignInCard(
      busy: busy, signingIn: signingIn, problem: problem,
      logIn: { item in Task { await logIn(item) } },
      passkey: { Task { await logInWithPasskey() } },
      recoveryKey: {
        signingInCard = false
        go(.logInWithWords)
      })
  }

  // MARK: Doing

  /// Asks the hub whether a name is free once typing paused (one question per pause, not per letter).
  private func check(_ typed: String) async {
    let skeleton = Username.skeleton(typed)
    guard Username.problem(typed) == nil, checked[skeleton] == nil,
      (try? await Task.sleep(for: .milliseconds(500))) != nil
    else { return }
    let transport = Hub.transport
    do {
      let free = try await Hub.run({ try usernameFree(transport: transport, username: typed) })
      // Taken by this phone's own lost try (its key is kept): Continue takes it (`NewAccountKey`).
      checked[skeleton] = free || NewAccountKey.has(typed)
    } catch YoError.Refused(status: 400) {
      // The minimum length is the hub's switch (spec 3.3); shown after the 1.5 s pause.
      short.insert(skeleton)
    } catch {}
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
    let asked = name  // the field may change while the hub answers
    do {
      try await accounts.register(asked)
    } catch AccountError.taken {
      checked[Username.skeleton(asked)] = false
      UINotificationFeedbackGenerator().notificationOccurred(.error)
    } catch {
      // A refusal (400: too short) reads as too short; anything else as one plain line.
      if case YoError.Refused = error {
        short.insert(Username.skeleton(asked))
        tooShort = true
      } else {
        problem = "Couldn’t sign up. Check your connection and try again."
      }
      UINotificationFeedbackGenerator().notificationOccurred(.error)
    }
  }

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
  var mark: Mark?
  enum Mark { case available, taken }
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

/// Sign In by recovery key: pasted, typed, or read from the card's QR code by camera or from Photos.
private struct LogIn: View {
  let back: () -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var words = ""
  @State private var problem: String?
  @State private var scanning = false
  @State private var picked: _PhotosUI_SwiftUI.PhotosPickerItem?
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
          Button { scanning = true } label: {
            Label("Scan", systemImage: "qrcode.viewfinder").font(.subheadline.weight(.semibold))
              .frame(maxWidth: .infinity, minHeight: 30)
          }
          .buttonStyle(.glass)
        }
        PhotosPicker(selection: $picked, matching: .images) {
          Label("Photos", systemImage: "photo").font(.subheadline.weight(.semibold))
            .frame(maxWidth: .infinity, minHeight: 30)
        }
        .buttonStyle(.glass)
      }
    } bottom: {
      PrimaryButton(ProductText.signIn.english, busy: busy, action: logIn)
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

/// Its own view with its own list: a sheet does not reliably redraw from its presenter's state.
private struct SignInCard: View {
  let busy: Bool
  let signingIn: Data?
  let problem: String?
  let logIn: (SyncedAccount.Item) -> Void
  let passkey: () -> Void
  let recoveryKey: () -> Void
  @State private var synced: [SyncedAccount.Item] = SyncedAccount.all()
  @State private var removing: SyncedAccount.Item?
  @State private var height: CGFloat = 300

  var body: some View {
    VStack(spacing: 14) {
      Text(ProductText.signIn.english)
        .font(.yo(28))
        .padding(.top, 28)
      if !synced.isEmpty {
        VStack(spacing: 10) {
          ForEach(synced) { item in
            Button { logIn(item) } label: {
              HStack(spacing: 12) {
                Avatar(name: item.username, size: 36)
                Text(item.username).font(.yo(20)).lineLimit(1).foregroundStyle(.white)
                Spacer(minLength: 0)
                if signingIn == item.userId {
                  ProgressView().tint(.white)
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
            .contextMenu {
              Button("Remove from iCloud Keychain", systemImage: "trash", role: .destructive) { removing = item }
            }
            .accessibilityAction(named: "Remove from iCloud Keychain") { removing = item }
          }
        }
        Text("or").font(.yo(17)).opacity(0.75)
      } else {
        Color.clear.frame(height: 18)
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
    .task {
      for _ in 0..<5 where synced.isEmpty {
        try? await Task.sleep(for: .milliseconds(300))
        synced = SyncedAccount.all()
      }
    }
    .alert(
      "Remove \(removing?.username ?? "")?",
      isPresented: $removing.isPresent
    ) {
      Button("Remove", role: .destructive) {
        if let item = removing {
          SyncedAccount.delete(item.userId)
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
    .presentationDragIndicator(.hidden)
    .preferredColorScheme(.dark)
  }
}
