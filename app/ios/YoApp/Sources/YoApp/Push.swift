// Push (spec 6.3, /v1/push): the device token from APNs, handed to the hub so a Yo wakes the phone while Yo is closed.
// "Yo Dev" is signed for APNs' sandbox, TestFlight and the App Store for production: the hub picks the APNs host by
// the platform the app names.
import SwiftUI
import UIKit

@MainActor @Observable
final class Push {
  static let shared = Push()

  /// The device token as hex, once APNs gave it.
  private(set) var token: String?
  /// What the hub has, so a token goes there once (per account).
  private var sent: (token: String, user: Data)?

  private static let production: UInt8 = 1
  private static let sandbox: UInt8 = 2

  /// Asks APNs for the token (no prompt: notifications themselves are allowed in `Inbox.askForNotifications`).
  func register() {
    UIApplication.shared.registerForRemoteNotifications()
  }

  fileprivate func received(_ deviceToken: Data) {
    token = deviceToken.map { String(format: "%02x", $0) }.joined()
  }

  /// Tells the hub this device's token, when there is an account and the hub does not have it yet.
  func send(_ accounts: AccountStore) async {
    guard let token, let client = accounts.client, let user = accounts.account?.userID,
      sent?.token != token || sent?.user != user
    else { return }
    let platform = Hub.isDevApp ? Self.sandbox : Self.production
    do {
      try await Hub.run { try client.setPush(platform: platform, token: token) }
      sent = (token, user)
    } catch {
      // Not now (offline): the next launch or token tries again.
    }
  }
}

/// The one thing SwiftUI's lifecycle cannot do: receive the APNs device token.
@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate {
  func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
    Push.shared.received(deviceToken)
  }
}
