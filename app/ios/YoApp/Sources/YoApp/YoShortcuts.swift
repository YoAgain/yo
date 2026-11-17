// Yo's actions as Shortcuts and Siri know them without setting anything up, and as the Action button offers them
// (Settings › Action Button › Shortcut). The intents are in YoIntents.swift.
import AppIntents

struct YoShortcuts: AppShortcutsProvider {
  static var appShortcuts: [AppShortcut] {
    AppShortcut(
      intent: SendYoIntent(),
      phrases: [
        "Send a Yo with \(.applicationName)",
        "Send a Yo to \(\.$friend) with \(.applicationName)",
        "\(.applicationName) \(\.$friend)",
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
