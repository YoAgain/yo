// Yo in Control Center, on the Lock Screen and on the Action button (Settings › Action Button › Controls): "Send Yo"
// to the friend or group picked when adding it, "Yo Back", "Yo Camera" and "Open Yo". Each runs its intent
// (YoIntents.swift, linked from the app's sources); sending works while the iPhone is locked. Beside them the widgets
// (YoWidgets.swift).
import AppIntents
import Foundation
import SwiftUI
import WidgetKit

// @main, as Xcode builds it: the extension starts at Foundation's NSExtensionMain (xtool links it so; App Store
// processing wants exactly that, ITMS-90898), which finds this bundle through the __swift5_entry section the
// compiler writes for @main (ITMS-90896).
@main
struct YoControls: WidgetBundle {
  var body: some Widget {
    SendYoControl()
    YoBackControl()
    YoCameraControl()
    OpenYoControl()
    // The widgets (YoWidgets.swift).
    YoFriendWidget()
    YoListWidget()
    YoBackWidget()
    // "Sent Yo! to Mia" in the Dynamic Island after a Yo from here (YoLiveActivity.swift).
    YoSentLiveActivity()
  }
}

/// The friend a "Send Yo" control sends to, picked when it is added.
struct SendYoConfiguration: ControlConfigurationIntent {
  static let title: LocalizedStringResource = "Send Yo"
  static let isDiscoverable = false

  @Parameter(title: "Friend")
  var friend: FriendEntity?

  init() {}

  func perform() async throws -> some IntentResult { .result() }
}

struct SendYoControl: ControlWidget {
  var body: some ControlWidgetConfiguration {
    AppIntentControlConfiguration(kind: "app.justyo.ios.controls.send", provider: Provider()) { friend in
      ControlWidgetButton(action: friend.map(SendYoIntent.init(friend:)) ?? SendYoIntent()) {
        Label(friend.map { "Yo \($0.name)" } ?? "Send Yo", systemImage: "hand.wave.fill")
      }
    }
    .displayName("Send Yo")
    .description("Sends one Yo to a friend or group.")
    .promptsForUserConfiguration()
  }

  struct Provider: AppIntentControlValueProvider {
    func previewValue(configuration: SendYoConfiguration) -> FriendEntity? { configuration.friend }
    func currentValue(configuration: SendYoConfiguration) async throws -> FriendEntity? { configuration.friend }
  }
}

struct YoBackControl: ControlWidget {
  var body: some ControlWidgetConfiguration {
    StaticControlConfiguration(kind: "app.justyo.ios.controls.back") {
      ControlWidgetButton(action: YoBackIntent()) {
        Label("Yo Back", systemImage: "arrowshape.turn.up.left.fill")
      }
    }
    .displayName("Yo Back")
    .description("Sends a Yo back to whoever sent you the last one.")
  }
}

struct YoCameraControl: ControlWidget {
  var body: some ControlWidgetConfiguration {
    StaticControlConfiguration(kind: "app.justyo.ios.controls.camera") {
      ControlWidgetButton(action: YoCameraIntent()) {
        Label("Yo Camera", systemImage: "camera.fill")
      }
    }
    .displayName("Yo Camera")
    .description("Opens Yo in the camera.")
  }
}

struct OpenYoControl: ControlWidget {
  var body: some ControlWidgetConfiguration {
    StaticControlConfiguration(kind: "app.justyo.ios.controls.open") {
      ControlWidgetButton(action: OpenYoIntent()) {
        Label("Open Yo", systemImage: "y.circle.fill")
      }
    }
    .displayName("Open Yo")
    .description("Opens Yo.")
  }
}
