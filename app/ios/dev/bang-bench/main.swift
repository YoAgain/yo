// Benchmark of BangSim (the dev "!" / "Yo" physics) on Linux: ../bang-bench.sh.
// Scenarios: 200 falling and piling, 200 at rest, tilt changes, and a stress run (random tilt every step, spawn bursts,
// broken positions) whose worst step must stay bounded.
#if canImport(Glibc)
  import Glibc
#else
  import Darwin
#endif

@inline(__always) func now() -> UInt64 {
  var t = timespec()
  clock_gettime(CLOCK_MONOTONIC, &t)
  return UInt64(t.tv_sec) * 1_000_000_000 + UInt64(t.tv_nsec)
}

struct Rng {
  var s: UInt64
  mutating func next() -> Double {
    s = s &* 6364136223846793005 &+ 1442695040888963407
    return Double(s >> 11) / Double(1 << 53)
  }
  mutating func range(_ a: Double, _ b: Double) -> Double { a + (b - a) * next() }
}

let W = 390.0, H = 844.0

/// Spawns as the app does: "!" near the top with a small speed, "Yo" from the right edge flying left.
func spawn(_ sim: BangSim, _ rng: inout Rng, yo: Bool) {
  let x = rng.range(20, W - 20), y = rng.range(40, H * 0.6)
  let t = Float(rng.range(-0.6, 0.6))
  if yo {
    let axis = SIMD2<Float>(cos(t), sin(t)) * (sim.length.yo / 2)
    let start = SIMD2<Float>(Float(W) - 10, Float(y))
    let v = SIMD2<Float>(-Float(rng.range(300, 470)), Float(rng.range(-40, 40))) / 120
    sim.add(.yo, origin: 1, a: start - axis, b: start + axis, oldA: start - axis - v, oldB: start + axis - v)
  } else {
    let axis = SIMD2<Float>(sin(t), -cos(t)) * (sim.length.bang / 2)
    let start = SIMD2<Float>(Float(x), Float(y))
    let v = SIMD2<Float>(Float(rng.range(-60, 60)), Float(rng.range(40, 140))) / 120
    sim.add(.bang, origin: 0, a: start + axis, b: start - axis, oldA: start + axis - v, oldB: start - axis - v)
  }
}

struct Stat {
  var total: UInt64 = 0, worst: UInt64 = 0, n = 0
  mutating func add(_ d: UInt64) { total += d; worst = max(worst, d); n += 1 }
  var mean: Double { n == 0 ? 0 : Double(total) / Double(n) }
}

func report(_ name: String, _ s: Stat, ticks: Int = 1) {
  let perSecond = s.mean * 120 / Double(ticks) / 1000
  print(name + String(repeating: " ", count: max(1, 34 - name.count)),
        "mean \(Int(s.mean)) ns/step   worst \(Int(s.worst)) ns   = \(Int(perSecond)) µs per simulated second")
}

func runNew(ticks: Int) {
  let tag = ticks == 2 ? "new 60 Hz" : "new 120 Hz"
  let sim = BangSim(yoRadius: 7, yoLength: 11)
  sim.setScreen(minX: 0, minY: 0, maxX: Float(W), maxY: Float(H))
  sim.setTicks(ticks)
  var rng = Rng(s: 42)
  for _ in 0..<200 { spawn(sim, &rng, yo: false) }
  var fall = Stat(), rest = Stat(), tilt = Stat()
  let fallSteps = 300 / ticks, settle = 1500 / ticks, measure = 600 / ticks
  for _ in 0..<fallSteps { let t = now(); _ = sim.step(); fall.add(now() - t) }
  for _ in 0..<settle { _ = sim.step() }
  for _ in 0..<measure { let t = now(); _ = sim.step(); rest.add(now() - t) }
  for s in 0..<measure {
    if s % (30 / ticks) == 0 {
      let a = Float(s) * 0.01
      sim.setGravity(SIMD2(sin(a) * 0.6, cos(a)))
    }
    let t = now(); _ = sim.step(); tilt.add(now() - t)
  }
  report("\(tag): 200 falling + piling", fall, ticks: ticks)
  report("\(tag): 200 at rest", rest, ticks: ticks)
  report("\(tag): tilt changes", tilt, ticks: ticks)
}

func stress() {
  // 200 objects, random tilt every step, bursts of 30 spawns (the oldest retiring), broken positions now and then,
  // half the steps at 60 Hz: the worst step must stay bounded.
  let sim = BangSim(yoRadius: 7, yoLength: 11)
  sim.setScreen(minX: 0, minY: 0, maxX: Float(W), maxY: Float(H))
  var rng = Rng(s: 7)
  var st = Stat()
  for s in 0..<20_000 {
    if s % 1000 == 0 { sim.setTicks(sim.ticks == 1 ? 2 : 1) }
    sim.setGravity(SIMD2(Float(rng.range(-1, 1)), Float(rng.range(-1, 1))))
    if s % 20 == 0 {
      for _ in 0..<30 {
        if sim.full { sim.retireOldest() } else { spawn(sim, &rng, yo: rng.next() < 0.5) }
      }
    }
    if s % 97 == 0 && sim.count > 0 { sim.pos[Int(rng.range(0, Double(2 * sim.count)))] = SIMD2(.nan, .infinity) }
    if s % 131 == 0 && sim.count > 0 { sim.old[Int(rng.range(0, Double(2 * sim.count)))].x = 1e30 }
    let t = now(); _ = sim.step(); st.add(now() - t)
    for i in 0..<(2 * sim.count) {
      precondition(BangSim.finite(sim.pos[i]), "not finite after a step")
    }
  }
  report("stress: tilt + bursts + NaN", st)
  precondition(st.worst < 5_000_000, "a step took more than 5 ms")
  print("stress: worst step bounded (< 5 ms), \(sim.count) objects at the end")
}

#if OLD
  func runOld() {
    let sim = OldSim()
    var rng = Rng(s: 42)
    for _ in 0..<200 {
      let x = rng.range(20, W - 20), y = rng.range(40, H * 0.6)
      let t = rng.range(-0.6, 0.6)
      let axis = SIMD2<Double>(sin(t), -cos(t)) * 7
      let start = SIMD2(x, y)
      let v = SIMD2(rng.range(-60, 60), rng.range(40, 140)) / 120
      sim.add(start + axis, start - axis, start + axis - v, start - axis - v)
    }
    var fall = Stat(), rest = Stat(), tilt = Stat()
    for _ in 0..<300 { let t = now(); _ = sim.step(); fall.add(now() - t) }
    for _ in 0..<1500 { _ = sim.step() }
    for _ in 0..<600 { let t = now(); _ = sim.step(); rest.add(now() - t) }
    for s in 0..<600 {
      if s % 30 == 0 { let a = Double(s) * 0.01; sim.gravity = SIMD2(sin(a) * 0.6, cos(a)) }
      let t = now(); _ = sim.step(); tilt.add(now() - t)
    }
    report("old: 200 falling + piling", fall)
    report("old: 200 at rest", rest)
    report("old: tilt changes", tilt)
  }
  runOld()
#endif
runNew(ticks: 1)
runNew(ticks: 2)
stress()
