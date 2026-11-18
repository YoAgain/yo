// The app's side of Perf (YO_PERF): when a finger lifts (the touch's own timestamp), when a change was handed to the
// screen and the frame it shows in, and every frame that came late (a hitch: the main thread was busy).
import QuartzCore
import UIKit

@MainActor enum PerfProbe {
  private static var installed = false
  private static var launchListMs = -1.0

  /// From the app's start, for 4 s: each time the main thread answered late (more than 12 ms), with the timed main
  /// spans meanwhile ("main.lag 230 ms [list.syncFriends]").
  nonisolated static func watchLaunchLag() {
    guard Perf.on else { return }
    Thread.detachNewThread {
      let end = CACurrentMediaTime() + 4
      while CACurrentMediaTime() < end {
        let sent = CACurrentMediaTime()
        let done = DispatchSemaphore(value: 0)
        DispatchQueue.main.async { done.signal() }
        done.wait()
        let lag = CACurrentMediaTime() - sent
        if lag > 0.012 {
          let names = Perf.names(from: sent, to: sent + lag).joined(separator: ",")
          Perf.mark("main.lag \(Int(lag * 1000)) ms [\(names)]", at: sent)
        }
        usleep(2000)
      }
    }
  }

  /// YO_PERF_PROFILE=1: the main thread sampled every 2 ms for the launch's first `YO_PERF_PROFILE_MS` (800) ms;
  /// the functions found on its stack most often, as marks ("prof 42% UIKitCore:-[UIApplication _run]").
  /// Call on the main thread.
  nonisolated static func profileLaunch() {
    let env = ProcessInfo.processInfo.environment
    guard Perf.on, env["YO_PERF_PROFILE"] != nil else { return }
    let span = (Double(env["YO_PERF_PROFILE_MS"] ?? "") ?? 800) / 1000
    let main = mach_thread_self()
    let me = pthread_self()
    let top = UInt(bitPattern: pthread_get_stackaddr_np(me))
    let bottom = top - UInt(pthread_get_stacksize_np(me))
    Thread.detachNewThread {
      let capacity = 160
      let buffer = UnsafeMutablePointer<UInt>.allocate(capacity: capacity)
      var counts: [String: Int] = [:]
      var leaf: [String: Int] = [:]
      var samples = 0
      let mask: UInt = 0x0000_007F_FFFF_FFFF  // pointer authentication bits off
      let end = CACurrentMediaTime() + span
      while CACurrentMediaTime() < end {
        var n = 0
        if thread_suspend(main) == KERN_SUCCESS {
          var state = arm_thread_state64_t()
          var count = mach_msg_type_number_t(MemoryLayout<arm_thread_state64_t>.size / MemoryLayout<UInt32>.size)
          let kr = withUnsafeMutablePointer(to: &state) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
              thread_get_state(main, ARM_THREAD_STATE64, $0, &count)
            }
          }
          if kr == KERN_SUCCESS {
            buffer[0] = UInt(state.__pc) & mask
            buffer[1] = UInt(state.__lr) & mask
            n = 2
            var fp = UInt(state.__fp)
            while n < capacity, fp >= bottom, fp + 16 <= top, fp % 8 == 0 {
              let frame = UnsafePointer<UInt>(bitPattern: fp)!
              let next = frame[0]
              let ret = frame[1] & mask
              if ret == 0 { break }
              buffer[n] = ret
              n += 1
              if next <= fp { break }
              fp = next
            }
          }
          thread_resume(main)
        }
        if n > 0 {
          samples += 1
          var seen = Set<String>()
          for i in 0..<n {
            var info = Dl_info()
            guard dladdr(UnsafeRawPointer(bitPattern: buffer[i]), &info) != 0 else { continue }
            let image = info.dli_fname.map { String(cString: $0).split(separator: "/").last.map(String.init) ?? "" } ?? "?"
            let symbol = info.dli_sname.map { String(cString: $0) } ?? "?"
            let key = image + ":" + String(symbol.prefix(90))
            if i == 0 { leaf[key, default: 0] += 1 }
            if seen.insert(key).inserted { counts[key, default: 0] += 1 }
          }
        }
        usleep(2000)
      }
      buffer.deallocate()
      Perf.mark("prof samples \(samples)")
      for (key, value) in counts.sorted(by: { $0.value > $1.value }).prefix(70) where value * 50 >= samples {
        Perf.mark("prof \(value * 100 / max(samples, 1))% \(key)")
      }
      for (key, value) in leaf.sorted(by: { $0.value > $1.value }).prefix(25) {
        Perf.mark("leaf \(value * 100 / max(samples, 1))% \(key)")
      }
      Perf.dump()
    }
  }

  /// Once there is a window: the touch clock on every window and the hitch watch.
  static func install() {
    guard Perf.on, !installed else { return }
    installed = true
    for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
      for window in scene.windows { window.addGestureRecognizer(TouchClock()) }
    }
    Watch.shared.start()
    // When the process started, by the kernel's clock, in Perf's.
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.size
    var name: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
    if sysctl(&name, 4, &info, &size, nil, 0) == 0 {
      let started = info.kp_proc.p_un.__p_starttime
      let wall = Double(started.tv_sec) + Double(started.tv_usec) / 1e6
      launchListMs = (Date.now.timeIntervalSince1970 - wall) * 1000
      Perf.mark("launch.process", at: CACurrentMediaTime() - (Date.now.timeIntervalSince1970 - wall))
    }
    Perf.mark("launch.list")
    frame("launch")
  }

  /// After a change on the main thread: when this turn of the run loop has committed it (the screen has it), and the
  /// time of the frame that shows it.
  static func frame(_ what: String) {
    guard Perf.on else { return }
    let observer = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.beforeWaiting.rawValue, false, CFIndex.max) {
      observer, _ in
      Perf.mark("\(what).commit")
      CFRunLoopObserverInvalidate(observer)
      MainActor.assumeIsolated { Next(what).start() }
    }
    CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
  }

  /// Also used in normal builds: one publication at the next display callback.
  static func nextFrame(_ action: @escaping @MainActor () -> Void) {
    Publication(action).start()
  }

  @MainActor private final class Publication: NSObject {
    let action: @MainActor () -> Void
    init(_ action: @escaping @MainActor () -> Void) { self.action = action }
    func start() {
      let link = CADisplayLink(target: self, selector: #selector(tick))
      link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
      link.add(to: .main, forMode: .common)
    }
    @objc func tick(_ link: CADisplayLink) { link.invalidate(); action() }
  }

  /// Marks every touch's start and end, taking none (it never recognizes anything).
  final class TouchClock: UIGestureRecognizer, UIGestureRecognizerDelegate {
    init() {
      super.init(target: nil, action: nil)
      cancelsTouchesInView = false
      delaysTouchesBegan = false
      delaysTouchesEnded = false
      delegate = self
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
      for touch in touches { Perf.mark("touch.down", at: touch.timestamp) }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
      for touch in touches { Perf.mark("touch.up", at: touch.timestamp) }
      state = .failed
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
      state = .failed
    }

    func gestureRecognizer(_ g: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer)
      -> Bool
    { true }
  }

  /// The next frame's time, once.
  @MainActor private final class Next: NSObject {
    let what: String
    var link: CADisplayLink?

    init(_ what: String) { self.what = what }

    func start() {
      link = CADisplayLink(target: self, selector: #selector(tick))
      link?.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
      link?.add(to: .main, forMode: .common)
    }

    @objc func tick(_ link: CADisplayLink) {
      Perf.mark("\(what).frame", at: link.targetTimestamp)
      link.invalidate()
      self.link = nil
    }
  }

  /// Callback intervals are a CPU scheduling proxy, not GPU presentation measurements.
  @MainActor private final class Watch: NSObject {
    static let shared = Watch()
    private var last = 0.0
    private var samples: [Double] = []
    private var hitches = 0
    private var hitchMs = 0.0
    private var worst = 0.0
    private var worstNames: [String] = []
    private var link: CADisplayLink?

    @objc private func reset() { last = 0 }

    func start() {
      let link = CADisplayLink(target: self, selector: #selector(tick))
      self.link = link
      NotificationCenter.default.addObserver(self, selector: #selector(reset), name: UIApplication.didBecomeActiveNotification, object: nil)
      NotificationCenter.default.addObserver(self, selector: #selector(reset), name: UIApplication.willResignActiveNotification, object: nil)
      link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
      link.add(to: .main, forMode: .common)
      let seconds = Double(ProcessInfo.processInfo.environment["YO_PERF_SECONDS"] ?? "10") ?? 10
      Task {
        try? await Task.sleep(for: .seconds(max(1, seconds)))
        finish()
      }
    }

    @objc func tick(_ link: CADisplayLink) {
      guard UIApplication.shared.applicationState == .active else { last = 0; return }
      let now = CACurrentMediaTime()
      let budget = link.targetTimestamp - link.timestamp
      if last > 0 {
        let elapsed = now - last
        samples.append(elapsed * 1000)
        let excess = max(0, elapsed - budget)
        // Half a millisecond of callback jitter is not a missed display deadline.
        if excess > 0.0005 { hitches += 1; hitchMs += excess * 1000 }
        if excess * 1000 > worst {
          worst = excess * 1000
          worstNames = Perf.names(from: last, to: now)
        }
      }
      last = now
    }

    private func finish() {
      link?.invalidate()
      let sorted = samples.sorted()
      guard !sorted.isEmpty else { return }
      func percentile(_ q: Double) -> Double { sorted[min(sorted.count - 1, Int(ceil(q * Double(sorted.count))) - 1)] }
      let env = ProcessInfo.processInfo.environment
      let record: [String: Any] = [
        "run": env["YO_PERF_RUN"] ?? "manual", "scenario": env["YO_PERF_SCENARIO"] ?? "manual",
        "metric": "display_callback_interval", "frames_ms": samples, "launch_list_ms": launchListMs,
        "p50_ms": percentile(0.5), "p95_ms": percentile(0.95), "p99_ms": percentile(0.99),
        "max_ms": sorted.last!, "hitches": hitches, "hitch_ms": hitchMs,
        "worst_stall_ms": worst, "spans": worstNames.isEmpty ? ["unattributed (layout/system/unsampled)"] : worstNames,
      ]
      // Keep the device log line small: raw samples are used locally only for percentiles.
      var summary = record
      summary.removeValue(forKey: "frames_ms")
      if let data = try? JSONSerialization.data(withJSONObject: summary, options: [.sortedKeys]),
        let json = String(data: data, encoding: .utf8) { Perf.report(json) }
      Perf.dump()
    }
  }
}
