// Crash reports, sent on their own (Settings › "Send Crash Reports", on by default): iOS hands the app its crashes
// at the next start (MetricKit, at most a day later); each goes to the Yo team as feedback (POST /v1/feedback), a
// few lines: the build, iOS, the kind of crash and the crashed thread's frames. Nothing of the person's Yos, names
// or photos is in them (the frames are offsets into the binaries).
import Foundation
import MetricKit
import YoCore

@MainActor
final class CrashReports: NSObject, MXMetricManagerSubscriber {
  static let shared = CrashReports()

  private static let onKey = "crashReports.on"
  private static let pendingKey = "crashReports.pending"

  /// Whether crashes go to the Yo team on their own.
  static var on: Bool {
    get { UserDefaults.standard.object(forKey: onKey) as? Bool ?? true }
    set {
      UserDefaults.standard.set(newValue, forKey: onKey)
      if !newValue { UserDefaults.standard.removeObject(forKey: pendingKey) }
    }
  }

  /// Reports waiting for the account to open (or the hub to answer): at most five, the newest.
  private var pending: [String] {
    get { UserDefaults.standard.stringArray(forKey: Self.pendingKey) ?? [] }
    set { UserDefaults.standard.set(Array(newValue.suffix(5)), forKey: Self.pendingKey) }
  }

  /// Listens from the app's start on.
  func start() { MXMetricManager.shared.add(self) }

  nonisolated func didReceive(_ payloads: [MXDiagnosticPayload]) {
    let texts = payloads.flatMap { $0.crashDiagnostics ?? [] }.map(Self.text)
    guard !texts.isEmpty else { return }
    Task { @MainActor in
      guard Self.on else { return }
      pending += texts
    }
  }

  /// Sends what waits, with the account's client; kept for the next time if the hub does not take it now.
  func flush(_ client: YoClient) {
    guard Self.on, !pending.isEmpty else { return }
    let reports = pending
    let version = FeedbackSheet.appVersion
    Task {
      var left: [String] = []
      for report in reports {
        do {
          try await Hub.run { try client.sendFeedback(text: report, appVersion: version) }
        } catch let error as YoError where retryLater(error: error) {
          left.append(report)
        } catch {}  // refused for good (too long, too many today): not again
      }
      pending = left
    }
  }

  /// A crash in a few lines, within the hub's 2000 bytes.
  nonisolated private static func text(_ crash: MXCrashDiagnostic) -> String {
    let meta = crash.metaData
    var lines = [
      "[crash] \(meta.applicationBuildVersion) · \(meta.osVersion) · \(meta.deviceType)",
      "exception \(crash.exceptionType.map { "\($0)" } ?? "-") signal \(crash.signal.map { "\($0)" } ?? "-")"
        + (crash.terminationReason.map { " · \($0)" } ?? ""),
    ]
    if let reason = crash.exceptionReason { lines.append("\(reason.exceptionName): \(reason.composedMessage)") }
    lines += frames(crash.callStackTree.jsonRepresentation())
    var text = lines.joined(separator: "\n")
    while text.utf8.count > 2000 { text = String(text.dropLast(100)) }
    return text
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
      let name = frame["binaryName"] as? String ?? "?"
      let offset = (frame["offsetIntoBinaryTextSegment"] as? NSNumber)?.uint64Value ?? 0
      out.append("\(name) +0x\(String(offset, radix: 16))")
      guard let next = (frame["subFrames"] as? [[String: Any]])?.first else { break }
      frame = next
    }
    return out
  }
}
