// The hub through the Rust core (spec 6): which hub, HTTP for the core, the state file and its key. Every core call
// blocks (the core does one at a time): `Hub.run` takes it off the main actor.
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

  /// "Yo Dev" (and its extensions) talks to the dev hub, "Yo" to the live one: two apps side by side.
  static let isDevApp = Bundle.main.bundleIdentifier?.split(separator: ".").contains("dev") == true

  /// Accounts are kept under its name (SyncedAccount).
  static var home: URL { isDevApp ? dev : live }
  /// Yo Dev only: YO_HUB lets dev/sim take the hub away while the accounts stay the dev hub's.
  static var url: URL {
    guard isDevApp, let other = ProcessInfo.processInfo.environment["YO_HUB"].flatMap(URL.init(string:)) else {
      return home
    }
    return other
  }
  static var transport: HubTransport { HubTransport(base: url) }

  /// Keys each hub signs key-handing answers with (spec 6.10, `yo-hub hub-key public`): the one in use and an offline
  /// spare, so a replaced hub key needs no app update. A hub with none here is not checked.
  static let keys: [String: [String]] = [
    "hub.justyo.app": ["X7-jqvmZLam-Oh_4VKIB2ozPLUACspOZ2K09v5R5yyY", "NJRBNxJ12bZa1JK8fCLCGFTgM1oJ658mrwKYCdgbmHY"],
    "dev.justyo.app": ["LPiW1TOvv_ricbn7bXmS_fNdVbgtEw_3o7OdelvEdVo", "nFQuDMd-2Ueoec6oYG-Ty7lHQELq1OHs4TVODmPmryg"],
  ]

  /// Sender-certificate trust roots (spec 6.1): in use and an offline spare. A hub with none fails closed: no sealed
  /// Yo opens and every Yo goes identified.
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

  /// zkgroup public params (spec 4.9). A hub with none fails closed: no groups.
  static let zkParams: [String: String] = [
    // live (`yo-hub zk public`; the secret in 1Password, Yo · Server secrets)
    "hub.justyo.app":
      "ALb95DB0Ug3_2sTqjQrBkwhU80zTvUUOepEXF_UIP94kpNslgM89eclAFgszuPloThXdfYihyreYIu_4EH7R8yoOGMKgWiIwxjUcjjuX-n664-nDWet72Epx7NRonkZpBtCTv5Wdy6KVNrRwyb6gS5K3kkOLza9T2wx-jUUri-Q4auRulgrLoJ5WTldL5x2HIXeLvv2lQHjEfvbKUKl3dm-A92r8aHYBP2nsoAykAJ5U4oWvsDRGTnLdtMy-CIM4O5TuZLI7NB6mLOUSsCJKU4cZgz0LsglhzS6V9Jsa64kZzoIsqOIqJm9a-zsRCo7zkWTDRK1C5M77AtPil6OdejogIsru0LXoD74GYO0OjY8XEMPrxcRdqrcVUb1mhFxIGwJxGkaEx9Z-a0q4Oti8mO11d0R1OrTBrM8R_SAuo_FnlKRAkyHrya3EsJGeUenYsNgeYkGV8Dw2QZdKTJZKQHPQHnl1HuT0XJU8QzsXMTFMymujijlIPgF3-5X0TKqPE86rDH2fzXcmfo2nsckJ560DRW8VA2jV3wcKPa3i_hFqJklg7iqMm9Zi-s0dkLxgY6GK69MspMf5imJ7eVYLzlIY2Dr9AxIDIytElgpXsugufEtCC4gBn2N2dpfC6qiQKOpZ7_TL3wmFiFayGsplxPz4alSfEPvfEceIU0x7ZzxlfPsBh48ne0GtldAyg4v8t0C2Pmz5qZSVqQSfbhu_AS3O2gVOeRf1E0wqBkfuz97PPbX0R_MpUD9FmAFC2DTnTkS4kU-ygDsFRDe8bRTTHoXNfn9Caby2jlp5ZQKSfipR_HmLO5mG8PwyxzP5EQ0PVcBIozgHanfhgSUku_cXiDFOxLZfbhALRp3yWAYnG5myHGnLJhzld8RJtIxsJ0QnAA",
    // dev
    "dev.justyo.app":
      "AABte-FyyNG7G0rm_Swi9dLOf6khh32C0-TmfHELCW5J4k4m_beuhn-LDZ6GFT19xzzza_-DQfEKUPdsKy99Knl4FgkgVTu8j8yZP5JoagLdul10PyCTaUcx649Z-JJuUWidD_T_ZYOgvSS61z9QixJQB6LAB_k0AYFY563qzWItEncY_vD68l2OUM1aTDNuXO6QeT0MgeRwrlsqLhF10C2SMQJotdkaEOGAMdr4E261qG0CNuVEua6ojEXs7BXCOpTIPvZhkxQf6N2-WWXVXyrm65kQSDkE32SYFSTxhP9dNNdVop_SDJE24rMYPJy1SH5DD1gs7Y67w0RNQIooUHmaUn3WlOjHXFMgwHpDDMGlDuMqgEMkz1D-ArhgAoYsXG6LduuWJgSRWi3KED6jZRm2QCUpd_LIkZKR2vmQp_Y68IvRf_myayJE33VuqTbfT0jp9KBeiJuIOdqubZBWb2vG_jI0ihgyFRJPTrFtpW0iv1tpngyN0m7_V3I-jUlzIGQSJRJDvDvlEvKPMDeA9Sc2Nh5tRY3JiMTqcFwCVMsbCujL_mhC4VBfXUFy-urGdLiqdTBZF53mInuT_yR8VFoSPLeTi7mHnRSlACAnh_GFnE80FlbXVEbW8EdMXzwycwLy3GajU4z0wOLmeI58jYlwZw8A-dNcB2CHG4zBwpkTiD2ZHZcdAL4kSljBUqE2Tml9cfKtGzqcvH7zrxOhjCL6Sn9IyV36TVf6lKf-oRjDHsPHP9pSp3apVZfG1dEdRQr2UEvHRHla3C8jQrte8e02zrqQwG0KxsUTueOVgiVsWtaUV3YohIHtxbSUOljwoaA-iRmPQsvUHgcpL2UoOghIMYvKSbwRIde_-NYhyQVSCBpcgz5bhRbHcc-uS_j7Qg",
  ]

  /// Right after a client is made.
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

  /// A blocking core call on threads of its own: blocked on Swift's few shared threads, a burst of calls would stall
  /// everything else async (the inbox's long wait too).
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

  /// The user id's 32 hex digits as 16 bytes (a passkey's user handle, the backup's id, spec 7).
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

