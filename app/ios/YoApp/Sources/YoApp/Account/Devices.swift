// Multi-device (spec 2.5): Settings › Devices lists the account's devices; one scanner reads a friend's code or a new
// device's link code; a new device shows its link code under "Got Yo? Sign In". Everything about linking shows only on
// a hub that adds devices (`DeviceLinking.check`).
import SwiftUI
import UIKit
import YoCore

/// Whether the hub adds devices (`YO_ADD_DEVICES`): `/.well-known/yo-hub` says `"link": true`. Asked once per run;
/// a failed ask (no connection) keeps the last answer and asks again next time.
@MainActor enum DeviceLinking {
  private static let key = "devices.linking"
  /// The last answer, kept for when no answer comes (no connection).
  private(set) static var on: Bool? = UserDefaults.standard.object(forKey: key) as? Bool
  private static var asked = false

  static func check() async -> Bool {
    if asked, let on { return on }
    var request = URLRequest(url: Hub.url.appending(path: ".well-known/yo-hub"), timeoutInterval: 8)
    request.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
    if let (data, response) = try? await HubTransport.publicData(for: request),
       let status = (response as? HTTPURLResponse)?.statusCode {
      if status == 200, let features = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
        remember(features["link"] as? Bool == true)
      } else if status == 404 {
        remember(false)  // a hub from before the flag
      }
    }
    return on ?? false
  }

  private static func remember(_ value: Bool) {
    asked = true
    on = value
    UserDefaults.standard.set(value, forKey: key)
  }
}

/// One of the account's devices as the pages show it.
struct OwnDevice: Sendable, Identifiable, Equatable {
  let number: UInt8
  let lastSeen: Date
  let app: String
  var id: UInt8 { number }

  init(_ device: YoDevice) {
    number = device.number
    lastSeen = Date(timeIntervalSince1970: Double(device.lastSeenMs) / 1000)
    app = device.app
  }

  private var platform: String { app.split(separator: "/").first.map { $0.lowercased() } ?? "" }

  var name: String {
    switch platform {
    case "ios": "iPhone"
    case "android": "Android"
    case "": "Device \(number)"
    default: "Computer"
    }
  }

  var symbol: String {
    switch platform {
    case "ios", "android": "iphone"
    case "cli", "yo": "terminal.fill"
    default: "laptopcomputer"
    }
  }

  var color: Color {
    switch platform {
    case "ios": .blue
    case "android": .green
    default: .gray
    }
  }

  /// The hub keeps last seen to the hour and writes it once a day (spec 1), so only the day is told (as Signal's
  /// "Last active"): "55 minutes ago" for a device linked a minute ago was the hour's rounding.
  var lastSeenText: String {
    let calendar = Calendar.current
    if calendar.isDateInToday(lastSeen) { return "active today" }
    if calendar.isDateInYesterday(lastSeen) { return "active yesterday" }
    return "active " + lastSeen.formatted(date: .abbreviated, time: .omitted)
  }
}

extension AccountStore {
  func ownDevices() async throws -> [OwnDevice] {
    try await ownDevicesAndMine().devices
  }

  /// The account's devices and this one's number, off the main actor (the client may be busy receiving).
  func ownDevicesAndMine() async throws -> (devices: [OwnDevice], mine: UInt8) {
    guard let client else { throw AccountError.hub(.State) }
    return try await Hub.run { (try client.ownDevices().map(OwnDevice.init), client.deviceNumber()) }
  }

  func removeDevice(_ number: UInt8) async throws {
    guard let client else { throw AccountError.hub(.State) }
    try await Hub.run { try client.removeDevice(number: number) }
  }

  func approveLink(_ offer: YoLinkOffer) async throws {
    guard let client else { throw AccountError.hub(.State) }
    try await Hub.run { try client.linkApprove(offer: offer) }
  }
}

/// Settings › Devices: every device of the account, this one marked; the others can be removed.
@MainActor
struct DevicesPage: View {
  @Environment(AccountStore.self) private var accounts
  @State private var devices: [OwnDevice] = []
  @State private var mine: UInt8?
  @State private var removing: OwnDevice?
  @State private var scanning = false
  @State private var problem: String?

