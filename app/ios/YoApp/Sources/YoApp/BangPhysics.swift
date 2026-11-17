// Dev prototype (YO_DEV only): from the 6th Yo of a run on, each further Yo drops one "!" into its bar as a small
// physical object: a capsule (two Verlet circles and a stick) under gravity that follows the phone's tilt. They bounce
// off the bar's "Yo!!!!!" (a solid rect), push each other, fill the bar (60 at most) and then spill over into the whole
// screen, piling up at its bottom (300 at most, the oldest fade). When the run ends they drop away.
// Battery: one Canvas, its TimelineView runs only while something moves; at rest it sleeps (no frames), motion drops to
// 5 Hz only to notice a tilt. No objects, app in the background, Low Power Mode or Reduce Motion: no motion, no frames.
// Release builds: the hooks below do nothing.
import SwiftUI
import UIKit
import simd
#if YO_DEV
  import CoreMotion
#endif

extension View {
  /// On a bar: where it is, and its run's Yos (0: none, or not a bar whose "!"s sweep in).
  func bangPhysicsBar(_ bar: String, total: Int) -> some View {
    #if YO_DEV
      modifier(BangBarHook(bar: bar, total: total))
    #else
      self
    #endif
  }

  /// On the bar's "Yo!!" mark: the obstacle the "!"s bounce off.
  func bangPhysicsLabel(_ bar: String, on: Bool) -> some View {
    #if YO_DEV
      onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { rect in
        if on { BangWorld.shared.label(bar, rect) }
      }
    #else
      self
    #endif
  }
}

/// Over the whole screen, never taking a touch.
struct BangPhysicsLayer: View {
  var body: some View {
    #if YO_DEV
      BangCanvas()
    #else
      EmptyView()
    #endif
  }
}