/// The `Yo-App` header (spec 6.3), and whether the hub said this build is too old (426).
@MainActor @Observable
final class AppUpdate {
  static let shared = AppUpdate()
  private(set) var needed = false

  /// "ios/202611140619".
  nonisolated static let header = "ios/" + (Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0")

  nonisolated static func tooOld() {
    Task { @MainActor in shared.needed = true }
  }
}

/// Blocking HTTP for the core (which runs off the main actor, `Hub.run`).
final class HubTransport: HttpTransport {
  /// After a 426 nothing more is sent by this process; retry loops stop on `unsupported`.
  private static let tooOld = Atomic<Bool>(false)
  /// The hub answered a request (the app ends its "No Connection" banner, `Problems`); nil in the extensions.
  nonisolated(unsafe) static var answered: (@Sendable () -> Void)?

  static var unsupported: Bool { tooOld.load(ordering: .relaxed) }

  let base: URL
  /// The inbox waits up to a minute; the notification extension has 30 s in all.
  let timeout: TimeInterval

  init(base: URL, timeout: TimeInterval = 70) {
    self.base = base
    self.timeout = timeout
  }

  /// Signed and anonymous requests never share a connection, cookies, cache or credentials (spec 1).
  private static let signedSession = URLSession(configuration: plain())
  private static let anonymousSession = URLSession(configuration: plain())

  /// The inbox's waits under way; cut off by `reconnect`.
  private static let polls = Mutex<[URLSessionTask]>([])