  var body: some View {
    Form {
      Section {
        Button { scanning = true } label: {
          ChevronRow {
            DisclosureChevron()
          } label: {
            Label("Add Device", systemImage: "qrcode").labelStyle(Tile(.amethyst))
          }
        }
        .tint(Color(.label))
        .accessibilityIdentifier("devices.add")
      }
      Section {
        ForEach(devices) { device in
          HStack {
            Label {
              VStack(alignment: .leading, spacing: 2) {
                Text(device.name)
                Text(device.number == mine ? "This Device" : device.lastSeenText)
                  .font(.footnote)
                  .foregroundStyle(.secondary)
              }
            } icon: {
              Image(systemName: device.symbol)
            }
            .labelStyle(Tile(device.color))
            Spacer(minLength: 0)
            if device.number != mine {
              Button("Remove", role: .destructive) { removing = device }
                .buttonStyle(.borderless)
            }
          }
        }
      } header: {
        Text("\(devices.count) of 16 Devices")
      }
    }
    .navigationTitle("Devices")
    .accessibilityIdentifier("devices.page")
    .refreshable { await load() }
    .task { await load() }
    .sheet(isPresented: $scanning, onDismiss: { Task { await load() } }) {
      // Half height: the camera in a card; "linked" shows in the same sheet.
      NavigationStack { ScanPage(close: { scanning = false }, done: { scanning = false }) }
        .presentationDetents([.medium])
        .presentationBackground(Color(.systemGroupedBackground))  // opaque, as Settings: no see-through glass
        .systemAppearance().appObjects()
    }
    .alert("Remove \(removing?.name ?? "")?", isPresented: $removing.isPresent) {
      Button("Remove", role: .destructive) {
        if let device = removing { Task { await remove(device) } }
      }
      Button("Cancel", role: .cancel) { removing = nil }
    } message: {
      Text("It gets signed out and can’t send or get Yos until it’s linked again.")
    }
    .problemAlert($problem)
  }

  private func load() async {
    do { (devices, mine) = try await accounts.ownDevicesAndMine() } catch { problem = Problems.words(for: error) }
  }

  private func remove(_ device: OwnDevice) async {
    do {
      try await accounts.removeDevice(device.number)
      withAnimation { devices.removeAll { $0.number == device.number } }
    } catch {
      problem = Problems.words(for: error)
    }
  }
}

/// The one scanner: a friend's code adds the friend, a new device's link code links it (after one confirming tap).
@MainActor
struct ScanPage: View {
  /// Closes the scanner with nothing done (nil: the sheet around it has its own close button).
  var close: (() -> Void)?
  /// Closes it and what opened it: a friend is added on the list, or a device linked.
  var done: () -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var linking = false
  @State private var pasting = false
  @State private var pasted = ""
  @State private var problem: String?
  @State private var friend: String?
  @State private var offer: YoLinkOffer?
  @State private var linked = false
  @State private var scannerKey = 0

