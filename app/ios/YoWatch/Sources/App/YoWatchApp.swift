import SwiftUI
import WatchKit

@main struct YoWatchApp: App {
  @WKApplicationDelegateAdaptor(AppDelegate.self) private var delegate

  var body: some Scene {
    WindowGroup { FriendList(link: .shared).tint(.amethyst) }
  }
}
