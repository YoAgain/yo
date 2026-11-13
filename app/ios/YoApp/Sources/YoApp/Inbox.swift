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
  /// Photos and videos not yet seen, by sender, oldest first: kept on this iPhone until then (ReceivedMedia).
  private(set) var media: [String: [Arrival]] = [:]

  init() {
    Task { @MainActor in
      for arrival in await ReceivedMedia.shared.waiting() { add(arrival) }
    }
  }

  /// A photo or video kept, on its sender's bar until seen.
  private func add(_ arrival: Arrival) {
    guard !(media[arrival.from] ?? []).contains(where: { $0.id == arrival.id }) else { return }
    media[arrival.from, default: []].append(arrival)
  }

  /// Signed out: what waited to be seen is gone (MediaStorage.clearAll deleted it).
  func forgetMedia() {
    media = [:]
  }


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
  /// Nil when it went; else what the bar says instead. A friend's Yos go one after the other, in the order tapped (a
  /// burst waits here, not in threads blocked on the core); Yos to other friends go meanwhile.
  func send(to friend: Friend, accounts: AccountStore, friends: Friends) async -> String? {
    let key = Username.skeleton(friend.name)
    let before = lastSend[key]
    let this = Task {
      _ = await before?.value
      return await sendNow(to: friend, accounts: accounts, friends: friends)
    }
    lastSend[key] = this
    let problem = await this.value
    if lastSend[key] == this { lastSend[key] = nil }
    return problem
  }

  /// Each friend's newest send, which the next one to them waits for.
  @ObservationIgnored private var lastSend: [String: Task<String?, Never>] = [:]

  /// What the hub says a typed name is.
  enum Lookup {
    /// The user id, and the name as its owner writes it ("YoGPT").
    case found(user: String, name: String?)
    case noSuchUser
    /// No answer (offline, the hub's trouble).
    case unknown
  }

  /// Who a typed name is, asked of the hub (no chat, no KeyPackage).
  func lookUp(_ name: String, accounts: AccountStore) async -> Lookup {
    guard let client = accounts.client else { return .unknown }
    do {
      guard let found = try await Hub.run({ try client.whois(name: name) }) else { return .noSuchUser }
      return .found(user: found.user, name: found.username)
    } catch {
      return .unknown
    }
  }

  /// Friends added by a typed name while the hub could not be asked: asked now, so each shows as its owner writes it
  /// ("yogpt" becomes "YoGPT"). One the hub does not know stays as typed (a Yo to it says "Not on Yo").
  func identify(_ names: [String], accounts: AccountStore, friends: Friends) async {
    for name in names {
      if case .found(let user, let spelled) = await lookUp(name, accounts: accounts) {
        friends.found(name, user: user, spelled: spelled)
      }
    }
  }

  /// When the list opens: friends take the spellings the core knows, and up to 20 still known only by a typed name
  /// are looked up (once per opening, so the hub is not asked again and again).
  private func respell(client: YoClient, accounts: AccountStore, friends: Friends) async {
    if let contacts = try? await Hub.run({ client.contacts() }) { friends.spell(contacts) }
    let typed = friends.all.filter { $0.user == nil && !$0.blocked }.prefix(20).map(\.name)
    await identify(Array(typed), accounts: accounts, friends: friends)
  }

  private func sendNow(to friend: Friend, accounts: AccountStore, friends: Friends) async -> String? {
    guard let client = accounts.client else { return "Not Sent" }
    let target = friend.user ?? friend.name
    do {
      let (events, contacts, waiting) = try await Hub.run {
        (try client.sendYo(to: target), client.contacts(), client.waiting())
      }
      if waiting > 0 { deliverLater(accounts: accounts, friends: friends) }
      if friend.user == nil, let contact = contacts.first(where: { contact in
        contact.handles.contains { Username.skeleton($0) == Username.skeleton(friend.name) }
      }) {
        _ = friends.friend(user: contact.user, handles: contact.handles)
      }
      await take(events, contacts: contacts, client: client, friends: friends)
      Stats.sentYo(to: friend.name)
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

  /// Yos the hub did not take yet (it takes so many a minute from one sender, spec 6.6; or no network): sent again
  /// from the core's outbox, in their order, after a second, then less and less often (up to half a minute), until
  /// none waits. Receiving goes on meanwhile; nothing is lost.
  private func deliverLater(accounts: AccountStore, friends: Friends) {
    deliverAgain = true
    guard delivering == nil else { return }
    delivering = Task {
      var pause = 1.0
      var before = UInt32.max
      while let client = accounts.client, !Task.isCancelled {
        deliverAgain = false
        try? await Task.sleep(for: .seconds(pause))
        guard let (events, contacts, waiting) = try? await Hub.run({
          (try client.deliver(), client.contacts(), client.waiting())
        }) else {
          pause = min(pause * 2, 30)
          continue
        }
        await take(events, contacts: contacts, client: client, friends: friends)
        if waiting == 0 && !deliverAgain { break }  // none waits, and no send asked meanwhile
        pause = waiting < before ? 1 : min(pause * 2, 30)  // going out: at the hub's pace; else waiting longer
        before = waiting
      }
      delivering = nil
    }
  }

  /// The outbox being sent again (`deliverLater`), and whether a send asked for it since the last look.
  @ObservationIgnored private var delivering: Task<Void, Never>?
  @ObservationIgnored private var deliverAgain = false

  /// While the list is on screen: waits at the hub for Yos (spec 6.3, the inbox's long wait) and shows each on its
  /// sender's bar. A pause after an error; this phone replaced elsewhere: signed out.
  func listen(accounts: AccountStore, friends: Friends) async {
    var respelled = false
    while !Task.isCancelled {
      guard let client = accounts.client else {
        try? await Task.sleep(for: .seconds(1))
        continue
      }
      if !respelled {
        respelled = true
        Task { await respell(client: client, accounts: accounts, friends: friends) }
      }
      do {
        let (events, contacts) = try await Hub.run { (try client.sync(waitS: 25), client.contacts()) }
        await take(events, contacts: contacts, client: client, friends: friends)
      } catch YoError.Replaced {
        accounts.reset()
        return
      } catch {
        try? await Task.sleep(for: .seconds(5))
      }
    }
  }

  /// What the core handed over while sending a photo or video (the outbox): as any.
  func handle(_ events: [YoEvent], client: YoClient, friends: Friends) async {
    guard !events.isEmpty, let contacts = try? await Hub.run({ client.contacts() }) else { return }
    await take(events, contacts: contacts, client: client, friends: friends)
  }

  /// What the core handed over: a Yo shows on its sender's bar, a new chat brings its friend into the list; a photo
  /// or video is noted, fetched and waits on the bar until seen. Then the core may forget them (they will not come
  /// again after a crash). `contacts`: the core's, read with the events.
  private func take(_ events: [YoEvent], contacts: [YoContact], client: YoClient, friends: Friends) async {
    guard !events.isEmpty else { return }
    func friend(_ user: String) -> Friend {
      friends.friend(user: user, handles: contacts.first { $0.user == user }?.handles ?? [])
    }
    for event in events {
      switch event {
      case .message(let user, let sentAtMs, .yo):
        let sender = friend(user)
        received(id: "\(user)/\(sentAtMs)", from: sender.name)
        Stats.receivedYo(from: sender.name)
        friends.promote(sender.name)
      case .message(let user, let sentAtMs, .photo(let media)):
        let sender = friend(user)
        received(id: "\(user)/\(sentAtMs)", from: sender.name)
        Stats.receivedYo(from: sender.name)
        friends.promote(sender.name)
        ReceivedMedia.shared.expect(media, id: "\(user)/\(sentAtMs)", from: sender.name, kind: .photo)
      case .message(let user, let sentAtMs, .video(let media, _)):
        let sender = friend(user)
        received(id: "\(user)/\(sentAtMs)", from: sender.name)
        Stats.receivedYo(from: sender.name)
        friends.promote(sender.name)
        ReceivedMedia.shared.expect(media, id: "\(user)/\(sentAtMs)", from: sender.name, kind: .video)
      case .newChat(let user):
        _ = friend(user)
      default:
        break
      }
    }
    // Photos and videos are noted (above) before the core may forget their Yos; fetched from here on.
    _ = try? await Hub.run { client.acknowledgeEvents() }
    fetchMedia(client: client)
  }

  /// Fetches the photos and videos noted and not kept yet (also those a broken network left).
  func fetchMedia(client: YoClient) {
    ReceivedMedia.shared.fetchWaiting(client: client) { [weak self] arrival in self?.add(arrival) }
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

  /// Photos and videos seen (each once): they are gone, from this iPhone too.
  func seen(_ shown: [Arrival]) {
    for arrival in shown {
      ReceivedMedia.shared.forget(arrival.id)
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
