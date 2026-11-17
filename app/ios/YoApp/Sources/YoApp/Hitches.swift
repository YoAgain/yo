import SwiftUI
import UIKit

#if YO_DEV
/// Callback intervals are a scheduling proxy, not GPU presentation measurements.
@MainActor final class HitchMonitor: NSObject {
  static let shared = HitchMonitor()
  var scene = "list"
  private var link: CADisplayLink?
  private var previous: CFTimeInterval?
  private let queue = DispatchQueue(label: "app.justyo.diagnostics", qos: .utility)
  struct Entry: Codable, Sendable {
    var kind = "hitch"
    let scene: String
    let milliseconds: Double
    let date: Date
  }
  nonisolated static var url: URL {
    URL.documentsDirectory.appending(path: "diagnostics-hitches.json")
  }
  nonisolated static func read() -> [Entry] {
    guard let data = try? Data(contentsOf: url) else { return [] }
    return (try? JSONDecoder().decode([Entry].self, from: data)) ?? []
  }
  func start() {
    guard link == nil else { return }
    NotificationCenter.default.addObserver(self, selector: #selector(reset), name: UIApplication.didBecomeActiveNotification, object: nil)
    NotificationCenter.default.addObserver(self, selector: #selector(reset), name: UIApplication.willResignActiveNotification, object: nil)
    let link = CADisplayLink(target: self, selector: #selector(tick))
    link.add(to: .main, forMode: .common)
    self.link = link
  }
  @objc private func reset() { previous = nil }
  @objc private func tick(_ link: CADisplayLink) {
    guard UIApplication.shared.applicationState == .active else { previous = nil; return }
    let now = CACurrentMediaTime()
    defer { previous = now }
    guard let previous else { return }
    let duration = link.targetTimestamp - link.timestamp
    guard duration > 0, now - previous > 1.5 * duration else { return }
    let entry = Entry(scene: scene, milliseconds: (now - previous) * 1000, date: .now)
    queue.async {
      let entries = Array((Self.read() + [entry]).suffix(100))
      if let data = try? JSONEncoder().encode(entries) { try? data.write(to: Self.url, options: .atomic) }
    }
  }
  func clear() async {
    await withCheckedContinuation { continuation in
      queue.async {
        try? FileManager.default.removeItem(at: Self.url)
        continuation.resume()
      }
    }
  }
}

struct HitchesRows: View {
  @State private var expanded = false
  @State private var entries: [HitchMonitor.Entry] = []
  var body: some View {
    Group {
      SettingsExpansion(expanded: $expanded) {
        Label("Hitches", systemImage: "waveform.path").labelStyle(Tile(.gray))
      }
      if expanded {
        ForEach(Dictionary(grouping: entries, by: \.scene).keys.sorted(), id: \.self) { scene in
          let group = entries.filter { $0.scene == scene }
          LabeledContent(scene, value: "\(group.count) · worst \(Int(group.map(\.milliseconds).max() ?? 0)) ms")
        }
        Text("Last 100").font(.footnote).foregroundStyle(.secondary)
        if entries.isEmpty {
          Text("No Hitches").foregroundStyle(.secondary)
        } else {
          ForEach(Array(entries.reversed().enumerated()), id: \.offset) { _, entry in
            LabeledContent(entry.scene, value: "\(Int(entry.milliseconds)) ms")
          }
        }
        Button("Clear", role: .destructive) {
          Task { await HitchMonitor.shared.clear(); entries = [] }
        }
        .disabled(entries.isEmpty)
      }
    }
    .task(id: expanded) {
      guard expanded else { return }
      entries = await Task.detached { HitchMonitor.read() }.value
    }
  }
}
#endif

extension View {
  func hitchScene(_ scene: String) -> some View {
    onAppear {
      #if YO_DEV
      HitchMonitor.shared.start()
      HitchMonitor.shared.scene = scene
      #endif
    }
  }
}

@MainActor enum HitchScene {
  static func event(_ name: String, then scene: String, seconds: Double = 0.7) {
    #if YO_DEV
    HitchMonitor.shared.scene = name
    Task {
      try? await Task.sleep(for: .seconds(seconds))
      if HitchMonitor.shared.scene == name { HitchMonitor.shared.scene = scene }
    }
    #endif
  }
}
