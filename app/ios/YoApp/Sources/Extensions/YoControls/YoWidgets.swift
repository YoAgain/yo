// Widgets: the list's bars on the Home Screen (one widget for every size, so the size can change in place), and on the
// Lock Screen: Yo to one friend (round and wide), Yo Back (round and wide, "Yo back · Sora, 2m"), Open Yo (round, and
// "Yo from Sora · 2m" above the clock) and Yo Camera. Only Yo opens Yo; a name sends. They read ShareBridge.List and
// LastYo, which the app and the notification service reload them for.
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
  /// Who sent the last Yo to this account.
  var last: ShareBridge.LastYo? = nil

  /// "2m" in Yo Back, at this entry's time.
  var lastAge: String? { last.map { ShareBridge.age($0.date, now: date) } }

  /// The last sender as the list shows them now, as Yo Back names them (IntentYo.back).
  var lastName: String? {
    last.map { last in ShareBridge.Friend.title(friends.first { $0.to == last.to }?.name ?? last.name, to: last.to) }
  }

  var friends: [ShareBridge.Friend] { list?.friends ?? [] }

  /// The last sender's name now, for the Yo Back control; nil if none (or another account's).
  static func lastSender() -> String? {
    let list = ShareBridge.readList()
    let last = ShareBridge.readLastYo().flatMap { list == nil || $0.owner == list?.owner ? $0 : nil }
    return YoEntry(date: .now, list: list, sent: nil, last: last).lastName
  }

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

  /// `ageing`: an entry at each change of the last Yo's age ("2m", "3m", … "1h"), the next 60 of them, then a reload.
  static func timeline(picked: [FriendEntity?] = [], ageing: Bool = false) -> Timeline<YoEntry> {
    let now = Date.now
    let list = ShareBridge.readList()
    let sent = WidgetSent.read(now: now)
    // Another account's last Yo is not this one's.
    let last = ageing ? ShareBridge.readLastYo().flatMap { list == nil || $0.owner == list?.owner ? $0 : nil } : nil
    var dates = [now]
    if let sent { dates.append(sent.until) }
    let steps = last.map { ageSteps(from: $0.date, after: now) } ?? []
    dates += steps
    let entries = Set(dates).sorted().map { date in
      YoEntry(date: date, list: list, sent: sent.flatMap { date < $0.until ? $0 : nil }, picked: picked, last: last)
    }
    return Timeline(entries: entries, policy: steps.isEmpty ? .never : .atEnd)
  }

  /// When "now" turns "1m", each minute to "59m", each hour to "23h", then each day: the next 60 after `now`.
  static func ageSteps(from start: Date, after now: Date) -> [Date] {
    let minutes = (1..<60).map { start + Double($0) * 60 }
    let hours = (1..<24).map { start + Double($0) * 3600 }
    let days = (1...30).map { start + Double($0) * 86400 }
    return Array((minutes + hours + days).filter { $0 > now }.prefix(60))
  }

  static let preview = YoEntry(
    date: .now,
    list: ShareBridge.List(
      owner: "",
      friends: [
        "Leo", "Mia", "Noah", "Emma", "Finn", "Lena", "Ben", "YoGPT", "Ida", "Paul", "Ella", "Max", "Lia", "Tom", "Zoe",
        "Jan",
      ].enumerated().map { ShareBridge.Friend(name: $1, color: $0, to: $1) }),
    sent: nil,
    last: ShareBridge.LastYo(owner: "", to: "Sora", name: "Sora", sentAtMs: UInt64((Date.now.timeIntervalSince1970 - 120) * 1000)))
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
    .supportedFamilies([.accessoryCircular, .accessoryRectangular])
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
    Group {
      if let friend = entry.friend {
        Button(intent: SendYoNowIntent(friend: FriendEntity(friend))) {
          if entry.justSent(friend) {
            SentLabel(wide: family == .accessoryRectangular)
          } else if family == .accessoryRectangular {
            // "Yo Sora": the wordmark, then the name in the brand's font.
            HStack(spacing: 6) {
              Wordmark(size: 30)
              Text(verbatim: friend.title).font(.yo(22, relativeTo: .title2)).lineLimit(1).minimumScaleFactor(0.5)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
          } else {
            VStack(spacing: 0) {
              Wordmark(size: 24)
              Text(verbatim: friend.title).font(.system(size: 11, weight: .semibold)).lineLimit(1).minimumScaleFactor(0.6)
            }
            .padding(4)
          }
        }
        .buttonStyle(.plain)
      } else {
        // Signed out or no friend yet: Yo only, so a tap opens Yo.
        Wordmark(size: family == .accessoryRectangular ? 30 : 26)
      }
    }
    .containerBackground(for: .widget) { AccessoryWidgetBackground() }
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
      .supportedFamilies([.accessoryCircular, .accessoryRectangular])
  }

  struct Provider: TimelineProvider {
    func placeholder(in context: Context) -> YoEntry { .preview }

    func getSnapshot(in context: Context, completion: @escaping (YoEntry) -> Void) {
      let entry = YoEntry.timeline(ageing: true).entries[0]
      completion(entry.last == nil && context.isPreview ? .preview : entry)
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<YoEntry>) -> Void) {
      completion(YoEntry.timeline(ageing: true))
    }
  }
}

