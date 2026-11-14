// The app's side of Perf (YO_PERF): when a finger lifts (the touch's own timestamp), when a change was handed to the
// screen and the frame it shows in, and every frame that came late (a hitch: the main thread was busy).
import QuartzCore
import UIKit

@MainActor enum PerfProbe {
  private static var installed = false

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
      link?.add(to: .main, forMode: .common)
    }

    @objc func tick(_ link: CADisplayLink) {
      Perf.mark("\(what).frame", at: link.targetTimestamp)
      link.invalidate()
      self.link = nil
    }
  }

  /// Frames that came later than one frame after the one before: how late.
  @MainActor private final class Watch: NSObject {
    static let shared = Watch()
    private var last = 0.0

    func start() {
      let link = CADisplayLink(target: self, selector: #selector(tick))
      link.add(to: .main, forMode: .common)
    }

    @objc func tick(_ link: CADisplayLink) {
      let frame = link.targetTimestamp - link.timestamp
      if last > 0, link.timestamp - last > frame * 1.5 {
        Perf.mark(String(format: "hitch %.1f ms", (link.timestamp - last) * 1000), at: link.timestamp)
      }
      last = link.timestamp
    }
  }
}