  var body: some View {
    Form {
      // While a code is pasted the camera steps aside, so the field and Link stay above the keyboard.
      if !pasting {
      Section {
        Group {
          if QRScanner.isAvailable {
            QRScanner { read($0) }.id(scannerKey)
          } else {
            ContentUnavailableView("No Camera", systemImage: "camera")
          }
        }
        .frame(height: 300)
        .listRowInsets(EdgeInsets())
      }
      }
      if let problem {
        Section { Text(problem).foregroundStyle(.red) }
      }
      // The camera alone; a code is pasted only in a debug build (the simulator has no camera).
      #if DEBUG
      Section {
        Button { withAnimation { pasting.toggle() } } label: {
          Label("Paste Code Instead", systemImage: "doc.on.clipboard").labelStyle(Tile(.blue))
        }
        .tint(Color(.label))
        .accessibilityIdentifier("scan.paste")
        if pasting {
          TextField("Code", text: $pasted)
            .font(.footnote.monospaced())
            .submitLabel(.go)
            .onSubmit { read(pasted) }
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .accessibilityIdentifier("scan.code")
          Button("Link") { read(pasted) }
            .disabled(pasted.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .accessibilityIdentifier("scan.link")
        }
      }
      #endif
    }
    .navigationTitle("Scan")
    .navigationBarTitleDisplayMode(.inline)
    .toolbar {
      if let close { ToolbarItem(placement: .topBarLeading) { Button(role: .close) { close() } } }
    }
    .navigationDestination(isPresented: $linked) { LinkedPage(done: done) }
    .task { linking = await DeviceLinking.check() }
    .alert("Add \(friend ?? "")?", isPresented: $friend.isPresent) {
      Button("Add") {
        Invite.shared.pending = friend
        friend = nil
        done()
      }
      Button("Cancel", role: .cancel) { friend = nil; scannerKey += 1 }
    } message: {
      Text("\(friend ?? "") can Yo you and you can Yo \(friend ?? "").")
    }
    .sheet(item: Binding(get: { offer.map(OfferBox.init) }, set: { offer = $0?.offer })) { box in
      LinkConfirmSheet(offer: box.offer) {
        offer = nil
        linked = true
      } cancel: {
        offer = nil
        scannerKey += 1
      }
      .systemAppearance().appObjects()
    }
  }

  private func read(_ raw: String) {
    let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    problem = nil
    if text.lowercased().hasPrefix("yo-link:") {
      guard linking, let found = try? linkOffer(text: text) else {
        problem = "That isn’t a current Yo link code."
        scannerKey += 1
        return
      }
      offer = found
    } else if let url = URL(string: text), let name = Invite.name(in: url) {
      friend = name
    } else {
      problem = linking ? "That isn’t a Yo code." : "That isn’t a friend’s code."
      scannerKey += 1
    }
  }
}

private struct OfferBox: Identifiable {
  let offer: YoLinkOffer
  var id: String { offer.id }
}

/// One confirming tap before this account goes to the device that showed the code.
@MainActor
struct LinkConfirmSheet: View {
  let offer: YoLinkOffer
  var linked: () -> Void
  var cancel: () -> Void
  @Environment(AccountStore.self) private var accounts
  @State private var busy = false
  @State private var problem: String?

  var body: some View {
    VStack(spacing: 16) {
      Image(systemName: "iphone")
        .font(.system(size: 44))
        .foregroundStyle(Color.amethyst)
        .padding(.top, 28)
      Text("Link this device?").font(.title2.bold())
      Text(
        "This device gets access to your account: your name, friends and Yos. Only continue if the code is on a device in front of you."
      )
      .multilineTextAlignment(.center)
      .foregroundStyle(.secondary)
      if let problem { Text(problem).font(.footnote).foregroundStyle(.red) }
      Spacer(minLength: 0)
      HStack(spacing: 12) {
        Button { cancel() } label: { Text("Cancel").frame(maxWidth: .infinity, minHeight: 40) }
          .buttonStyle(.glass)
        Button { Task { await approve() } } label: {
          Group { if busy { ProgressView() } else { Text("Link") } }.frame(maxWidth: .infinity, minHeight: 40)
        }
        .buttonStyle(.glassProminent)
        .tint(.amethyst)
        .disabled(busy)
        .accessibilityIdentifier("link.confirm")
      }
    }
    .padding(.horizontal, 24)
    .padding(.bottom, 16)
    .presentationDetents([.medium])
    .presentationDragIndicator(.visible)
    .interactiveDismissDisabled(busy)
  }

  private func approve() async {
    busy = true
    defer { busy = false }
    do {
      try await accounts.approveLink(offer)
      linked()
    } catch YoError.Refused(let status) where status == 404 {
      problem = "The code expired."
    } catch {
      problem = Problems.words(for: error)
    }
  }
}

/// After the tap: the new device has the account.
struct LinkedPage: View {
  var done: () -> Void
  @State private var showingDevices = false

