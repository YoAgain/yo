import SwiftUI
import UserNotifications
import YoCore

/// Compact summaries with cycling diagnostic values.
struct StatusPage: View {
  var summaryOnly = false
  /// Just a coloured dot (its word for VoiceOver), for the Account title.
  var wordOnly = false
  @Environment(Friends.self) private var friends
  @Environment(AccountStore.self) private var accounts
  @Environment(Inbox.self) private var inbox
  @State private var serverExpanded = false
  @State private var yosView = 0
  @State private var connectionView = 0
  @State private var showingKeys = false
  @State private var syncMilliseconds: UInt64?
  @State private var storage: LocalStorageUsage?
  @State private var storageValues = ["Calculating…", "—", "—", "—"]
  @State private var usage = ["Checking…", "Checking…", "Checking…", "Checking…"]
  @State private var syncTime = "No successful sync"
  @State private var mostYos = "—"
  @State private var tally: EncryptionTally?
  /// Read once when the page opens (and on a change), never in the body: reading touches UserDefaults.
  @State private var crashes: [CrashReports.Record] = []
  @State private var status: SecurityStatus?
  @State private var lastAnswer: Date?
  @State private var syncChecking = true
  @State private var syncSucceeded: Bool?
  @State private var reachable: Bool?
  @State private var milliseconds: Int?
  @State private var allowed: Bool?
  @State private var accepted: Bool?
  @State private var savedAt: Date?
  @State private var backupAvailable = false
  @AppStorage("stats.sent") private var sent = 0
  @AppStorage("stats.received") private var received = 0

  @State private var keysExpanded = false
  @State private var crashesOn = CrashReports.on

  private var keyChecks: [Check] {
    guard let status else { return [.waiting, .waiting, .waiting, .waiting] }
    return [
      status.hubSigned ? .ok : .waiting,
      status.prekeysAtHub >= 16
        ? .ok : status.prekeysAtHub == 0 && !status.refillPending ? .problem : .waiting,
      status.signedPrekeyDays <= 8 ? .ok : .waiting,
      status.pqSessions == status.devices ? .ok : .waiting,
    ]
  }

  private var overallResult: (check: Check, word: String?) {
    if reachable == false { return (.waiting, "Offline") }
    if serverCheck == .waiting { return (.waiting, "Slow") }
    if accepted == false { return (.waiting, "Update") }
    if allowed == false || !Push.shared.registered(for: accounts.client) {
      return (.waiting, "No Push")
    }
    if encryptionCheck == .problem { return (.problem, "Keys") }
    if encryptionCheck == .waiting { return (.waiting, "Encryption") }
    if backupAvailable && savedAt == nil { return (.waiting, "Account Backup") }
    if reachable == nil || accepted == nil || allowed == nil || storage == nil
      || lastAnswer == nil || !backupAvailable || encryptionCheck != .ok
    {
      return (.open, "Checking")
    }
    return (.ok, nil)
  }

  private var connectionValue: String {
    switch connectionView {
    case 0:
      let probe = RoundTripProbe.shared
      return probe.measured ? probe.value : "Measuring…"
    case 1: return reachable == false ? "Offline" : milliseconds.map { "\($0) ms" } ?? "Checking…"
    default: return syncChecking ? "Checking…" : syncMilliseconds.map { "\($0) ms" } ?? "Unavailable"
    }
  }

  private var connectionCheck: Check {
    switch connectionView {
    case 0:
      let probe = RoundTripProbe.shared
      return !probe.measured ? .open : probe.milliseconds == nil ? .waiting : .ok
    case 1: return serverCheck
    default: return syncSucceeded.map { $0 ? .ok : .waiting } ?? .open
    }
  }

  private var connectionSummary: (check: Check, value: String) {
    let token = Push.shared.registered(for: accounts.client)
    let checks: [(check: Check, value: String)] = [
      (
        serverCheck, reachable == false ? "Offline" : milliseconds.map { "\($0) ms" } ?? "Checking…"
      ),
      (
        syncSucceeded.map { $0 ? .ok : .waiting } ?? .open,
        syncChecking ? "Checking…" : syncSucceeded == true ? "Checked just now" : "Unavailable"
      ),
      (
        allowed.map { $0 && token ? .ok : .waiting } ?? .open,
        allowed.map { $0 && token ? "Ready" : "Setup incomplete" } ?? "Checking…"
      ),
    ]
    func severity(_ check: Check) -> Int {
      switch check {
      case .ok: 0
      case .open: 1
      case .waiting: 2
      case .problem: 3
      }
    }
    return checks.dropFirst().reduce(checks[0]) { worst, next in
      severity(next.check) > severity(worst.check) ? next : worst
    }
  }

