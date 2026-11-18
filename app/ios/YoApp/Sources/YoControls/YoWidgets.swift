// Yo's widgets: the friends' bars on the Home Screen as in the app, each a Yo on a tap without opening it (small: one
// friend, picked when adding it, else the top one, on Yo's purple; medium: the top 4; large: the top 8), and on the
// Lock Screen "Yo" to one friend and "Yo Back". The list is the one the app writes for its extensions
// (ShareBridge.List, in its order), which reloads the widgets when it changes (ShareInbox); a sent Yo reads "Sent Yo!"
// on its bar for a moment (WidgetSent), then the name again.
import AppIntents
import SwiftUI
import WidgetKit

/// What a widget shows at one moment: the list (nil: signed out) and whom a Yo just went to.
struct YoEntry: TimelineEntry {
  let date: Date
  let list: ShareBridge.List?
  let sent: WidgetSent?
  /// The friend a one-friend widget was set to (nil: the top one).
  var picked: FriendEntity? = nil

  var friends: [ShareBridge.Friend] { list?.friends ?? [] }

  /// The friend a one-friend widget sends to: the one picked (by whom the Yo goes to, else by name: the target turns
  /// from a name into a user id after the first Yo), else the top one.
  var friend: ShareBridge.Friend? {
    guard let picked else { return friends.first }
    return friends.first { $0.to == picked.id } ?? friends.first { $0.name == picked.name } ?? friends.first
  }

  func justSent(_ friend: ShareBridge.Friend) -> Bool { sent?.to == friend.to }

  /// Now, and if a Yo was just sent, again once its "Sent Yo!" is over.
  static func timeline(picked: FriendEntity? = nil) -> Timeline<YoEntry> {
    let now = Date.now
    let list = ShareBridge.readList()
    var entries = [YoEntry(date: now, list: list, sent: WidgetSent.read(now: now), picked: picked)]
    if let sent = entries[0].sent { entries.append(YoEntry(date: sent.until, list: list, sent: nil, picked: picked)) }
    return Timeline(entries: entries, policy: .never)
  }

  static let preview = YoEntry(
    date: .now,
    list: ShareBridge.List(
      owner: "",
      friends: ["Leo", "Mia", "Noah", "Emma", "Finn", "Lena", "Ben", "YoGPT"].enumerated().map {
        ShareBridge.Friend(name: $1, color: $0, to: $1)
      }),
    sent: nil)
}

// MARK: - One friend (Home Screen small, Lock Screen)

/// The friend a one-friend widget sends to, picked when adding it; none: the top one.
struct YoFriendConfiguration: WidgetConfigurationIntent {
  static let title: LocalizedStringResource = "Yo a Friend"
  static let description = IntentDescription("Sends a Yo to a friend or group.")
  static let isDiscoverable = false

  @Parameter(title: "Friend")
  var friend: FriendEntity?

  init() {}
}

struct YoFriendWidget: Widget {
  var body: some WidgetConfiguration {
    AppIntentConfiguration(kind: "app.justyo.ios.widget.friend", intent: YoFriendConfiguration.self, provider: Provider()) {
      YoFriendView(entry: $0)
    }
    .configurationDisplayName("Yo a Friend")
    .description("A tap sends a Yo. Your top friend, or the one you pick.")
    .supportedFamilies([.systemSmall, .accessoryCircular])
    .contentMarginsDisabled()
  }

  struct Provider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> YoEntry { .preview }

    func snapshot(for configuration: YoFriendConfiguration, in context: Context) async -> YoEntry {
      let entry = YoEntry.timeline(picked: configuration.friend).entries[0]
      return entry.list == nil && context.isPreview ? .preview : entry
    }

    func timeline(for configuration: YoFriendConfiguration, in context: Context) async -> Timeline<YoEntry> {
      YoEntry.timeline(picked: configuration.friend)
    }
  }
}

struct YoFriendView: View {
  let entry: YoEntry
  @Environment(\.widgetFamily) private var family

  var body: some View {
    if family == .accessoryCircular {
      circle
    } else if let friend = entry.friend {
      // Always Yo's own purple, whatever the friend's bar colour in the list: the widget is Yo's, the name on it.
      Button(intent: SendYoIntent(friend: FriendEntity(friend))) {
        BarLabel(text: entry.justSent(friend) ? "Sent Yo!" : friend.name, size: 28)
      }
      .buttonStyle(.plain)
      .containerBackground(.amethyst, for: .widget)
    } else {
      SignedOut()
    }
  }

  /// The Lock Screen's: "Yo" over the name.
  @ViewBuilder private var circle: some View {
    if let friend = entry.friend {
      Button(intent: SendYoIntent(friend: FriendEntity(friend))) {
        CircleLines(top: entry.justSent(friend) ? "Sent" : "Yo", bottom: entry.justSent(friend) ? "Yo!" : friend.name)
      }
      .buttonStyle(.plain)
      .containerBackground(for: .widget) { AccessoryWidgetBackground() }
    } else {
      CircleLines(top: "Yo", bottom: "")
        .containerBackground(for: .widget) { AccessoryWidgetBackground() }
    }
  }
}

