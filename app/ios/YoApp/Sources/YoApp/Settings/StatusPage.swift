import SwiftUI
import UIKit
import UserNotifications
import YoCore

struct StatusPage: View {
  var summaryOnly = false
  /// Just a coloured dot (its word for VoiceOver), for the Account title.
  var wordOnly = false
  @Environment(Friends.self) private var friends
  @Environment(AccountStore.self) private var accounts
  @Environment(Inbox.self) private var inbox
  @State private var yosView = 0
  @State private var connectionView = 0
  @State private var showingKeys = false
  @State private var syncMilliseconds: UInt64?
  @State private var storage: LocalStorageUsage?
  @State private var storageTotal = "Calculating…"
  @State private var usage = "Checking…"
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
  @State private var crashesOn = CrashReports.on
  @State private var renewFailed: String?

  /// The last answers shown at once (no "Checking" while one is known); the checks refresh them quietly.
  init(summaryOnly: Bool = false, wordOnly: Bool = false) {
    self.summaryOnly = summaryOnly
    self.wordOnly = wordOnly
    guard let last = StatusMemory.last else { return }
    restoredFor = last.client
    _reachable = State(initialValue: last.reachable)
    _milliseconds = State(initialValue: last.milliseconds)
    _lastAnswer = State(initialValue: last.lastAnswer)
    _accepted = State(initialValue: last.accepted)
    _allowed = State(initialValue: last.allowed)
    _status = State(initialValue: last.status)
    _tally = State(initialValue: last.tally)
    _savedAt = State(initialValue: last.savedAt)
    _backupAvailable = State(initialValue: last.backupAvailable)
    _storage = State(initialValue: last.storage)
    if let total = last.storageTotal { _storageTotal = State(initialValue: total) }
    if let usage = last.usage { _usage = State(initialValue: usage) }
    _syncMilliseconds = State(initialValue: last.syncMilliseconds)
    _syncSucceeded = State(initialValue: last.syncSucceeded)
    _syncChecking = State(initialValue: last.syncSucceeded == nil)
  }

  /// The client whose answers this page started from.
  private var restoredFor: ObjectIdentifier?

  /// The answers it started from are this client's: another account starts from nothing.
  private var remembered: Bool {
    restoredFor != nil && restoredFor == accounts.client.map(ObjectIdentifier.init)
  }

  private func remember() {
    StatusMemory.last = StatusMemory.Snapshot(
      client: accounts.client.map(ObjectIdentifier.init), reachable: reachable, milliseconds: milliseconds,
      lastAnswer: lastAnswer, accepted: accepted, allowed: allowed, status: status, tally: tally, savedAt: savedAt,
      backupAvailable: backupAvailable, storage: storage, storageTotal: storage == nil ? nil : storageTotal,
      usage: usage, syncMilliseconds: syncMilliseconds, syncSucceeded: syncSucceeded)
  }

  private var serverCheck: Check { productServerCheck(reachable: reachable, milliseconds: milliseconds.map(UInt32.init)) }

