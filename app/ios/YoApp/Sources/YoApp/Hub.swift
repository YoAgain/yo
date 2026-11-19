// The hub, through the Rust core (`YoCore`, spec 6): which hub (live, or the dev one while Yo is in development), HTTP
// for the core, the file its sealed state lives in and the key that seals it. Every core call blocks, also on another
// call still at the hub (the core does one at a time): `Hub.run` takes it off the main actor, and none is made there.
import Foundation
import Observation
import os
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

  /// The public keys each hub may sign its answers that hand out keys with (spec 6.10, Curve25519 and XEdDSA),
  /// base64url as `yo-hub hub-key new` printed them: the one in use and a spare kept offline, so a lost or replaced
  /// hub key needs no app update first. A hub not listed or with none here (a placeholder left empty) is not checked:
  /// its answers are taken as they come, as before the hubs signed.
  static let keys: [String: [String]] = [
    // The hub keys (Curve25519, XEdDSA; `yo-hub hub-key public`): the key in use and a spare kept offline (1Password).
    "hub.justyo.app": ["X7-jqvmZLam-Oh_4VKIB2ozPLUACspOZ2K09v5R5yyY", "NJRBNxJ12bZa1JK8fCLCGFTgM1oJ658mrwKYCdgbmHY"],
    "dev.justyo.app": ["LPiW1TOvv_ricbn7bXmS_fNdVbgtEw_3o7OdelvEdVo", "nFQuDMd-2Ueoec6oYG-Ty7lHQELq1OHs4TVODmPmryg"],
  ]

  /// The sender-certificate trust roots of each hub (spec 6.1), base64url as `yo-hub trust-root new` printed them:
  /// the one in use and a spare kept offline. Sealed Yos open only with a certificate from one of them; a hub with
  /// none here (a placeholder left empty) opens no sealed Yo and sends every Yo identified: it fails closed.
  static let trustRoots: [String: [String]] = [
    "hub.justyo.app": [
      "yj7lt_zG5IrePgCcIsK7gQoVqFDsmOBfUvO4NyK1GF0",  // live: the root in use
      "mJjpj2WXfUy95PRcuA4tl9b_gZxJzRZhyGFq9iVglHU",  // live: the spare, kept offline
    ],
    "dev.justyo.app": [
      "kKVskVCuVwESdvLeYWAEVAQmT9mdwvkQ2uhOlnxXO3Q",  // dev: the root in use
      "Fj-t9EAOsTDPSHPT1Y39FdbcIvl2I7aEEdqlcRAQc0c",  // dev: the spare, kept offline
    ],
  ]

  /// The zkgroup public params of each hub's groups (spec 4.9), base64url as `yo-hub zk new` printed them. A hub with
  /// none here (a placeholder left empty) makes, changes and fetches no group: it fails closed.
  static let zkParams: [String: String] = [
    // live (`yo-hub zk public`; the secret in 1Password, Yo · Server secrets)
    "hub.justyo.app":
      "ALb95DB0Ug3_2sTqjQrBkwhU80zTvUUOepEXF_UIP94kpNslgM89eclAFgszuPloThXdfYihyreYIu_4EH7R8yoOGMKgWiIwxjUcjjuX-n664-nDWet72Epx7NRonkZpBtCTv5Wdy6KVNrRwyb6gS5K3kkOLza9T2wx-jUUri-Q4auRulgrLoJ5WTldL5x2HIXeLvv2lQHjEfvbKUKl3dm-A92r8aHYBP2nsoAykAJ5U4oWvsDRGTnLdtMy-CIM4O5TuZLI7NB6mLOUSsCJKU4cZgz0LsglhzS6V9Jsa64kZzoIsqOIqJm9a-zsRCo7zkWTDRK1C5M77AtPil6OdejogIsru0LXoD74GYO0OjY8XEMPrxcRdqrcVUb1mhFxIGwJxGkaEx9Z-a0q4Oti8mO11d0R1OrTBrM8R_SAuo_FnlKRAkyHrya3EsJGeUenYsNgeYkGV8Dw2QZdKTJZKQHPQHnl1HuT0XJU8QzsXMTFMymujijlIPgF3-5X0TKqPE86rDH2fzXcmfo2nsckJ560DRW8VA2jV3wcKPa3i_hFqJklg7iqMm9Zi-s0dkLxgY6GK69MspMf5imJ7eVYLzlIY2Dr9AxIDIytElgpXsugufEtCC4gBn2N2dpfC6qiQKOpZ7_TL3wmFiFayGsplxPz4alSfEPvfEceIU0x7ZzxlfPsBh48ne0GtldAyg4v8t0C2Pmz5qZSVqQSfbhu_AS3O2gVOeRf1E0wqBkfuz97PPbX0R_MpUD9FmAFC2DTnTkS4kU-ygDsFRDe8bRTTHoXNfn9Caby2jlp5ZQKSfipR_HmLO5mG8PwyxzP5EQ0PVcBIozgHanfhgSUku_cXiDFOxLZfbhALRp3yWAYnG5myHGnLJhzld8RJtIxsJ0QnAA",
    // dev
    "dev.justyo.app":
      "AABte-FyyNG7G0rm_Swi9dLOf6khh32C0-TmfHELCW5J4k4m_beuhn-LDZ6GFT19xzzza_-DQfEKUPdsKy99Knl4FgkgVTu8j8yZP5JoagLdul10PyCTaUcx649Z-JJuUWidD_T_ZYOgvSS61z9QixJQB6LAB_k0AYFY563qzWItEncY_vD68l2OUM1aTDNuXO6QeT0MgeRwrlsqLhF10C2SMQJotdkaEOGAMdr4E261qG0CNuVEua6ojEXs7BXCOpTIPvZhkxQf6N2-WWXVXyrm65kQSDkE32SYFSTxhP9dNNdVop_SDJE24rMYPJy1SH5DD1gs7Y67w0RNQIooUHmaUn3WlOjHXFMgwHpDDMGlDuMqgEMkz1D-ArhgAoYsXG6LduuWJgSRWi3KED6jZRm2QCUpd_LIkZKR2vmQp_Y68IvRf_myayJE33VuqTbfT0jp9KBeiJuIOdqubZBWb2vG_jI0ihgyFRJPTrFtpW0iv1tpngyN0m7_V3I-jUlzIGQSJRJDvDvlEvKPMDeA9Sc2Nh5tRY3JiMTqcFwCVMsbCujL_mhC4VBfXUFy-urGdLiqdTBZF53mInuT_yR8VFoSPLeTi7mHnRSlACAnh_GFnE80FlbXVEbW8EdMXzwycwLy3GajU4z0wOLmeI58jYlwZw8A-dNcB2CHG4zBwpkTiD2ZHZcdAL4kSljBUqE2Tml9cfKtGzqcvH7zrxOhjCL6Sn9IyV36TVf6lKf-oRjDHsPHP9pSp3apVZfG1dEdRQr2UEvHRHla3C8jQrte8e02zrqQwG0KxsUTueOVgiVsWtaUV3YohIHtxbSUOljwoaA-iRmPQsvUHgcpL2UoOghIMYvKSbwRIde_-NYhyQVSCBpcgz5bhRbHcc-uS_j7Qg",
  ]

  /// Pins this hub's keys in a client right after it is made: from then on it takes keys only the hub signed, and
  /// sealed Yos only with a sender certificate from its trust roots.
  static func pin(_ client: YoClient) {
    guard let host = url.host() else { return }
    let roots = (trustRoots[host] ?? []).compactMap { Data(base64URL: $0) }.filter { $0.count == 32 }
    if roots.isEmpty {
      print("Yo: no trust root pinned for \(host): sealed Yos do not open, Yos go identified (Hub.trustRoots)")
    }
    Diagnostics.attempt({ try client.setTrustRoots(keys: roots) })
    if let params = zkParams[host].flatMap({ Data(base64URL: $0) }), !params.isEmpty {
      Diagnostics.attempt({ try client.setZkParams(params: params) })
    } else {
      print("Yo: no group params pinned for \(host): no groups (Hub.zkParams)")
    }
    guard let texts = keys[host] else { return }
    let pinned = texts.compactMap { Data(base64URL: $0) }.filter { $0.count == 32 }
    guard !pinned.isEmpty else { return }
    Diagnostics.attempt({ try client.setHubKeys(keys: pinned) })
  }

  /// A blocking core call, off the main actor: on threads of its own, not Swift's few shared ones. A core call waits
  /// for the network and for the core (one call at a time); blocked there, a burst of calls would hold every shared
  /// thread, and everything else async (the inbox's long wait too) would stand still.
  static func run<T: Sendable>(reportErrors: Bool = true, file: String = #fileID, line: Int = #line, place: String = #function, _ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { done in
      queue.async {
        do { done.resume(returning: try work()) }
        catch {
          if reportErrors { Diagnostics.recordUnreported(error, file: file, line: line, place: place) }
          done.resume(throwing: error)
        }
      }
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

  /// Signed requests and anonymous ones (sealed deliveries, blob fetches) never share a connection, nor cookies, a
  /// cache or credentials: an anonymous request carries nothing a signed one left (spec 1).
  private static let signedSession = URLSession(configuration: plain())
  private static let anonymousSession = URLSession(configuration: plain())

  /// A public, account-free request primes the same pool used by sealed deliveries.
  static func warmAnonymous() async {
    var request = URLRequest(url: Hub.url, timeoutInterval: 10)
    request.httpMethod = "HEAD"
    _ = try? await anonymousSession.data(for: request)
  }

  private static func plain() -> URLSessionConfiguration {
    let config = URLSessionConfiguration.ephemeral
    config.httpAdditionalHeaders = ["Connection": "keep-alive"]
    config.httpCookieStorage = nil
    config.httpShouldSetCookies = false
    config.urlCache = nil
    config.urlCredentialStorage = nil
    return config
  }

  func request(method: String, path: String, headers: [Header], body: Data) -> HttpResult {
    guard !Self.unsupported else { return HttpResult(status: 426, body: Data(), error: nil) }
    guard let url = URL(string: path, relativeTo: base) else {
      Diagnostics.record(NSError(domain: "HubTransport", code: -1000))
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
    let signed = headers.contains(where: { $0.name.lowercased() == "yo-device" })
    // Only what names nobody goes on the anonymous connections (yo_wire::api::anonymous): a registration or a backup's
    // fetch names the account, so never on a connection that also carries sealed deliveries (spec 1).
    let anonymous = !signed && anonymousRequest(method: method, path: path)
    let task = (anonymous ? Self.anonymousSession : Self.signedSession).dataTask(with: request)
    // Read as it comes, and cut off past the largest answer the hub may give (`Limited`): a faulty or hostile hub
    // never fills the memory of the app or of an extension (which has 24 MB).
    let diagnosticUpload = path == "/v1/errors" || path == "/v1/crash" || path == "/v1/diagnostics/crash"
    // Only static route components: never account, blob, group or username path values.
    let codePlace = "HubTransport.request \(method) " + path.split(separator: "/").prefix(2).joined(separator: "/")
    // Expected, never reported: a cancel is always ours (a revoked turn; too large is reported as 413), the inbox's
    // long wait cut off (timed out, connection lost: the next wait follows), a name too short or taken while it is
    // typed, no storage written yet. Every other failure is.
    let route = path.split(separator: "?", maxSplits: 1).first.map(String.init) ?? path
    let longPoll = method == "GET" && path.hasPrefix("/v1/inbox?wait=")
    let quietErrors: Set<Int> = longPoll
      ? [NSURLErrorCancelled, NSURLErrorTimedOut, NSURLErrorNetworkConnectionLost] : [NSURLErrorCancelled]
    let quietStatus: Set<UInt16> =
      method != "GET" ? [] : route.hasPrefix("/v1/username/") ? [400, 409] : route == "/v1/storage" ? [404] : []
    task.delegate = Limited(diagnosticPlace: diagnosticUpload ? nil : codePlace, quiet: quietErrors) { data, response, error in
      answer.result =
        if let error {
          HttpResult(status: 0, body: Data(), error: error)
        } else {
          HttpResult(status: UInt16((response as? HTTPURLResponse)?.statusCode ?? 0), body: data, error: nil)
        }
      DataUsage.record(sent: Int(task.countOfBytesSent), received: Int(task.countOfBytesReceived))
      if answer.result.status == 426 {
        Self.tooOld.store(true, ordering: .relaxed)
        AppUpdate.tooOld()
      }
      done.signal()
    }
    let access = anonymous && path.hasPrefix("/v1/blobs/") && (method == "GET" || method == "DELETE")
      ? nil : SharedAccess.current
    do {
      if let access { try access.start(task) }
      else { task.resume() }
    } catch {
      // The turn ended before the request could start (see below): not reported either.
      task.cancel()
      return HttpResult(status: 0, body: Data(), error: "shared storage time ended")
    }
    done.wait()
    if access?.expired == true {
      // Not reported: the turn ends on leaving the foreground or with the background time, and the request with it
      // (cancelled); the caller tries again in its next turn.
      return HttpResult(status: 0, body: Data(), error: "shared storage time ended")
    }
    if !diagnosticUpload, answer.result.status >= 400, !quietStatus.contains(answer.result.status) {
      Diagnostics.record(NSError(domain: "HTTP", code: Int(answer.result.status)), place: codePlace)
    }
    Availability.network(answer.result.status == 0 || answer.result.status >= 500)
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

  /// The most an answer may be: a blob (10 MiB, its tag and padding) is the largest the hub gives; an inbox of 100
  /// envelopes of at most 64 KiB is less (spec 6.3, 6.8).
  static let largestAnswer = 11 * 1024 * 1024

  /// One task's answer, read as it arrives: cancelled once it says or grows past `largestAnswer`. Its callback runs
  /// once, at the end.
  private final class Limited: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private var data = Data()
    private var response: URLResponse?
    private var tooLarge = false
    private let diagnosticPlace: String?
    /// URL error codes that are expected here (see `request`): not reported.
    private let quiet: Set<Int>
    private let done: (Data, URLResponse?, String?) -> Void

    init(diagnosticPlace: String?, quiet: Set<Int>, done: @escaping (Data, URLResponse?, String?) -> Void) {
      self.done = done
      self.diagnosticPlace = diagnosticPlace
      self.quiet = quiet
    }

    func urlSession(
      _ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
      completionHandler: @escaping (URLSession.ResponseDisposition) -> Void
    ) {
      self.response = response
      if response.expectedContentLength > Int64(HubTransport.largestAnswer) {
        tooLarge = true
        return completionHandler(.cancel)
      }
      completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
      guard !tooLarge else { return }
      self.data.append(data)
      if self.data.count > HubTransport.largestAnswer {
        tooLarge = true
        self.data = Data()
        dataTask.cancel()
      }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
      if let error, let diagnosticPlace,
        !((error as NSError).domain == NSURLErrorDomain && quiet.contains((error as NSError).code)) {
        Diagnostics.record(error, place: diagnosticPlace)
      }
      if tooLarge {
        if let diagnosticPlace { Diagnostics.record(NSError(domain: "HubTransport", code: 413), place: diagnosticPlace) }
        done(Data(), nil, "answer too large")
      } else {
        done(data, response, error?.localizedDescription)
      }
    }
  }
}

/// The core's state in a database of the app's own, as the group's (SharedState.swift) but used by the app alone:
/// The database used by builds without an App Group.
enum StateFile {
  static let folder = URL.applicationSupportDirectory.appending(path: "yo-state-v3", directoryHint: .isDirectory)
  private static let opening = NSLock()
  nonisolated(unsafe) private static var opened: StateDb?

  static func database() throws -> StateDb {
    guard let key = try StateKey.load() else { throw YoError.State }
    return try StateStorage.open(folder, gate: opening, cached: &opened, key: key)
  }

  static var exists: Bool { StateStorage.exists(folder) }

  static func forget() {
    if exists { Diagnostics.attempt({ try database().clear() }) }
  }
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

  /// The group's key for a new state (a registration, which replaces whatever state is there): the one there, else a
  /// new one. Gone while the group kept a state (the keychain reset, or erased apart from the group's folder), the old
  /// state opens with no key any more; without this every registration would fail for good. A keychain that does not
  /// answer now throws, as `shared`.
  static func makeShared() throws -> Data {
    if let key = try shared() { return key }
    let key = random()
    try Keychain.write(inGroup, key, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    return key
  }

  /// Yo Dev (`Keychain.scoped`): the keys of this name in the team's other groups, which its profile lets it read (Yo
  /// from TestFlight on the same phone). Builds before took the first one found for their own, so a state Yo Dev made
  /// meanwhile may be sealed under one of them: tried only when its own key does not open its state, and never
  /// changed or deleted there.
  static func others() -> [Data] {
    guard Keychain.scoped, let own = Keychain.ownGroup() else { return [] }
    var find = query
    find[kSecReturnAttributes as String] = true
    find[kSecReturnData as String] = true
    find[kSecMatchLimit as String] = kSecMatchLimitAll
    var items: CFTypeRef?
    guard SecItemCopyMatching(find as CFDictionary, &items) == errSecSuccess, let found = items as? [[String: Any]]
    else { return [] }
    return found.filter { $0[kSecAttrAccessGroup as String] as? String != own }
      .compactMap { $0[kSecValueData as String] as? Data }.filter { $0.count == 32 }
  }

  /// The app's own item (in its default access group), the group's kept: found with its access group, deleted by it.
  private static func deleteOwn() {
    var items: CFTypeRef?
    guard var find = Keychain.own(query) else { return }
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
  struct Failure: Error, CustomNSError {
    let status: OSStatus
    static var errorDomain: String { "Keychain" }
    var errorCode: Int { Int(status) }
    init(status: OSStatus, file: String = #fileID, line: Int = #line, place: String = #function) {
      self.status = status
      Diagnostics.record(self, place: place, file: file, line: line)
    }
  }

  /// Yo Dev scopes its queries to the concrete app group set by dev/sign.sh. Older builds used the profile's
  /// wildcard default; StateKey.others tries those keys only against this app's sealed state before adopting one.
  static let scoped = Hub.isDevApp && ShareBridge.container == nil

  /// The query within the app's own access group (`scoped`; else as it is). Nil while that group is not known yet
  /// (the keychain does not answer now, before the first unlock): the caller takes it as "not now", never as none.
  static func own(_ query: [String: Any]) -> [String: Any]? {
    guard scoped, query[kSecAttrAccessGroup as String] == nil else { return query }
    guard let group = ownGroup() else { return nil }
    return query.merging([kSecAttrAccessGroup as String: group]) { $1 }
  }

  /// The group an item without one goes into, found once per launch from an item of its own (added without a group,
  /// read back with its attributes) and kept: the app's default access group.
  static func ownGroup() -> String? {
    if let group = Bundle.main.object(forInfoDictionaryKey: "YoKeychainAccessGroup") as? String,
      !group.isEmpty { return group }
    probeLock.lock()
    defer { probeLock.unlock() }
    if let probed { return probed }
    let probe: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.keychain-group",
      kSecAttrAccount as String: Bundle.main.bundleIdentifier ?? "app",
    ]
    var add = probe
    add[kSecValueData as String] = Data()
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    add[kSecReturnAttributes as String] = true
    var result: CFTypeRef?
    var status = SecItemAdd(add as CFDictionary, &result)
    if status == errSecDuplicateItem {
      // There from an earlier launch: only this app adds it (its bundle id), so the one found is its own.
      var find = probe
      find[kSecReturnAttributes as String] = true
      find[kSecMatchLimit as String] = kSecMatchLimitOne
      status = SecItemCopyMatching(find as CFDictionary, &result)
    }
    guard status == errSecSuccess, let group = (result as? [String: Any])?[kSecAttrAccessGroup as String] as? String,
      !group.isEmpty
    else {
      Logger(subsystem: "app.justyo", category: "account").error("keychain: own group not known now (\(status))")
      return nil
    }
    Logger(subsystem: "app.justyo", category: "account").notice("keychain: own group \(group, privacy: .public)")
    probed = group
    return group
  }

  private static let probeLock = NSLock()
  nonisolated(unsafe) private static var probed: String?

  /// Deletes the item (in the app's own group, `own`); errSecInteractionNotAllowed while that is not known.
  @discardableResult
  static func delete(_ query: [String: Any]) -> OSStatus {
    guard let query = own(query) else { return errSecInteractionNotAllowed }
    return SecItemDelete(query as CFDictionary)
  }

  /// The item's data; nil only when there is none.
  static func read(_ query: [String: Any]) throws -> Data? {
    return try Perf.timed("keychain.read") {
      guard let query = own(query) else { throw Failure(status: errSecInteractionNotAllowed) }
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
  }

  /// Writes the item: updated in place (never deleted first), added when there is none; then read back.
  static func write(_ query: [String: Any], _ data: Data, accessible: CFString) throws {
    guard let query = own(query) else { throw Failure(status: errSecInteractionNotAllowed) }
    let attributes: [String: Any] = [kSecValueData as String: data, kSecAttrAccessible as String: accessible]
    var status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    if status == errSecItemNotFound {
      status = SecItemAdd(query.merging(attributes) { $1 } as CFDictionary, nil)
    }
    guard status == errSecSuccess else { throw Failure(status: status) }
    guard try read(query) == data else { throw Failure(status: errSecDecode) }
  }
}

extension Data {
  /// base64url without padding, as the hub's keys are written (spec 6.1).
  init?(base64URL text: String) {
    var plain = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
    plain += String(repeating: "=", count: (4 - plain.count % 4) % 4)
    self.init(base64Encoded: plain)
  }
}

/// Independent signals: a reachable hub does not hide an unavailable local state.
@MainActor @Observable
final class Availability {
  static let shared = Availability()
  var local = false { didSet { settle() } }
  var remote = false { didSet { settle() } }
  /// Shown only once the hub or the state stayed unreachable for a few seconds (a launch's first second is not
  /// "offline"); gone at the first success.
  private(set) var offline = false
  private var pending: Task<Void, Never>?

  private func settle() {
    guard local || remote else {
      pending?.cancel()
      pending = nil
      offline = false
      return
    }
    guard !offline, pending == nil else { return }
    pending = Task { @MainActor [weak self] in
      try? await Task.sleep(for: .seconds(4))
      guard let self, !Task.isCancelled else { return }
      self.pending = nil
      self.offline = self.local || self.remote
    }
  }
  nonisolated static func network(_ failed: Bool) {
    Task { @MainActor in shared.remote = failed }
  }
}

/// Payload bytes, including media, counted once at the transport boundary. Serialised across request callbacks.
enum DataUsage {
  private static let lock = NSLock()
  static func record(sent: Int, received: Int) {
    lock.lock()
    defer { lock.unlock() }
    let defaults = UserDefaults.standard
    let month = monthKey
    for (direction, count) in [("sent", sent), ("received", received)] {
      for period in ["all", month] {
        let key = "usage.\(period).\(direction)"
        defaults.set(defaults.double(forKey: key) + Double(max(0, count)), forKey: key)
      }
    }
  }
  private static var monthKey: String {
    let parts = Calendar(identifier: .gregorian).dateComponents([.year, .month], from: .now)
    return "\(parts.year ?? 0)-\(parts.month ?? 0)"
  }
  static func total(month: Bool) -> String {
    let defaults = UserDefaults.standard
    let period = month ? monthKey : "all"
    let bytes = defaults.double(forKey: "usage.\(period).sent") + defaults.double(forKey: "usage.\(period).received")
    return (bytes / 1_000_000).formatted(.number.precision(.fractionLength(1))) + " MB"
  }
  static func megabytes(month: Bool, direction: String, unit: Bool = true) -> String {
    let bytes = UserDefaults.standard.double(forKey: "usage.\(month ? monthKey : "all").\(direction)")
    return (bytes / 1_000_000).formatted(.number.precision(.fractionLength(1))) + (unit ? " MB" : "")
  }
}