  /// Back in front or on another network: the connections from before (often dead after a suspension, where a request
  /// would hang until its timeout) are not used again, and the inbox's wait from before is cut off to be asked anew.
  static func reconnect() {
    signedSession.flush {}
    anonymousSession.flush {}
    for task in polls.withLock({ polls in defer { polls.removeAll() }; return polls }) { task.cancel() }
  }

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
    let started = ContinuousClock.now
    let signed = headers.contains(where: { $0.name.lowercased() == "yo-device" })
    // Only what names nobody goes anonymous (yo_wire::api::anonymous, spec 1).
    let anonymous = !signed && anonymousRequest(method: method, path: path)
    let task = (anonymous ? Self.anonymousSession : Self.signedSession).dataTask(with: request)
    let diagnosticUpload = path == "/v1/help/error" || path == "/v1/help/crash"
    // Only static route components: never account, blob, group or username path values.
    let codePlace = "HubTransport.request \(method) " + path.split(separator: "/").prefix(2).joined(separator: "/")
    // Expected, never reported: our own cancels (too large is reported as 413), the long poll cut off, a username
    // checked while typed, no storage yet.
    let route = path.split(separator: "?", maxSplits: 1).first.map(String.init) ?? path
    let longPoll = method == "GET" && path.hasPrefix("/v1/inbox?wait=")
    let quietErrors: Set<Int> = longPoll
      ? [NSURLErrorCancelled, NSURLErrorTimedOut, NSURLErrorNetworkConnectionLost] : [NSURLErrorCancelled]
    let quietStatus: Set<UInt16> =
      method != "GET" ? [] : route.hasPrefix("/v1/username/") ? [400, 409] : route == "/v1/storage" ? [404] : []
    // Answers the core takes as answers, counted as expected (not a problem): a 401 it signs again for (a time the
    // hub saw already, a device still registering), a limit, the hub restarting behind its proxy, no such name or no
    // commitment told (invited instead), a name too short at sign-up (the hub's 400, shown as such).
    var expectedStatus: Set<UInt16> = [401, 429, 502, 503, 504]
    if method == "POST", route == "/v1/lookup" || route == "/v1/group-credential" { expectedStatus.insert(404) }
    if method == "POST", route == "/v1/register" { expectedStatus.insert(400) }
    // Cut off past the largest answer: a hostile hub never fills an extension's 24 MB.
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
    if longPoll { Self.polls.withLock { $0.append(task) } }
    do {
      if let access { try access.start(task) }
      else { task.resume() }
    } catch {
      task.cancel()
      if longPoll { Self.polls.withLock { $0.removeAll { $0 === task } } }
      return HttpResult(status: 0, body: Data(), error: "shared storage time ended")
    }
    done.wait()
    if longPoll { Self.polls.withLock { $0.removeAll { $0 === task } } }
    if access?.expired == true {
      // Not reported: the caller tries again in its next turn.
      return HttpResult(status: 0, body: Data(), error: "shared storage time ended")
    }
    if !diagnosticUpload, answer.result.status >= 400, !quietStatus.contains(answer.result.status) {
      if expectedStatus.contains(answer.result.status) {
        CountedErrors.record(domain: "HTTP", place: codePlace, code: String(answer.result.status), expected: true)
      } else {
        Diagnostics.record(NSError(domain: "HTTP", code: Int(answer.result.status)), place: codePlace)
      }
    }
    Availability.network(answer.result.status == 0 || answer.result.status >= 500, started: started)
    if answer.result.status > 0 && answer.result.status < 500 { Self.answered?() }
    if Perf.on {
      let ms = (CACurrentMediaTime() - start) * 1000
      let route = path.split(separator: "/").prefix(2).joined(separator: "/")  // not the ids after it
      Perf.mark("http \(method) \(route) \(answer.result.status) " + String(format: "%.1f ms", ms))
    }
    return answer.result
  }

  /// Written once before the semaphore is signalled.
  private final class Answer: @unchecked Sendable {
    var result = HttpResult(status: 0, body: Data(), error: "no answer")
  }

  /// A 10 MiB blob with tag and padding is the largest answer (spec 6.3, 6.8).
  static let largestAnswer = 11 * 1024 * 1024

