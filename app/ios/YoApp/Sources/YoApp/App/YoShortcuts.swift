// Yo's App Shortcuts for Shortcuts, Siri and the Action button; the intents are in YoIntents.swift.
import AppIntents

struct YoShortcuts: AppShortcutsProvider {
  static var appShortcuts: [AppShortcut] {
    AppShortcut(
      intent: SendYoIntent(),
      phrases: ["\(.applicationName) \(\.$friend)"],
      shortTitle: "Send Yo", systemImageName: "hand.wave.fill")
    AppShortcut(
      intent: YoBackIntent(),
      phrases: ["\(.applicationName) back"],
      shortTitle: "Yo Back", systemImageName: "arrowshape.turn.up.left.fill")
    AppShortcut(
      intent: YoCameraIntent(),
      phrases: ["\(.applicationName) camera"],
      shortTitle: "Yo Camera", systemImageName: "camera.fill")
    AppShortcut(
      intent: OpenYoIntent(),
      phrases: ["Open \(.applicationName)"],
      shortTitle: "Open Yo", systemImageName: "y.circle.fill")
  }
}
