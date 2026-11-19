// Yo's App Shortcuts for Shortcuts, Siri and the Action button; the intents are in YoIntents.swift.
import AppIntents

struct YoShortcuts: AppShortcutsProvider {
  static var appShortcuts: [AppShortcut] {
    AppShortcut(
      intent: SendYoIntent(),
      phrases: [
        "Send a Yo with \(.applicationName)",
        "Send a Yo to \(\.$friend) with \(.applicationName)",
        "\(.applicationName) \(\.$friend)",
        "Send a \(.applicationName) to \(\.$friend)",
        "Send \(\.$friend) a \(.applicationName)",
        "Send a \(.applicationName)",
      ],
      shortTitle: "Send Yo", systemImageName: "hand.wave.fill")
    AppShortcut(
      intent: YoBackIntent(),
      phrases: ["\(.applicationName) back", "Send a Yo back with \(.applicationName)"],
      shortTitle: "Yo Back", systemImageName: "arrowshape.turn.up.left.fill")
    AppShortcut(
      intent: YoCameraIntent(),
      phrases: ["\(.applicationName) camera", "Open the camera in \(.applicationName)"],
      shortTitle: "Yo Camera", systemImageName: "camera.fill")
    AppShortcut(
      intent: OpenYoIntent(),
      phrases: ["Open \(.applicationName)"],
      shortTitle: "Open Yo", systemImageName: "y.circle.fill")
  }
}
