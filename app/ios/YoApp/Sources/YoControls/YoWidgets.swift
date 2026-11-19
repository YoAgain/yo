import YoCore
// Yo's widgets: the friends' bars on the Home Screen as in the app, each a Yo on a tap without opening it, and on the
// Lock Screen "Yo" to one friend and "Yo Back". On the Home Screen one widget for every size, so the size can change
// in place: whom it shows is picked in Edit Widget (Friend 1 to 16, in that order), each size shows as many of them
// as it holds (small 4, medium 4, large 8, tall 16), the bars sharing its height; none picked: the top of the list
// (small 1, on Yo's purple). Groups read as in the app ("#Spanien 2024"), each bar in its colour on the app's list.
// The list is the one the app writes for its extensions (ShareBridge.List, in its order), which reloads the widgets
// when it changes (ShareInbox); a sent Yo reads "Sent Yo!" on its bar for a moment (WidgetSent), then the name again.
import AppIntents
import Foundation
import SwiftUI
import WidgetKit

/// What a widget shows at one moment: the list (nil: signed out) and whom a Yo just went to.
struct YoEntry: TimelineEntry {
  let date: Date
  let list: ShareBridge.List?
  let sent: WidgetSent?
  /// The friends picked in Edit Widget, slot by slot (nil: an empty slot).
  var picked: [FriendEntity?] = []

  var friends: [ShareBridge.Friend] { list?.friends ?? [] }

  /// The friend a Lock Screen circle sends to.
  var friend: ShareBridge.Friend? { shown(1, else: 1).first }

  /// At most `count` bars: the picks in their slots' order (by whom the Yo goes to, else by name: the target turns from
  /// a name into a user id after the first Yo), each once; none picked (or none still on the list): the top `top`.
  func shown(_ count: Int, else top: Int) -> [ShareBridge.Friend] {
    let friends = self.friends
    return productWidgetSelection(
      friends: friends.map { ProductWidgetFriend(id: $0.to.precomposedStringWithCanonicalMapping, name: $0.name.precomposedStringWithCanonicalMapping) },
      picks: picked.map { $0.map { ProductWidgetFriend(id: $0.id.precomposedStringWithCanonicalMapping, name: $0.name.precomposedStringWithCanonicalMapping) } },
      slots: UInt32(count), fallback: UInt32(top)
    ).map { friends[Int($0)] }
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
      friends: [
        "Leo", "Mia", "Noah", "Emma", "Finn", "Lena", "Ben", "YoGPT", "Ida", "Paul", "Ella", "Max", "Lia", "Tom", "Zoe",
        "Jan",
      ].enumerated().map { ShareBridge.Friend(name: $1, color: $0, to: $1) }),
    sent: nil)
}

// MARK: - One friend (Lock Screen)

/// The friend or group a Lock Screen circle sends to, picked in Edit Widget; none: the top one.
struct YoFriendConfiguration: WidgetConfigurationIntent {
  // The App Intents metadata extractor reads these compile-time values.
  static let title: LocalizedStringResource = "Yo a Friend"
  static let description = IntentDescription("Sends a Yo to a friend or group.")
  static let isDiscoverable = false

  @Parameter(title: "Friend or Group")
  var friend: FriendEntity?

  init() {}
}

