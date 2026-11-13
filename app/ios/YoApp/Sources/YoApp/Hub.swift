// The hub, through the Rust core (`YoCore`, spec 6): which hub (live, or the dev one while Yo is in development), HTTP
// for the core, the file its sealed state lives in and the key that seals it. Every core call blocks: `Hub.run` takes
// it off the main actor.
import Foundation
import Security
import YoCore

enum Hub {
  static let live = URL(string: "https://hub.justyo.app")!
  static let dev = URL(string: "https://dev.justyo.app")!

  /// "Yo Dev" (app.justyo.ios.dev, what dev/push.sh installs) talks to the dev hub, always; "Yo" (TestFlight, the
  /// App Store) to the live one. Two apps side by side, each with its own accounts.
  static let isDevApp = Bundle.main.bundleIdentifier?.hasSuffix(".dev") == true

  static var url: URL { isDevApp ? dev : live }
  static var transport: HubTransport { HubTransport(base: url) }

  /// A blocking core call, off the main actor.
  static func run<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await Task.detached(priority: .userInitiated) { try work() }.value
  }

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

/// HTTP for the core: a blocking request with URLSession (the core runs off the main actor, `Hub.run`).
final class HubTransport: HttpTransport {
  let base: URL

  init(base: URL) { self.base = base }

  func request(method: String, path: String, headers: [Header], body: Data) -> HttpResult {
    guard let url = URL(string: path, relativeTo: base) else {
      return HttpResult(status: 0, body: Data(), error: "bad path")
    }
    var request = URLRequest(url: url, timeoutInterval: 70)  // the inbox waits up to a minute
    request.httpMethod = method
    for header in headers { request.setValue(header.value, forHTTPHeaderField: header.name) }
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
/// record, spec 7, not by an old copy of the state).
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

/// The 32-byte key that seals the core's state: in the keychain, this device only.
enum StateKey {
  private static var query: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.state-key",
      kSecAttrAccount as String: "state",
    ]
  }

  static func load() -> Data {
    var item: CFTypeRef?
    var find = query
    find[kSecReturnData as String] = true
    if SecItemCopyMatching(find as CFDictionary, &item) == errSecSuccess, let key = item as? Data, key.count == 32 {
      return key
    }
    let key = Passkeys.random(32)
    var add = query
    add[kSecValueData as String] = key
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    SecItemDelete(query as CFDictionary)
    SecItemAdd(add as CFDictionary, nil)
    return key
  }
}
