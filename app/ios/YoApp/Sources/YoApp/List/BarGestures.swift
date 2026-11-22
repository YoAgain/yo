import SwiftUI
import UIKit

/// Observes the entire touch without competing with scroll, row or edge pans. Once moved,
/// a touch stays ineligible even if it returns to its origin before the Button fires.
@MainActor
final class BarTouchGate {
  var blocked = false
  weak var touch: UITouch?
  var origin = CGPoint.zero
  /// The bar in window points, kept as it moves (no redraw): a filming finger zooms by how far it is out of it.
  var frame = CGRect.zero

  var isStationary: Bool {
    !blocked && !(touch?.gestureRecognizers ?? []).contains {
      $0 is UIPanGestureRecognizer && ($0.state == .began || $0.state == .changed || $0.state == .ended)
    }
  }
}

struct BarTouchObserver: UIGestureRecognizerRepresentable {
  let gate: BarTouchGate
  var held: (Bool) -> Void = { _ in }
  var observesHold = false
  /// Camera mode: a finger landing in the bottom controls' strip, beside them, is for the controls, not this bar.
  var spareControls = false
  var pressed: (Bool) -> Void = { _ in }
  /// While held: where the finger is, the bar and the window's height (window points).
  var dragged: (_ y: CGFloat, _ bar: CGRect, _ height: CGFloat) -> Void = { _, _, _ in }
  let cancelled: () -> Void

  func makeUIGestureRecognizer(context: Context) -> Observer {
    let observer = Observer()
    observer.gate = gate
    observer.cancelled = cancelled
    observer.pressed = pressed
    observer.held = held
    observer.dragged = dragged
    observer.observesHold = observesHold
    observer.spareControls = spareControls
    observer.cancelsTouchesInView = false
    observer.delaysTouchesBegan = false
    observer.delaysTouchesEnded = false
    return observer
  }

  func updateUIGestureRecognizer(_ observer: Observer, context: Context) {
    observer.cancelled = cancelled
    observer.pressed = pressed
    observer.held = held
    observer.dragged = dragged
    observer.observesHold = observesHold
    observer.spareControls = spareControls
  }

  func handleUIGestureRecognizerAction(_ observer: Observer, context: Context) {}

  final class Observer: UIGestureRecognizer {
    var gate: BarTouchGate?
    var cancelled: () -> Void = {}
    var held: (Bool) -> Void = { _ in }
    var observesHold = false
    var spareControls = false
    var pressed: (Bool) -> Void = { _ in }
    var dragged: (CGFloat, CGRect, CGFloat) -> Void = { _, _, _ in }
    private var holdTask: Task<Void, Never>?
    private var holding = false
    /// While filming, each frame reads the held finger itself: the moves of a finger that rested through the hold did
    /// not reliably reach this recognizer (on the iPhone they did only after a pinch), the touch itself always moves.
    private var follow: CADisplayLink?
    private var followedY: CGFloat?

    override func reset() {
      stopFollowing()
      holdTask?.cancel()
      pressed(false)
      holding = false
      super.reset()
    }

    override func canPrevent(_ other: UIGestureRecognizer) -> Bool { false }
    override func canBePrevented(by other: UIGestureRecognizer) -> Bool { false }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
      guard let gate, let touch = touches.first else { return }
      // A finger that lands while the list still moves only stops it, as in a table view: no Yo, no photo.
      gate.blocked = touches.count != 1 || listMoving || (spareControls && underControls(touch))
      gate.touch = touch
      gate.origin = touch.location(in: nil)
      pressed(!gate.blocked)
      if observesHold {
        holdTask = Task { @MainActor [weak self] in
          do { try await Task.sleep(for: .milliseconds(300)) } catch { return }
          guard let self, self.gate?.isStationary == true else { return }
          self.holding = true
          self.held(true)
          self.startFollowing()
        }
      }
    }

    /// The scroll view around the bar is gliding or being dragged.
    private var listMoving: Bool {
      var view = self.view?.superview
      while let current = view {
        if let scroll = current as? UIScrollView { return scroll.isDecelerating || scroll.isDragging }
        view = current.superview
      }
      return false
    }

    /// The finger is in the strip of the navigation stack's bottom toolbar (its own frame, a few points more each way).
    private func underControls(_ touch: UITouch) -> Bool {
      var responder: UIResponder? = view
      while let current = responder {
        if let toolbar = (current as? UIViewController)?.navigationController?.toolbar {
          guard !toolbar.isHidden, toolbar.window != nil else { return false }
          let strip = toolbar.convert(toolbar.bounds, to: nil).insetBy(dx: 0, dy: -8)
          let y = touch.location(in: nil).y
          return y >= strip.minY && y <= strip.maxY
        }
        responder = current.next
      }
      return false
    }

    private func checkMovement() {
      // A video once started runs while the finger is down, wherever it goes; only lifting it ends the video.
      guard !holding, let gate, let touch = gate.touch, !gate.blocked else { return }
      let point = touch.location(in: nil)
      if hypot(point.x - gate.origin.x, point.y - gate.origin.y) > 10 || !gate.isStationary {
        gate.blocked = true
        holdTask?.cancel()
        pressed(false)
        cancelled()
      }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
      checkMovement()
      followed()
    }

