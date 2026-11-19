// Timings (../../TESTING.md, "Speed"): with YO_PERF each mark is a line in perf.log in the temporary folder
// (YO_PERF=stderr, =syslog elsewhere): CACurrentMediaTime seconds, wall-clock ms (to compare across processes), what.
// Without YO_PERF nothing is written or kept.
import Foundation
import QuartzCore
import os

enum Perf {
  private static let logger = Logger(subsystem: "app.justyo.perf", category: "run")
  private static let signposts = OSLog(subsystem: "app.justyo.perf", category: .pointsOfInterest)
  private static let spans = NSLock()
  nonisolated(unsafe) private static var mainSpans: [(Double, Double, String)] = []

  static func names(from start: Double, to end: Double) -> [String] {
    spans.lock()
    defer { spans.unlock() }
    return Array(Set(mainSpans.filter { $0.0 < end && $0.1 > start }.map { $0.2 })).sorted()
  }

  static func report(_ json: String) { logger.notice("YO_PERF_RUN \(json, privacy: .public)") }

  static let on = ProcessInfo.processInfo.environment["YO_PERF"] != nil
  private static let toStderr = ProcessInfo.processInfo.environment["YO_PERF"] == "stderr"
  /// For a real phone's cold start ("YO_PERF_MARK").
  private static let toSyslog = ProcessInfo.processInfo.environment["YO_PERF"] == "syslog"
  private static let queue = DispatchQueue(label: "app.justyo.perf", qos: .utility)
  /// Written only on `queue`.
  nonisolated(unsafe) private static var file: FileHandle?
  /// For `dump`: the device's log drops lines in a launch's rush. Only on `queue`.
  nonisolated(unsafe) private static var kept: [String] = []

  /// All marks so far again, a few per log line ("YO_PERF_DUMP").
  static func dump() {
    guard toSyslog else { return }
    queue.async {
      var chunk = ""
      for line in kept {
        if chunk.count + line.count > 800 {
          logger.notice("YO_PERF_DUMP \(chunk, privacy: .public)")
          chunk = ""
        }
        chunk += line
      }
      if !chunk.isEmpty { logger.notice("YO_PERF_DUMP \(chunk, privacy: .public)") }
      kept.removeAll()
    }
  }

  static var url: URL { FileManager.default.temporaryDirectory.appending(path: "perf.log") }

  static func mark(_ what: @autoclosure () -> String, at time: Double = CACurrentMediaTime()) {
    guard on else { return }
    let line = String(format: "%.6f %.3f ", time, Date.now.timeIntervalSince1970 * 1000) + what() + "\n"
    queue.async { write(line) }
  }

  /// `work`, marked with how long it took ("open 12.3 ms").
  static func timed<T>(_ what: String, _ work: () throws -> T) rethrows -> T {
    guard on else { return try work() }
    let start = CACurrentMediaTime()
    let signpost = OSSignpostID(log: signposts)
    os_signpost(.begin, log: signposts, name: "Yo work", signpostID: signpost, "%{public}@", what)
    let main = Thread.isMainThread
    defer {
      let end = CACurrentMediaTime()
      os_signpost(.end, log: signposts, name: "Yo work", signpostID: signpost)
      if main {
        spans.lock()
        mainSpans.append((start, end, what))
        if mainSpans.count > 4096 { mainSpans.removeFirst(2048) }
        spans.unlock()
      }
      mark("\(what) " + String(format: "%.1f ms", (end - start) * 1000))
    }
    return try work()
  }

  private static func write(_ line: String) {
    if toStderr { return FileHandle.standardError.write(Data(line.utf8)) }
    if toSyslog {
      kept.append(line)
      return logger.notice("YO_PERF_MARK \(line, privacy: .public)")
    }
    if file == nil {
      // Appending: the file may be emptied between measurements.
      let fd = open(url.path, O_WRONLY | O_APPEND | O_CREAT, 0o644)
      if fd >= 0 { file = FileHandle(fileDescriptor: fd, closeOnDealloc: true) }
    }
    file?.write(Data(line.utf8))
  }
}
