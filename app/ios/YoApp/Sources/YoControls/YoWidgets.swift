// Yo's widgets: the friends' bars on the Home Screen as in the app, each a Yo on a tap without opening it (small: one
// friend or group, on Yo's purple; medium: 4 bars; large: 8), and on the Lock Screen "Yo" to one friend and "Yo Back".
// Whom they show is picked in Edit Widget (a list widget shows just those, its bars sharing its height; none picked:
// the top of the list). Groups read as in the app ("#Spanien 2024"). The list is the one the app writes for its extensions
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
  /// The friends picked in Edit Widget, slot by slot (nil: that slot shows the top of the list).
  var picked: [FriendEntity?] = []

  var friends: [ShareBridge.Friend] { list?.friends ?? [] }

  /// The friend a one-friend widget sends to.
  var friend: ShareBridge.Friend? { shown(1).first ?? friends.first }

  /// At most `count` bars: the picks in their slots' order (by whom the Yo goes to, else by name: the target turns from
  /// a name into a user id after the first Yo), each once; none picked: the top `count` of the list.
  func shown(_ count: Int) -> [ShareBridge.Friend] {
    let picks = picked.prefix(count).compactMap { $0 }
    guard !picks.isEmpty else { return Array(friends.prefix(count)) }
    var seen = Set<String>()
    return picks.compactMap { pick in friends.first { $0.to == pick.id } ?? friends.first { $0.name == pick.name } }
      .filter { seen.insert($0.to).inserted }
  }

  func justSent(_ friend: ShareBridge.Friend) -> Bool { sent?.to == friend.to }

  /// Now, and if a Yo was just sent, again once its "Sent Yo!" is over.
  static func timeline(picked: [FriendEntity?] = []) -> Timeline<YoEntry> {
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

/// The friend or group a one-friend widget sends to, picked in Edit Widget; none: the top one.
struct YoFriendConfiguration: WidgetConfigurationIntent {
  static let title: LocalizedStringResource = "Yo a Friend"
  static let description = IntentDescription("Sends a Yo to a friend or group.")
  static let isDiscoverable = false

  @Parameter(title: "Friend or Group")
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
      let entry = YoEntry.timeline(picked: [configuration.friend]).entries[0]
      return entry.list == nil && context.isPreview ? .preview : entry
    }

    func timeline(for configuration: YoFriendConfiguration, in context: Context) async -> Timeline<YoEntry> {
      YoEntry.timeline(picked: [configuration.friend])
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
        BarLabel(text: entry.justSent(friend) ? "Sent Yo!" : friend.title, size: 28)
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
        CircleLines(top: entry.justSent(friend) ? "Sent" : "Yo", bottom: entry.justSent(friend) ? "Yo!" : friend.title)
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

/// The bars of a list widget, picked in Edit Widget in their order: medium shows the first 4, large all 8, just those
/// picked; none picked: the top of the list. One set for both sizes (the metadata knows no per-size parameters).
struct YoListConfiguration: WidgetConfigurationIntent {
  static let title: LocalizedStringResource = "Yo"
  static let description = IntentDescription("Your friends and groups, in your order. None: the top of your list.")
  static let isDiscoverable = false

  @Parameter(title: "Friend 1") var friend1: FriendEntity?
  @Parameter(title: "Friend 2") var friend2: FriendEntity?
  @Parameter(title: "Friend 3") var friend3: FriendEntity?
  @Parameter(title: "Friend 4") var friend4: FriendEntity?
  @Parameter(title: "Friend 5 (large)") var friend5: FriendEntity?
  @Parameter(title: "Friend 6 (large)") var friend6: FriendEntity?
  @Parameter(title: "Friend 7 (large)") var friend7: FriendEntity?
  @Parameter(title: "Friend 8 (large)") var friend8: FriendEntity?

  var picked: [FriendEntity?] { [friend1, friend2, friend3, friend4, friend5, friend6, friend7, friend8] }

  init() {}
}

struct YoListWidget: Widget {
  var body: some WidgetConfiguration {
    AppIntentConfiguration(kind: "app.justyo.ios.widget.list", intent: YoListConfiguration.self, provider: Provider()) {
      YoListView(entry: $0)
    }
    .configurationDisplayName("Yo")
    .description("Your friends and groups, as in Yo, in the order you pick. A tap on a bar sends a Yo.")
    .supportedFamilies([.systemMedium, .systemLarge])
    .contentMarginsDisabled()
  }

  struct Provider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> YoEntry { .preview }

    func snapshot(for configuration: YoListConfiguration, in context: Context) async -> YoEntry {
      let entry = YoEntry.timeline(picked: configuration.picked).entries[0]
      return entry.list == nil && context.isPreview ? .preview : entry
    }

    func timeline(for configuration: YoListConfiguration, in context: Context) async -> Timeline<YoEntry> {
      YoEntry.timeline(picked: configuration.picked)
    }
  }
}

struct YoListView: View {
  let entry: YoEntry
  @Environment(\.widgetFamily) private var family

  var body: some View {
    let friends = entry.shown(family == .systemLarge ? 8 : 4)
    if friends.isEmpty {
      SignedOut()
    } else {
      // The bars share the widget's height evenly (2 picked: 2 half-height bars), the name growing with its bar up to
      // the app's own size.
      GeometryReader { space in
        let size = min(Metrics.rowFont, space.size.height / CGFloat(friends.count) * 0.42)
        VStack(spacing: 0) {
          ForEach(friends, id: \.to) { friend in
            Button(intent: SendYoIntent(friend: FriendEntity(friend))) {
              BarLabel(text: entry.justSent(friend) ? "Sent Yo!" : friend.title, size: size)
                .background(Color.row(friend.color))
            }
            .buttonStyle(.plain)
          }
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