    private func startFollowing() {
      stopFollowing()
      let link = CADisplayLink(target: self, selector: #selector(followed))
      link.add(to: .main, forMode: .common)
      follow = link
    }

    private func stopFollowing() {
      follow?.invalidate()
      follow = nil
      followedY = nil
    }

    /// Filming, the same finger drawn out of the bar, down or up, zooms the camera.
    @objc private func followed() {
      guard holding, let gate, let touch = gate.touch, let window = touch.window,
        touch.phase != .ended, touch.phase != .cancelled
      else { return }
      let y = touch.location(in: nil).y
      guard y != followedY else { return }
      followedY = y
      dragged(y, gate.frame, window.bounds.height)
    }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
      stopFollowing()
      checkMovement()
      holdTask?.cancel()
      if holding { held(false) }
      pressed(false)
      state = .failed
    }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
      stopFollowing()
      gate?.blocked = true
      holdTask?.cancel()
      pressed(false)
      cancelled()
      state = .failed
    }
  }
}

/// A pan that begins only when it starts sideways: the scroll view keeps vertical moves, and a tap stays a tap.
struct HorizontalPan: UIGestureRecognizerRepresentable {
  var enabled = true
  let changed: (CGFloat) -> Void
  let ended: (_ translation: CGFloat, _ velocity: CGFloat) -> Void

  func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
    let pan = UIPanGestureRecognizer()
    pan.delegate = context.coordinator
    pan.maximumNumberOfTouches = 1
    return pan
  }

  func updateUIGestureRecognizer(_ pan: UIPanGestureRecognizer, context: Context) {
    pan.isEnabled = enabled
  }

  func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
    let x = pan.translation(in: pan.view).x
    switch pan.state {
    case .began, .changed: changed(x)
    case .ended, .cancelled, .failed: ended(x, pan.velocity(in: pan.view).x)
    default: break
    }
  }

  func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

  final class Coordinator: NSObject, UIGestureRecognizerDelegate {
    func gestureRecognizer(_ gesture: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
      guard let window = gesture.view?.window else { return false }
      let x = touch.location(in: window).x
      return x > 24 && x < window.bounds.width - 24
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer,
                           shouldRequireFailureOf other: UIGestureRecognizer) -> Bool {
      other is UIScreenEdgePanGestureRecognizer
    }

    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
      guard let pan = gesture as? UIPanGestureRecognizer else { return false }
      let velocity = pan.velocity(in: pan.view)
      return abs(velocity.x) > abs(velocity.y) * 1.5
    }
  }
}

/// A horizontal pan beginning within the outer 8% of either edge; vertical scrolls keep priority.
struct CameraEdgePan: UIGestureRecognizerRepresentable {
  let left: Bool
  let enabled: Bool
  let touched: () -> Void
  let released: () -> Void
  let changed: (CGFloat) -> Void
  let ended: (CGFloat, CGFloat, Bool) -> Void

  func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
    let pan = EdgeRecognizer()
    pan.released = released
    pan.maximumNumberOfTouches = 1
    context.coordinator.left = left
    context.coordinator.touched = touched
    pan.delegate = context.coordinator
    return pan
  }

  func updateUIGestureRecognizer(_ pan: UIPanGestureRecognizer, context: Context) {
    // Store the next edge even while the previous pan is ending. UIKit resets to
    // .possible later, without necessarily triggering another SwiftUI update.
    context.coordinator.left = left
    context.coordinator.touched = touched
    (pan as? EdgeRecognizer)?.released = released
    pan.isEnabled = enabled
  }

  func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
    let x = pan.translation(in: pan.view).x
    switch pan.state {
    case .began, .changed: changed(x)
    case .ended: ended(x, pan.velocity(in: pan.view).x, false)
    case .cancelled, .failed: ended(x, 0, true)
    default: break
    }
  }

  func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

  /// UIKit may reject a vertical edge touch without ever beginning the pan.
  /// Release its speculative capture start too.
  final class EdgeRecognizer: UIPanGestureRecognizer {
    var released: () -> Void = {}
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
      super.touchesEnded(touches, with: event)
      released()
    }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
      super.touchesCancelled(touches, with: event)
      released()
    }
  }

  final class Coordinator: NSObject, UIGestureRecognizerDelegate {
    var left = true
    var touched: () -> Void = {}
    private var touchLeft = true

    func gestureRecognizer(_ gesture: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
      guard let view = gesture.view else { return false }
      touchLeft = left
      let x = touch.location(in: view).x
      let zone = view.bounds.width * (touchLeft ? 0.25 : 0.08)
      let accepted = touchLeft ? x <= zone : x >= view.bounds.width - zone
      if accepted { touched() }
      return accepted
    }

    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
      guard let pan = gesture as? UIPanGestureRecognizer else { return false }
      let delta = pan.translation(in: pan.view)
      let inward = touchLeft ? delta.x : -delta.x
      let accepted = inward > abs(delta.y) * 1.5
      return accepted
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer,
                           shouldRequireFailureOf other: UIGestureRecognizer) -> Bool {
      false
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer,
                           shouldBeRequiredToFailBy other: UIGestureRecognizer) -> Bool {
      // Vertical scrolling never waits for the camera edge decision.
      // Buttons still wait so an inward swipe cannot also fire the shutter.
      !(other.view is UIScrollView) && other !== gesture
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
      other.view is UIScrollView && other is UIPanGestureRecognizer
    }
  }
}
