// The bar on top of the list while Yos can't reach this phone: notifications turned off for Yo. A tap opens Yo's
// notification settings; back in the app it is gone once they are on. No dismiss: without them Yo is mute.
import SwiftUI
import UserNotifications
import YoCore

struct NotificationsBar: View {
  /// Camera mode: no bar, the view itself stays (the list keeps its first child).
  var hidden = false
  @Environment(\.scenePhase) private var scenePhase
  @State private var denied = false
  /// Asked in this launch: a "Don't Allow" just given is respected, the bar waits for the next launch (no nudging).
  @MainActor private static var askedThisLaunch = false

  var body: some View {
    VStack(spacing: 0) {
      if denied && !hidden {
        Button {
          if let url = URL(string: UIApplication.openNotificationSettingsURLString) {
            UIApplication.shared.open(url)
          }
        } label: {
          Text(productText(text: .turnOnNotifications, lang: .english))
            .font(.yo(Metrics.rowFont))
            .foregroundStyle(.white)
            .lineLimit(1)
            .minimumScaleFactor(0.5)
            .padding(.horizontal, 16)
            .frame(maxWidth: .infinity)
            .frame(height: Metrics.rowHeight)
            .background(.alizarin)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(productText(text: .notificationsOff, lang: .english))
        .accessibilityHint(productText(text: .notificationsHint, lang: .english))
      }
    }
    // The answer is asked for when the list first shows (YoApp); the prompt and Settings both leave the app inactive,
    // so coming back to active reads the answer.
    .task(id: scenePhase) {
      guard scenePhase == .active else { return }
      let status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
      if status == .notDetermined { Self.askedThisLaunch = true }
      denied = productShowNotificationRow(denied: status == .denied, askedThisLaunch: Self.askedThisLaunch)
    }
  }
}