/// The Lock Screen's "Yo" to one friend. (Its kind once also had a small Home Screen size, now the list widget's.)
struct YoFriendWidget: Widget {
  var body: some WidgetConfiguration {
    AppIntentConfiguration(kind: "app.justyo.ios.widget.friend", intent: YoFriendConfiguration.self, provider: Provider()) {
      YoFriendView(entry: $0)
    }
    .configurationDisplayName(LocalizedStringResource(String.LocalizationValue(productText(text: .widgetFriend, lang: .english))))
    .description(LocalizedStringResource(String.LocalizationValue(productText(text: .widgetFriendDescription, lang: .english))))
    .supportedFamilies([.accessoryCircular])
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

/// "Yo" over the name.
struct YoFriendView: View {
  let entry: YoEntry

  var body: some View {
    if let friend = entry.friend {
      Button(intent: SendYoNowIntent(friend: FriendEntity(friend))) {
        CircleLines(top: entry.justSent(friend) ? productText(text: .sent, lang: .english) : productText(text: .yo, lang: .english), bottom: entry.justSent(friend) ? productText(text: .yoBang, lang: .english) : friend.title)
      }
      .buttonStyle(.plain)
      .containerBackground(for: .widget) { AccessoryWidgetBackground() }
    } else {
      CircleLines(top: productText(text: .yo, lang: .english), bottom: "")
        .containerBackground(for: .widget) { AccessoryWidgetBackground() }
    }
  }
}

// MARK: - The list (Home Screen, every size)

/// The bars of the Home Screen widget, picked in Edit Widget in their order; each size shows as many as it holds. One
/// set for every size (the metadata knows no per-size parameters).
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
  @Parameter(title: "Friend 9 (tall)") var friend9: FriendEntity?
  @Parameter(title: "Friend 10 (tall)") var friend10: FriendEntity?
  @Parameter(title: "Friend 11 (tall)") var friend11: FriendEntity?
  @Parameter(title: "Friend 12 (tall)") var friend12: FriendEntity?
  @Parameter(title: "Friend 13 (tall)") var friend13: FriendEntity?
  @Parameter(title: "Friend 14 (tall)") var friend14: FriendEntity?
  @Parameter(title: "Friend 15 (tall)") var friend15: FriendEntity?
  @Parameter(title: "Friend 16 (tall)") var friend16: FriendEntity?

  var picked: [FriendEntity?] {
    [
      friend1, friend2, friend3, friend4, friend5, friend6, friend7, friend8, friend9, friend10, friend11, friend12,
      friend13, friend14, friend15, friend16,
    ]
  }

  init() {}
}

struct YoListWidget: Widget {
  var body: some WidgetConfiguration {
    AppIntentConfiguration(kind: "app.justyo.ios.widget.list", intent: YoListConfiguration.self, provider: Provider()) {
      YoListView(entry: $0)
    }
    .configurationDisplayName(LocalizedStringResource(String.LocalizationValue(productText(text: .yo, lang: .english))))
    .description(LocalizedStringResource(String.LocalizationValue(productText(text: .widgetListDescription, lang: .english))))
    .supportedFamilies(Self.families)
    .contentMarginsDisabled()
  }

  /// Small to large, and the tall one from iOS 27 (its SDK's compiler: the iOS 26 SDK has no such size).
  static var families: [WidgetFamily] {
    #if compiler(>=6.4)
      if #available(iOS 27, *) { return [.systemSmall, .systemMedium, .systemLarge, .systemExtraLargePortrait] }
    #endif
    return [.systemSmall, .systemMedium, .systemLarge]
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

  /// How many bars this size holds, and how many of the list's top it shows when none are picked.
  private var room: (most: Int, top: Int) {
    switch family {
    case .systemSmall: (4, 1)
    case .systemMedium: (4, 4)
    case .systemLarge: (8, 8)
    default: (16, 16)
    }
  }

  var body: some View {
    let friends = entry.shown(room.most, else: room.top)
    if friends.isEmpty {
      SignedOut()
    } else if family == .systemSmall, friends.count == 1, let friend = friends.first {
      // One friend on a small widget: Yo's own purple, whatever their bar colour on the list.
      Button(intent: SendYoNowIntent(friend: FriendEntity(friend))) {
        BarLabel(text: entry.justSent(friend) ? productSendWords(mark: productCounted(count: 1), sending: false, lang: .english) : friend.title, size: 28)
      }
      .buttonStyle(.plain)
      .containerBackground(.amethyst, for: .widget)
    } else {
      // The bars share the widget's height evenly (2 picked: 2 half-height bars), the name growing with its bar up to
      // the app's own size.
      GeometryReader { space in
        let size = min(Metrics.rowFont, space.size.height / CGFloat(friends.count) * 0.42)
        VStack(spacing: 0) {
          ForEach(friends, id: \.to) { friend in
            Button(intent: SendYoNowIntent(friend: FriendEntity(friend))) {
              BarLabel(text: entry.justSent(friend) ? productSendWords(mark: productCounted(count: 1), sending: false, lang: .english) : friend.title, size: size)
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
      .configurationDisplayName(LocalizedStringResource(String.LocalizationValue(productText(text: .yoBack, lang: .english))))
      .description(LocalizedStringResource(String.LocalizationValue(productText(text: .widgetBackDescription, lang: .english))))
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
    Button(intent: YoBackNowIntent()) {
      if entry.sent?.back == true {
        CircleLines(top: productText(text: .sent, lang: .english), bottom: productText(text: .yoBang, lang: .english))
      } else {
        VStack(spacing: 1) {
          Image(systemName: "arrowshape.turn.up.left.fill").font(.system(size: 13, weight: .bold))
          Text(productText(text: .yoBack, lang: .english)).font(.yo(11, relativeTo: .caption)).lineLimit(1).minimumScaleFactor(0.6)
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
      Text(verbatim: productText(text: .yo, lang: .english)).font(.yo(34))
      Text(productText(text: .widgetEmpty, lang: .english)).font(.caption.weight(.semibold)).multilineTextAlignment(.center)
    }
    .foregroundStyle(.white)
    .padding()
    .containerBackground(.amethyst, for: .widget)
  }
}
