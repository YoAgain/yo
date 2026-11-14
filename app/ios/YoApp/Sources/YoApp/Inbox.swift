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

  /// When it was sent: a photo's or video's id starts with its sender and the time (`Inbox.mediaID`).
  var sentAt: Date? {
    let parts = id.split(separator: "/")
    guard parts.count >= 2, let ms = UInt64(parts[1]) else { return nil }
    return Date(timeIntervalSince1970: Double(ms) / 1000)
  }

  /// How long ago, as stories say it: "now", "2m", "3h", "1d".
  static func age(_ date: Date, now: Date = .now) -> String {
    let minutes = Int(now.timeIntervalSince(date) / 60)
    return switch minutes {
    case ..<1: "now"
    case ..<60: "\(minutes)m"
    case ..<1440: "\(minutes / 60)h"
    default: "\(minutes / 1440)d"
    }
  }

  init(id: String, from: String, photo: UIImage? = nil, video: URL? = nil) {
    self.id = id
    self.from = from
    self.photo = photo
    self.video = video
  }
}

@MainActor @Observable
final class Inbox {
  /// Counts each time Yos are ready for the list (`takeArrivals`): at most every `batchSeconds`, so a burst (twenty
  /// Yos in a second) redraws the list ten times, not twenty, and each time with all that came meanwhile.
  private(set) var arrived = 0
  /// The Yos that came since the list last took them, in order. Who and which only; the photos stay in `media` and go
  /// once seen.
  @ObservationIgnored private var ready: [Arrival] = []
  /// A batch is being gathered: what comes now waits for its end.
  @ObservationIgnored private var gathering = false
  /// Counts Yos as they come, to tell whether more came during a batch.
  @ObservationIgnored private var came = 0
  static let batchSeconds = 0.1
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

