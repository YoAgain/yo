// Dev-only free-falling "!" (sent) and "Yo" (incoming): capped at 200 objects, 30 spawns/s (15/bar), 4 ms/frame. The
// engine is BangSim.swift; this is its place in the app: spawning, tilt, drawing and the guards that keep the main
// thread free (a step and a frame within budget, off for the session after two frames over 50 ms).
import SwiftUI
import UIKit
import os
#if YO_DEV
  import CoreMotion
  import Synchronization
#endif

@MainActor enum BangPhysics {
  /// Whether "!"s beyond the label's five become physics objects now.
  static var on: Bool {
    #if YO_DEV
      !UIAccessibility.isReduceMotionEnabled && !BangHost.shared.disabled
    #else
      false
    #endif
  }

  /// The label on `bar` shows `count` "!"s beyond its own (`up`: one more came; else they are going).
  static func sync(_ bar: String?, incoming: Bool, target: Int, up: Bool) {
    #if YO_DEV
      if let bar { BangHost.shared.sync(bar, incoming: incoming, target: target, up: up) }
    #endif
  }

  /// The label went (perhaps only re-laid out): its "!"s fade unless it is back within a second.
  static func vanish(_ bar: String?, incoming: Bool) {
    #if YO_DEV
      if let bar { BangHost.shared.world?.vanish(bar, incoming: incoming) }
    #endif
  }
}

extension View {
  /// On a bar: where new "!"s start.
  func bangPhysicsBar(_ bar: String) -> some View {
    #if YO_DEV
      onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
        BangHost.shared.bar(bar, $0)
      }
    #else
      self
    #endif
  }

  /// On the bar's "Yo!!" (or "Sent Yo!!"): where a sent one starts.
  func bangPhysicsLabel(_ bar: String, incoming: Bool, on: Bool) -> some View {
    #if YO_DEV
      onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { rect in
        if on { BangHost.shared.label(bar, incoming: incoming, rect) }
      }
      .onDisappear { if on { BangHost.shared.label(bar, incoming: incoming, .null) } }
    #else
      self
    #endif
  }
}

/// Over the whole screen, never taking a touch.
struct BangPhysicsLayer: View {
  var body: some View {
    #if YO_DEV
      if let world = BangHost.shared.world { BangCanvas(world: world) }
    #else
      EmptyView()
    #endif
  }
}

