// Crash reports from MetricKit, sent anonymously by default (off under Status › Send Crashes) and without the account
// (POST /v1/diagnostics/crash): build, iOS, crash kind and sanitized frames (binary offsets), nothing of Yos, names or photos.
import Foundation
import MetricKit
import SwiftUI
import CryptoKit
import YoCore

@MainActor @Observable
final class CrashReports: NSObject, MXMetricManagerSubscriber {
  static let shared = CrashReports()

  private static let onKey = "crashReports.on"
  struct Record: Codable, Identifiable, Hashable {
    var id: UUID
    let date: Date
    let text: String
    var summary: String {
      if let data = text.data(using: .utf8),
        let payload = try? JSONSerialization.jsonObject(with: data) as? [String: String] {
        return String((payload["exception"] ?? "Crash").prefix(60))
      }
      let lines = text.split(separator: "\n", omittingEmptySubsequences: true)
      return lines.count > 1 ? String(lines[1].prefix(60)) : "Crash"
    }
  }

  var history: [Record] {
    _ = revision
    return Diagnostics.retainedCrashes().compactMap { row in
      guard let id = row["id"].flatMap(UUID.init(uuidString:)), let text = row["text"] else { return nil }
      return Record(id: id, date: Date(timeIntervalSince1970: Double(row["at"] ?? "") ?? 0), text: text)
    }
  }
  private var revision = 0

  static var on: Bool {
    get { UserDefaults.standard.object(forKey: onKey) as? Bool ?? true }
    set {
      UserDefaults.standard.set(newValue, forKey: onKey)
      if newValue { shared.flush() }
    }
  }

  private var pending: [String] { Diagnostics.pending("crash") }

  private var sending = false
  private var started = false
  func start() {
    guard !started else { return }; started = true
    MXMetricManager.shared.add(self)
  }

  nonisolated func didReceive(_ payloads: [MXDiagnosticPayload]) {
    let reports = payloads.flatMap { payload in
      (payload.crashDiagnostics ?? []).map { crash in
        (String(describing: payload.timeStampBegin) + String(describing: payload.timeStampEnd)
          + SHA256.hash(data: crash.jsonRepresentation()).map { String(format: "%02x", $0) }.joined(), Self.text(crash))
      }
    }
    guard !reports.isEmpty else { return }
    let epoch = UserDefaults.standard.string(forKey: "diagnostics.generation")
    Task { @MainActor in
      guard UserDefaults.standard.string(forKey: "diagnostics.generation") == epoch else { return }
      revision += 1
      var seen = UserDefaults.standard.stringArray(forKey: "crashReports.seen") ?? []
      for (id, text) in reports where !seen.contains(id) {
        Diagnostics.append(text, kind: "crash", epoch: epoch)
        seen.append(id)
      }
      UserDefaults.standard.set(seen, forKey: "crashReports.seen")
      if Self.on { flush() }
    }
  }

  /// Sends what waits, without the account; kept for the next time if the hub does not take it now.
  func flush() {
    CountedErrors.collect()  // the app's and extensions' counters, as errors
    Diagnostics.flush()
    guard Self.on, !pending.isEmpty, !sending else { return }
    sending = true
    let transport = Hub.transport
    let epoch = UserDefaults.standard.string(forKey: "diagnostics.generation")
    let version = HelpPage.appVersion
    Task {
      defer { sending = false }
      while Self.on, UserDefaults.standard.string(forKey: "diagnostics.generation") == epoch, let report = pending.first {
        do {
          try await Hub.run(reportErrors: false) {
            if report.hasPrefix("{") {
              let answer = transport.request(method: "POST", path: "/v1/diagnostics/crash", headers: [], body: Data(report.utf8))
              guard answer.status == 204 else { throw YoError.Refused(status: answer.status) }
            } else {
              try sendCrashReport(transport: transport, text: report, appVersion: version)
            }
          }
          if Self.on, UserDefaults.standard.string(forKey: "diagnostics.generation") == epoch, pending.first == report { Diagnostics.markSent(report, epoch: epoch) }
        } catch { return }
      }
    }
  }

