// The physics of the dev-only falling "!" and "Yo" (BangPhysics.swift): plain Swift, no UIKit or SwiftUI, so it is
// measured on Linux too (app/ios/dev/bang-bench/). Structure of arrays in memory of its own, allocated once: no
// allocation, no copy-on-write and no bounds or exclusivity checks per step. Every loop is bounded by the cap.
#if YO_DEV || BANG_BENCH
  final class BangSim {
    /// Object k: particles 2k (head of a "!", left end of a "Yo") and 2k + 1 (foot, right end), a stick between them.
    enum Kind: UInt8 { case bang = 0, yo = 1 }

    static let capacity = 200
    /// The step's unit: 120 Hz. Slow piles step at 60 Hz (two ticks a step), drawn in between (`prev`).
    static let tick = 1.0 / 120
    /// Heavy: they drop briskly (2.5 × the first prototype's 1400).
    static let g: Float = 3500
    /// The most a particle goes in a step, 528 pt/s: two approaching ones close under a diameter of the smallest
    /// ("!", 9) a step, so nothing passes through another; the screen's edges clamp.
    static let maxStep: Float = 4.4
    /// Overlaps up to this much stay (no correcting back and forth at rest).
    static let slop: Float = 0.25
    /// Slower than this a tick (in points), a particle rests; so many ticks resting, its object sleeps.
    static let rest: Float = 0.03
    static let sleepTicks: UInt16 = 30
    /// Between two objects: the share of the approach speed they part with, and of their sliding a resting pair loses.
    static let bounce: Float = 0.2
    static let friction: Float = 0.2
    static let maxIterations = 6
    /// Pairs are found once a step this much beyond touching (what the solver's pushes may close in a step).
    static let margin: Float = 3
    static let maxPairs = 8_000
    static let maxCells = 128 * 128
    /// Woken by a touch: an impact faster than this (a tick), or pushed deeper than this.
    static let wakeSpeed: Float = 0.3
    static let wakeDepth: Float = 1.0

    let radius: (bang: Float, yo: Float)
    let length: (bang: Float, yo: Float)
    private(set) var count = 0
    private(set) var time = 0.0
    /// Ticks a step: 1 (120 Hz) or 2 (60 Hz).
    private(set) var ticks = 1
    var iterations = BangSim.maxIterations
    /// Unit vector-ish (the tilt), screen coordinates (y down).
    private(set) var gravity = SIMD2<Float>(0, 1)
    private(set) var minX: Float = 0, minY: Float = 0, maxX: Float = 390, maxY: Float = 844

    // Per object.
    let kind: UnsafeMutablePointer<UInt8>
    let origin: UnsafeMutablePointer<Int32>
    let alpha: UnsafeMutablePointer<Float>
    let fading: UnsafeMutablePointer<Bool>
    /// When it starts to fade (`.infinity`: not yet).
    let leaveAt: UnsafeMutablePointer<Double>
    let still: UnsafeMutablePointer<UInt16>
    let asleep: UnsafeMutablePointer<Bool>
    private let span: UnsafeMutablePointer<Float>
    // Per particle.
    let pos: UnsafeMutablePointer<SIMD2<Float>>
    let old: UnsafeMutablePointer<SIMD2<Float>>
    /// Where it was before the last step (drawing interpolates from there).
    let prev: UnsafeMutablePointer<SIMD2<Float>>
    private let rad: UnsafeMutablePointer<Float>
    // The grid: a list per cell (head, next), only the used cells reset.
    private let head: UnsafeMutablePointer<Int32>
    private let next: UnsafeMutablePointer<Int32>
    private let used: UnsafeMutablePointer<Int32>
    private var usedCount = 0
    private var cols = 1, rows = 1
    private var cell: Float
    private var gridX: Float = 0, gridY: Float = 0
    // Candidate pairs (at least one awake), found once a step.
    private let pairA: UnsafeMutablePointer<Int32>
    private let pairB: UnsafeMutablePointer<Int32>
    private var pairCount = 0
    private var pairsStale = true
    private var gridReady = false

    init(bangRadius: Float = 4.5, bangLength: Float = 14, yoRadius: Float = 7, yoLength: Float = 11) {
      radius = (bangRadius, max(1, min(20, yoRadius)))
      length = (bangLength, max(0, min(60, yoLength)))
      cell = 2 * max(radius.bang, radius.yo) + Self.margin
      let n = Self.capacity, p = 2 * n
      kind = .allocate(capacity: n); origin = .allocate(capacity: n); alpha = .allocate(capacity: n)
      fading = .allocate(capacity: n); leaveAt = .allocate(capacity: n); still = .allocate(capacity: n)
      asleep = .allocate(capacity: n); span = .allocate(capacity: n)
      pos = .allocate(capacity: p); old = .allocate(capacity: p); prev = .allocate(capacity: p)
      rad = .allocate(capacity: p); next = .allocate(capacity: p); used = .allocate(capacity: p)
      head = .allocate(capacity: Self.maxCells)
      head.initialize(repeating: -1, count: Self.maxCells)
      pairA = .allocate(capacity: Self.maxPairs); pairB = .allocate(capacity: Self.maxPairs)
    }

    deinit {
      kind.deallocate(); origin.deallocate(); alpha.deallocate(); fading.deallocate(); leaveAt.deallocate(); still.deallocate()
      asleep.deallocate(); span.deallocate(); pos.deallocate(); old.deallocate(); prev.deallocate()
      rad.deallocate(); next.deallocate(); used.deallocate(); head.deallocate(); pairA.deallocate()
      pairB.deallocate()
    }

    // MARK: Objects

    var full: Bool { count >= Self.capacity }
    var awakeCount: Int { (0..<count).reduce(0) { $0 + (asleep[$1] ? 0 : 1) } }
    var candidatePairs: Int { pairCount }

    /// A new object from its two ends now and a step ago (their speed); false when full or not finite.
    @discardableResult
    func add(_ k: Kind, origin o: Int32, a: SIMD2<Float>, b: SIMD2<Float>, oldA: SIMD2<Float>, oldB: SIMD2<Float>)
      -> Bool
    {
      guard count < Self.capacity, Self.finite(a), Self.finite(b), Self.finite(oldA), Self.finite(oldB) else {
        return false
      }
      let i = count
      kind[i] = k.rawValue; origin[i] = o; alpha[i] = 1; fading[i] = false; leaveAt[i] = .infinity
      still[i] = 0; asleep[i] = false
      span[i] = k == .yo ? length.yo : length.bang
      let r = k == .yo ? radius.yo : radius.bang
      pos[2 * i] = a; pos[2 * i + 1] = b
      prev[2 * i] = a; prev[2 * i + 1] = b
      // In step units: a 60 Hz step goes twice as far.
      let s = Float(ticks)
      old[2 * i] = a - (a - oldA) * s; old[2 * i + 1] = b - (b - oldB) * s
      rad[2 * i] = r; rad[2 * i + 1] = r
      count += 1
      pairsStale = true
      return true
    }

    func live(_ o: Int32) -> Int {
      var n = 0
      for i in 0..<count where origin[i] == o && !fading[i] && leaveAt[i] == .infinity { n += 1 }
      return n
    }

    /// The oldest object not going yet starts to go now.
    func retireOldest() {
      for i in 0..<count where !fading[i] && leaveAt[i] == .infinity {
        leaveAt[i] = time
        return
      }
    }

    /// All of `o` go, oldest first, `gap` apart; how many.
    @discardableResult
    func leave(_ o: Int32, gap: Double) -> Int {
      var n = 0
      for i in 0..<count where origin[i] == o && !fading[i] && leaveAt[i] == .infinity {
        leaveAt[i] = time + Double(n) * gap
        n += 1
      }
      return n
    }

    var busy: Bool {
      for i in 0..<count where fading[i] || leaveAt[i] != .infinity { return true }
      return false
    }

    func clear() {
      count = 0
      pairsStale = true
    }

    func wakeAll() {
      for i in 0..<count { asleep[i] = false; still[i] = 0 }
      pairsStale = true
    }

    private func wake(_ i: Int) {
      asleep[i] = false
      still[i] = 0
      pairsStale = true
    }

    // MARK: World

    /// The screen (global coordinates); false when it is not a usable one.
    @discardableResult
    func setScreen(minX x0: Float, minY y0: Float, maxX x1: Float, maxY y1: Float) -> Bool {
      guard x0.isFinite, y0.isFinite, x1.isFinite, y1.isFinite, x1 - x0 >= 24, y1 - y0 >= 24,
            x1 - x0 < 10_000, y1 - y0 < 10_000, abs(x0) < 100_000, abs(y0) < 100_000 else { return false }
      if x0 != minX || y0 != minY || x1 != maxX || y1 != maxY || !gridReady {
        gridReady = true
        minX = x0; minY = y0; maxX = x1; maxY = y1
        // One cell's border around the screen, the grid at most `maxCells` (bigger cells on a huge screen).
        cell = 2 * max(radius.bang, radius.yo) + Self.margin
        while true {
          cols = Int(((x1 - x0) / cell).rounded(.up)) + 3
          rows = Int(((y1 - y0) / cell).rounded(.up)) + 3
          if cols * rows <= Self.maxCells { break }
          cell *= 2
        }
        gridX = x0 - cell; gridY = y0 - cell
        wakeAll()
      }
      return true
    }

    /// A new tilt: everything awake again.
    func setGravity(_ g: SIMD2<Float>) {
      guard Self.finite(g) else { return }
      gravity = SIMD2(max(-1, min(1, g.x)), max(-1, min(1, g.y)))
      wakeAll()
    }

    /// 1 tick a step (120 Hz) or 2 (60 Hz); the speeds kept as they are.
    func setTicks(_ k: Int) {
      let k = k == 2 ? 2 : 1
      guard k != ticks else { return }
      let f = Float(k) / Float(ticks)
      for i in 0..<(2 * count) { old[i] = pos[i] - (pos[i] - old[i]) * f }
      ticks = k
    }

    // MARK: Step

    /// One step (`ticks` long); the largest distance a particle went in a tick. `over()` is asked between the solver's
    /// passes: the step ends early once its time is used.
    func step(over: () -> Bool = { false }) -> Float {
      let k = Float(ticks)
      time += Double(ticks) * Self.tick
      // Scheduled fades start; anything not finite or far away is removed.
      var drop = false
      for i in 0..<count {
        if !fading[i] && leaveAt[i] <= time { fading[i] = true; leaveAt[i] = .infinity; wake(i) }
        let a = pos[2 * i], b = pos[2 * i + 1]
        if !Self.finite(a) || !Self.finite(b) || !Self.finite(old[2 * i]) || !Self.finite(old[2 * i + 1])
          || !near(a) || !near(b)
        {
          fading[i] = true; alpha[i] = 0; drop = true
        }
      }
      if drop { compact() }
      prev.update(from: pos, count: 2 * count)
      var awake = 0
      for i in 0..<count where !asleep[i] { awake += 1 }
      guard awake > 0 else { return 0 }
      if pairsStale { pairs() }
      contacts(k)
      if over() { return Self.maxStep }
      // Verlet, the awake only.
      let damp: Float = ticks == 2 ? 0.996 * 0.996 : 0.996
      let dt = Float(Double(ticks) * Self.tick)
      let fall = gravity * (Self.g * dt * dt)
      for p in 0..<(2 * count) where !asleep[p >> 1] {
        var v = (pos[p] - old[p]) * damp
        let speed2 = Self.dot(v, v)
        if speed2 > Self.maxStep * Self.maxStep { v *= Self.maxStep / speed2.squareRoot() }
        old[p] = pos[p]
        pos[p] += v + fall
      }
      pairs()
      for pass in 0..<max(1, min(iterations, Self.maxIterations)) {
        if pass > 0 && over() { break }
        stick()
        let pushed = collide()
        let hits = walls(k)
        if !pushed && !hits { break }  // nothing left to correct
      }
      // Fading objects keep colliding until they go.
      var gone = false
      let below = maxY + 40
      for i in 0..<count where fading[i] {
        alpha[i] -= Float(Double(ticks) * Self.tick * 1.2)
        if alpha[i] <= 0 || min(pos[2 * i].y, pos[2 * i + 1].y) > below { gone = true }
      }
      if gone { compact() }
      // Tiny speeds are none; resting long enough, an object sleeps (skipped until something wakes it).
      var moved: Float = 0
      let rest2 = (Self.rest * k) * (Self.rest * k)
      for i in 0..<count where !asleep[i] {
        var resting = true
        for p in (2 * i)...(2 * i + 1) {
          let d = pos[p] - old[p]
          let s2 = Self.dot(d, d)
          if s2 < rest2 { old[p] = pos[p] } else { resting = false; moved = max(moved, s2) }
        }
        if resting && !fading[i] && leaveAt[i] == .infinity {
          still[i] &+= UInt16(ticks)
          if still[i] >= Self.sleepTicks { asleep[i] = true }
        } else {
          still[i] = 0
        }
      }
      return moved.squareRoot() / k
    }

    private func near(_ p: SIMD2<Float>) -> Bool {
      p.x > minX - 5_000 && p.x < maxX + 5_000 && p.y > minY - 5_000 && p.y < maxY + 5_000
    }

    /// Gone ones out; the rest keep their order (oldest first). Whatever rested on a gone one falls again.
    private func compact() {
      var n = 0
      let below = maxY + 40
      for i in 0..<count {
        if fading[i] && (alpha[i] <= 0 || min(pos[2 * i].y, pos[2 * i + 1].y) > below) { continue }
        if n != i {
          kind[n] = kind[i]; origin[n] = origin[i]; alpha[n] = alpha[i]; fading[n] = fading[i]
          leaveAt[n] = leaveAt[i]; span[n] = span[i]
          for e in 0...1 {
            pos[2 * n + e] = pos[2 * i + e]; old[2 * n + e] = old[2 * i + e]
            prev[2 * n + e] = prev[2 * i + e]; rad[2 * n + e] = rad[2 * i + e]
          }
        }
        n += 1
      }
      count = n
      wakeAll()
    }

    /// Every particle into its cell; the candidate pairs (at least one awake) from the 3 × 3 cells around.
    private func pairs() {
      for u in 0..<usedCount { head[Int(used[u])] = -1 }
      usedCount = 0
      let inv = 1 / cell
      let cx1 = Float(cols - 1), cy1 = Float(rows - 1)
      for p in 0..<(2 * count) {
        // Clamped before the conversion (positions are finite here, `step` removed the others).
        let cx = Int(max(0, min(cx1, (pos[p].x - gridX) * inv)))
        let cy = Int(max(0, min(cy1, (pos[p].y - gridY) * inv)))
        let c = cy * cols + cx
        if head[c] < 0 { used[usedCount] = Int32(c); usedCount += 1 }
        next[p] = head[c]
        head[c] = Int32(p)
      }
      pairCount = 0
      let reach = Self.margin
      for p in 0..<(2 * count) where !asleep[p >> 1] {
        let cx = Int(max(0, min(cx1, (pos[p].x - gridX) * inv)))
        let cy = Int(max(0, min(cy1, (pos[p].y - gridY) * inv)))
        let me = p >> 1
        for ny in max(0, cy - 1)...min(rows - 1, cy + 1) {
          for nx in max(0, cx - 1)...min(cols - 1, cx + 1) {
            var q = Int(head[ny * cols + nx])
            var guardCount = 0
            while q >= 0 && guardCount < 400 {
              guardCount += 1
              let other = q >> 1
              if other != me && (asleep[other] || q > p) {
                let d = pos[q] - pos[p]
                let r = rad[p] + rad[q] + reach
                if Self.dot(d, d) < r * r {
                  guard pairCount < Self.maxPairs else { pairsStale = false; return }
                  pairA[pairCount] = Int32(p); pairB[pairCount] = Int32(q)
                  pairCount += 1
                }
              }
              q = Int(next[q])
            }
          }
        }
      }
      pairsStale = false
    }

    /// Once a step, on the relative speed of each touching pair only (what they share, a common fall, stays): they
    /// bump apart (restitution 0.2) and rub only where one rests on the other along gravity, never side by side. A
    /// sleeping one holds still unless hit hard (then it wakes).
    private func contacts(_ k: Float) {
      let g2 = Self.dot(gravity, gravity)
      let down = g2 > 0.0001 ? gravity / g2.squareRoot() : SIMD2<Float>(0, 1)
      let rub = (1 - (ticks == 2 ? (1 - Self.friction) * (1 - Self.friction) : 1 - Self.friction)) / 2
      for t in 0..<pairCount {
        let i = Int(pairA[t]), j = Int(pairB[t])
        var si = asleep[i >> 1], sj = asleep[j >> 1]
        if si && sj { continue }
        let d = pos[j] - pos[i]
        let dist2 = Self.dot(d, d)
        let rr = rad[i] + rad[j]
        guard dist2 < rr * rr, dist2 > 0.0001 else { continue }
        let normal = d / dist2.squareRoot()
        let rel = (pos[j] - old[j]) - (pos[i] - old[i])
        let vn = Self.dot(rel, normal)
        if (si || sj) && vn < -Self.wakeSpeed * k {
          if si { wake(i >> 1); si = false } else { wake(j >> 1); sj = false }
        }
        // The change to j's speed; i gets the opposite (equal masses: momentum kept).
        var dv = SIMD2<Float>.zero
        if vn < 0 { dv += normal * (-(1 + Self.bounce) * vn / 2) }
        let along = Self.dot(normal, down)
        if along > 0.5 || along < -0.5 { dv -= (rel - normal * vn) * rub }
        if sj { old[i] += dv * 2 } else if si { old[j] -= dv * 2 } else { old[j] -= dv; old[i] += dv }
      }
    }

    /// The stick between an object's ends.
    private func stick() {
      for o in 0..<count where !asleep[o] {
        let a = 2 * o, b = a + 1
        let d = pos[a] - pos[b]
        let len = max(Self.dot(d, d).squareRoot(), 0.001)
        let fix = d * ((len - span[o]) / len / 2)
        pos[a] -= fix
        pos[b] += fix
      }
    }

    /// Circles push each other out (positions only: a touch in the air never brakes the fall); whether any did.
    private func collide() -> Bool {
      var any = false
      for t in 0..<pairCount {
        let i = Int(pairA[t]), j = Int(pairB[t])
        var si = asleep[i >> 1], sj = asleep[j >> 1]
        if si && sj { continue }
        let d = pos[j] - pos[i]
        let dist2 = Self.dot(d, d)
        let rr = rad[i] + rad[j]
        guard dist2 < rr * rr, dist2 > 0.0001 else { continue }
        let dist = dist2.squareRoot()
        let deep = rr - dist - Self.slop
        guard deep > 0 else { continue }
        any = true
        if (si || sj) && deep > Self.wakeDepth {
          if si { wake(i >> 1); si = false } else { wake(j >> 1); sj = false }
        }
        let push = d * (deep / dist / 2)
        if sj { pos[i] -= push * 2 } else if si { pos[j] += push * 2 } else { pos[i] -= push; pos[j] += push }
      }
      return any
    }

    /// Left, right and bottom screen edges (the top stays open): out, the speed into it reflected by 0.2, a little
    /// friction along it; whether any was hit.
    private func walls(_ k: Float) -> Bool {
      var any = false
      let along: Float = ticks == 2 ? 0.81 : 0.9
      for p in 0..<(2 * count) where !asleep[p >> 1] {
        let r = rad[p]
        var q = pos[p]
        var n = SIMD2<Float>.zero
        if q.x < minX + r { q.x = minX + r; n.x = 1 } else if q.x > maxX - r { q.x = maxX - r; n.x = -1 }
        if q.y > maxY - r { q.y = maxY - r; n.y = -1 }
        guard n != .zero else { continue }
        any = true
        pos[p] = q
        var v = q - old[p]
        if n.x != 0 && v.x * n.x < 0 { v.x = -v.x * 0.2; v.y *= along }
        if n.y != 0 && v.y * n.y < 0 { v.y = -v.y * 0.2; v.x *= along }
        old[p] = q - v
      }
      return any
    }

    @inline(__always) static func dot(_ a: SIMD2<Float>, _ b: SIMD2<Float>) -> Float { a.x * b.x + a.y * b.y }
    @inline(__always) static func finite(_ a: SIMD2<Float>) -> Bool { a.x.isFinite && a.y.isFinite }
  }
#endif
