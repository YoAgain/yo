// The hub, through the Rust core (`YoCore`, spec 6): which hub (live, or the dev one while Yo is in development), HTTP
// for the core, the file its sealed state lives in and the key that seals it. Every core call blocks, also on another
// call still at the hub (the core does one at a time): `Hub.run` takes it off the main actor, and none is made there.
import Foundation
import Observation
import Security
import YoCore

enum Hub {
  static let live = URL(string: "https://hub.justyo.app")!
  static let dev = URL(string: "https://dev.justyo.app")!

  /// "Yo Dev" (app.justyo.ios.dev, what dev/push.sh installs, and its extensions app.justyo.ios.dev.notify, …) talks
  /// to the dev hub, always; "Yo" (TestFlight, the App Store) to the live one. Two apps side by side, each with its
  /// own accounts.
  static let isDevApp = Bundle.main.bundleIdentifier?.split(separator: ".").contains("dev") == true

  /// The hub this app belongs to; its accounts are kept under its name (SyncedAccount).
  static var home: URL { isDevApp ? dev : live }
  /// Yo Dev only: another address for its requests (YO_HUB in the environment, as dev/sim's driver launches it), so a
  /// test can take the hub away (an address nothing answers on) while the accounts stay the dev hub's.
  static var url: URL {
    guard isDevApp, let other = ProcessInfo.processInfo.environment["YO_HUB"].flatMap(URL.init(string:)) else {
      return home
    }
    return other
  }
  static var transport: HubTransport { HubTransport(base: url) }

  /// A blocking core call, off the main actor: on threads of its own, not Swift's few shared ones. A core call waits
  /// for the network and for the core (one call at a time); blocked there, a burst of calls would hold every shared
  /// thread, and everything else async (the inbox's long wait too) would stand still.
  static func run<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { done in
      queue.async { done.resume(with: Result { try work() }) }
    }
  }

  private static let queue = DispatchQueue(label: "app.justyo.hub", qos: .userInitiated, attributes: .concurrent)

  /// The user id as the core writes it (32 hex digits) as its 16 bytes: a passkey's user handle, the backup's id
  /// (spec 7).
  static func bytes(_ id: String) -> Data? {
    let digits = Array(id.utf8)
    guard digits.count == 32 else { return nil }
    var bytes = Data()
    for i in stride(from: 0, to: 32, by: 2) {
      guard let byte = UInt8(String(decoding: digits[i..<i + 2], as: UTF8.self), radix: 16) else { return nil }
      bytes.append(byte)
    }
    return bytes
  }
}

/// The app's version for the hub (`Yo-App`, spec 6.3), and whether the hub said it is too old (426).
@MainActor @Observable
final class AppUpdate {
  static let shared = AppUpdate()
  /// The hub answered 426 once: this build must be updated; everything else waits behind the screen.
  private(set) var needed = false

  /// "ios/202611140619": the platform and this build, on every request.
  nonisolated static let header = "ios/" + (Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0")

  /// From any thread: the hub's answer to some request was 426.
  nonisolated static func tooOld() {
    Task { @MainActor in shared.needed = true }
  }
}

/// HTTP for the core: a blocking request with URLSession (the core runs off the main actor, `Hub.run`).
final class HubTransport: HttpTransport {
  let base: URL
  /// Seconds a request may take: the inbox waits up to a minute; the notification extension has 30 s in all.
  let timeout: TimeInterval

  init(base: URL, timeout: TimeInterval = 70) {
    self.base = base
    self.timeout = timeout
  }

  func request(method: String, path: String, headers: [Header], body: Data) -> HttpResult {
    guard let url = URL(string: path, relativeTo: base) else {
      return HttpResult(status: 0, body: Data(), error: "bad path")
    }
    var request = URLRequest(url: url, timeoutInterval: timeout)
    request.httpMethod = method
    for header in headers { request.setValue(header.value, forHTTPHeaderField: header.name) }
    request.setValue(AppUpdate.header, forHTTPHeaderField: "Yo-App")
    if !body.isEmpty || method != "GET" { request.httpBody = body }
    let done = DispatchSemaphore(value: 0)
    let answer = Answer()
    URLSession.shared.dataTask(with: request) { data, response, error in
      answer.result =
        if let error {
          HttpResult(status: 0, body: Data(), error: error.localizedDescription)
        } else {
          HttpResult(status: UInt16((response as? HTTPURLResponse)?.statusCode ?? 0), body: data ?? Data(), error: nil)
        }
      if answer.result.status == 426 { AppUpdate.tooOld() }
      done.signal()
    }.resume()
    done.wait()
    return answer.result
  }

