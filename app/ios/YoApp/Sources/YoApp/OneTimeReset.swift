import Foundation
import Security

/// Once per device (2026-11-16): the local state some phones could no longer open is cleared as a reinstall would —
/// the app's own and the App Group's files and defaults. The account comes back from the keychain, which this keeps.
/// Done once: a marker in the keychain (it outlives updates, this reset and a reinstall). Removed in a later release
/// (admin to-do).
enum OneTimeReset {
  private static var marker: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "app.justyo.local-reset",
      kSecAttrAccount as String: "local-reset-2026-11-16",
    ]
  }

  static func runIfDue() {
    var probe = marker
    probe[kSecReturnData as String] = false
    let status = SecItemCopyMatching(probe as CFDictionary, nil)
    guard status == errSecItemNotFound else { return }  // done before, or the keychain cannot answer now: not now
    LocalReset.wipe()
    var add = marker
    add[kSecValueData as String] = Data([1])
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    SecItemAdd(add as CFDictionary, nil)
    print("Yo: one-time local reset done")
  }
}

/// Files and preferences only: keychain policy belongs to the caller.
enum LocalReset {
  static func wipe() {
    // Fence existing turns before removing anything. Preserve this coordination value when clearing defaults.
    SharedStateFile.rotateLock()
    let generation = SharedStateFile.generation
    let files = FileManager.default
    var places: [URL] = []
    if let group = files.containerURL(forSecurityApplicationGroupIdentifier: ShareBridge.group) { places.append(group) }
    for directory in [FileManager.SearchPathDirectory.documentDirectory, .applicationSupportDirectory, .cachesDirectory] {
      if let url = files.urls(for: directory, in: .userDomainMask).first { places.append(url) }
    }
    places.append(files.temporaryDirectory)
    for place in places {
      for item in (try? files.contentsOfDirectory(at: place, includingPropertiesForKeys: nil)) ?? [] {
        if item.lastPathComponent == "yo-state.lock." + generation { continue }
        if item.lastPathComponent == "Library" { continue }  // the App Group's Preferences go below
        try? files.removeItem(at: item)
      }
    }
    if let group = files.containerURL(forSecurityApplicationGroupIdentifier: ShareBridge.group) {
      let library = group.appending(path: "Library")
      for item in (try? files.contentsOfDirectory(at: library, includingPropertiesForKeys: nil)) ?? []
      where item.lastPathComponent != "Preferences" { try? files.removeItem(at: item) }
    }
    let defaults = UserDefaults(suiteName: ShareBridge.group)
    defaults?.setPersistentDomain([SharedStateFile.generationKey: generation], forName: ShareBridge.group)
    defaults?.synchronize()
    if let id = Bundle.main.bundleIdentifier { UserDefaults.standard.removePersistentDomain(forName: id) }
  }
}
