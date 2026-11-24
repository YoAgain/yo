import SwiftUI
import WidgetKit

private struct Entry: TimelineEntry {
  let date: Date
  let last: LastYo?

  var relevance: TimelineEntryRelevance? {
    guard let last else { return nil }
    let remaining = last.at / 1000 + 15 * 60 - date.timeIntervalSince1970
    guard remaining > 0 else { return nil }
    return TimelineEntryRelevance(score: 10, duration: min(remaining, 15 * 60))
  }
}

private struct Provider: TimelineProvider {
  let showsLast: Bool

  func placeholder(in context: Context) -> Entry { Entry(date: .now, last: nil) }

  func getSnapshot(in context: Context, completion: @escaping (Entry) -> Void) {
    completion(entry())
  }

  func getTimeline(in context: Context, completion: @escaping (Timeline<Entry>) -> Void) {
    completion(Timeline(entries: [entry()], policy: .never))
  }

  private func entry() -> Entry {
    Entry(date: .now, last: showsLast ? WatchState.read()?.last : nil)
  }
}

private struct BackView: View {
  @Environment(\.widgetFamily) private var family
  let last: LastYo?

  var body: some View {
    Group {
      if family == .accessoryCircular {
        ZStack {
          AccessoryWidgetBackground()
          VStack(spacing: 0) {
            Image("yo.back").font(.system(size: last == nil ? 24 : 18)).widgetAccentable()
            if let last {
              Text(last.name).font(.yo(10)).padding(.horizontal, 2)
            }
          }
        }
        .containerBackground(.clear, for: .widget)
      } else {
        HStack(spacing: 6) {
          if let last {
            Image("yo.back").font(.system(size: 28)).widgetAccentable()
            Text(last.name).font(.yo(20))
          } else {
            Image("yo.wordmark").font(.system(size: 28)).widgetAccentable()
          }
        }
        .frame(maxWidth: .infinity, alignment: last == nil ? .center : .leading)
        .foregroundStyle(.white)
        .containerBackground(Color.amethyst, for: .widget)
      }
    }
    .lineLimit(1)
    .minimumScaleFactor(0.6)
    .widgetURL(URL(string: "yo://back"))
  }
}

private struct YoBack: Widget {
  var body: some WidgetConfiguration {
    StaticConfiguration(kind: "app.justyo.ios.watch.back", provider: Provider(showsLast: true)) { entry in
      BackView(last: entry.last)
    }
    .configurationDisplayName("Yo Back")
    .description("Send a Yo back to the last sender.")
    .supportedFamilies([.accessoryCircular, .accessoryRectangular])
  }
}

private struct Yo: Widget {
  var body: some WidgetConfiguration {
    StaticConfiguration(kind: "app.justyo.ios.watch.yo", provider: Provider(showsLast: false)) { _ in
      Image("yo.wordmark")
        .resizable()
        .scaledToFit()
        .widgetAccentable()
        .containerBackground(.clear, for: .widget)
        .widgetURL(URL(string: "yo://open"))
    }
    .configurationDisplayName("Yo")
    .description("Open your Yo list.")
    .supportedFamilies([.accessoryCorner, .accessoryCircular])
  }
}

@main struct YoWatchWidgets: WidgetBundle {
  var body: some Widget {
    YoBack()
    Yo()
  }
}