  private var diagnostics: some View {
    SettingsForm {
      Section {
        Toggle(
          isOn: Binding(
            get: { crashesOn },
            set: {
              crashesOn = $0
              CrashReports.on = $0
            })
        ) {
          Text("Send Crashes")
        }
        .accessibilityIdentifier("status.sendCrashes")
        // The reports on a page of their own; here only how many.
        NavigationLink {
          CrashListPage(crashes: crashes)
        } label: {
          SettingsNavigationLabel {
            LabeledContent("Crash Reports", value: crashes.isEmpty ? "None" : "\(crashes.count)")
          }
        }
        .disabled(crashes.isEmpty)
      } header: {
        Text("Crashes")
      } footer: {
        Text("Crash reports don’t include names, Yos or photos.")
      }
      Section("Usage") {
        // Total by default; a tap goes on to sent/received, then the friend with the most Yos.
        Button {
          withAnimation(.smooth) { yosView = (yosView + 1) % 3 }
        } label: {
          LabeledContent {
            Text(["\(sent + received)", "↑\(sent) · ↓\(received)", mostYos][yosView])
              .foregroundStyle(.secondary)
          } label: {
            Text(["Total Yos", "Yos", "Most Yos With"][yosView])
          }
          .contentTransition(.interpolate)
          .frame(maxWidth: .infinity, minHeight: 44)
          .contentShape(Rectangle())
        }
        .foregroundStyle(Color(.label))
        LabeledContent("Data Usage", value: "\(usage[1]) this month")
        LabeledContent("Storage", value: storageValues[0])
      }
      Section("Server") {
        let token = Push.shared.registered(for: accounts.client)
        StatusRow(
          title: "Notifications",
          value: allowed.map { $0 && token ? "Ready" : $0 ? "No Token" : "Not Allowed" } ?? "Checking…",
          check: allowed.map { $0 && token ? .ok : .waiting } ?? .open)
        // One row for the connection: Round Trip by default (a tap measures it the first time), then a tap goes
        // on to the plain server ping and the sync time, and back to Round Trip (measured again).
        Button {
          withAnimation(.smooth) { connectionView = (connectionView + 1) % 3 }
        } label: {
          LabeledContent {
            HStack(spacing: 6) {
              if connectionView == 0 && RoundTripProbe.shared.running {
                ProgressView().controlSize(.small)
              }
              CheckValue(value: connectionValue, check: connectionCheck)
                .monospacedDigit()
                .contentTransition(.interpolate)
            }
          } label: {
            Text(["Round Trip", "Server Ping", "Sync"][connectionView])
              .contentTransition(.interpolate)
              .modifier(RoundTripWaiting())
          }
          .frame(maxWidth: .infinity, minHeight: 44)
          .contentShape(Rectangle())
        }
        .foregroundStyle(Color(.label))
        .disabled(accounts.client == nil)
        // A tap brings the whole identity up as a card: its checks, then the keys.
        Button {
          withAnimation(.smooth(duration: 0.3)) { showingKeys = true }
        } label: {
          LabeledContent {
            // The same as Encryption on Help: this phone's keys and the contacts' sessions.
            let checks = keyChecks + [contactsCheck == .open ? .waiting : contactsCheck]
            let ok = checks.allSatisfy { $0 == .ok }
            let worst: Check = checks.contains(.problem) ? .problem : ok ? .ok : .waiting
            Image(systemName: worst.symbol).foregroundStyle(worst.color)
          } label: {
            Text("Identity")
          }
          .frame(maxWidth: .infinity, minHeight: 44)
          .contentShape(Rectangle())
        }
        .foregroundStyle(Color(.label))
        .disabled(status == nil)
      }
      #if YO_DEV
        Section {
          HitchesRows()
        }
      #endif
    }
    .lineLimit(1)
    .minimumScaleFactor(0.6)
  }

