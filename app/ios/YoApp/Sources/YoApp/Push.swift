// Push (spec 6.3, /v1/push): the device token from APNs, handed to the hub so a Yo wakes the phone while Yo is closed.
// "Yo Dev" is signed for APNs' sandbox, TestFlight and the App Store for production: the hub picks the APNs host by
// the platform the app names.
import SwiftUI
import UIKit
import YoCore

@MainActor @Observable
final class Push {
  static let shared = Push()

  /// The device token as hex, once APNs gave it.
  private(set) var token: String?
  /// What the hub has, so a token goes there once per device registration (a sign-in again is a new one).
  private var sentToken: String?
  private weak var sentTo: YoClient?

  func registered(for client: YoClient?) -> Bool {
    token != nil && sentToken == token && sentTo === client
  }

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
    guard let token, let client = accounts.client, accounts.account != nil,
      sentToken != token || sentTo !== client
    else { return }
    let platform = Hub.isDevApp ? Self.sandbox : Self.production
    do {
      try await Hub.run { try client.setPush(platform: platform, token: token) }
      sentToken = token
      sentTo = client
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
  func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
    Diagnostics.record(error)
  }

  /// The inbox's one short look for a silent push (YoApp sets it once the inbox is there).
  static var silentSync: (@MainActor () async -> Bool?)?

  /// A silent push (`content-available`: the hub's non-urgent deliveries): one short inbox sync in the background,
  /// answered within 25 s (iOS gives 30), no UI. The async form on the main actor answers on the main thread.
  func application(
    _ application: UIApplication, didReceiveRemoteNotification userInfo: [AnyHashable: Any]
  ) async -> UIBackgroundFetchResult {
    guard let sync = Self.silentSync else { return .noData }
    return await withCheckedContinuation { done in
      let answer = Answer(done)
      Task { @MainActor in
        let result = await sync()
        answer.give(result.map { $0 ? .newData : .noData } ?? .failed)
      }
      Task { @MainActor in
        try? await Task.sleep(for: .seconds(25))
        answer.give(.failed)
      }
    }
  }
}

/// A background fetch's answer, given once: by the sync or by the deadline, whichever comes first.
private final class Answer: @unchecked Sendable {
  private let lock = NSLock()
  private var done: CheckedContinuation<UIBackgroundFetchResult, Never>?

  init(_ done: CheckedContinuation<UIBackgroundFetchResult, Never>) { self.done = done }

  func give(_ result: UIBackgroundFetchResult) {
    lock.withLock { defer { done = nil }; return done }?.resume(returning: result)
  }
}
