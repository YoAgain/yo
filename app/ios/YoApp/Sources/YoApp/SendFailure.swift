import SwiftUI
import UIKit

@MainActor @Observable final class SendFailure {
  static let shared = SendFailure()
  var text: String?
  var retry: (@MainActor () -> Void)?
  private var expiry: Task<Void, Never>?

  func show(name: String? = nil, retry: (@MainActor () -> Void)? = nil) {
    self.retry = retry
    // Availability debounces failures; wait for it before choosing the banner.
    expiry?.cancel()
    expiry = Task {
      try? await Task.sleep(for: .seconds(1))
      guard !Task.isCancelled, !Availability.shared.offline else { return }
      let message = name.map { $0.count <= 18 ? "Failed to Send to \($0)" : "Failed to Send" } ?? "Failed to Send"
      text = message
      UIAccessibility.post(notification: .announcement, argument: message)
      try? await Task.sleep(for: .seconds(5))
      guard !Task.isCancelled else { return }
      text = nil
    }
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
