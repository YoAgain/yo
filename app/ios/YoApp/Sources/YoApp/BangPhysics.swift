// Dev-only free-falling "!": capped at 200 objects, 30 spawns/s (15/bar), 4 ms/frame.
import SwiftUI
import UIKit
import simd
#if YO_DEV
  import CoreMotion
#endif

@MainActor enum BangPhysics {
  /// Whether "!"s beyond the label's five become physics objects now.
  static var on: Bool {
    #if YO_DEV
      !UIAccessibility.isReduceMotionEnabled
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
        guard up, target > 0, BangPhysics.on else { return }
        world = BangWorld()
      }
      world?.sync(id, incoming: incoming, target: target, up: up)
      if world?.empty == true { tearDown() }
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
      TimelineView(.animation(paused: !world.running || !world.active)) { timeline in
        Canvas { context, _ in
          world.advance(to: timeline.date)
          world.draw(&context)
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

  @MainActor @Observable final class BangWorld {
    // The only observed state (BangCanvas alone reads it): the list never updates for the physics.
    private(set) var running = false
    private(set) var active = true

    private struct Bang {
      let origin: String
      let glow: Bool
      var alpha = 1.0
      var fading = false
      var leaveAt: Double?
    }

    // Bang k: particles 2k (head) and 2k + 1 (foot).
    @ObservationIgnored private var bangs: [Bang] = []
    @ObservationIgnored private var pos: [SIMD2<Double>] = []
    @ObservationIgnored private var old: [SIMD2<Double>] = []
    private var bars: [String: BangBar] {
      get { BangHost.shared.bars }
      set { BangHost.shared.bars = newValue }
    }
    @ObservationIgnored var screen: CGRect = UIApplication.shared.connectedScenes
      .compactMap { ($0 as? UIWindowScene)?.screen.bounds }.first ?? .zero
    @ObservationIgnored private var gravity = SIMD2<Double>(0, 1)
    /// The tilt, low-pass filtered (the accelerometer's noise never shakes a pile).
    @ObservationIgnored private var tiltFiltered = SIMD2<Double>(0, 1)
    @ObservationIgnored private var last: Date?
    @ObservationIgnored private var pending = 0.0
    @ObservationIgnored private var time = 0.0
    @ObservationIgnored private var quiet = 0
    @ObservationIgnored private var calm = 0
    @ObservationIgnored private var iterations = 6
    @ObservationIgnored private var tokens = BangWorld.totalRate
    @ObservationIgnored private var filled = 0.0
    @ObservationIgnored private var seen: [String: Int] = [:]
    // A uniform grid for the collisions (counting sort).
    @ObservationIgnored private var cellOf: [Int] = []
    @ObservationIgnored private var cellStart: [Int] = []
    @ObservationIgnored private var cellFill: [Int] = []
    @ObservationIgnored private var cellItems: [Int] = []
    @ObservationIgnored private var cols = 1
    @ObservationIgnored private var rows = 1
    @ObservationIgnored private let motion = CMMotionManager()
    @ObservationIgnored private var observers: [NSObjectProtocol] = []

    private static let radius = 4.5
    private static let length = 14.0
    private static let onScreen = 200
    static let perBarRate = 15.0
    private static let totalRate = 30.0
    private static let step = 1.0 / 120
    private static let g = 1400.0
    /// The most a particle goes in a step (under its radius: nothing passes through another or the screen's edge).
    private static let maxStep = 0.8 * radius
    private static let cell = 12.0
    private static let budget = 0.004
    private static let maxIterations = 6
    /// Overlaps up to this much stay (no correcting back and forth at rest).
    private static let slop = 0.25
    /// Slower than this a step (in points), a particle rests.
    private static let rest = 0.03
    /// Between two "!": the share of the approach speed they part with, and of their sliding a resting pair loses a step.
    private static let bounce = 0.2
    private static let friction = 0.2
    @ObservationIgnored private var glowing: GraphicsContext.ResolvedText?
    @ObservationIgnored private var plain: GraphicsContext.ResolvedText?
    @ObservationIgnored private var deadline = 0.0

    static func valid(_ rect: CGRect) -> Bool {
      !rect.isNull && rect.minX.isFinite && rect.minY.isFinite
        && rect.maxX.isFinite && rect.maxY.isFinite && rect.width >= 24 && rect.height >= 24
        && abs(rect.minX) < 100_000 && abs(rect.minY) < 100_000
        && rect.width < 10_000 && rect.height < 10_000
    }

    var empty: Bool { bangs.isEmpty }

    init() {
      bangs.reserveCapacity(Self.onScreen)
      pos.reserveCapacity(Self.onScreen * 2)
      old.reserveCapacity(Self.onScreen * 2)
      cellOf = Array(repeating: 0, count: Self.onScreen * 2)
      cellItems = cellOf
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

    func sync(_ id: String, incoming: Bool, target: Int, up: Bool) {
      let origin = key(id, incoming)
      seen[origin, default: 0] &+= 1
      var live = 0
      for bang in bangs where bang.origin == origin && !bang.fading && bang.leaveAt == nil { live += 1 }
      if up {
        guard target > live, BangPhysics.on, active, let bar = bars[id], !bar.frame.isNull else { return }
        // The brakes: the screen's cap, then so many a second per bar and in all; the rest still count on the label.
        refill(id)
        let room = Self.onScreen - bangs.count
        if room <= 0 {
          if let oldest = bangs.firstIndex(where: { !$0.fading && $0.leaveAt == nil }) {
            bangs[oldest].leaveAt = time
            wake()
          }
          return
        }
        let n = min(target - live, room, Int(tokens), Int(bars[id]?.tokens ?? 0))
        guard n > 0 else { return }
        tokens -= Double(n)
        bars[id]?.tokens -= Double(n)
        for _ in 0..<n { add(origin: origin, glow: incoming, bar: bar) }
        wake()
      } else if target < live {
        // The run is over: all of them go, oldest first, one quickly after the other.
        leave(origin)
      }
    }

    func vanish(_ id: String, incoming: Bool) {
      let origin = key(id, incoming)
      let token = seen[origin, default: 0]
      Task { @MainActor in
        try? await Task.sleep(for: .seconds(1))
        if self.seen[origin, default: 0] == token { self.leave(origin) }
      }
    }

    private func refill(_ id: String) {
      let now = ProcessInfo.processInfo.systemUptime
      tokens = min(Self.totalRate, tokens + (now - filled) * Self.totalRate)
      filled = now
      if var bar = bars[id] {
        bar.tokens = min(Self.perBarRate, bar.tokens + (now - bar.filled) * Self.perBarRate)
        bar.filled = now
        bars[id] = bar
      }
    }

    private func add(origin: String, glow: Bool, bar: BangBar) {
      guard bangs.count < Self.onScreen, Self.valid(bar.frame), Self.valid(screen) else { return }
      let frame = bar.frame
      let measured = glow ? bar.incoming : bar.sent
      let label = Self.valid(measured) ? measured : CGRect.null
      // Incoming: from the bar's right end, flying left. Sent: from the end of "Sent Yo!!!!!", dropping.
      let sent = !glow && !label.isNull
      let x = sent ? Double(label.maxX) - 6 : Double(screen.maxX) - Self.radius
      let y = sent ? Double(label.midY) : Double(frame.midY)
      let start = SIMD2(min(max(x, Double(screen.minX) + Self.radius), Double(screen.maxX) - Self.radius),
                        min(y, Double(screen.maxY) - Self.length / 2 - Self.radius))
      let speed = sent
        ? SIMD2(Double.random(in: -60...60), Double.random(in: 40...140))
        : SIMD2(-Double.random(in: 300...470), Double.random(in: -40...40))
      let tilt = Double.random(in: -0.6...0.6)
      let axis = SIMD2(sin(tilt), -cos(tilt)) * (Self.length / 2)
      let spin = SIMD2(Double.random(in: -40...40), 0)
      bangs.append(Bang(origin: origin, glow: glow))
      pos.append(start + axis)
      pos.append(start - axis)
      old.append(start + axis - (speed + spin) * Self.step)
      old.append(start - axis - (speed - spin) * Self.step)
    }

    private func leave(_ origin: String) {
      var n = 0.0
      for k in bangs.indices where bangs[k].origin == origin && !bangs[k].fading && bangs[k].leaveAt == nil {
        bangs[k].leaveAt = time + n * 0.05
        n += 1
      }
      if n > 0 { wake() }
    }

    fileprivate func clear() {
      bangs.removeAll()
      pos.removeAll()
      old.removeAll()
      settle()
    }

    fileprivate func setActive(_ now: Bool) {
      active = now
      last = nil
      updateMotion()
    }

    fileprivate func tilt(_ raw: SIMD2<Double>) {
      guard raw.x.isFinite, raw.y.isFinite else { return }
      tiltFiltered += (simd_clamp(raw, SIMD2(repeating: -1), SIMD2(repeating: 1)) - tiltFiltered) * 0.1
      // Small changes are noise: gravity stays.
      guard simd_length(tiltFiltered - gravity) > 0.02 else { return }
      let moved = simd_length(tiltFiltered - gravity) > 0.06
      gravity = tiltFiltered
      if moved && !running { wake() }
    }

    private func wake() {
      if !running {
        running = true
        last = nil
      }
      quiet = 0
      calm = 0
      updateMotion()
    }

    private func settle() {
      Task { @MainActor in
        guard BangHost.shared.world === self else { return }
        if self.bangs.isEmpty { BangHost.shared.tearDown(); return }
        self.running = false
        self.updateMotion()
      }
    }

    /// Device motion only while "!"s exist and the app is in front: 30 Hz while they move, 5 Hz asleep (a tilt wakes).
    private func updateMotion() {
      let want = !bangs.isEmpty && active && BangPhysics.on && motion.isDeviceMotionAvailable
      guard want else {
        if motion.isDeviceMotionActive { motion.stopDeviceMotionUpdates() }
        if bangs.isEmpty { gravity = SIMD2(0, 1) }
        return
      }
      motion.deviceMotionUpdateInterval = running ? 1.0 / 30 : 1.0 / 5
      guard !motion.isDeviceMotionActive else { return }
      motion.startDeviceMotionUpdates(to: .main) { data, _ in
        guard let g = data?.gravity else { return }
        let x = g.x, y = g.y
        MainActor.assumeIsolated { BangHost.shared.world?.tilt(SIMD2(x, -y)) }
      }
    }

    // MARK: Simulation

    /// In the frame callback only, within its budget: a slow frame drops steps and the next one iterates less.
    func advance(to date: Date) {
      guard running, active, Self.valid(screen) else { return }
      // All paired-array accesses below share this invariant.
      guard bangs.count <= Self.onScreen, pos.count == bangs.count * 2, old.count == pos.count else {
        clear(); return
      }
      let began = CACurrentMediaTime()
      deadline = began + Self.budget
      let elapsed = date.timeIntervalSince(last ?? date)
      guard elapsed.isFinite else { last = nil; return }
      let dt = min(1.0 / 20, max(0, elapsed))
      last = date
      pending += dt
      var moved = 0.0
      var steps = 0
      while pending >= Self.step && steps < 4 && CACurrentMediaTime() < deadline {
        pending -= Self.step
        steps += 1
        moved = max(moved, step())
        if CACurrentMediaTime() - began > Self.budget {
          pending = 0
          break
        }
      }
      if steps == 4 { pending = 0 }
      let spent = CACurrentMediaTime() - began
      if spent > Self.budget * 0.75 { iterations = max(1, iterations - 1) }
      else if spent < Self.budget * 0.3 { iterations = min(Self.maxIterations, iterations + 1) }
      guard steps > 0 else { return }
      let busy = bangs.contains { $0.fading || $0.leaveAt != nil }
      quiet = busy || moved > 0.06 ? 0 : quiet + steps
      calm = busy || moved > 0.4 ? 0 : calm + steps
      // At rest (or close enough for long enough): no more frames until something happens.
      if bangs.isEmpty || quiet > 50 || calm > 600 { settle() }
    }

    /// One fixed step; the largest distance a particle went.
    private func step() -> Double {
      time += Self.step
      prepare()
      let dt2 = Self.step * Self.step
      let fall = gravity * Self.g * dt2
      grid()
      contacts()
      for i in pos.indices {
        var v = (pos[i] - old[i]) * 0.996
        let speed = simd_length(v)
        if speed > Self.maxStep { v *= Self.maxStep / speed }
        old[i] = pos[i]
        pos[i] += v + fall
      }
      for _ in 0..<min(iterations, Self.maxIterations) {
        if CACurrentMediaTime() >= deadline { break }
        stick()
        grid()
        collide()
        walls()
      }
      // Fading objects keep colliding until removed.
      var gone = false
      for k in bangs.indices where bangs[k].fading {
        bangs[k].alpha -= Self.step * 1.2
        let below = min(pos[2 * k].y, pos[2 * k + 1].y) > Double(screen.maxY) + 40
        if bangs[k].alpha <= 0 || below { gone = true }
      }
      if gone { compact() }
      // Tiny speeds are none: a particle that hardly moves rests.
      var moved = 0.0
      for i in pos.indices {
        let speed = simd_length(pos[i] - old[i])
        if speed < Self.rest { old[i] = pos[i] } else { moved = max(moved, speed) }
      }
      return moved
    }

    /// Start scheduled fades and repair invalid positions.
    private func prepare() {
      for k in bangs.indices {
        if let at = bangs[k].leaveAt, at <= time {
          bangs[k].fading = true
          bangs[k].leaveAt = nil
        }
      }
      for i in pos.indices {
        if !pos[i].x.isFinite || !pos[i].y.isFinite || !old[i].x.isFinite || !old[i].y.isFinite {
          pos[i] = SIMD2(Double(screen.midX), Double(screen.midY))
          old[i] = pos[i]
        }
        pos[i] = simd_clamp(pos[i], SIMD2(repeating: -100_000), SIMD2(repeating: 100_000))
      }
    }


    private func compact() {
      var n = 0
      for k in bangs.indices {
        let below = min(pos[2 * k].y, pos[2 * k + 1].y) > Double(screen.maxY) + 40
        if bangs[k].fading && (bangs[k].alpha <= 0 || below) { continue }
        bangs[n] = bangs[k]
        pos[2 * n] = pos[2 * k]; pos[2 * n + 1] = pos[2 * k + 1]
        old[2 * n] = old[2 * k]; old[2 * n + 1] = old[2 * k + 1]
        n += 1
      }
      bangs.removeLast(bangs.count - n)
      pos.removeLast(pos.count - 2 * n)
      old.removeLast(old.count - 2 * n)
    }

    /// The particles into a grid of cells a little larger than two radii: neighbours only from the 9 cells around.
    private func grid() {
      let s = Self.cell
      cols = max(1, min(256, Int(screen.width / s) + 3))
      rows = max(1, min(256, Int(screen.height / s) + 3))
      let cells = cols * rows
      if cellStart.count != cells + 1 {
        cellStart = Array(repeating: 0, count: cells + 1)
        cellFill = Array(repeating: 0, count: cells + 1)
      } else {
        for c in cellStart.indices { cellStart[c] = 0 }
      }
      if cellOf.count < pos.count {
        cellOf = Array(repeating: 0, count: pos.count * 2)
        cellItems = Array(repeating: 0, count: pos.count * 2)
      }
      let minX = Double(screen.minX), minY = Double(screen.minY)
      for i in pos.indices {
        if !pos[i].x.isFinite || !pos[i].y.isFinite {
          pos[i] = SIMD2(Double(screen.midX), Double(screen.midY))
          old[i] = pos[i]
        }
        // Clamp before conversion: Int traps on infinity and out-of-range values.
        let cx = Int(min(max((pos[i].x - minX) / s + 1, 0), Double(cols - 1)))
        let cy = Int(min(max((pos[i].y - minY) / s + 1, 0), Double(rows - 1)))
        let c = cy * cols + cx
        cellOf[i] = c
        cellStart[c + 1] += 1
      }
      for c in 0..<cells { cellStart[c + 1] += cellStart[c] }
      for c in 0...cells { cellFill[c] = cellStart[c] }
      for i in pos.indices {
        cellItems[cellFill[cellOf[i]]] = i
        cellFill[cellOf[i]] += 1
      }
    }

    /// The stick between head and foot.
    private func stick() {
      for k in bangs.indices {
        let a = 2 * k, b = a + 1
        let d = pos[a] - pos[b]
        let len = max(simd_length(d), 0.001)
        let fix = d * ((len - Self.length) / len / 2)
        pos[a] -= fix
        pos[b] += fix
      }
    }

    /// Once a step, on the relative speed of each touching pair only (what they share, a common fall, stays): they
    /// bump apart (restitution 0.2) and rub only where one rests on the other along gravity, never side by side.
    private func contacts() {
      let r2 = 4 * Self.radius * Self.radius
      let down = simd_length(gravity) > 0.01 ? simd_normalize(gravity) : SIMD2<Double>(0, 1)
      var checks = 0
      for i in pos.indices {
        if CACurrentMediaTime() >= deadline { return }
        let c = cellOf[i]
        let cx = c % cols, cy = c / cols
        for ny in max(0, cy - 1)...min(rows - 1, cy + 1) {
          for nx in max(0, cx - 1)...min(cols - 1, cx + 1) {
            let n = ny * cols + nx
            for t in cellStart[n]..<cellStart[n + 1] {
              checks += 1
              if checks > 16_000 || (checks % 64 == 0 && CACurrentMediaTime() >= deadline) { return }
              let j = cellItems[t]
              if j <= i || j >> 1 == i >> 1 { continue }
              let d = pos[j] - pos[i]
              let dist2 = simd_length_squared(d)
              guard dist2 < r2, dist2 > 0.0001 else { continue }
              let normal = d / dist2.squareRoot()
              let rel = (pos[j] - old[j]) - (pos[i] - old[i])
              let vn = simd_dot(rel, normal)
              // The change to j's speed; i gets the opposite (equal masses: momentum kept).
              var dv = SIMD2<Double>.zero
              if vn < 0 { dv += normal * (-(1 + Self.bounce) * vn / 2) }
              if abs(simd_dot(normal, down)) > 0.5 { dv -= (rel - normal * vn) * (Self.friction / 2) }
              old[j] -= dv
              old[i] += dv
            }
          }
        }
      }
    }

    /// Circles push each other across the whole screen.
    private func collide() {
      let r2 = 4 * Self.radius * Self.radius
      var checks = 0
      for i in pos.indices {
        if CACurrentMediaTime() >= deadline { return }
        let c = cellOf[i]
        let cx = c % cols, cy = c / cols
        for ny in max(0, cy - 1)...min(rows - 1, cy + 1) {
          for nx in max(0, cx - 1)...min(cols - 1, cx + 1) {
            let n = ny * cols + nx
            for t in cellStart[n]..<cellStart[n + 1] {
              checks += 1
              if checks > 16_000 || (checks % 64 == 0 && CACurrentMediaTime() >= deadline) { return }
              let j = cellItems[t]
              if j <= i || j >> 1 == i >> 1 { continue }
              let d = pos[j] - pos[i]
              let dist2 = simd_length_squared(d)
              guard dist2 < r2, dist2 > 0.0001 else { continue }
              let dist = dist2.squareRoot()
              let overlap = 2 * Self.radius - dist
              // Positions only: no speed is taken here (a touch in the air never brakes the fall).
              guard overlap > Self.slop else { continue }
              let push = d * ((overlap - Self.slop) / dist / 2)
              pos[i] -= push
              pos[j] += push
            }
          }
        }
      }
    }

    /// Left, right and bottom screen edges; the top stays open.
    private func walls() {
      for i in pos.indices { inside(i, screen, bounce: 0.2) }
    }

    /// Hit a surface with normal `n` (unit) `depth` deep: out of it, the speed into it reflected by `bounce`, a little
    /// friction along it.
    private func hit(_ i: Int, _ n: SIMD2<Double>, _ depth: Double, bounce: Double) {
      pos[i] += n * depth
      var v = pos[i] - old[i]
      let vn = simd_dot(v, n)
      if vn < 0 {
        let tangent = v - n * vn
        v = tangent * 0.9 - n * vn * bounce
        old[i] = pos[i] - v
      }
    }

    private func inside(_ i: Int, _ box: CGRect, bounce: Double) {
      let r = Self.radius
      let p = pos[i]
      if p.x < Double(box.minX) + r { hit(i, SIMD2(1, 0), Double(box.minX) + r - p.x, bounce: bounce) }
      if p.x > Double(box.maxX) - r { hit(i, SIMD2(-1, 0), p.x - Double(box.maxX) + r, bounce: bounce) }
      if p.y > Double(box.maxY) - r { hit(i, SIMD2(0, -1), p.y - Double(box.maxY) + r, bounce: bounce) }
    }

    // MARK: Drawing

    func draw(_ context: inout GraphicsContext) {
      guard Self.valid(screen), pos.count == bangs.count * 2 else { return }
      if glowing == nil {
        glowing = context.resolve(Text("!").font(.yo(Metrics.rowFont)).foregroundStyle(Color.white.exposureAdjust(3)))
        plain = context.resolve(Text("!").font(.yo(Metrics.rowFont)).foregroundStyle(Color.white))
      }
      guard let glowing, let plain else { return }
      for k in bangs.indices {
        let head = pos[2 * k], foot = pos[2 * k + 1]
        guard head.x.isFinite, head.y.isFinite, foot.x.isFinite, foot.y.isFinite else { continue }
        let mid = (head + foot) / 2
        let axis = head - foot
        var c = context
        c.opacity = max(0, bangs[k].alpha)
        c.translateBy(x: mid.x - Double(screen.minX), y: mid.y - Double(screen.minY))
        c.rotate(by: .radians(atan2(axis.y, axis.x) + .pi / 2))
        c.draw(bangs[k].glow ? glowing : plain, at: .zero, anchor: .center)
      }
    }
  }
#endif
