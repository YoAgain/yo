import Foundation

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