// MARK: - The list (Home Screen medium and large)

struct YoListWidget: Widget {
  var body: some WidgetConfiguration {
    StaticConfiguration(kind: "app.justyo.ios.widget.list", provider: Provider()) { YoListView(entry: $0) }
      .configurationDisplayName("Yo")
      .description("Your top friends, as in Yo. A tap on a bar sends a Yo.")
      .supportedFamilies([.systemMedium, .systemLarge])
      .contentMarginsDisabled()
  }

  struct Provider: TimelineProvider {
    func placeholder(in context: Context) -> YoEntry { .preview }

    func getSnapshot(in context: Context, completion: @escaping (YoEntry) -> Void) {
      let entry = YoEntry.timeline().entries[0]
      completion(entry.list == nil && context.isPreview ? .preview : entry)
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<YoEntry>) -> Void) {
      completion(YoEntry.timeline())
    }
  }
}

struct YoListView: View {
  let entry: YoEntry
  @Environment(\.widgetFamily) private var family

  var body: some View {
    let friends = Array(entry.friends.prefix(family == .systemLarge ? 8 : 4))
    if friends.isEmpty {
      SignedOut()
    } else {
      VStack(spacing: 0) {
        ForEach(friends, id: \.to) { friend in
          Button(intent: SendYoIntent(friend: FriendEntity(friend))) {
            BarLabel(text: entry.justSent(friend) ? "Sent Yo!" : friend.name, size: 22)
              .background(Color.row(friend.color))
          }
          .buttonStyle(.plain)
        }
      }
      .containerBackground(Color.row(friends[friends.count - 1].color), for: .widget)
    }
  }
}

// MARK: - Yo Back (Lock Screen)

struct YoBackWidget: Widget {
  var body: some WidgetConfiguration {
    StaticConfiguration(kind: "app.justyo.ios.widget.back", provider: Provider()) { YoBackView(entry: $0) }
      .configurationDisplayName("Yo Back")
      .description("Sends a Yo back to whoever sent you the last one.")
      .supportedFamilies([.accessoryCircular])
  }

  struct Provider: TimelineProvider {
    func placeholder(in context: Context) -> YoEntry { .preview }

    func getSnapshot(in context: Context, completion: @escaping (YoEntry) -> Void) {
      completion(YoEntry.timeline().entries[0])
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<YoEntry>) -> Void) {
      completion(YoEntry.timeline())
    }
  }
}

struct YoBackView: View {
  let entry: YoEntry

  var body: some View {
    Button(intent: YoBackIntent()) {
      if entry.sent?.back == true {
        CircleLines(top: "Sent", bottom: "Yo!")
      } else {
        VStack(spacing: 1) {
          Image(systemName: "arrowshape.turn.up.left.fill").font(.system(size: 13, weight: .bold))
          Text("Yo Back").font(.yo(11, relativeTo: .caption)).lineLimit(1).minimumScaleFactor(0.6)
        }
        .padding(4)
      }
    }
    .buttonStyle(.plain)
    .containerBackground(for: .widget) { AccessoryWidgetBackground() }
  }
}

// MARK: - Parts

/// A bar's text: the name (or "Sent Yo!") in white Montserrat Bold, the whole bar its button.
private struct BarLabel: View {
  let text: String
  let size: CGFloat

  var body: some View {
    Text(verbatim: text)
      .font(.yo(size))
      .foregroundStyle(.white)
      .lineLimit(1)
      .minimumScaleFactor(0.5)
      .padding(.horizontal, 12)
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .contentShape(.rect)
      // A tinted Home Screen: the name in the accent, as the system's own widgets.
      .widgetAccentable()
  }
}

/// A Lock Screen circle's two lines: "Yo" over a name.
private struct CircleLines: View {
  let top: String
  let bottom: String

  var body: some View {
    VStack(spacing: 0) {
      Text(verbatim: top).font(.yo(16, relativeTo: .headline))
      if !bottom.isEmpty {
        Text(verbatim: bottom).font(.system(size: 11, weight: .semibold)).lineLimit(1).minimumScaleFactor(0.6)
      }
    }
    .padding(4)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }
}

/// No list (signed out, or the app not opened yet): Yo's own purple, a tap opens it.
private struct SignedOut: View {
  var body: some View {
    VStack(spacing: 4) {
      Text(verbatim: "Yo").font(.yo(34))
      Text("Open Yo to see your friends here.").font(.caption.weight(.semibold)).multilineTextAlignment(.center)
    }
    .foregroundStyle(.white)
    .padding()
    .containerBackground(.amethyst, for: .widget)
  }
}
