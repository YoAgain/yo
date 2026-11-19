import AudioToolbox
import SwiftUI
import UIKit

/// A tap's haptic, played in the tap's own handler from a generator kept ready: `.sensoryFeedback` plays only in
/// SwiftUI's next update, once the list has been rebuilt for the change it watches (8 to 10 ms after the tap in the
/// Simulator, later when the list is busy), and a generator not prepared starts late.
@MainActor enum Haptic {
  private static let heavy = Perf.timed("inbox.haptic.create") { UIImpactFeedbackGenerator(style: .heavy) }

  static func prepareArrival() {
    Perf.timed("inbox.haptic.prepare") { heavy.prepare() }
  }

  private static var arrivalPlayed = false

  static func arrival() {
    guard !arrivalPlayed else { return }
    arrivalPlayed = true
    Perf.timed("inbox.haptic.play") { heavy.impactOccurred() }
    PerfProbe.nextFrame {
      arrivalPlayed = false
      prepareArrival()
    }
  }

  private static let medium = UIImpactFeedbackGenerator(style: .medium)
  private static let light = UIImpactFeedbackGenerator(style: .light)
  private static let rigid = UIImpactFeedbackGenerator(style: .rigid)

  /// A photo taken: short and hard, as the Camera app's shutter.
  static func shutter() {
    rigid.impactOccurred(intensity: 1)
    Perf.mark("tap.haptic")
    rigid.prepare()
  }

  static func tap(_ style: UIImpactFeedbackGenerator.FeedbackStyle = .medium) {
    let generator = style == .light ? light : medium
    generator.impactOccurred()
    Perf.mark("tap.haptic")
    generator.prepare()  // ready for the next tap
  }

  /// The keyboard's click: a system sound, silent with the ring switch off, never interrupting music.
  static func sendClick() {
    AudioServicesPlaySystemSound(1104)
  }

  /// A finger is down: the next `tap` plays without the engine's start.
  static func prepare() {
    medium.prepare()
    rigid.prepare()
  }
}
