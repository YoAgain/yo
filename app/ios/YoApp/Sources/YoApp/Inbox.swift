// Yos, through the hub (Rust core): sent from the list, and received while the list is on screen (the inbox's long
// wait); a push brings them otherwise (to come). In the app a Yo shows on its sender's bar, not in a banner.
// A plain Yo is the whole message: it shows for a moment and is gone, as in the original (no unread mark). Only
// photos and videos wait, until seen once.
import AVFoundation
import Observation
import UIKit
import UserNotifications
import YoCore

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


  /// The Yo sound, a few players taking turns: every Yo sounds in full, Yos close together over each other, as the
  /// original's did (none cuts off the one before).
  @ObservationIgnored private let sounds: [AVAudioPlayer] = {
    guard let url = Bundle.main.url(forResource: "yo", withExtension: "caf") else { return [] }
    try? AVAudioSession.sharedInstance().setCategory(.ambient)
    return (0..<8).compactMap { _ in
      let player = try? AVAudioPlayer(contentsOf: url)
      player?.prepareToPlay()  // loaded at launch: the first Yo sounds at once
      return player
    }
  }()
  @ObservationIgnored private var nextSound = 0

  /// The notification's category: "Yo back" right on it, as the original had, sent without opening the app.
  nonisolated static let category = "yo"
  nonisolated static let yoBack = "yo.back"

  func askForNotifications() async {
    let center = UNUserNotificationCenter.current()
    let yoBack = UNNotificationAction(identifier: Self.yoBack, title: "Yo Back", options: [])
    center.setNotificationCategories([
      UNNotificationCategory(identifier: Self.category, actions: [yoBack], intentIdentifiers: [])
    ])
    _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
  }

  // MARK: The hub

  /// A Yo to a friend through the hub (spec 4.7): by user id once known, else by name (the first Yo makes the chat).
  /// Nil when it went; else what the bar says instead.
  func send(to friend: Friend, accounts: AccountStore, friends: Friends) async -> String? {
    guard let client = accounts.client else { return "Not Sent" }
    let target = friend.user ?? friend.name
    do {
      let events = try await Hub.run { try client.sendYo(to: target) }
      if friend.user == nil, let contact = client.contacts().first(where: { contact in
        contact.handles.contains { Username.skeleton($0) == Username.skeleton(friend.name) }
      }) {
        _ = friends.friend(user: contact.user, handles: contact.handles)
      }
      take(events, client: client, friends: friends)
      return nil
    } catch YoError.NotOnYo {
      return "Not on Yo"
    } catch YoError.Blocked {
      return "Blocked"
    } catch YoError.Replaced {
      accounts.reset()
      return "Not Sent"
    } catch {
      return "Not Sent"
    }
  }

  /// While the list is on screen: waits at the hub for Yos (spec 6.3, the inbox's long wait) and shows each on its
  /// sender's bar. A pause after an error; this phone replaced elsewhere: signed out.
  func listen(accounts: AccountStore, friends: Friends) async {
    while !Task.isCancelled {
      guard let client = accounts.client else {
        try? await Task.sleep(for: .seconds(1))
        continue
      }
      do {
        let events = try await Hub.run { try client.sync(waitS: 25) }
        take(events, client: client, friends: friends)
      } catch YoError.Replaced {
        accounts.reset()
        return
      } catch {
        try? await Task.sleep(for: .seconds(5))
      }
    }
  }

  /// What the core handed over: a Yo shows on its sender's bar, a new chat brings its friend into the list. Then the
  /// core may forget them (they will not come again after a crash).
  private func take(_ events: [YoEvent], client: YoClient, friends: Friends) {
    let contacts = client.contacts()
    func friend(_ user: String) -> Friend {
      friends.friend(user: user, handles: contacts.first { $0.user == user }?.handles ?? [])
    }
    for event in events {
      switch event {
      case .message(let user, let sentAtMs, .yo):
        let sender = friend(user)
        received(id: "\(user)/\(sentAtMs)", from: sender.name)
        friends.promote(sender.name)
      case .newChat(let user):
        _ = friend(user)
      default:
        break
      }
    }
    if !events.isEmpty { client.acknowledgeEvents() }
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
    show(Arrival(id: id, from: sender))
  }

  /// On opening the app: the Yo notifications waiting in Notification Center come in, oldest first, and leave it.
  func collectDelivered() async {
    let center = UNUserNotificationCenter.current()
    let waiting = await center.deliveredNotifications()
      .filter { $0.request.content.categoryIdentifier == Self.category }
      .sorted { $0.date < $1.date }
    for notification in waiting {
      received(id: notification.request.identifier, from: notification.request.content.userInfo["from"] as? String ?? "Unknown sender")
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

  /// The Yo sound, once, on a player not busy if there is one (over the ones still playing).
  func playYo() {
    guard !sounds.isEmpty else { return }
    let player = sounds.first { !$0.isPlaying } ?? sounds[nextSound % sounds.count]
    nextSound += 1
    player.currentTime = 0
    player.play()
  }

  /// The sound, and the sender's row takes the Yo (as Messages and Signal show a new message in the list: on the
  /// bar, not in a banner).
  private func show(_ arrival: Arrival) {
    playYo()  // one sound per Yo
    arrivals.append(Arrival(id: arrival.id, from: arrival.from))
    if arrival.photo != nil { media[arrival.from, default: []].append(arrival) }
  }
}