  /// Signed out: what waited to be seen is gone (MediaStorage.clearAll deleted it), and Yos not shown yet.
  func forgetMedia() {
    media = [:]
    ready = []
    uncounted = [:]
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
  /// When the Yo sound last played for Yos that came: at most `soundsPerSecond` in any second, so a burst sounds like
  /// many Yos without twenty players fighting over the speaker.
  @ObservationIgnored private var sounded: [ContinuousClock.Instant] = []
  static let soundsPerSecond = 10

  /// The notification's category: "Yo back" right on it, as the original had, sent without opening the app.
  nonisolated static let category = ShareBridge.yoCategory
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
    let client = accounts.client  // the account tapped in: a Yo waiting here never goes from the next one
    let this = Task { () async -> String? in
      _ = await before?.value
      guard let client, client === accounts.client else { return "Not Sent" }
      return await sendNow(to: friend, client: client, accounts: accounts, friends: friends)
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
    if let contacts = try? await Hub.run({ client.contacts() }), client === accounts.client {
      friends.spell(contacts)
      friends.markGone(contacts)
    }
    let typed = friends.all.filter { $0.user == nil && !$0.blocked }.prefix(20).map(\.name)
    await identify(Array(typed), accounts: accounts, friends: friends)
  }

  private func sendNow(to friend: Friend, client: YoClient, accounts: AccountStore, friends: Friends) async -> String? {
    let target = friend.user ?? friend.name
    do {
      let (events, contacts, waiting) = try await Hub.run {
        (try client.sendYo(to: target), client.contacts(), client.waiting())
      }
      guard client === accounts.client else { return nil }  // signed out meanwhile: nothing of it stays
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
      // Gone (deleted, or the name released): the bar says so from now on.
      if let contacts = try? await Hub.run({ client.contacts() }), client === accounts.client {
        friends.markGone(contacts)
      }
      return "Not on Yo"
    } catch YoError.Blocked {
      return "Blocked"
    } catch YoError.Replaced {
      accounts.replaced(client)
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
        guard client === accounts.client else {  // signed out meanwhile: on with the account in now, if any
          pause = 1
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
    var storageTaken = false
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
        guard client === accounts.client else { continue }  // signed out meanwhile: not for this account
        await take(events, contacts: contacts, client: client, friends: friends)
        if !storageTaken {  // read by the core before its first receive, if registering could not
          storageTaken = true
          await accounts.takeStorage(into: friends, read: true)
        }
        fetchMedia(client: client)  // what a broken network left, also after a start
      } catch YoError.Replaced {
        accounts.replaced(client)
        return
      } catch {
        try? await Task.sleep(for: .seconds(5))
      }
    }
  }

  /// What the core handed over while sending a photo or video (the outbox): as any.
  func handle(_ events: [YoEvent], contacts: [YoContact], client: YoClient, friends: Friends) async {
    await take(events, contacts: contacts, client: client, friends: friends)
  }

  /// A photo or video shown in the viewer: it will not show again.
  func viewed(_ arrival: Arrival) {
    ReceivedMedia.shared.consumed(arrival.id)
  }

  /// What the core handed over: a Yo shows on its sender's bar, a new chat brings its friend into the list; a photo
  /// or video is noted, fetched and waits on the bar until seen. Then the core may forget them (they will not come
  /// again after a crash). `contacts`: the core's, read with the events.
  private func take(_ events: [YoEvent], contacts: [YoContact], client: YoClient, friends: Friends) async {
    friends.markGone(contacts)  // a delivery found their account gone (`NotOnYo` from now on)
    guard !events.isEmpty else { return }
    // Per sender: how many Yos, the newest's kind, name and id; for a notification when the app is not in front.
    var unshown: [String: (count: Int, kind: MediaFile.Kind?, name: String, id: String)] = [:]
    func friend(_ user: String) -> Friend {
      friends.friend(user: user, handles: contacts.first { $0.user == user }?.handles ?? [])
    }
    for event in events {
      let media: (YoMedia, MediaFile.Kind)?
      switch event {
      case .message(let user, let sentAtMs, let kind):
        switch kind {
        case .yo: media = nil
        case .photo(let photo): media = (photo, .photo)
        case .video(let video, _): media = (video, .video)
        default: continue  // text, links, places: not shown in Yo
        }
        let sender = friend(user)
        received(id: "\(user)/\(sentAtMs)", from: sender.name)
        unshown[user] = ((unshown[user]?.count ?? 0) + 1, media?.1, sender.name, "\(user)/\(sentAtMs)")
        // On top it goes by the list, in one step with the others and never under a finger (FriendList.rise).
        count(from: sender.name)
        if let (media, kind) = media {
          ReceivedMedia.shared.expect(media, id: Self.mediaID(user, sentAtMs, media), from: sender.name, kind: kind)
        }
      case .newChat(let user):
        _ = friend(user)  // a friend's chat restarted (spec 4.9) is the same friend, by user id
      default:
        break
      }
    }
    if UIApplication.shared.applicationState == .background { await notify(unshown) }
    // Photos and videos are noted (above) before the core may forget their Yos; fetched from here on.
    _ = try? await Hub.run { client.acknowledgeEvents() }
    fetchMedia(client: client)
  }

  /// Yos the app took itself while not in front (just left, its wait at the hub still running): the notification the
  /// notification service would have written, which finds nothing new then. Not for a Yo it already wrote.
  private func notify(_ unshown: [String: (count: Int, kind: MediaFile.Kind?, name: String, id: String)]) async {
    let center = UNUserNotificationCenter.current()
    let written = Set(await center.deliveredNotifications().compactMap { $0.request.content.userInfo["id"] as? String })
    for (user, yo) in unshown where !written.contains(yo.id) {
      let content = UNMutableNotificationContent()
      ShareBridge.yo(content, [ShareBridge.Yos(name: yo.name, user: user, count: yo.count, kind: yo.kind, id: yo.id)])
      try? await center.add(UNNotificationRequest(identifier: yo.id, content: content, trigger: nil))
    }
  }

  /// A photo's or video's own id: who, when, and which blob (two at the same moment are two).
  private static func mediaID(_ user: String, _ sentAtMs: UInt64, _ media: YoMedia) -> String {
    "\(user)/\(sentAtMs)/" + media.id.map { String(format: "%02x", $0) }.joined()
  }

  /// Fetches the photos and videos noted and not kept yet (also those a broken network left).
  func fetchMedia(client: YoClient) {
    ReceivedMedia.shared.fetchWaiting(client: client) { [weak self] arrival in self?.add(arrival) }
  }

  /// A Yo notification that arrived while the app is open, or was tapped.
  func received(id: String, from sender: String) {
    guard taken.insert(id).inserted else { return }
    show(Arrival(id: id, from: sender))
  }

  /// On opening the app: the Yo notifications waiting in Notification Center come in, oldest first, and leave it, with
  /// the pushes that named nobody ("Yo": the core brings their Yos, if any).
  func collectDelivered() async {
    let center = UNUserNotificationCenter.current()
    let delivered = await center.deliveredNotifications()
    let waiting = delivered.filter { $0.request.content.categoryIdentifier == Self.category }.sorted { $0.date < $1.date }
    for notification in waiting {
      let info = notification.request.content.userInfo
      guard let sender = info["from"] as? String else { continue }
      // The Yo's own id when the notification service knew it, as when one is tapped: the core's Yo is not shown again.
      received(id: info["id"] as? String ?? notification.request.identifier, from: sender)
    }
    center.removeDeliveredNotifications(
      withIdentifiers: delivered.filter { $0.request.identifier != SecureReminder.id }.map(\.request.identifier))
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

  /// Yos received, for the statistics: written a second after the first of them, once per sender (not twice per Yo).
  private func count(from sender: String) {
    uncounted[sender, default: 0] += 1
    guard uncounted.count == 1 && uncounted[sender] == 1 else { return }
    Task {
      try? await Task.sleep(for: .seconds(1))
      for (name, n) in uncounted { Stats.receivedYos(from: name, n) }
      uncounted = [:]
    }
  }

  @ObservationIgnored private var uncounted: [String: Int] = [:]

  /// The Yos that came since the last time, for the list to show (each once).
  func takeArrivals() -> [Arrival] {
    defer { ready = [] }
    return ready
  }

  /// The sound, and the sender's row takes the Yo (as Messages and Signal show a new message in the list: on the
  /// bar, not in a banner). The first Yo after a quiet moment goes to the list at once; what comes within
  /// `batchSeconds` after it goes together, at the end of them.
  private func show(_ arrival: Arrival) {
    chime()
    ready.append(Arrival(id: arrival.id, from: arrival.from))
    if arrival.photo != nil { media[arrival.from, default: []].append(arrival) }
    came += 1
    guard !gathering else { return }
    gathering = true
    arrived += 1
    Task {
      var told = came
      while true {
        try? await Task.sleep(for: .seconds(Self.batchSeconds))
        guard came != told else { break }  // a quiet moment: the next Yo goes at once again
        told = came
        arrived += 1
      }
      gathering = false
    }
  }

  /// One sound per Yo, but no more than `soundsPerSecond` in a second (a burst of twenty sounds ten times).
  private func chime() {
    let now = ContinuousClock.now
    sounded.removeAll { now - $0 > .seconds(1) }
    guard sounded.count < Self.soundsPerSecond else { return }
    sounded.append(now)
    playYo()
  }
}
