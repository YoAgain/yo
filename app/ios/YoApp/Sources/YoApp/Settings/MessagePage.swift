import SwiftUI
import YoCore

@MainActor
struct SupportBadge: View {
  @Environment(AccountStore.self) private var accounts
  var body: some View {
    if accounts.supportUnread > 0 {
      Text("\(accounts.supportUnread)")
        .font(.caption2.bold()).foregroundStyle(.white)
        .padding(.horizontal, 5).padding(.vertical, 2)
        .background(.red, in: Capsule())
        .accessibilityLabel("\(accounts.supportUnread) unread replies")
    }
  }
}

@MainActor
struct MessagePage: View {
  @Environment(AccountStore.self) private var accounts
  @Environment(\.dismiss) private var dismiss
  @State private var text = ""
  @State private var messages: [SupportMessage] = []
  @State private var sending = false
  @State private var loading = false
  @State private var problem: String?
  @State private var failedSend = false
  @FocusState private var focused: Bool
  private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

  var body: some View {
    NavigationStack {
      ScrollViewReader { scroll in
        ScrollView {
          LazyVStack(spacing: 4) {
            ForEach(Array(messages.enumerated()), id: \.element.id) { index, message in
              let previous = index > 0 ? messages[index - 1] : nil
              if previous == nil || gap(previous!, message) > 300_000 {
                Text(stamp(message.createdAt))
                  .font(.caption2.weight(.medium))
                  .foregroundStyle(.secondary)
                  .padding(.top, 14)
                  .padding(.bottom, 4)
              }
              bubble(message, grouped: previous.map { $0.fromTeam == message.fromTeam && gap($0, message) <= 300_000 } ?? false)
                .id(message.id)
            }
            Color.clear.frame(height: 1).id("bottom")
          }
          .padding(.horizontal, 12)
          .padding(.top, 8)
        }
        .overlay { if messages.isEmpty { empty } }
        .defaultScrollAnchor(.bottom)
        .scrollDismissesKeyboard(.interactively)
        .onChange(of: messages.last?.id) { _, _ in
          withAnimation(.smooth) { scroll.scrollTo("bottom", anchor: .bottom) }
        }
        .onChange(of: focused) { _, now in
          if now { withAnimation(.smooth) { scroll.scrollTo("bottom", anchor: .bottom) } }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { composer }
      }
      .background(Color(.systemBackground))
      .navigationTitle("Yo Team")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Close", systemImage: "xmark") { dismiss() }
        }
      }
    }
    .hitchScene("Contact Us")
    // One look when the chat opens (and after sending); no polling: replies are rare and data stays small.
    .task { await refresh() }
    .onChange(of: text) { _, new in
      if new.utf8.count > 2000 {
        var cut = new
        while cut.utf8.count > 2000 { cut.removeLast() }
        text = cut
      }
    }
  }

  /// The empty chat: one incoming message from the team, centred, and a quiet word on privacy under it.
  @ViewBuilder private var empty: some View {
    if loading {
      ProgressView()
    } else {
      VStack(alignment: .leading, spacing: 6) {
        bubble(SupportMessage(id: 0, fromTeam: true, text: "How can we help?", createdAt: 0, readAt: 0), grouped: true)
        Text("Not end-to-end encrypted")
          .font(.caption2)
          .foregroundStyle(.tertiary)
          .padding(.leading, 14)
      }
      .padding(.horizontal, 12)
    }
  }

  /// A bubble like Messages: yours right in Yo purple, the team's left in gray; consecutive ones close together.
  private func bubble(_ message: SupportMessage, grouped: Bool) -> some View {
    HStack {
      if !message.fromTeam { Spacer(minLength: 60) }
      Text(message.text)
        .font(.body)
        .textSelection(.enabled)
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .foregroundStyle(message.fromTeam ? Color(.label) : .white)
        .background(
          message.fromTeam ? Color(.systemGray5) : Color.amethyst,
          in: .rect(cornerRadius: 20))
        .accessibilityLabel("\(message.fromTeam ? "Yo team" : "You"): \(message.text)")
      if message.fromTeam { Spacer(minLength: 60) }
    }
    .padding(.top, grouped ? 0 : 6)
  }

  private var composer: some View {
    VStack(spacing: 6) {
      if let problem {
        Button {
          Task { if failedSend { await send() } else { await refresh() } }
        } label: {
          Label(problem, systemImage: "exclamationmark.circle")
            .font(.footnote)
            .foregroundStyle(.secondary)
        }
        .buttonStyle(.plain)
      }
      HStack(alignment: .bottom, spacing: 6) {
        TextField("Message the Yo team", text: $text, axis: .vertical)
          .lineLimit(1...6)
          .focused($focused)
          .disabled(sending)
          .padding(.leading, 16)
          .padding(.vertical, 9)
        Button {
          Task { await send() }
        } label: {
          Group {
            if sending {
              ProgressView().tint(.white)
            } else {
              Image(systemName: "arrow.up").font(.system(size: 15, weight: .bold))
            }
          }
          .foregroundStyle(.white)
          .frame(width: 30, height: 30)
          .background(Color.amethyst.opacity(trimmed.isEmpty ? 0.35 : 1), in: .circle)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Send")
        .disabled(trimmed.isEmpty || sending)
        .padding(.trailing, 4)
        .padding(.bottom, 4)
      }
      .glassEffect(.regular, in: .rect(cornerRadius: 20))
    }
    .padding(.horizontal, 12)
    .padding(.bottom, 8)
  }

  private func gap(_ a: SupportMessage, _ b: SupportMessage) -> Int64 {
    Int64(b.createdAt) - Int64(a.createdAt)
  }

  private func stamp(_ ms: UInt64) -> String {
    let date = Date(timeIntervalSince1970: Double(ms) / 1000)
    return Calendar.current.isDateInToday(date)
      ? date.formatted(date: .omitted, time: .shortened)
      : date.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated).hour().minute())
  }

  private func refresh(quiet: Bool = false) async {
    guard !loading else { return }
    // By the install id: none yet, nothing written yet.
    guard let install = InstallID.existing else { messages = []; accounts.supportUnread = 0; return }
    if !quiet { loading = true }
    defer { loading = false }
    do {
      let thread = try await Hub.run { try helpThread(transport: Hub.transport, install: install) }
      if thread.messages.map(\.id) != messages.map(\.id) {
        withAnimation(.smooth) { messages = thread.messages }
      }
      accounts.supportUnread = thread.unread
      if let through = messages.map(\.id).max() {
        try await Hub.run { try helpRead(transport: Hub.transport, install: install, through: through) }
        accounts.supportUnread = 0
      }
      problem = nil
    } catch {
      if !quiet { failedSend = false; problem = "Couldn’t load messages. Tap to try again." }
    }
  }

  private func send() async {
    guard !trimmed.isEmpty, !sending else { return }
    let words = trimmed, version = HelpPage.appVersion, install = InstallID.current
    sending = true
    failedSend = false
    problem = nil
    defer { sending = false }
    do {
      try await Hub.run { try helpSend(transport: Hub.transport, install: install, text: words, appVersion: version) }
      text = ""
      await refresh()
    } catch YoError.Refused(let status) where status == 429 {
      failedSend = true
      problem = "Message limit reached. Please try again later."
    } catch {
      failedSend = true
      problem = "Not sent. Tap to try again."
    }
  }
}