  /// The one word for the Status row and the Account title's dot (the core's rule).
  private var overall: ProductStatus {
    productStatusSummary(
      health: ProductHealth(
        reachable: reachable, milliseconds: milliseconds.map(UInt32.init), accepted: accepted,
        notificationsAllowed: allowed, pushRegistered: Push.shared.registered(for: accounts.account?.userID), status: status,
        encryptedContacts: tally.map { UInt32($0.encrypted) }, contacts: UInt32(tally?.total ?? 0),
        backupAvailable: backupAvailable, backupSaved: savedAt != nil, storageKnown: storage != nil,
        answered: lastAnswer != nil),
      lang: .english)
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

  private var diagnostics: some View {
    Form {
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
        NavigationLink {
          CrashListPage(crashes: crashes)
        } label: {
          LabeledContent("Crash Reports", value: crashes.isEmpty ? "None" : "\(crashes.count)")
        }
        .disabled(crashes.isEmpty)
      } header: {
        Text("Crashes")
      } footer: {
        Text("Crash reports don’t include names, Yos or photos.")
      }
      Section("Usage") {
        // A tap cycles total, sent/received, the friend with the most Yos.
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
        }
        .tint(Color(.label))
        LabeledContent("Data Usage", value: "\(usage) this month")
        LabeledContent("Storage", value: storageTotal)
      }
      Section("Server") {
        let token = Push.shared.registered(for: accounts.account?.userID)
        StatusRow(
          title: "Notifications",
          value: allowed.map { $0 && token ? "Ready" : $0 ? "No Token" : "Not Allowed" } ?? "Checking…",
          check: allowed.map { $0 && token ? .ok : .waiting } ?? .open)
        // As Signal's: a fresh token from APNs to the hub; the row above turns Ready, or the red banner says why not.
        Button {
          Task {
            do {
              renewFailed = nil
              try await Push.shared.renew(accounts)
              let settings = await UNUserNotificationCenter.current().notificationSettings()
              allowed = [.authorized, .provisional, .ephemeral].contains(settings.authorizationStatus)
            } catch {
              // Said on the row as well: the red banner is under this sheet.
              renewFailed = productProblemText(problem: Problems.problem(of: error), lang: .english)
              Problems.shared.report(error) { Task { try? await Push.shared.renew(accounts) } }
            }
          }
        } label: {
          LabeledContent {
            if Push.shared.renewing { ProgressView().controlSize(.small) }
            else if let renewFailed { CheckValue(value: renewFailed, check: .waiting) }
          } label: {
            Text(ProductText.renewPush.english)
          }
        }
        .tint(Color(.label))
        .disabled(Push.shared.renewing || accounts.client == nil)
        // A tap cycles Round Trip, Server Ping and Sync.
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
        }
        .tint(Color(.label))
        .disabled(accounts.client == nil)
        // Oblivious HTTP (spec 4.1): the hub's proxy sees only IP, time and size; direct only in a measuring build.
        let relay = HubTransport.viaOhttp
        StatusRow(
          title: "Privacy Relay", value: relay.map { $0 ? "OHTTP" : "Direct" } ?? "Checking…",
          check: relay.map { $0 ? .ok : .problem } ?? .open)
      }
      Section("Encryption") {
        Button {
          showingKeys = true
        } label: {
          LabeledContent {
            let worst = productIdentityCheck(
              status: status, encrypted: tally.map { UInt32($0.encrypted) }, total: UInt32(tally?.total ?? 0))
            Image(systemName: worst.symbol).foregroundStyle(worst.color)
          } label: {
            Text("Identity")
          }
        }
        .tint(Color(.label))
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
        let result = overall
        Circle()
          .fill(result.check == .open ? Color(.systemGray3) : result.check.color)
          .frame(width: 10, height: 10)
          .animation(.smooth, value: result.word)
          .accessibilityElement(children: .ignore)
          .accessibilityLabel("Status: \(result.word)")
      } else if summaryOnly {
        let result = overall
        NavigationLink(value: SettingsView.Page.status) {
          LabeledContent {
            CheckValue(value: result.word, check: result.check).contentTransition(.interpolate)
          } label: {
            Label("Status", systemImage: "waveform.path.ecg").labelStyle(Tile(.green))
          }
        }
        .accessibilityIdentifier("help.status")
        .animation(.smooth, value: result.word)
      } else {
        diagnostics
          // Measured again a second after each answer while Status is open.
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
          .sheet(isPresented: $showingKeys) {
            if let status { IdentitySheet(status: status, tally: tally).systemAppearance().appObjects() }
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
      storageTotal = await Task.detached(priority: .utility) { LocalStorageUsage.formatted(loaded.total) }.value
      storage = loaded
      remember()
    }
    .task(id: "\(sent)/\(received)") {
      mostYos =
        Stats.byFriend.max(by: { $0.value < $1.value }).map { "\($0.key) · \($0.value)" } ?? "—"
    }
    .task {
      while !Task.isCancelled {
        usage = DataUsage.total(month: true)
        remember()
        do { try await Task.sleep(for: .seconds(2)) } catch { return }
      }
    }
    .task {
      let settings = await UNUserNotificationCenter.current().notificationSettings()
      allowed = [.authorized, .provisional, .ephemeral].contains(settings.authorizationStatus)
      remember()
    }
    .task(id: accounts.client.map(ObjectIdentifier.init)) {
      if !remembered {
        syncMilliseconds = nil
        syncSucceeded = nil
      }
      syncChecking = syncSucceeded == nil
      defer {
        syncChecking = false
        remember()
      }
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
      if !remembered {
        reachable = nil
        milliseconds = nil
        lastAnswer = nil
      }
      while !Task.isCancelled {
        if UIApplication.shared.applicationState == .background {
          do { try await Task.sleep(for: .seconds(1)) } catch { return }
          continue
        }
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
        remember()
        do { try await Task.sleep(for: .seconds(15)) } catch { return }
      }
    }
    .task(id: accounts.client.map(ObjectIdentifier.init)) {
      if !remembered {
        savedAt = nil
        backupAvailable = false
      }
      while !Task.isCancelled {
        if let client = accounts.client {
          do {
            let ms = try await Hub.run { try client.storageSavedAt() }
            guard client === accounts.client, !Task.isCancelled else { return }
            savedAt = ms.map { Date(timeIntervalSince1970: Double($0) / 1000) }
            backupAvailable = true
          } catch { backupAvailable = false }
          remember()
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
        remember()
        do { try await Task.sleep(for: .seconds(5)) } catch { return }
      }
    }
  }
}

/// The Status checks' last answers while the app runs, so a page opened again starts from them.
@MainActor enum StatusMemory {
  struct Snapshot {
    var client: ObjectIdentifier?
    var reachable: Bool?
    var milliseconds: Int?
    var lastAnswer: Date?
    var accepted: Bool?
    var allowed: Bool?
    var status: SecurityStatus?
    var tally: EncryptionTally?
    var savedAt: Date?
    var backupAvailable: Bool
    var storage: LocalStorageUsage?
    var storageTotal: String?
    var usage: String?
    var syncMilliseconds: UInt64?
    var syncSucceeded: Bool?
  }

