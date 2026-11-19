// Push (spec 6.3, /v1/push): the APNs device token to the hub. Yo Dev is signed for APNs' sandbox, TestFlight and the
// App Store for production: the hub picks the APNs host by the platform the app names.
import SwiftUI
import UIKit
import YoCore

@MainActor @Observable
final class Push {
  static let shared = Push()

  private(set) var token: String?
  /// Sent once per device registration (signing in again is a new one).
  private var sentToken: String?
  private weak var sentTo: YoClient?

  func registered(for client: YoClient?) -> Bool {
    token != nil && sentToken == token && sentTo === client
  }

  private static var platform: UInt8 { Hub.isDevApp ? 2 : 1 }  // sandbox : production

  /// No prompt: notifications themselves are asked for in `Inbox.askForNotifications`.
  func register() {
    UIApplication.shared.registerForRemoteNotifications()
  }

  fileprivate func received(_ deviceToken: Data) {
    token = deviceToken.map { String(format: "%02x", $0) }.joined()
  }

  /// What a registration carries; without a token yet 0 and empty (`send` sends it later).
  var registration: (platform: UInt8, token: String) {
    guard let token else { return (0, "") }
    return (Self.platform, token)
  }

  func send(_ accounts: AccountStore) async {
    guard let token, let client = accounts.client, accounts.account != nil,
      sentToken != token || sentTo !== client
    else { return }
    let platform = Self.platform
    do {
      try await Hub.run { try client.setPush(platform: platform, token: token) }
      sentToken = token
      sentTo = client
    } catch {}  // offline: the next launch or token tries again
  }
}

/// For the APNs device token and silent pushes, which SwiftUI's lifecycle cannot receive.
@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate {
  func application(
    _ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    Perf.mark("launch.didFinish")
    application.registerForRemoteNotifications()
    return true
  }
  func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
    Push.shared.received(deviceToken)
  }
  func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
    Diagnostics.record(error)
  }

  static var silentSync: (@MainActor () async -> Bool?)?

  /// A silent push (the hub's non-urgent deliveries): one short inbox sync, answered within 25 s (iOS gives 30).
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

private final class Answer: @unchecked Sendable {
  private let lock = NSLock()
  private var done: CheckedContinuation<UIBackgroundFetchResult, Never>?

  init(_ done: CheckedContinuation<UIBackgroundFetchResult, Never>) { self.done = done }

  func give(_ result: UIBackgroundFetchResult) {
    lock.withLock { defer { done = nil }; return done }?.resume(returning: result)
  }
}