#if YO_DEV
  /// Where the bars and their labels are (only that, kept always: a dictionary write when one moves), and the engine
  /// while something overflows.
  @MainActor @Observable final class BangHost {
    static let shared = BangHost()
    private(set) var world: BangWorld?
    @ObservationIgnored var bars: [String: BangBar] = [:]
    /// Off for the rest of the session (a guard tripped).
    @ObservationIgnored private(set) var disabled = false
    @ObservationIgnored private var lastCreated = 0.0

    func bar(_ id: String, _ frame: CGRect) {
      guard BangWorld.valid(frame) else { return }
      if bars[id] == nil { bars[id] = BangBar(frame: frame) }
      else { bars[id]?.frame = frame }
    }

    func label(_ id: String, incoming: Bool, _ rect: CGRect) {
      guard bars[id] != nil else { return }
      if incoming { bars[id]?.incoming = rect } else { bars[id]?.sent = rect }
    }

    func sync(_ id: String, incoming: Bool, target: Int, up: Bool) {
      if world == nil {
        // Only when something can start now (no engine made and dropped again for nothing), at most twice a second.
        guard up, target > 0, BangPhysics.on, let bar = bars[id], BangWorld.valid(bar.frame),
              UIApplication.shared.applicationState == .active else { return }
        let now = ProcessInfo.processInfo.systemUptime
        guard now - lastCreated > 0.5 else { return }
        lastCreated = now
        world = BangWorld()
      }
      world?.sync(id, incoming: incoming, target: target, up: up)
      if world?.empty == true { tearDown() }
    }

    /// A guard tripped: no physics for the rest of the session.
    func disable(_ why: String) {
      guard !disabled else { return }
      disabled = true
      Logger(subsystem: "app.justyo", category: "bang-physics").error("physics off for this session: \(why, privacy: .public)")
      tearDown()
    }

    /// The last one went: the engine, its overlay and motion with it.
    func tearDown() {
      world?.stop()
      world = nil
    }
  }

  struct BangBar {
    var frame: CGRect = .null
    var incoming: CGRect = .null
    var sent: CGRect = .null
    var tokens = 15.0  // BangWorld.perBarRate
    var filled = 0.0
  }

  private struct BangCanvas: View {
    let world: BangWorld

    var body: some View {
      // 60 frames a second while nothing moves fast (the steps stay 120 Hz, drawn in between).
      TimelineView(.animation(minimumInterval: world.fast ? nil : 1.0 / 60, paused: !world.running || !world.active)) {
        timeline in
        Canvas { context, _ in
          let began = CACurrentMediaTime()
          world.advance(to: timeline.date)
          world.draw(&context)
          world.watch(work: CACurrentMediaTime() - began)
        }
      }
      .allowedDynamicRange(.high)
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { world.screen = $0 }
      .ignoresSafeArea()
      .allowsHitTesting(false)
      .accessibilityHidden(true)
    }
  }

  /// The tilt, written on CoreMotion's own queue and read once a frame on the main thread (no callbacks piling up there).
  final class BangTilt: Sendable {
    struct State {
      var raw = SIMD2<Double>(0, 1)
      /// Where gravity stands now; while asleep, a tilt this far off wakes the engine (one hop to the main thread).
      var gravity = SIMD2<Double>(0, 1)
      var sleeping = false
      var wakeQueued = false
    }
    let state = Mutex(State())
  }

  @MainActor @Observable final class BangWorld {
    // The only observed state (BangCanvas alone reads it): the list never updates for the physics.
    private(set) var running = false
    private(set) var active = true
    /// Something moves fast: frames as fast as the screen (else 60 a second).
    private(set) var fast = true

    @ObservationIgnored private let sim: BangSim
    @ObservationIgnored private var origins: [String: Int32] = [:]
    @ObservationIgnored var screen: CGRect = UIApplication.shared.connectedScenes
      .compactMap { ($0 as? UIWindowScene)?.screen.bounds }.first ?? .zero
    @ObservationIgnored private var gravity = SIMD2<Double>(0, 1)
    /// The tilt, low-pass filtered (the accelerometer's noise never shakes a pile).
    @ObservationIgnored private var tiltFiltered = SIMD2<Double>(0, 1)
    @ObservationIgnored private var last: Date?
    @ObservationIgnored private var pending = 0.0
    @ObservationIgnored private var quiet = 0
    @ObservationIgnored private var calm = 0
    @ObservationIgnored private var slowFrames = 0
    @ObservationIgnored private var tokens = BangWorld.totalRate
    @ObservationIgnored private var filled = 0.0
    @ObservationIgnored private var seen: [String: Int] = [:]
    @ObservationIgnored private let motion = CMMotionManager()
    @ObservationIgnored private let motionQueue: OperationQueue = {
      let queue = OperationQueue()
      queue.maxConcurrentOperationCount = 1
      queue.qualityOfService = .utility
      return queue
    }()
    @ObservationIgnored private let tiltBox = BangTilt()
    @ObservationIgnored private var observers: [NSObjectProtocol] = []
    @ObservationIgnored private var strikes: [Double] = []
    @ObservationIgnored private var lastFrame: Double?
    /// "!" (sent) as before; "Yo" (incoming) in a smaller size, its capsule measured from the glyph.
    @ObservationIgnored private var bangGlyph: GraphicsContext.ResolvedText?
    @ObservationIgnored private var yoGlyph: GraphicsContext.ResolvedText?

    private static let onScreen = BangSim.capacity
    static let perBarRate = 15.0
    private static let totalRate = 30.0
    private static let step = BangSim.tick
    private static let budget = 0.004
    private static let yoSize: CGFloat = Metrics.rowFont  // exactly the bar font: a Yo never smaller
    private static let bangLength: Float = 14

    static func valid(_ rect: CGRect) -> Bool {
      !rect.isNull && rect.minX.isFinite && rect.minY.isFinite
        && rect.maxX.isFinite && rect.maxY.isFinite && rect.width >= 24 && rect.height >= 24
        && abs(rect.minX) < 100_000 && abs(rect.minY) < 100_000
        && rect.width < 10_000 && rect.height < 10_000
    }

    var empty: Bool { sim.count == 0 }

    init() {
      // The "Yo" capsule: two circles as high as the glyph's capitals, as far apart as it is wide.
      let font = UIFont(name: "Montserrat-Bold", size: Self.yoSize) ?? .boldSystemFont(ofSize: Self.yoSize)
      let width = ("Yo" as NSString).size(withAttributes: [.font: font]).width
      let r = min(10, max(4, font.capHeight / 2 + 1))
      let length = max(0, min(40, width - 2 * r))
      sim = BangSim(yoRadius: Float(r), yoLength: Float(length.isFinite ? length : 12))
      let center = NotificationCenter.default
      observers = [
        center.addObserver(forName: UIApplication.willResignActiveNotification, object: nil, queue: .main) { _ in
          MainActor.assumeIsolated { BangHost.shared.world?.setActive(false) }
        },
        center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
          MainActor.assumeIsolated { BangHost.shared.world?.setActive(true) }
        },
        center.addObserver(forName: UIAccessibility.reduceMotionStatusDidChangeNotification, object: nil,
                           queue: .main) { _ in
          MainActor.assumeIsolated { if !BangPhysics.on { BangHost.shared.world?.clear() } }
        },
      ]
    }

    // MARK: Hooks

    func stop() {
      if motion.isDeviceMotionActive { motion.stopDeviceMotionUpdates() }
      for observer in observers { NotificationCenter.default.removeObserver(observer) }
      observers = []
    }

    private func key(_ bar: String, _ incoming: Bool) -> String { bar + (incoming ? "|in" : "|out") }

    private func id(_ origin: String) -> Int32 {
      if let id = origins[origin] { return id }
      // Bars come and go; the ids only ever grow, the map at most a few hundred (else started over).
      if origins.count > 512 { origins.removeAll() }
      let id = Int32(truncatingIfNeeded: origins.count + 1)
      origins[origin] = id
      return id
    }

    func sync(_ bar: String, incoming: Bool, target: Int, up: Bool) {
      let origin = key(bar, incoming)
      seen[origin, default: 0] &+= 1
      let o = id(origin)
      let live = sim.live(o)
      if up {
        guard target > live, BangPhysics.on, active, let frame = BangHost.shared.bars[bar], !frame.frame.isNull else {
          return
        }
        // The brakes: the screen's cap, then so many a second per bar and in all; the rest still count on the label.
        refill(bar)
        if sim.full {
          sim.retireOldest()
          wake()
          return
        }
        let room = Self.onScreen - sim.count
        let n = min(target - live, room, Int(tokens), Int(BangHost.shared.bars[bar]?.tokens ?? 0))
        guard n > 0 else { return }
        tokens -= Double(n)
        BangHost.shared.bars[bar]?.tokens -= Double(n)
        for _ in 0..<n { add(origin: o, incoming: incoming, bar: frame) }
        wake()
      } else if target < live {
        // The run is over: all of them go, oldest first, one quickly after the other.
        if sim.leave(o, gap: 0.05) > 0 { wake() }
      }
    }

    func vanish(_ bar: String, incoming: Bool) {
      let origin = key(bar, incoming)
      let token = seen[origin, default: 0]
      Task { @MainActor in
        try? await Task.sleep(for: .seconds(1))
        guard BangHost.shared.world === self, self.seen[origin, default: 0] == token else { return }
        if self.sim.leave(self.id(origin), gap: 0.05) > 0 { self.wake() }
      }
    }

    private func refill(_ id: String) {
      let now = ProcessInfo.processInfo.systemUptime
      tokens = min(Self.totalRate, tokens + max(0, now - filled) * Self.totalRate)
      filled = now
      if var bar = BangHost.shared.bars[id] {
        bar.tokens = min(Self.perBarRate, bar.tokens + max(0, now - bar.filled) * Self.perBarRate)
        bar.filled = now
        BangHost.shared.bars[id] = bar
      }
    }

    private func add(origin: Int32, incoming: Bool, bar: BangBar) {
      guard !sim.full, Self.valid(bar.frame), Self.valid(screen) else { return }
      let frame = bar.frame
      let measured = incoming ? bar.incoming : bar.sent
      let label = Self.valid(measured) ? measured : CGRect.null
      let r = Double(incoming ? sim.radius.yo : sim.radius.bang)
      // Incoming: a "Yo" from the bar's right end, flying left. Sent: a "!" from the end of "Sent Yo!!!!!", dropping.
      let sent = !incoming && !label.isNull
      let x = sent ? Double(label.maxX) - 6 : Double(screen.maxX) - r
      let y = sent ? Double(label.midY) : Double(frame.midY)
      let half = Double(incoming ? sim.length.yo : sim.length.bang) / 2
      let start = SIMD2(min(max(x, Double(screen.minX) + r), Double(screen.maxX) - r),
                        min(y, Double(screen.maxY) - half - r))
      let speed = sent
        ? SIMD2(Double.random(in: -60...60), Double.random(in: 40...140))
        : SIMD2(-Double.random(in: 300...470), Double.random(in: -40...40))
      let tilt = Double.random(in: -0.6...0.6)
      let spin = SIMD2(Double.random(in: -40...40), 0)
      // "!": a head above its foot; "Yo": its left end and its right end.
      let axis = incoming ? SIMD2(-cos(tilt), -sin(tilt)) * half : SIMD2(sin(tilt), -cos(tilt)) * half
      let a = start + axis, b = start - axis
      let f = { (p: SIMD2<Double>) in SIMD2<Float>(Float(p.x), Float(p.y)) }
      sim.add(incoming ? .yo : .bang, origin: origin, a: f(a), b: f(b),
              oldA: f(a - (speed + spin) * Self.step), oldB: f(b - (speed - spin) * Self.step))
    }

    fileprivate func clear() {
      sim.clear()
      settle()
    }

    fileprivate func setActive(_ now: Bool) {
      active = now
      last = nil
      lastFrame = nil
      updateMotion()
    }

    /// The filtered tilt as gravity (the engine wakes only for a real change).
    private func tilt(_ raw: SIMD2<Double>, weight: Double) {
      guard raw.x.isFinite, raw.y.isFinite else { return }
      let clamped = SIMD2(min(max(raw.x, -1), 1), min(max(raw.y, -1), 1))
      tiltFiltered += (clamped - tiltFiltered) * weight
      let change = tiltFiltered - gravity
      let off = (change.x * change.x + change.y * change.y).squareRoot()
      // Small changes are noise: gravity stays.
      guard off > 0.02 else { return }
      gravity = tiltFiltered
      sim.setGravity(SIMD2(Float(gravity.x), Float(gravity.y)))
      tiltBox.state.withLock { $0.gravity = gravity }
      if off > 0.06 && !running { wake() }
    }

    /// From the motion queue while asleep: a real tilt came.
    fileprivate func tiltWoke() {
      tiltBox.state.withLock { $0.wakeQueued = false }
      let raw = tiltBox.state.withLock { $0.raw }
      tilt(raw, weight: 1)
    }

    private func wake() {
      if !running {
        running = true
        last = nil
        lastFrame = nil
      }
      quiet = 0
      calm = 0
      updateMotion()
    }

    private func settle() {
      Task { @MainActor in
        guard BangHost.shared.world === self else { return }
        if self.sim.count == 0 { BangHost.shared.tearDown(); return }
        self.running = false
        self.updateMotion()
      }
    }

    /// Device motion only while objects exist and the app is in front: 30 Hz while they move, 5 Hz asleep (a tilt
    /// wakes). Delivered on its own queue; the main thread reads the latest once a frame.
    private func updateMotion() {
      let want = sim.count > 0 && active && BangPhysics.on && motion.isDeviceMotionAvailable
      tiltBox.state.withLock { $0.sleeping = !running }
      guard want else {
        if motion.isDeviceMotionActive { motion.stopDeviceMotionUpdates() }
        if sim.count == 0 { gravity = SIMD2(0, 1); sim.setGravity(SIMD2(0, 1)) }
        return
      }
      motion.deviceMotionUpdateInterval = running ? 1.0 / 30 : 1.0 / 5
      guard !motion.isDeviceMotionActive else { return }
      // The handler is built outside the main actor: CoreMotion calls it on its own queue, and a main-actor closure
      // there traps (Swift checks the executor), which crashed the app as soon as the physics started.
      motion.startDeviceMotionUpdates(to: motionQueue, withHandler: Self.motionHandler(tiltBox))
    }

    nonisolated private static func motionHandler(_ box: BangTilt) -> CMDeviceMotionHandler {
      { data, _ in
        guard let g = data?.gravity, g.x.isFinite, g.y.isFinite else { return }
        let raw = SIMD2(g.x, -g.y)
        let wakeNow = box.state.withLock { s -> Bool in
          s.raw = raw
          let d = raw - s.gravity
          guard s.sleeping, !s.wakeQueued, (d.x * d.x + d.y * d.y).squareRoot() > 0.1 else { return false }
          s.wakeQueued = true
          return true
        }
        if wakeNow { DispatchQueue.main.async { MainActor.assumeIsolated { BangHost.shared.world?.tiltWoke() } } }
      }
    }

    // MARK: Simulation

    /// In the frame callback only, within its budget: a slow frame drops steps and the next one iterates less.
    func advance(to date: Date) {
      guard running, active else { return }
      guard Self.valid(screen), sim.setScreen(minX: Float(screen.minX), minY: Float(screen.minY),
                                              maxX: Float(screen.maxX), maxY: Float(screen.maxY)) else {
        settle(); return  // no usable screen: no frames
      }
      let began = CACurrentMediaTime()
      let deadline = began + Self.budget
      let elapsed = date.timeIntervalSince(last ?? date)
      guard elapsed.isFinite else { last = nil; return }
      let dt = min(1.0 / 20, max(0, elapsed))
      last = date
      // The tilt CoreMotion left, filtered as at 30 Hz.
      let raw = tiltBox.state.withLock { $0.raw }
      tilt(raw, weight: 1 - pow(0.9, dt * 30))
      pending += dt
      var moved: Float = 0
      var steps = 0
      while pending >= Self.step && steps < 4 && CACurrentMediaTime() < deadline {
        pending -= Self.step
        steps += 1
        moved = max(moved, sim.step { CACurrentMediaTime() >= deadline })
      }
      if steps == 4 || CACurrentMediaTime() >= deadline { pending = min(pending, Self.step) }
      let spent = CACurrentMediaTime() - began
      if spent > Self.budget * 0.75 { sim.iterations = max(1, sim.iterations - 1) }
      else if spent < Self.budget * 0.3 { sim.iterations = min(BangSim.maxIterations, sim.iterations + 1) }
      guard steps > 0 else { return }
      let busy = sim.busy
      quiet = busy || moved > 0.06 ? 0 : quiet + steps
      calm = busy || moved > 0.4 ? 0 : calm + steps
      // Fast frames only while something moves fast (with a little hysteresis).
      if moved > 1.2 || busy { slowFrames = 0; if !fast { fast = true } }
      else { slowFrames += 1; if slowFrames > 30 && fast { fast = false } }
      // At rest (or close enough for long enough): no more frames until something happens.
      if sim.count == 0 || quiet > 50 || calm > 600 { settle() }
    }

    /// After each frame: the frame's own work, and the time since the last frame while running. Twice over 50 ms
    /// within half a minute, the physics turns itself off for the session.
    func watch(work: Double) {
      let now = CACurrentMediaTime()
      defer { lastFrame = running && active ? now : nil }
      var slow = work > 0.05
      if let lastFrame, now - lastFrame > 0.05, now - lastFrame < 2 { slow = true }
      guard slow else { return }
      strikes = strikes.filter { now - $0 < 30 } + [now]
      if strikes.count >= 2 {
        BangHost.shared.disable(String(format: "frame %.0f ms, work %.1f ms, %d objects",
                                       (now - (lastFrame ?? now)) * 1000, work * 1000, sim.count))
      }
    }

    // MARK: Drawing

    /// One pass over the objects, between their last two steps; the faded and the off-screen skipped.
    func draw(_ context: inout GraphicsContext) {
      guard Self.valid(screen), sim.count > 0 else { return }
      if bangGlyph == nil {
        bangGlyph = context.resolve(Text("!").font(.yo(Metrics.rowFont)).foregroundStyle(Color.white))
        yoGlyph = context.resolve(Text("Yo").font(.yo(Self.yoSize)).foregroundStyle(Color.white.exposureAdjust(3)))
      }
      guard let bangGlyph, let yoGlyph else { return }
      let t = Float(min(1, max(0, pending / Self.step)))
      let ox = Float(screen.minX), oy = Float(screen.minY)
      let x0 = Float(screen.minX) - 40, x1 = Float(screen.maxX) + 40
      let y0 = Float(screen.minY) - 40, y1 = Float(screen.maxY) + 40
      let base = context.transform
      let baseOpacity = context.opacity
      for k in 0..<sim.count {
        let alpha = sim.alpha[k]
        guard alpha > 0.01 else { continue }
        let a = sim.prev[2 * k] + (sim.pos[2 * k] - sim.prev[2 * k]) * t
        let b = sim.prev[2 * k + 1] + (sim.pos[2 * k + 1] - sim.prev[2 * k + 1]) * t
        let mid = (a + b) / 2
        guard BangSim.finite(mid), mid.x > x0, mid.x < x1, mid.y > y0, mid.y < y1 else { continue }
        let d = a - b
        let len = BangSim.dot(d, d).squareRoot()
        guard len > 0.001 else { continue }
        let yo = sim.kind[k] == BangSim.Kind.yo.rawValue
        // "!": upright along head − foot; "Yo": along its right end − left end.
        let cos = yo ? -d.x / len : -d.y / len
        let sin = yo ? -d.y / len : d.x / len
        context.transform = CGAffineTransform(a: CGFloat(cos), b: CGFloat(sin), c: CGFloat(-sin), d: CGFloat(cos),
                                              tx: CGFloat(mid.x - ox), ty: CGFloat(mid.y - oy)).concatenating(base)
        context.opacity = baseOpacity * Double(min(1, alpha))
        context.draw(yo ? yoGlyph : bangGlyph, at: .zero, anchor: .center)
      }
      context.transform = base
      context.opacity = baseOpacity
    }
  }
#endif
