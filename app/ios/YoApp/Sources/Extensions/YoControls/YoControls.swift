// Controls (Control Center, Lock Screen, Action button), widgets and the Live Activity; intents in YoIntents.swift.
// The controls' Yo symbols (yo.wordmark, yo.back, yo.letter) are custom SF Symbols in Resources/Symbols.xcassets
// (dev/symbols/make_symbols.py), compiled into this extension by dev/actool.sh.
import AppIntents
import Foundation
import SwiftUI
import WidgetKit

// @main: NSExtensionMain (ITMS-90898) finds the bundle through the __swift5_entry section @main writes (ITMS-90896).
@main
struct YoControls: WidgetBundle {
  var body: some Widget {
    SendYoControl()
    YoBackControl()
    YoCameraControl()
    OpenYoControl()
    YoFriendWidget()
    YoListWidget()
    YoBackWidget()
    YoOpenWidget()
    YoCameraWidget()
    YoSentLiveActivity()
  }
}

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
      // Without a friend (its configuration skipped) the intent opens Yo, where a tap on a name sends the Yo.
      ControlWidgetButton(action: friend.map(SendYoNowIntent.init(friend:)) ?? SendYoNowIntent()) {
        Label(friend.map { "Yo \($0.title)" } ?? "Send Yo", image: "yo.wordmark")
          .controlWidgetActionHint(friend.map { "Yo \($0.title)" } ?? "Open Yo")
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
      ControlWidgetButton(action: YoBackNowIntent()) {
        Label("Yo Back", image: "yo.back")
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
        Label("Open Yo", image: "yo.letter")
      }
    }
    .displayName("Open Yo")
    .description("Opens Yo.")
  }
}