  /// Carries the answer out of URLSession's callback; written once before the semaphore is signalled.
  private final class Answer: @unchecked Sendable {
    var result = HttpResult(status: 0, body: Data(), error: "no answer")
  }
}

/// The core's sealed state, in one file of the app's own (excluded from backups: the account comes back by its backup
/// record, spec 7, not by an old copy of the state). Yo Dev's; Yo's moves into the App Group (SharedState.swift).
final class StateFile: StateStore {
  static let url = URL.applicationSupportDirectory.appending(path: "yo-state")

  func save(blob: Data) {
    try? FileManager.default.createDirectory(at: URL.applicationSupportDirectory, withIntermediateDirectories: true)
    try? blob.write(to: Self.url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    var url = Self.url
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? url.setResourceValues(values)
  }

  static func load() -> Data? { try? Data(contentsOf: url) }
  static func delete() { try? FileManager.default.removeItem(at: url) }
}

/// The 32-byte key that seals the core's state: in the keychain, this device only. Yo from the App Store and TestFlight
/// keeps it in the App Group's keychain group (an App Group is a keychain access group too, by its own name), where its
/// extensions read it; Yo Dev in its own, as before.
enum StateKey {
  private static var query: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.state-key",
      kSecAttrAccount as String: "state",
    ]
  }

  private static var inGroup: [String: Any] {
    query.merging([kSecAttrAccessGroup as String: ShareBridge.group]) { $1 }
  }

  /// The app's own key (Yo Dev, or a Yo whose state did not move into the group): any of the app's items.
  static func load() -> Data {
    var item: CFTypeRef?
    var find = query
    find[kSecReturnData as String] = true
    if SecItemCopyMatching(find as CFDictionary, &item) == errSecSuccess, let key = item as? Data, key.count == 32 {
      return key
    }
    let key = random()
    var add = query
    add[kSecValueData as String] = key
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    SecItemDelete(query as CFDictionary)
    SecItemAdd(add as CFDictionary, nil)
    return key
  }

  /// The key in the group (the app and its extensions); nil while the keychain does not give it (locked, no group).
  static func shared() -> Data? {
    var item: CFTypeRef?
    var find = inGroup
    find[kSecReturnData as String] = true
    guard SecItemCopyMatching(find as CFDictionary, &item) == errSecSuccess, let key = item as? Data, key.count == 32
    else { return nil }
    return key
  }

  /// The app, once: the key into the group, the same key (its state opens with no other). A copy first, read back,
  /// then the app's own item deleted; a new key in the group when there was none. False when the keychain will not
  /// (locked, no group in the signature): then the state stays where it is too.
  static func moveIn() -> Bool {
    if shared() != nil {
      deleteOwn()
      return true
    }
    var item: CFTypeRef?
    var find = query
    find[kSecReturnData as String] = true
    let found = SecItemCopyMatching(find as CFDictionary, &item)
    let key: Data
    switch found {
    case errSecSuccess:
      guard let old = item as? Data, old.count == 32 else { return false }
      key = old
    case errSecItemNotFound:
      key = random()
    default:
      return false  // locked: not now
    }
    var add = inGroup
    add[kSecValueData as String] = key
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    guard SecItemAdd(add as CFDictionary, nil) == errSecSuccess, shared() == key else { return false }
    deleteOwn()
    return true
  }

  /// The app's own item (in its default access group), the group's kept: found with its access group, deleted by it.
  private static func deleteOwn() {
    var items: CFTypeRef?
    var find = query
    find[kSecReturnAttributes as String] = true
    find[kSecMatchLimit as String] = kSecMatchLimitAll
    guard SecItemCopyMatching(find as CFDictionary, &items) == errSecSuccess,
      let found = items as? [[String: Any]]
    else { return }
    for attributes in found {
      guard let group = attributes[kSecAttrAccessGroup as String] as? String, group != ShareBridge.group else {
        continue
      }
      SecItemDelete(query.merging([kSecAttrAccessGroup as String: group]) { $1 } as CFDictionary)
    }
  }

  private static func random() -> Data { Data((0..<32).map { _ in UInt8.random(in: .min ... .max) }) }
}
