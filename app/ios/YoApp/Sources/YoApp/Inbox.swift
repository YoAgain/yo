// Yos that arrive. For now the fake ones you send yourself: one without delay shows at once; one with a delay goes
// out as a local notification, so it comes as a push would (in front the app shows it, locked the system does).
// A Yo may carry a photo or a video (camera mode). In the app a Yo shows on its sender's bar, not in a banner.
// A plain Yo is the whole message: it shows for a moment and is gone, as in the original (no unread mark). Only
// photos and videos wait, until seen once.
import AVFoundation
import Observation
import UIKit
import UserNotifications

struct Arrival: Equatable, Identifiable {
  let id: String
  let from: String
  /// The photo, or the first frame of the video.
  let photo: UIImage?
  let video: URL?

  init(id: String, from: String, photo: UIImage? = nil, video: URL? = nil) {
    self.id = id
    self.from = from
    self.photo = photo
    self.video = video
  }
}

@MainActor @Observable
final class Inbox {
  /// Every Yo that arrived, in order: the list shows each one (several can come at once, e.g. on opening the app). Who
  /// and which only; the photos stay in `media` and go once seen.
  private(set) var arrivals: [Arrival] = []
  /// Notifications already taken in, so one tapped and one collected on opening count once.
  @ObservationIgnored private var taken: Set<String> = []
  /// Photos and videos not yet seen, by sender, oldest first.
  private(set) var media: [String: [Arrival]] = [:]

  /// Yos with a photo or video still on their way, until their notification arrives.
  @ObservationIgnored private var onTheirWay: [String: Arrival] = [:]

  /// The Yo sound: one player, started from the beginning by every Yo, as the original did; two Yos close together
  /// cut each other off ("Yo-Y-Yo").
  @ObservationIgnored private let sound: AVAudioPlayer? = {
    guard let url = Bundle.main.url(forResource: "yo", withExtension: "caf") else { return nil }
    try? AVAudioSession.sharedInstance().setCategory(.ambient)
    let player = try? AVAudioPlayer(contentsOf: url)
    player?.prepareToPlay()  // loaded at launch: the first Yo sounds at once
    return player
  }()

  /// The notification's category: "Yo back" right on it, as the original had, sent without opening the app.
  nonisolated static let category = "yo"
  nonisolated static let yoBack = "yo.back"

  func askForNotifications() async {
    let center = UNUserNotificationCenter.current()
    let yoBack = UNNotificationAction(identifier: Self.yoBack, title: "Yo back", options: [])
    center.setNotificationCategories([
      UNNotificationCategory(identifier: Self.category, actions: [yoBack], intentIdentifiers: [])
    ])
    _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
  }

  /// A fake Yo to yourself, as if `sender` had sent it, arriving after `delay`; with a photo (JPEG or any image
  /// data) or a video file if one is given.
  func fakeYo(from sender: String, after delay: Duration, photo data: Data? = nil, video: URL? = nil) async {
    let id = UUID().uuidString
    var photo = data.flatMap(UIImage.init(data:))?.preparingThumbnail(of: CGSize(width: 900, height: 1200))
    if let video, photo == nil { photo = await Self.firstFrame(of: video) }
    let arrival = Arrival(id: id, from: sender, photo: photo, video: video)
    guard delay > .zero else { return show(arrival) }

    onTheirWay[id] = arrival
    let content = UNMutableNotificationContent()
    // The original's texts (yo-api, payload.py): "Yo from LEO", "📷 Yo Photo from LEO", "Yo Video from LEO".
    // No picture in the notification: a photo or video is seen once, in the app.
    content.body =
      video != nil ? "🎥 Yo Video from \(sender)" : photo != nil ? "📷 Yo Photo from \(sender)" : "Yo from \(sender)"
    content.sound = UNNotificationSound(named: UNNotificationSoundName("yo.caf"))
    content.userInfo = ["id": id, "from": sender]
    content.categoryIdentifier = Self.category
    content.threadIdentifier = sender  // one stack per friend
    let seconds = Double(delay.components.seconds) + Double(delay.components.attoseconds) / 1e18
    let trigger = UNTimeIntervalNotificationTrigger(timeInterval: seconds, repeats: false)
    let center = UNUserNotificationCenter.current()
    let allowed = await [.authorized, .provisional, .ephemeral].contains(center.notificationSettings().authorizationStatus)
    do {
      guard allowed else { throw CancellationError() }
      try await center.add(UNNotificationRequest(identifier: id, content: content, trigger: trigger))
    } catch {
      // Notifications off (or refused): the Yo still comes, in the app, after the same delay.
      try? await Task.sleep(for: delay)
      received(id: id, from: sender)
    }
  }

  private static func firstFrame(of video: URL) async -> UIImage? {
    let generator = AVAssetImageGenerator(asset: AVURLAsset(url: video))
    generator.appliesPreferredTrackTransform = true
    guard let (image, _) = try? await generator.image(at: .zero) else { return nil }
    return UIImage(cgImage: image)
  }

  /// A Yo notification that arrived while the app is open, or was tapped.
  func received(id: String, from sender: String) {
    guard taken.insert(id).inserted else { return }
    show(onTheirWay.removeValue(forKey: id) ?? Arrival(id: id, from: sender))
  }

  /// On opening the app: the Yo notifications waiting in Notification Center come in, oldest first, and leave it.
  func collectDelivered() async {
    let center = UNUserNotificationCenter.current()
    let waiting = await center.deliveredNotifications()
      .filter { $0.request.content.categoryIdentifier == Self.category }
      .sorted { $0.date < $1.date }
    for notification in waiting {
      received(id: notification.request.identifier, from: notification.request.content.userInfo["from"] as? String ?? "?")
    }
    center.removeDeliveredNotifications(withIdentifiers: waiting.map(\.request.identifier))
  }

  /// Photos and videos seen (each once): they are gone.
  func seen(_ shown: [Arrival]) {
    for arrival in shown {
      if let video = arrival.video { try? FileManager.default.removeItem(at: video) }
      media[arrival.from]?.removeAll { $0.id == arrival.id }
      if media[arrival.from]?.isEmpty == true { media[arrival.from] = nil }
    }
  }

  /// The Yo sound, from the beginning (cutting off one still playing).
  func playYo() {
    sound?.currentTime = 0
    sound?.play()
  }

  /// The sound, and the sender's row takes the Yo (as Messages and Signal show a new message in the list: on the
  /// bar, not in a banner).
  private func show(_ arrival: Arrival) {
    playYo()  // one sound per Yo
    arrivals.append(Arrival(id: arrival.id, from: arrival.from))
    if arrival.photo != nil { media[arrival.from, default: []].append(arrival) }
  }
}
