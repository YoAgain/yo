// Widgets: the list's bars on the Home Screen (one widget for every size, so the size can change in place), and "Yo"
// to one friend and "Yo Back" on the Lock Screen. They read ShareBridge.List, which the app reloads them for.
import AppIntents
import Foundation
import SwiftUI
import WidgetKit
import YoCore

struct YoEntry: TimelineEntry {
  let date: Date
  let list: ShareBridge.List?
  let sent: WidgetSent?
  var picked: [FriendEntity?] = []

  var friends: [ShareBridge.Friend] { list?.friends ?? [] }

  var friend: ShareBridge.Friend? { shown(1, else: 1).first }

  /// Picks match by target, else by name: a target turns from a name into a user id after the first Yo.
  func shown(_ count: Int, else top: Int) -> [ShareBridge.Friend] {
    let friends = self.friends
    return productWidgetSelection(
      friends: friends.map { ProductWidgetFriend(id: $0.to.precomposedStringWithCanonicalMapping, name: $0.name.precomposedStringWithCanonicalMapping) },
      picks: picked.map { $0.map { ProductWidgetFriend(id: $0.id.precomposedStringWithCanonicalMapping, name: $0.name.precomposedStringWithCanonicalMapping) } },
      slots: UInt32(count), fallback: UInt32(top)
    ).map { friends[Int($0)] }
  }

  func justSent(_ friend: ShareBridge.Friend) -> Bool { sent?.to == friend.to }

  /// "Sent Yo!" for a moment after a Yo, else the name.
  func barText(_ friend: ShareBridge.Friend) -> String {
    justSent(friend) ? productSendWords(mark: productCounted(count: 1), sending: false, lang: .english) : friend.title
  }

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

struct YoFriendConfiguration: WidgetConfigurationIntent {
  // The App Intents metadata extractor reads these compile-time values.
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

/// One set for every size: the metadata knows no per-size parameters.
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

  /// The tall size needs the iOS 27 SDK.
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

  /// Bars this size holds, and how many of the list's top it shows when none are picked.
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
        BarLabel(text: entry.barText(friend), size: 28)
      }
      .buttonStyle(.plain)
      .containerBackground(.amethyst, for: .widget)
    } else {
      // The bars share the height evenly; the name grows with its bar up to the app's size.
      GeometryReader { space in
        let size = min(Metrics.rowFont, space.size.height / CGFloat(friends.count) * 0.42)
        VStack(spacing: 0) {
          ForEach(friends, id: \.to) { friend in
            Button(intent: SendYoNowIntent(friend: FriendEntity(friend))) {
              BarLabel(text: entry.barText(friend), size: size)
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
      .widgetAccentable()
  }
}

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

/// Signed out, or the app not opened yet.
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