  /// Cancelled once the answer says or grows past `largestAnswer`; `done` runs once.
  private final class Limited: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private var data = Data()
    private var response: URLResponse?
    private var tooLarge = false
    private let diagnosticPlace: String?
    private let quiet: Set<Int>
    private let done: (Data, URLResponse?, String?) -> Void
    /// No way through the phone's network: timed out, no host, no connection, lost, offline, roaming or cellular
    /// data off, a TLS handshake broken on the way (a captive portal), a "bad URL" said of the fixed hub address (a
    /// proxy or VPN on the way; a path that is no URL is caught before the request as HubTransport -1000).
    static let offline: Set<Int> = [
      NSURLErrorBadURL, NSURLErrorTimedOut, NSURLErrorCannotFindHost, NSURLErrorCannotConnectToHost, NSURLErrorNetworkConnectionLost,
      NSURLErrorDNSLookupFailed, NSURLErrorNotConnectedToInternet, NSURLErrorInternationalRoamingOff,
      NSURLErrorDataNotAllowed, NSURLErrorSecureConnectionFailed,
    ]

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
      if let error, let diagnosticPlace {
        let error = error as NSError
        if error.domain == NSURLErrorDomain, quiet.contains(error.code) {
          // Not counted.
        } else if error.domain == NSURLErrorDomain, Self.offline.contains(error.code) {
          // The phone's network, not Yo: counted as expected; the core tries again.
          CountedErrors.record(domain: error.domain, place: diagnosticPlace, code: String(error.code), expected: true)
        } else {
          Diagnostics.record(error, place: diagnosticPlace)
        }
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

/// The state database for builds without an App Group (else SharedState.swift).
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

/// The 32-byte state key, this device only: in the App Group's keychain group (extensions read it), Yo Dev in its own.
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

  /// The app's own key. Nil when there is none; a keychain that does not answer now throws: never a new key for it.
  static func load() throws -> Data? { try read(query) }

  /// For a registration: the one there, else a new one, used only once it reads back as stored.
  static func make() throws -> Data {
    if let key = try Keychain.read(query), key.count == 32 { return key }
    let key = random()
    try Keychain.write(query, key, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    return key
  }

  /// The group's key, as `load`.
  static func shared() throws -> Data? { try read(inGroup) }

  private static func read(_ query: [String: Any]) throws -> Data? {
    guard let key = try Keychain.read(query) else { return nil }
    guard key.count == 32 else { throw Keychain.Failure(status: errSecDecode) }
    return key
  }

  /// For a registration: without a new key when the old one is gone, every registration would fail for good.
  static func makeShared() throws -> Data {
    if let key = try shared() { return key }
    let key = random()
    try Keychain.write(inGroup, key, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    return key
  }

  /// Yo Dev (`Keychain.scoped`): keys of this name in the team's other groups. Older builds took the first one found,
  /// so its state may be sealed under one; tried only when its own key fails, never changed there.
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

  private static func random() -> Data { Data((0..<32).map { _ in UInt8.random(in: .min ... .max) }) }
}

/// A keychain that does not answer now is never taken for one without the item: only `errSecItemNotFound` is "none".
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

  /// Nil while the own group is not known yet (before the first unlock): "not now", never none.
  static func own(_ query: [String: Any]) -> [String: Any]? {
    guard scoped, query[kSecAttrAccessGroup as String] == nil else { return query }
    guard let group = ownGroup() else { return nil }
    return query.merging([kSecAttrAccessGroup as String: group]) { $1 }
  }

  /// The app's default access group, probed once per launch from an item added without a group.
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
      // Only this app adds it (its bundle id), so the one found is its own.
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

  /// errSecInteractionNotAllowed while the own group is not known.
  @discardableResult
  static func delete(_ query: [String: Any]) -> OSStatus {
    guard let query = own(query) else { return errSecInteractionNotAllowed }
    return SecItemDelete(query as CFDictionary)
  }

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

  /// Updated in place (never deleted first), then read back.
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
  /// base64url without padding (spec 6.1).
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
  /// A request asked since Yo came to the front failed. One from before (cut off or hung by the suspension) is no sign.
  var remote = false { didSet { settle() } }
  /// Only after a moment unreachable (a launch's or a return's first try is not "offline").
  private(set) var offline = false
  private var pending: Task<Void, Never>?
  @ObservationIgnored private var front = true
  @ObservationIgnored private var since = ContinuousClock.now

  /// In front again, or on another network: what failed before counts no more; only a new try can say "Offline".
  func foreground() {
    front = true
    since = .now
    remote = false
  }

  /// Out of front: nothing on screen to warn, and a failure there (the suspension) says nothing about the return.
  func background() {
    front = false
    remote = false
  }

  private func settle() {
    guard local || remote else {
      pending?.cancel()
      pending = nil
      offline = false
      return
    }
    guard !offline, pending == nil else { return }
    pending = Task { @MainActor [weak self] in
      try? await Task.sleep(for: .seconds(2))
      guard let self, !Task.isCancelled else { return }
      self.pending = nil
      self.offline = self.local || self.remote
    }
  }
  nonisolated static func network(_ failed: Bool, started: ContinuousClock.Instant) {
    Task { @MainActor in
      if failed && (!shared.front || started < shared.since) { return }
      shared.remote = failed
    }
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
    return format(bytes, unit: true)
  }
  private static func format(_ bytes: Double, unit: Bool) -> String {
    (bytes / 1_000_000).formatted(.number.precision(.fractionLength(1))) + (unit ? " MB" : "")
  }
}
