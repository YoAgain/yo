import Observation
import SwiftUI
import UIKit
import YoCore

/// Volatile result shared by the Account title and Status. Never persisted or run automatically.
@Observable @MainActor final class RoundTripProbe {
  static let shared = RoundTripProbe()
  private(set) var running = false
  private(set) var measured = false
  private(set) var milliseconds: UInt64?
  /// The fresh result beside the title's dot, for two seconds.
  private(set) var showResult = false
  private var run = UUID()

  var value: String { milliseconds.map { "\($0) ms" } ?? (measured ? "No answer" : "—") }
  var color: Color { !measured ? .secondary : milliseconds.map { $0 < 1500 ? .green : .orange } ?? .orange }

  /// `quiet`: Status's continuous measuring — no haptic, nothing beside the title.
  func start(client: YoClient?, quiet: Bool = false) {
    guard !running, let client else { return }
    running = true
    if !quiet { showResult = false }
    let token = UUID()
    run = token
    Task {
      let result = try? await Hub.run { try client.roundTripProbe(target: "PING", timeoutMs: 10_000) }
      guard run == token else { return }
      milliseconds = result
      measured = true
      running = false
      if quiet { return }
      UINotificationFeedbackGenerator().notificationOccurred(result == nil ? .warning : .success)
      withAnimation(.smooth(duration: 0.25)) { showResult = true }
      try? await Task.sleep(for: .seconds(2))
      if run == token { withAnimation(.smooth(duration: 0.4)) { showResult = false } }
    }
  }
}

/// Kept for the Status row's anchor (no flight any more).
struct RoundTripAnchor: PreferenceKey {
  static var defaultValue: [String: Anchor<CGRect>] { [:] }
  static func reduce(value: inout [String: Anchor<CGRect>], nextValue: () -> [String: Anchor<CGRect>]) {
    value.merge(nextValue(), uniquingKeysWith: { _, new in new })
  }
}

/// The Account title: the name and its status dot; a tap measures, and the result shows beside the dot for two
/// seconds (the milliseconds in green, right of the dot, the name staying put), then fades.
struct RoundTripTitle: View {
  let name: String
  let client: YoClient?
  private var probe: RoundTripProbe { .shared }

  var body: some View {
    Button { probe.start(client: client) } label: {
      // Equal empty slots left and right keep the name centred; the result fills the right slot.
      HStack(spacing: 8) {
        Color.clear.frame(width: 72, height: 1)
        Text(name).font(.headline)
        StatusPage(wordOnly: true)
        ZStack(alignment: .leading) {
          Color.clear.frame(width: 72, height: 1)
          if probe.showResult {
            Text(probe.milliseconds.map { "\($0) ms" } ?? "No answer")
              .font(.subheadline.weight(.semibold))
              .monospacedDigit()
              .foregroundStyle(probe.milliseconds == nil ? Color.orange : Color.green)
              .lineLimit(1)
              .minimumScaleFactor(0.7)
              .frame(width: 72, alignment: .leading)
              .transition(.opacity)
          }
        }
      }
      .lineLimit(1)
      .minimumScaleFactor(0.6)
      .modifier(RoundTripWaiting())
      .frame(minHeight: 44)
      .contentShape(Rectangle())
    }
    .task {
      while !Task.isCancelled {
        await HubTransport.warmAnonymous()
        try? await Task.sleep(for: .seconds(20))
      }
    }
    .buttonStyle(.plain)
    .accessibilityLabel(probe.showResult ? "\(name), Yo to Yo roundtrip: \(probe.value)" : "\(name), test Yo to Yo roundtrip")
  }
}

/// While measuring, a gentle pulse.
struct RoundTripWaiting: ViewModifier {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  func body(content: Content) -> some View {
    content.phaseAnimator([false, true]) { view, pulse in
      view.opacity(RoundTripProbe.shared.running && pulse ? 0.55 : 1)
    } animation: { _ in .easeInOut(duration: reduceMotion ? 0.3 : 0.65) }
  }
}

/// No overlay any more (the flight is gone); kept so the sheet's modifier stays valid.
struct RoundTripOverlay: ViewModifier {
  func body(content: Content) -> some View { content }
}
