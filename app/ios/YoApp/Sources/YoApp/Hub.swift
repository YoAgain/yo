// The hub, through the Rust core (`YoCore`, spec 6): which hub (live, or the dev one while Yo is in development), HTTP
// for the core, the file its sealed state lives in and the key that seals it. Every core call blocks, also on another
// call still at the hub (the core does one at a time): `Hub.run` takes it off the main actor, and none is made there.
import Foundation
import Observation
import QuartzCore
import Security
import Synchronization
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
  /// The hub answered 426 (this build is too old, spec 6.3): from then on nothing more is sent, by any transport of
  /// this process; every request is answered 426 here, and the loops that send again stop (`unsupported`).
  private static let tooOld = Atomic<Bool>(false)
  static var unsupported: Bool { tooOld.load(ordering: .relaxed) }

  let base: URL
  /// Seconds a request may take: the inbox waits up to a minute; the notification extension has 30 s in all.
  let timeout: TimeInterval

  init(base: URL, timeout: TimeInterval = 70) {
    self.base = base
    self.timeout = timeout
  }

  func request(method: String, path: String, headers: [Header], body: Data) -> HttpResult {
    guard !Self.unsupported else { return HttpResult(status: 426, body: Data(), error: nil) }
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
    let start = Perf.on ? CACurrentMediaTime() : 0
    URLSession.shared.dataTask(with: request) { data, response, error in
      answer.result =
        if let error {
          HttpResult(status: 0, body: Data(), error: error.localizedDescription)
        } else {
          HttpResult(status: UInt16((response as? HTTPURLResponse)?.statusCode ?? 0), body: data ?? Data(), error: nil)
        }
      if answer.result.status == 426 {
        Self.tooOld.store(true, ordering: .relaxed)
        AppUpdate.tooOld()
      }
      done.signal()
    }.resume()
    done.wait()
    if Perf.on {
      let ms = (CACurrentMediaTime() - start) * 1000
      let route = path.split(separator: "/").prefix(2).joined(separator: "/")  // not the ids after it
      Perf.mark("http \(method) \(route) \(answer.result.status) " + String(format: "%.1f ms", ms))
    }
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

  /// The app's own key (Yo Dev, or a Yo whose state did not move into the group): any of the app's items. Nil when
  /// there is none; a keychain that does not answer now (locked, a passing error) throws: never a new key for it.
  static func load() throws -> Data? {
    guard let key = try Keychain.read(query) else { return nil }
    guard key.count == 32 else { throw Keychain.Failure(status: errSecDecode) }
    return key
  }

  /// The key for a new state (a registration): the one there, else a new one, used only once it reads back as stored.
  static func make() throws -> Data {
    if let key = try Keychain.read(query), key.count == 32 { return key }
    // None, or not a key (nothing sealed under it opens): a new one in its place.
    let key = random()
    try Keychain.write(query, key, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    return key
  }

  /// The key in the group (the app and its extensions); nil when there is none, a keychain that does not answer now
  /// (locked, no group) throws.
  static func shared() throws -> Data? {
    guard let key = try Keychain.read(inGroup) else { return nil }
    guard key.count == 32 else { throw Keychain.Failure(status: errSecDecode) }
    return key
  }

  /// The app, once: the key into the group, the same key (its state opens with no other). A copy first, read back,
  /// then the app's own item deleted; a new key in the group when there was none. False when the keychain will not
  /// (locked, no group in the signature): then the state stays where it is too.
  static func moveIn() -> Bool {
    // The keychain does not answer now (locked): not now.
    guard case .success(let there) = Result(catching: { try shared() }),
      case .success(let own) = Result(catching: { try load() })
    else { return false }
    if there != nil {
      deleteOwn()
      return true
    }
    let key = own ?? random()
    guard (try? Keychain.write(inGroup, key, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)) != nil
    else { return false }
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

/// The app's keychain items, read and written so that a keychain that does not answer now (locked, a passing error)
/// is never taken for one without the item: only `errSecItemNotFound` is "none".
enum Keychain {
  struct Failure: Error {
    let status: OSStatus
  }

  /// The item's data; nil only when there is none.
  static func read(_ query: [String: Any]) throws -> Data? {
    var find = query
    find[kSecReturnData as String] = true
    find[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let status = SecItemCopyMatching(find as CFDictionary, &item)
    switch status {
    case errSecSuccess:
      guard let data = item as? Data else { throw Failure(status: errSecDecode) }
      return data
    case errSecItemNotFound:
      return nil
    default:
      throw Failure(status: status)
    }
  }

  /// Writes the item: updated in place (never deleted first), added when there is none; then read back.
  static func write(_ query: [String: Any], _ data: Data, accessible: CFString) throws {
    let attributes: [String: Any] = [kSecValueData as String: data, kSecAttrAccessible as String: accessible]
    var status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    if status == errSecItemNotFound {
      status = SecItemAdd(query.merging(attributes) { $1 } as CFDictionary, nil)
    }
    guard status == errSecSuccess else { throw Failure(status: status) }
    guard try read(query) == data else { throw Failure(status: errSecDecode) }
  }
}