  var body: some View {
    Form {
      Section {
        VStack(spacing: 12) {
          Image(systemName: "checkmark.circle.fill").font(.system(size: 56)).foregroundStyle(.green)
          Text("New device is linked").font(.title2.bold())
          Text("It now has your friends and Yos. You can remove it any time in Settings › Devices.")
            .multilineTextAlignment(.center)
            .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
        .listRowBackground(Color.clear)
      }
      Section {
        NavigationLink {
          DevicesPage()
        } label: {
          Label("Show Devices", systemImage: "iphone").labelStyle(Tile(.blue))
        }
      }
    }
    .navigationBarBackButtonHidden()
    .toolbar {
      ToolbarItem(placement: .topBarLeading) { Button(role: .close) { done() } }
    }
  }
}

/// A bar on top of the list after another device joined the account, the Yo way (as the notifications bar): a
/// tap opens Settings › Devices; × or a swipe sideways lets it go.
struct DeviceLinkedBar: View {
  /// Camera mode: no bar.
  var hidden = false
  @Environment(Inbox.self) private var inbox
  @Environment(\.rowMetrics) private var rowMetrics
  @State private var showingDevices = false
  @State private var drag: CGFloat = 0

  var body: some View {
    VStack(spacing: 0) {
      if inbox.deviceLinked && !hidden {
        ZStack(alignment: .trailing) {
          Button { showingDevices = true } label: {
            Text("New Device Linked")
              .font(.yo(rowMetrics.font))
              .foregroundStyle(.white)
              .lineLimit(1)
              .minimumScaleFactor(0.5)
              .padding(.horizontal, 56)
              .frame(maxWidth: .infinity)
              .frame(height: rowMetrics.baseHeight)
              .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .accessibilityHint("Opens your devices")
          Button("Close", systemImage: "xmark") { dismiss() }
            .labelStyle(.iconOnly)
            .font(.title3.weight(.bold))
            .foregroundStyle(.white)
            .frame(width: 44, height: 44)
            .padding(.trailing, 8)
            .accessibilityIdentifier("devices.bar.close")
        }
        .background(Color(hex: 0x2980B9))  // belize hole, one of the bars' own colours
        .offset(x: drag)
        .gesture(
          DragGesture(minimumDistance: 20)
            .onChanged { drag = $0.translation.width }
            .onEnded { value in
              if abs(value.translation.width) > 80 { dismiss() } else { withAnimation(.snappy) { drag = 0 } }
            }
        )
        .transition(.move(edge: .top).combined(with: .opacity))
        .accessibilityIdentifier("devices.bar")
        .sheet(isPresented: $showingDevices, onDismiss: dismiss) {
          NavigationStack {
            DevicesPage().toolbar {
              ToolbarItem(placement: .topBarLeading) { Button(role: .close) { showingDevices = false } }
            }
          }
          .presentationDetents([.medium, .large])
          .systemAppearance().appObjects()
        }
      }
    }
  }

  private func dismiss() {
    withAnimation(.smooth) { inbox.deviceLinked = false }
    drag = 0
  }
}

/// A new device: its link code for the other device to scan, then the account it hands over.
@MainActor
struct LinkThisDevicePage: View {
  var close: () -> Void
  @Environment(AccountStore.self) private var accounts
  private enum Phase: Equatable { case starting, showing, linking, replace([OwnDevice]), failed(String) }
  @State private var phase = Phase.starting
  @State private var request: YoLinkRequest?
  @State private var linked: YoLinked?
  @State private var until = Date.now.addingTimeInterval(600)
  @State private var showCode = false
  @State private var pick: UInt8?
  @State private var busy = false
  @State private var problem: String?

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(spacing: 16) {
          content
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 16)
        .frame(maxWidth: .infinity)
      }
      .scrollBounceBehavior(.basedOnSize)
      .background(Color.amethyst.ignoresSafeArea())
      .toolbar {
        ToolbarItem(placement: .topBarLeading) { Button(role: .close) { close() } }
      }
    }
    .foregroundStyle(.white)
    .preferredColorScheme(.dark)
    .task { await run() }
  }

