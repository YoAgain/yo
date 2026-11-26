// Push (spec 4.1, /v1/push): the APNs device token to the hub. Yo Dev is signed for APNs' sandbox, TestFlight and the
// App Store for production: the hub picks the APNs host by the platform the app names.
import Intents
import SwiftUI
import UIKit
import YoCore

@MainActor @Observable
final class Push {
  static let shared = Push()

  private(set) var token: String?
  /// The token the hub took last, per account (user id, base64): kept across launches and clients, so Status says
  /// what the hub holds, not only what this process happened to send to this client object.
  private static let sentKey = "push.sent"
  private var sent: [String: String] = UserDefaults.standard.dictionary(forKey: sentKey) as? [String: String] ?? [:]
  /// Sent once per client in this process as well (signing in again is a new device registration).
  private weak var sentTo: YoClient?

  /// Whether the hub has this phone's token for the account: the one APNs gave this launch, or before APNs answered,
  /// the last one the hub took.
  func registered(for userID: Data?) -> Bool {
    guard let userID, let held = sent[userID.base64EncodedString()] else { return false }
    return token == nil || held == token
  }

  /// Signed out: the next registration is a new device, its token goes again.
  func forget(_ userID: Data) {
    sent[userID.base64EncodedString()] = nil
    UserDefaults.standard.set(sent, forKey: Self.sentKey)
  }

  private static var platform: UInt8 { Hub.isDevApp ? 2 : 1 }  // sandbox : production

  /// No prompt: notifications themselves are asked for in `Inbox.askForNotifications`.
  func register() {
    UIApplication.shared.registerForRemoteNotifications()
  }

  fileprivate func received(_ deviceToken: Data) {
    token = deviceToken.map { String(format: "%02x", $0) }.joined()
    answers += 1
  }

  fileprivate func failed(_ error: Error) {
    refusal = error
    answers += 1
  }

  /// APNs' answers so far (a token or a failure), for `renew` to wait for the next one.
  @ObservationIgnored private var answers = 0
  @ObservationIgnored private var refusal: Error?
  private(set) var renewing = false

  /// Settings › Status › Renew Push Registration (as Signal's): the token asked of APNs again and sent to the hub even
  /// if the hub has it. Throws what failed (APNs, the connection, the hub) for the red banner.
  func renew(_ accounts: AccountStore) async throws {
    guard !renewing, let client = accounts.client, let userID = accounts.account?.userID else { return }
    renewing = true
    defer { renewing = false }
    let before = answers
    refusal = nil
    UIApplication.shared.registerForRemoteNotifications()
    for _ in 0..<100 where answers == before { try await Task.sleep(for: .milliseconds(100)) }
    if let refusal { throw refusal }
    guard let token else { throw YoError.Network(message: "no push token") }
    let platform = Self.platform
    try await Hub.run { try client.setPush(platform: platform, token: token) }
    sent[userID.base64EncodedString()] = token
    sentTo = client
    UserDefaults.standard.set(sent, forKey: Self.sentKey)
  }

  /// What a registration carries; without a token yet 0 and empty (`send` sends it later).
  var registration: (platform: UInt8, token: String) {
    guard let token else { return (0, "") }
    return (Self.platform, token)
  }

  /// The token to the hub; not taken (offline, the hub busy), tried again after a while as long as this task runs
  /// (it is started again for a new token, account or client).
  func send(_ accounts: AccountStore) async {
    guard let token, let client = accounts.client, let userID = accounts.account?.userID else { return }
    let user = userID.base64EncodedString()
    guard sent[user] != token || sentTo !== client else { return }
    let platform = Self.platform
    for wait in [5, 30, 120, 600] {
      do {
        try await Hub.run { try client.setPush(platform: platform, token: token) }
        guard client === accounts.client else { return }
        sent[user] = token
        sentTo = client
        UserDefaults.standard.set(sent, forKey: Self.sentKey)
        return
      } catch {
        try? await Task.sleep(for: .seconds(wait))
        if Task.isCancelled || client !== accounts.client { return }
      }
    }
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
    Push.shared.failed(error)
  }

  func application(_ application: UIApplication, handlerFor intent: INIntent) -> Any? {
    switch intent {
    case is INSendMessageIntent, is INSearchForMessagesIntent, is INSetMessageAttributeIntent: SiriMessages()
    default: nil
    }
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