  static var last: Snapshot?
}

private struct IdentitySheet: View {
  let status: SecurityStatus
  let tally: EncryptionTally?
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    let checks = productKeyChecks(status: status)
    NavigationStack {
      Form {
        Section {
          StatusRow(title: "Signed by Yo", value: status.hubSigned ? "" : "No", check: checks[0])
          StatusRow(
            title: "One-Time Keys", value: "\(status.prekeysAtHub)" + (status.refillPending ? " · Refilling" : ""),
            check: checks[1])
          StatusRow(title: "Signed Key", value: "\(status.signedPrekeyDays) days old", check: checks[2])
          StatusRow(title: "Quantum-Safe Sessions", value: "\(status.pqSessions) of \(status.devices)", check: checks[3])
          // A new friend waits for a session until the first Yo between you.
          let contacts = productContactsCheck(encrypted: tally.map { UInt32($0.encrypted) }, total: UInt32(tally?.total ?? 0))
          StatusRow(
            title: "Encrypted Friends", value: tally.map { "\($0.encrypted) of \($0.total)" } ?? "…",
            check: contacts == .open ? .waiting : contacts)
        }
        Section {
          if !status.accountKey.isEmpty { PublicKeyRow(title: "Account Key", value: status.accountKey) }
          if !status.deviceKey.isEmpty { PublicKeyRow(title: "Device Key", value: status.deviceKey) }
        } footer: {
          Text("Friends use these public keys to start encrypted chats with you. Your private keys stay with you.")
        }
      }
      .navigationTitle("Identity")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .confirmationAction) { Button("Done", systemImage: "checkmark") { dismiss() } }
      }
    }
    .presentationDetents([.large])
    .presentationDragIndicator(.hidden)
  }
}