struct YoBackView: View {
  let entry: YoEntry
  @Environment(\.widgetFamily) private var family

  var body: some View {
    Button(intent: YoBackNowIntent()) {
      if entry.sent?.back == true {
        SentLabel(wide: family == .accessoryRectangular)
      } else if family == .accessoryRectangular {
        // The Yo-back symbol says what it does; beside it just who Yo'd you last ("Sora").
        HStack(spacing: 8) {
          Image("yo.back").font(.system(size: 30, weight: .bold)).widgetAccentable()
          VStack(alignment: .leading, spacing: 0) {
            Text(verbatim: entry.lastName ?? productText(text: .noAnswerYet, lang: .english)).font(.yo(17, relativeTo: .headline))
          }
          .lineLimit(1)
          .minimumScaleFactor(0.6)
          Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
      } else {
        VStack(spacing: 1) {
          Image("yo.back").font(.system(size: 18, weight: .bold)).widgetAccentable()
          // Who it would Yo back (the symbol says what), as the friend widget shows its friend.
          if let name = entry.lastName {
            Text(verbatim: name).font(.yo(11, relativeTo: .caption)).lineLimit(1).minimumScaleFactor(0.6)
          }
        }
        .padding(4)
      }
    }
    .buttonStyle(.plain)
    .containerBackground(for: .widget) { AccessoryWidgetBackground() }
  }
}

// MARK: - Open Yo (Lock Screen): Yo alone, and "Yo from Sora · 2m" above the clock

struct YoOpenWidget: Widget {
  var body: some WidgetConfiguration {
    StaticConfiguration(kind: "app.justyo.ios.widget.open", provider: YoBackWidget.Provider()) { YoOpenView(entry: $0) }
      .configurationDisplayName("Open Yo")
      .description("Opens Yo. Above the clock: who sent you the last Yo.")
      .supportedFamilies([.accessoryCircular, .accessoryInline])
  }
}

struct YoOpenView: View {
  let entry: YoEntry
  @Environment(\.widgetFamily) private var family

  var body: some View {
    Group {
      if family == .accessoryInline {
        // One line beside the date: the system draws it (one symbol and the text, no background).
        Label {
          Text(verbatim: entry.lastName.map {
            productNotificationLine(name: $0, kind: .yo, request: false, lang: .english)
          } ?? productText(text: .yo, lang: .english))
        } icon: {
          Image("yo.letter")
        }
      } else {
        Wordmark(size: 26)
      }
    }
    .widgetURL(URL(string: "yo://open"))
    .containerBackground(for: .widget) { AccessoryWidgetBackground() }
  }
}

// MARK: - Yo Camera (Lock Screen)

struct YoCameraWidget: Widget {
  var body: some WidgetConfiguration {
    StaticConfiguration(kind: "app.justyo.ios.widget.camera", provider: Provider()) { _ in YoCameraView() }
      .configurationDisplayName("Yo Camera")
      .description("Opens Yo in the camera.")
      .supportedFamilies([.accessoryCircular])
  }

  struct Provider: TimelineProvider {
    func placeholder(in context: Context) -> SimpleEntry { SimpleEntry(date: .now) }
    func getSnapshot(in context: Context, completion: @escaping (SimpleEntry) -> Void) { completion(SimpleEntry(date: .now)) }
    func getTimeline(in context: Context, completion: @escaping (Timeline<SimpleEntry>) -> Void) {
      completion(Timeline(entries: [SimpleEntry(date: .now)], policy: .never))
    }
  }

  struct SimpleEntry: TimelineEntry { let date: Date }
}

struct YoCameraView: View {
  var body: some View {
    VStack(spacing: 1) {
      Image(systemName: "camera.fill").font(.system(size: 20, weight: .semibold)).widgetAccentable()
      Text(verbatim: productText(text: .yo, lang: .english)).font(.yo(11, relativeTo: .caption))
    }
    .padding(4)
    // yo://camera: the app opens in the camera (YoApp's onOpenURL), as Yo Camera's control does.
    .widgetURL(URL(string: "yo://camera"))
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

/// Yo's wordmark (the custom symbol yo.wordmark, as on the controls), the widget's accent on a tinted Lock Screen.
private struct Wordmark: View {
  let size: CGFloat

  var body: some View {
    Image("yo.wordmark")
      .font(.system(size: size, weight: .bold))
      .widgetAccentable()
      .accessibilityLabel(Text(verbatim: productText(text: .yo, lang: .english)))
  }
}

/// "Sent Yo!" for a moment after a tap: stacked in a circle, one line in a rectangle.
private struct SentLabel: View {
  let wide: Bool

  var body: some View {
    let sent = productText(text: .sent, lang: .english)
    let yo = productText(text: .yoBang, lang: .english)
    Group {
      if wide {
        Text(verbatim: "\(sent) \(yo)").font(.yo(22, relativeTo: .title2))
      } else {
        VStack(spacing: 0) {
          Text(verbatim: sent).font(.yo(16, relativeTo: .headline))
          Text(verbatim: yo).font(.system(size: 11, weight: .semibold))
        }
      }
    }
    .lineLimit(1)
    .minimumScaleFactor(0.6)
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