#if YO_DEV
  private struct BangBarHook: ViewModifier {
    let bar: String
    let total: Int

    func body(content: Content) -> some View {
      content
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { BangWorld.shared.bar(bar, $0) }
        .onChange(of: total) { old, new in
          if new == 0 { BangWorld.shared.release(bar) }
          else if new > old, new > 5 { BangWorld.shared.spawn(bar, count: min(new - max(old, 5), 6)) }
        }
        .onDisappear { BangWorld.shared.release(bar) }
    }
  }

  private struct BangCanvas: View {
    @State private var world = BangWorld.shared

    var body: some View {
      let version = world.version
      ZStack {
        if world.visible {
          TimelineView(.animation(paused: !world.running || !world.active)) { timeline in
            Canvas { context, _ in
              _ = version
              world.advance(to: timeline.date)
              world.draw(&context)
            }
          }
          .allowedDynamicRange(.high)
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { world.screen = $0 }
      .ignoresSafeArea()
      .allowsHitTesting(false)
      .accessibilityHidden(true)
    }
  }

  @MainActor @Observable final class BangWorld {
    static let shared = BangWorld()

    /// Something to draw.
    private(set) var visible = false
    /// Frames run (false: asleep at rest).
    private(set) var running = false
    private(set) var active = true
    /// Bumped when the picture changes without a frame (a bar scrolled while asleep).
    private(set) var version = 0

    private struct Bang {
      var bar: String?  // nil: loose on the screen
      let origin: String
      var alpha = 1.0
      var leaving = false
      var fading = false
    }

    private struct Bar {
      var frame: CGRect = .null
      var label: CGRect = .null
    }

    // Bang k: particles 2k (head) and 2k + 1 (foot).
    @ObservationIgnored private var bangs: [Bang] = []
    @ObservationIgnored private var pos: [SIMD2<Double>] = []
    @ObservationIgnored private var old: [SIMD2<Double>] = []
    @ObservationIgnored private var bars: [String: Bar] = [:]
    @ObservationIgnored var screen: CGRect = .zero
    @ObservationIgnored private var gravity = SIMD2<Double>(0, 1)
    @ObservationIgnored private var last: Date?
    @ObservationIgnored private var pending = 0.0
    @ObservationIgnored private var quiet = 0
    @ObservationIgnored private var calm = 0
    @ObservationIgnored private let motion = CMMotionManager()
    @ObservationIgnored private var observers: [NSObjectProtocol] = []

    private static let radius = 4.5
    private static let length = 14.0
    private static let perBar = 60
    private static let onScreen = 300
    private static let step = 1.0 / 120
    private static let g = 1400.0

    private init() {
      let center = NotificationCenter.default
      observers = [
        center.addObserver(forName: UIApplication.willResignActiveNotification, object: nil, queue: .main) { _ in
          MainActor.assumeIsolated { BangWorld.shared.setActive(false) }
        },
        center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
          MainActor.assumeIsolated { BangWorld.shared.setActive(true) }
        },
        center.addObserver(forName: .NSProcessInfoPowerStateDidChange, object: nil, queue: .main) { _ in
          MainActor.assumeIsolated { if !BangWorld.allowed { BangWorld.shared.clear() } }
        },
        center.addObserver(forName: UIAccessibility.reduceMotionStatusDidChangeNotification, object: nil,
                           queue: .main) { _ in
          MainActor.assumeIsolated { if !BangWorld.allowed { BangWorld.shared.clear() } }
        },
      ]
    }

    private static var allowed: Bool {
      !ProcessInfo.processInfo.isLowPowerModeEnabled && !UIAccessibility.isReduceMotionEnabled
    }

    // MARK: Hooks

    func bar(_ id: String, _ frame: CGRect) {
      let before = bars[id]?.frame ?? .null
      bars[id, default: Bar()].frame = frame
      guard !before.isNull, before.origin != frame.origin, bangs.contains(where: { $0.bar == id }) else { return }
      let delta = SIMD2(Double(frame.minX - before.minX), Double(frame.minY - before.minY))
      for k in bangs.indices where bangs[k].bar == id {
        for i in [2 * k, 2 * k + 1] {
          pos[i] += delta
          old[i] += delta
        }
      }
      if !running { version &+= 1 }
    }

    func label(_ id: String, _ rect: CGRect) {
      bars[id, default: Bar()].label = rect
    }

    func spawn(_ id: String, count: Int) {
      guard Self.allowed, active, let bar = bars[id], !bar.frame.isNull, !bar.label.isNull else { return }
      for n in 0..<count {
        let inBar = bangs.count(where: { $0.bar == id }) < Self.perBar
        // From the screen's edge toward the "Yo!!!!!", a little high: it hits and bounces off.
        let start = SIMD2(Double(bar.frame.maxX) - 8, Double(bar.label.midY) - Double.random(in: 0...10))
        let speed = SIMD2(-Double.random(in: 380...620), -Double.random(in: 60...260)) * (1 + Double(n) * 0.08)
        let tilt = Double.random(in: -0.6...0.6)
        let axis = SIMD2(sin(tilt), -cos(tilt)) * (Self.length / 2)
        let spin = SIMD2(Double.random(in: -40...40), 0)
        bangs.append(Bang(bar: inBar ? id : nil, origin: id))
        pos.append(start + axis)
        pos.append(start - axis)
        old.append(start + axis - (speed + spin) * Self.step)
        old.append(start - axis - (speed - spin) * Self.step)
      }
      // The oldest fade once the screen holds too many.
      let live = bangs.indices.filter { !bangs[$0].fading }
      for k in live.prefix(max(0, live.count - Self.onScreen)) { bangs[k].fading = true }
      wake()
    }

    /// The run ended (or the bar went): its "!"s drop away, out of the screen's bottom.
    func release(_ id: String) {
      var any = false
      for k in bangs.indices where bangs[k].origin == id && !bangs[k].leaving {
        bangs[k].leaving = true
        bangs[k].bar = nil
        any = true
      }
      if any { wake() }
    }

    private func clear() {
      bangs.removeAll()
      pos.removeAll()
      old.removeAll()
      settle()
    }

    private func setActive(_ now: Bool) {
      active = now
      last = nil
      updateMotion()
    }

    private func tilt(_ g: SIMD2<Double>) {
      let moved = simd_length(g - gravity) > 0.06
      gravity = g
      if moved && !running { wake() }
    }

    private func wake() {
      if !bangs.isEmpty, !visible { visible = true }
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
        self.running = false
        if self.bangs.isEmpty { self.visible = false }
        self.updateMotion()
      }
    }

    /// Device motion only while "!"s exist and the app is in front: 30 Hz while they move, 5 Hz asleep (a tilt wakes).
    private func updateMotion() {
      let want = !bangs.isEmpty && active && Self.allowed && motion.isDeviceMotionAvailable
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
        MainActor.assumeIsolated { BangWorld.shared.tilt(SIMD2(x, -y)) }
      }
    }

    // MARK: Simulation

    func advance(to date: Date) {
      guard running, active else { return }
      let dt = min(1.0 / 20, max(0, date.timeIntervalSince(last ?? date)))
      last = date
      pending += dt
      var moved = 0.0
      var steps = 0
      while pending >= Self.step && steps < 6 {
        pending -= Self.step
        steps += 1
        moved = max(moved, step())
      }
      if steps == 6 { pending = 0 }
      guard steps > 0 else { return }
      let busy = bangs.contains { $0.leaving || $0.fading }
      quiet = busy || moved > 0.06 ? 0 : quiet + steps
      calm = busy || moved > 0.4 ? 0 : calm + steps
      // At rest (or close enough for long enough): no more frames until something happens.
      if bangs.isEmpty || quiet > 50 || calm > 600 { settle() }
    }

    /// One fixed step; the largest distance a particle went.
    private func step() -> Double {
      let r = Self.radius
      let before = pos
      let fall = gravity * Self.g * Self.step * Self.step
      for i in pos.indices {
        let v = (pos[i] - old[i]) * 0.996
        old[i] = pos[i]
        pos[i] += v + fall
      }
      // Pairs close in x, sorted once per step.
      let order = pos.indices.sorted { pos[$0].x < pos[$1].x }
      for _ in 0..<3 {
        // The stick between head and foot.
        for k in bangs.indices {
          let a = 2 * k, b = a + 1
          let d = pos[a] - pos[b]
          let len = max(simd_length(d), 0.001)
          let fix = d * ((len - Self.length) / len / 2)
          pos[a] -= fix
          pos[b] += fix
        }
        // Circles push each other (same bar, or both loose on the screen).
        for n in order.indices {
          let i = order[n]
          let bi = bangs[i / 2]
          if bi.leaving { continue }
          var m = n + 1
          while m < order.count {
            let j = order[m]
            m += 1
            let d = pos[j] - pos[i]
            if d.x > 2 * r + 2 { break }
            if j / 2 == i / 2 { continue }
            let bj = bangs[j / 2]
            if bj.leaving || bj.bar != bi.bar { continue }
            let dist2 = simd_length_squared(d)
            guard dist2 < 4 * r * r, dist2 > 0.0001 else { continue }
            let dist = dist2.squareRoot()
            let push = d * ((2 * r - dist) / dist / 2)
            pos[i] -= push
            pos[j] += push
          }
        }
        // Walls: the bar's box and its label for a bar's "!"s, the screen for loose ones.
        for i in pos.indices {
          let bang = bangs[i / 2]
          if let id = bang.bar, let bar = bars[id], !bar.frame.isNull {
            inside(i, bar.frame, bounce: 0.35)
            if !bar.label.isNull { outside(i, bar.label.insetBy(dx: 2, dy: bar.label.height * 0.2), bounce: 0.5) }
          } else if !bang.leaving {
            inside(i, screen, bounce: 0.3)
          }
        }
      }
      // Fading and falling away; gone ones are dropped.
      var keep: [Int] = []
      for k in bangs.indices {
        if bangs[k].fading || bangs[k].leaving { bangs[k].alpha -= Self.step * (bangs[k].leaving ? 1.2 : 2) }
        let below = pos[2 * k].y > screen.maxY + 40 && pos[2 * k + 1].y > screen.maxY + 40
        if bangs[k].alpha > 0 && !below { keep.append(k) }
      }
      if keep.count != bangs.count {
        bangs = keep.map { bangs[$0] }
        pos = keep.flatMap { [pos[2 * $0], pos[2 * $0 + 1]] }
        old = keep.flatMap { [old[2 * $0], old[2 * $0 + 1]] }
        return 1
      }
      var moved = 0.0
      for i in pos.indices { moved = max(moved, simd_length(pos[i] - before[i])) }
      return moved
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
      if p.y < Double(box.minY) + r { hit(i, SIMD2(0, 1), Double(box.minY) + r - p.y, bounce: bounce) }
      if p.y > Double(box.maxY) - r { hit(i, SIMD2(0, -1), p.y - Double(box.maxY) + r, bounce: bounce) }
    }

    private func outside(_ i: Int, _ rect: CGRect, bounce: Double) {
      let r = Self.radius
      let p = pos[i]
      let closest = SIMD2(min(max(p.x, Double(rect.minX)), Double(rect.maxX)),
                          min(max(p.y, Double(rect.minY)), Double(rect.maxY)))
      let d = p - closest
      let dist = simd_length(d)
      if dist > 0.0001 {
        if dist < r { hit(i, d / dist, r - dist, bounce: bounce) }
        return
      }
      // The centre inside: out along the shortest way.
      let ways = [(Double(rect.minX) - p.x, SIMD2<Double>(-1, 0)), (p.x - Double(rect.maxX), SIMD2<Double>(1, 0)),
                  (Double(rect.minY) - p.y, SIMD2<Double>(0, -1)), (p.y - Double(rect.maxY), SIMD2<Double>(0, 1))]
      let way = ways.max { $0.0 < $1.0 }!
      hit(i, way.1, r - way.0, bounce: bounce)
    }

    // MARK: Drawing

    func draw(_ context: inout GraphicsContext) {
      let glyph = context.resolve(
        Text("!").font(.yo(Metrics.rowFont)).foregroundStyle(Color.white.exposureAdjust(3)))
      for k in bangs.indices {
        let head = pos[2 * k], foot = pos[2 * k + 1]
        let mid = (head + foot) / 2
        let axis = head - foot
        var c = context
        c.opacity = max(0, bangs[k].alpha)
        c.translateBy(x: mid.x - Double(screen.minX), y: mid.y - Double(screen.minY))
        c.rotate(by: .radians(atan2(axis.y, axis.x) + .pi / 2))
        c.draw(glyph, at: .zero, anchor: .center)
      }
    }
  }
#endif