  /// Crash code metadata and sanitized stack details, within the hub's 60 KiB cap.
  nonisolated private static func text(_ crash: MXCrashDiagnostic) -> String {
    let meta = crash.metaData
    var payload = ["uuid": UUID().uuidString, "app": "ios/" + meta.applicationBuildVersion, "os": meta.osVersion, "model": meta.deviceType,
      "exception": "\(crash.exceptionType.map { "\($0)" } ?? "-")/\(crash.exceptionCode.map { "\($0)" } ?? "-")/\(crash.signal.map { "\($0)" } ?? "-")",
      "termination": String((crash.terminationReason ?? "").prefix(2000)),
      "frames": frames(crash.callStackTree.jsonRepresentation()).joined(separator: "\n"),
      "details": stackDetails(crash.callStackTree.jsonRepresentation())]
    var encoded = try! JSONSerialization.data(withJSONObject: payload)
    while encoded.count > 60 * 1024, let details = payload["details"], !details.isEmpty {
      payload["details"] = String(details.dropLast(1000))
      encoded = try! JSONSerialization.data(withJSONObject: payload)
    }
    return String(data: encoded, encoding: .utf8)!
  }

  /// Keep only binary names and offsets, never arbitrary diagnostic strings or exception messages.
  nonisolated private static func stackDetails(_ data: Data) -> String {
    let fields = Set(["callStacks", "callStackRootFrames", "subFrames", "threadAttributed", "binaryName", "offsetIntoBinaryTextSegment"])
    func clean(_ value: Any, depth: Int = 0) -> Any {
      guard depth < 128 else { return NSNull() }
      if let array = value as? [Any] { return array.map { clean($0, depth: depth + 1) } }
      if let object = value as? [String: Any] {
        return object.filter { fields.contains($0.key) }.mapValues { clean($0, depth: depth + 1) }
      }
      if let name = value as? String { return URL(fileURLWithPath: name).lastPathComponent }
      return value as? NSNumber ?? 0
    }
    guard let tree = try? JSONSerialization.jsonObject(with: data),
      let safe = try? JSONSerialization.data(withJSONObject: clean(tree), options: [.sortedKeys]),
      let text = String(data: safe, encoding: .utf8) else { return "" }
    return String(decoding: text.utf8.prefix(40_000), as: UTF8.self)
  }

  /// The crashed thread's frames, innermost first: "YoApp +0x1c2f4c".
  nonisolated private static func frames(_ json: Data) -> [String] {
    guard let tree = try? JSONSerialization.jsonObject(with: json) as? [String: Any],
      let stacks = tree["callStacks"] as? [[String: Any]],
      let crashed = stacks.first(where: { $0["threadAttributed"] as? Bool == true }) ?? stacks.first,
      var frame = (crashed["callStackRootFrames"] as? [[String: Any]])?.first
    else { return [] }
    var out: [String] = []
    while out.count < 25 {
      let name = URL(fileURLWithPath: frame["binaryName"] as? String ?? "?").lastPathComponent
      let offset = (frame["offsetIntoBinaryTextSegment"] as? NSNumber)?.uint64Value ?? 0
      out.append("\(name) +0x\(String(offset, radix: 16))")
      guard let next = (frame["subFrames"] as? [[String: Any]])?.first else { break }
      frame = next
    }
    return out
  }
}

struct CrashListPage: View {
  let crashes: [CrashReports.Record]

  var body: some View {
    List(crashes.reversed()) { crash in
      NavigationLink {
        CrashDetailPage(crash: crash)
      } label: {
        LabeledContent(crash.summary, value: crash.date.formatted(date: .abbreviated, time: .shortened))
      }
    }
    .navigationTitle("Crash Reports")
    .navigationBarTitleDisplayMode(.inline)
  }
}

struct CrashDetailPage: View {
  let crash: CrashReports.Record

  var body: some View {
    ScrollView {
      Text(crash.text)
        .font(.footnote.monospaced())
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(20)
    }
    .background(Color(.systemGroupedBackground))
    .navigationTitle(crash.date.formatted(date: .abbreviated, time: .shortened))
    .navigationBarTitleDisplayMode(.inline)
    .toolbar {
      ToolbarItem(placement: .primaryAction) {
        ShareLink(item: crash.text)
      }
    }
  }
}
