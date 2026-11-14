// Timings, for measuring in the Simulator (../../TESTING.md, "Speed"): with YO_PERF in the environment each mark is
// a line in perf.log in the process's temporary folder (YO_PERF=stderr: on standard error): the time in seconds as
// UIEvent timestamps count it (CACurrentMediaTime), the wall clock in milliseconds (to compare across processes and
// with the hub), and what. Without YO_PERF nothing is written or kept.
import Foundation
import QuartzCore

enum Perf {
  static let on = ProcessInfo.processInfo.environment["YO_PERF"] != nil
  private static let toStderr = ProcessInfo.processInfo.environment["YO_PERF"] == "stderr"
  private static let queue = DispatchQueue(label: "app.justyo.perf", qos: .utility)
  /// Written only on `queue`.
  nonisolated(unsafe) private static var file: FileHandle?

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
    defer { mark("\(what) " + String(format: "%.1f ms", (CACurrentMediaTime() - start) * 1000)) }
    return try work()
  }

  private static func write(_ line: String) {
    if toStderr { return FileHandle.standardError.write(Data(line.utf8)) }
    if file == nil {
      // Appending: the file may be emptied between measurements.
      let fd = open(url.path, O_WRONLY | O_APPEND | O_CREAT, 0o644)
      if fd >= 0 { file = FileHandle(fileDescriptor: fd, closeOnDealloc: true) }
    }
    file?.write(Data(line.utf8))
  }
}