  @ViewBuilder private var content: some View {
    switch phase {
    case .starting:
      ProgressView().padding(.top, 80)
    case .showing:
      Text("Link This Device").font(.yo(28)).multilineTextAlignment(.center)
      Text("On your other device open Yo › Settings › Devices › Add Device and scan this code.")
        .font(.subheadline)
        .multilineTextAlignment(.center)
        .opacity(0.85)
      if let request {
        RecoveryQRView(key: request.text, size: 220)
          .accessibilityLabel("QR code to link this device")
        HStack(spacing: 8) {
          ProgressView()
          Text("Waiting for your other device… · expires in ") + Text(until, style: .timer)
        }
        .font(.footnote.weight(.semibold))
        .opacity(0.85)
        if showCode {
          Text(request.text)
            .font(.caption.monospaced())
            .textSelection(.enabled)
            .padding(10)
            .background(.white.opacity(0.2), in: .rect(cornerRadius: 14))
            .accessibilityIdentifier("link.code")
        }
        PrimaryButton(showCode ? "Copy Code" : "Can’t Scan? Show Code") {
          if showCode { UIPasteboard.general.string = request.text } else { showCode = true }
        }
        .accessibilityIdentifier("link.showCode")
      }
    case .linking:
      ProgressView().padding(.top, 80)
      Text("Linking…").font(.yo(28))
      Text("Getting your account from your other device.").font(.subheadline).opacity(0.85)
    case .replace(let devices):
      Text("16 Devices").font(.yo(28))
      Text("Your account has the most devices it can have. Choose one to remove; it gets signed out.")
        .font(.subheadline)
        .multilineTextAlignment(.center)
        .opacity(0.85)
      VStack(spacing: 8) {
        ForEach(devices) { device in
          Button { pick = device.number } label: {
            HStack(spacing: 12) {
              Label {
                VStack(alignment: .leading, spacing: 2) {
                  Text(device.name).foregroundStyle(.black)
                  Text(device.lastSeenText).font(.footnote).foregroundStyle(.secondary)
                }
              } icon: {
                Image(systemName: device.symbol)
              }
              .labelStyle(Tile(device.color))
              Spacer(minLength: 0)
              if pick == device.number { Image(systemName: "checkmark").foregroundStyle(Color.amethyst) }
            }
            .padding(.horizontal, 16)
            .frame(minHeight: 52)
            .background(.white, in: .capsule)
          }
          .buttonStyle(.plain)
        }
      }
      Note(problem)
      PrimaryButton("Remove and Link", busy: busy) { Task { await replace() } }
        .disabled(pick == nil || busy)
    case .failed(let text):
      Text("Link This Device").font(.yo(28))
      Note(text)
      PrimaryButton("Try Again") { Task { await run() } }
    }
  }

  private func run() async {
    phase = .starting
    showCode = false
    let transport = Hub.transport
    let opened: YoLinkRequest
    do {
      opened = try await Hub.run(reportErrors: false) { try linkStart(transport: transport) }
    } catch {
      phase = .failed((error as? YoError).map { AccountError.hub($0).errorDescription ?? "" } ?? "No connection. Try again when connected.")
      return
    }
    request = opened
    until = .now.addingTimeInterval(600)
    phase = .showing
    while !Task.isCancelled, Date.now < until {
      do {
        if let got = try await Hub.run(reportErrors: false, { try linkWait(transport: transport, request: opened, waitS: 25) }) {
          linked = got
          phase = .linking
          await register(got, replace: nil)
          return
        }
      } catch YoError.Refused(let status) where status == 404 {
        break
      } catch {
        try? await Task.sleep(for: .seconds(2))
      }
    }
    if !Task.isCancelled { phase = .failed("The code expired. Make a new one.") }
  }

  private func register(_ got: YoLinked, replace: UInt8?) async {
    do {
      try await accounts.link(got, replace: replace)
      close()
    } catch AccountError.hub(.DeviceLimit(let devices)) {
      pick = nil
      phase = .replace(devices.map(OwnDevice.init))
    } catch {
      if replace != nil {
        problem = Problems.words(for: error)
      } else {
        phase = .failed(Problems.words(for: error))
      }
    }
  }

  private func replace() async {
    guard let linked, let pick else { return }
    busy = true
    defer { busy = false }
    await register(linked, replace: pick)
  }
}
