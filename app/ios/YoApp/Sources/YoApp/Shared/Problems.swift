import SwiftUI
import UIKit
import YoCore

/// The one way a user action tells that it did not happen: the red banner at the top (DeleteNotice's, under
/// "Offline", which the core puts first). Every send, add, group, block, sync and upload reports here; nothing fails
/// silently. The words are the core's (`ProductProblem`), the same on Android. One problem shows at a time (the
/// newest); the same again only restarts its time. No connection and the server go when the hub answers again, the
/// others after a few seconds or when they are over (`resolve`: "Waiting to Send" once what waited went).
@MainActor @Observable final class Problems {
  static let shared = Problems()
  private(set) var text: String?
  private(set) var retry: (@MainActor () -> Void)?
  @ObservationIgnored private(set) var shown: ProductProblem?
  @ObservationIgnored private var expiry: Task<Void, Never>?

  /// How long a problem that does not go by itself stays.
  static let seconds = 6.0

  func report(_ problem: ProductProblem, retry: (@MainActor () -> Void)? = nil) {
    if retry != nil || shown != problem { self.retry = retry }
    let message = productProblemText(problem: problem, lang: .english)
    if text != message {
      text = message
      UIAccessibility.post(notification: .announcement, argument: message)
    }
    shown = problem
    expiry?.cancel()
    expiry = nil
    // These stay until the hub answers again.
    guard problem != .noConnection, problem != .serverUnreachable else { return }
    expiry = Task {
      try? await Task.sleep(for: .seconds(Self.seconds))
      guard !Task.isCancelled else { return }
      self.clear()
    }
  }

  /// A core error (or any other) as the user's problem.
  func report(_ error: Error, retry: (@MainActor () -> Void)? = nil) {
    report(Self.problem(of: error), retry: retry)
  }

  static func problem(of error: Error) -> ProductProblem {
    if let error = error as? YoError { return productProblemOf(error: error) }
    if (error as NSError).domain == NSURLErrorDomain { return .noConnection }
    return .failed
  }

  /// An error's words for an alert: the core's problem text for a core or network error (its own description is
  /// the debug form, "YoCore.YoError.Network(message: …)"), else the error's own description.
  static func words(for error: Error) -> String {
    guard error is YoError || (error as NSError).domain == NSURLErrorDomain else { return error.localizedDescription }
    return productProblemText(problem: problem(of: error), lang: .english)
  }

  /// It is over (sent, answered): its banner goes, another's stays.
  func resolve(_ problems: ProductProblem...) {
    guard let shown, problems.contains(shown) else { return }
    clear()
  }

  /// The hub answered: no connection or server problem any more.
  nonisolated static func hubAnswered() {
    Task { @MainActor in shared.resolve(.noConnection, .serverUnreachable) }
  }

  private func clear() {
    expiry?.cancel()
    expiry = nil
    text = nil
    retry = nil
    shown = nil
  }
}

struct VideoPlayGlyph: View {
  var visible: Bool
  var side: CGFloat
  var body: some View {
    if visible {
      Image(systemName: "play.fill")
        .font(.system(size: side * 0.35, weight: .semibold))
        .foregroundStyle(.white)
        .shadow(color: .black.opacity(0.6), radius: 2, y: 1)
        .accessibilityLabel("Video")
    }
  }
}
