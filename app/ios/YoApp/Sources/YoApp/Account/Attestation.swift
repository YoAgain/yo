import CryptoKit
import DeviceCheck
import Foundation
import Security
import YoCore

enum Attestation {
  /// Once per client, not per launch: a second account made in the same run is attested too.
  @MainActor private static weak var started: YoClient?

  @MainActor static func start(_ client: YoClient) {
    guard started !== client else { return }
    started = client
    Task.detached(priority: .background) {
      guard client.attestDue(nowMs: now()) else { return }
      do {
        guard let challenge = try client.attestChallenge() else { return }
        let service = DCAppAttestService.shared
        guard service.isSupported else {
          try? client.attestFailed(nowMs: now())
          return
        }
        let key = try await service.generateKey()
        let query: [String: Any] = [
          kSecClass as String: kSecClassGenericPassword,
          kSecAttrService as String: "app.justyo.app-attest",
          kSecAttrAccount as String: "key",
        ]
        guard Keychain.put(query, Data(key.utf8), accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly) == errSecSuccess
        else {
          try? client.attestFailed(nowMs: now())
          return
        }
        let object = try await service.attestKey(key, clientDataHash: Data(SHA256.hash(data: challenge)))
        _ = try? client.attestSend(platform: 1, payload: object) // The core records send failures.
      } catch {
        try? client.attestFailed(nowMs: now())
      }
    }
  }

  private static func now() -> UInt64 { UInt64(Date().timeIntervalSince1970 * 1000) }
}