  var body: some View {
    Group {
      if wordOnly {
        let result = overallResult
        let word =
          result.check == .ok
          ? "Online" : result.check == .problem ? "Problem" : result.word ?? "Checking"
        HStack(spacing: 8) {
          Circle()
            .fill(result.check == .open ? Color(.systemGray3) : result.check == .ok ? .green : result.check == .problem ? .red : .orange)
            .frame(width: 10, height: 10)
        }
        .animation(.smooth, value: word)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Status: \(word)")
      } else if summaryOnly {
        let result = overallResult
        let word =
          result.check == .ok
          ? "Online" : result.check == .problem ? "Problem" : result.word ?? "Checking"
        let check = result.check == .open ? Check.open : result.check
        NavigationLink(value: SettingsView.Page.status) {
          SettingsNavigationLabel {
            HStack {
              Label("Status", systemImage: "waveform.path.ecg").labelStyle(Tile(.green))
              Spacer(minLength: 8)
              CheckValue(value: word, check: check)
                .contentTransition(.interpolate)
            }
          }
        }
        .accessibilityIdentifier("help.status")
        .animation(.smooth, value: word)
      } else {
        diagnostics
          // While Status is open the round trip is measured again and again, a second after the last answer.
          .task(id: accounts.client.map(ObjectIdentifier.init)) {
            while !Task.isCancelled {
              if !RoundTripProbe.shared.running {
                RoundTripProbe.shared.start(client: accounts.client, quiet: true)
              }
              while RoundTripProbe.shared.running && !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(50))
              }
              do { try await Task.sleep(for: .seconds(1)) } catch { return }
            }
          }
          .overlay {
            ZStack(alignment: .bottom) {
              if showingKeys, let status {
                Color.black.opacity(0.25).ignoresSafeArea()
                  .onTapGesture { withAnimation(.smooth(duration: 0.3)) { showingKeys = false } }
                  .transition(.opacity)
                VStack(alignment: .leading, spacing: 4) {
                  HStack {
                    Text("Identity").font(.headline)
                    Spacer()
                    Button("Done") { withAnimation(.smooth(duration: 0.3)) { showingKeys = false } }
                      .font(.body.weight(.semibold))
                  }
                  .padding(.bottom, 8)
                  StatusRow(title: "Signed by Yo", value: status.hubSigned ? "" : "No", check: keyChecks[0])
                  StatusRow(
                    title: "One-Time Keys",
                    value: "\(status.prekeysAtHub)" + (status.refillPending ? " · Refilling" : ""),
                    check: keyChecks[1])
                  StatusRow(title: "Signed Key", value: "\(status.signedPrekeyDays) days old", check: keyChecks[2])
                  StatusRow(
                    title: "Quantum-Safe Sessions", value: "\(status.pqSessions) of \(status.devices)",
                    check: keyChecks[3])
                  // Friends whose messages are end-to-end encrypted already (a session made); a new friend waits
                  // until the first Yo between you.
                  StatusRow(
                    title: "Encrypted Friends", value: tally.map { "\($0.encrypted) of \($0.total)" } ?? "…",
                    check: contactsCheck == .open ? .waiting : contactsCheck)
                  Divider().padding(.vertical, 4)
                  if !status.accountKey.isEmpty { PublicKeyRow(title: "Account Key", value: status.accountKey) }
                  if !status.accountKey.isEmpty && !status.deviceKey.isEmpty { Divider() }
                  if !status.deviceKey.isEmpty { PublicKeyRow(title: "Device Key", value: status.deviceKey) }
                  Text("Friends use these public keys to start encrypted chats with you. Your private keys stay with you.")
                    .font(.footnote).foregroundStyle(.secondary).lineLimit(nil)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 6)
                }
                .padding(.horizontal, 20)
                .padding(.top, 22)
                .padding(.bottom, 34)
                .background(Color(.systemBackground), in: ConcentricRectangle(corners: .concentric, isUniform: true))
                .padding(.horizontal, 6)
                .padding(.bottom, 6)
                .transition(.move(edge: .bottom))
              }
            }
            .ignoresSafeArea(.container, edges: .bottom)
          }
          .navigationTitle("Status")
          .navigationBarTitleDisplayMode(.inline)
      }
    }
    .task(id: crashesOn) { crashes = CrashReports.shared.history }
    .task {
      while !Task.isCancelled {
        await HubTransport.warmAnonymous()
        try? await Task.sleep(for: .seconds(20))
      }
    }
    .task {
      let loaded = await LocalStorageUsage.load()
      storageValues = await Task.detached(priority: .utility) {
        [loaded.total, loaded.state, loaded.media, loaded.other].map(LocalStorageUsage.formatted)
      }.value
      storage = loaded
    }
    .task(id: inbox.lastSync) { syncTime = inbox.lastSync.map(shortDate) ?? "No successful sync" }
    .task(id: "\(sent)/\(received)") {
      mostYos =
        Stats.byFriend.max(by: { $0.value < $1.value }).map { "\($0.key) · \($0.value)" } ?? "—"
    }
    .task {
      while !Task.isCancelled {
        usage = [
          DataUsage.total(month: false), DataUsage.total(month: true),
          DataUsage.megabytes(month: true, direction: "sent"),
          DataUsage.megabytes(month: true, direction: "received"),
        ]
        do { try await Task.sleep(for: .seconds(2)) } catch { return }
      }
    }
    .task {
      let settings = await UNUserNotificationCenter.current().notificationSettings()
      allowed = [.authorized, .provisional, .ephemeral].contains(settings.authorizationStatus)
    }
    .task(id: accounts.client.map(ObjectIdentifier.init)) {
      syncChecking = true
      syncSucceeded = nil
      defer { syncChecking = false }
      guard let client = accounts.client else {
        syncSucceeded = false
        return
      }
      do {
        let began = ContinuousClock.now
        let (events, contacts) = try await Hub.run {
          (try client.sync(waitS: 0), client.contacts())
        }
        let took = ContinuousClock.now - began
        syncMilliseconds = UInt64(took.components.seconds * 1000 + took.components.attoseconds / 1_000_000_000_000_000)
        guard !Task.isCancelled, client === accounts.client else { return }
        await inbox.handle(
          events, contacts: contacts, client: client, accounts: accounts, friends: friends)
        inbox.lastSync = .now
        syncSucceeded = true
      } catch {
        guard !Task.isCancelled, client === accounts.client else { return }
        syncSucceeded = false
      }
    }
    .task(id: accounts.client.map(ObjectIdentifier.init)) {
      reachable = nil
      milliseconds = nil
      lastAnswer = nil
      while !Task.isCancelled {
        let client = accounts.client
        let start = ContinuousClock.now
        let code = try? await Hub.run {
          if let client { return try client.statusProbe() }
          return HubTransport(base: Hub.url, timeout: 10).request(
            method: "GET", path: "/.well-known/yo-hub-key", headers: [], body: Data()
          ).status
        }
        guard !Task.isCancelled, client === accounts.client else { return }
        let elapsed = start.duration(to: .now).components
        milliseconds = Int(elapsed.seconds * 1000 + elapsed.attoseconds / 1_000_000_000_000_000)
        reachable = code.map { (200..<300).contains($0) } ?? false
        if reachable == true { lastAnswer = .now }
        if code == 426 { accepted = false } else if reachable == true { accepted = true }
        do { try await Task.sleep(for: .seconds(15)) } catch { return }
      }
    }
    .task(id: accounts.client.map(ObjectIdentifier.init)) {
      savedAt = nil
      backupAvailable = false
      while !Task.isCancelled {
        if let client = accounts.client {
          do {
            let ms = try await Hub.run { try client.storageSavedAt() }
            guard client === accounts.client, !Task.isCancelled else { return }
            savedAt = ms.map { Date(timeIntervalSince1970: Double($0) / 1000) }
            backupAvailable = true
          } catch { backupAvailable = false }
        }
        do { try await Task.sleep(for: .seconds(5)) } catch { return }
      }
    }
    .task(id: Reload(accounts.client, friends.contacts)) {
      while !Task.isCancelled {
        if let loaded = await EncryptionTally.load(accounts, friends) { tally = loaded }
        if let client = accounts.client {
          let loaded = try? await Hub.run { client.securityStatus() }
          guard client === accounts.client, !Task.isCancelled else { return }
          status = loaded
        }
        do { try await Task.sleep(for: .seconds(5)) } catch { return }
      }
    }
  }

  /// The contacts' sessions only: exactly the rows under it (verifying safety numbers is optional, not counted).
  private var contactsCheck: Check {
    guard let tally else { return .open }
    return tally.allGood ? .ok : .waiting
  }

  /// This phone's own keys at the hub: signed, quantum-safe sessions, enough one-time keys, a fresh signed key.
  private var keysCheck: Check {
    guard let status else { return .open }
    if status.prekeysAtHub == 0 && !status.refillPending { return .problem }
    return status.hubSigned && status.pqSessions == status.devices && status.prekeysAtHub >= 16
      && status.signedPrekeyDays <= 8 ? .ok : .waiting
  }

  private var encryptionCheck: Check {
    if keysCheck == .problem { return .problem }
    if contactsCheck == .open || keysCheck == .open { return .open }
    return contactsCheck == .ok && keysCheck == .ok ? .ok : .waiting
  }

  private var serverCheck: Check {
    guard let reachable else { return .open }
    return reachable && (milliseconds ?? 0) < 2000 ? .ok : .waiting
  }

  private func shortDate(_ date: Date) -> String {
    if Calendar.current.isDateInToday(date) {
      return date.formatted(date: .omitted, time: .shortened)
    }
    if Calendar.current.isDateInYesterday(date) { return "Yesterday" }
    return date.formatted(.dateTime.day().month(.abbreviated))
  }

}
