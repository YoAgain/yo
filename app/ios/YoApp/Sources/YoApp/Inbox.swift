// Yos, through the hub (Rust core): sent from the list, and received while the list is on screen (the inbox's long
// wait); a push brings them otherwise (to come). In the app a Yo shows on its sender's bar, not in a banner.
// A plain Yo is the whole message: it shows for a moment and is gone, as in the original (no unread mark). Only
// photos and videos wait, until seen once.
import AVFoundation
import Observation
import SwiftUI
import UIKit
import UserNotifications
import YoCore

struct Arrival: Equatable, Identifiable {
  let id: String
  let from: String
  /// A small picture of the photo, or of the video's first frame (the viewer reads the photo itself from `file`).
  let photo: UIImage?
  let video: URL?
  /// The photo's file.
  var file: URL? = nil
  /// Not downloaded yet: what it will be. On its bar from the moment its message comes; the viewer waits for it.
  var pending: MediaFile.Kind? = nil

  /// A video, downloaded or on its way.
  var isVideo: Bool { video != nil || pending == .video }

  /// When it was sent: a photo's or video's id starts with its sender and the time (`Inbox.mediaID`).
  var sentAt: Date? { Self.sentAtMs(id).map { Date(timeIntervalSince1970: Double($0) / 1000) } }

  /// The time in an id ("user/ms/…").
  static func sentAtMs(_ id: String) -> UInt64? {
    let parts = id.split(separator: "/")
    return parts.count >= 2 ? UInt64(parts[1]) : nil
  }

  /// Oldest sent first, then by id: the order on a bar, whichever download finished first.
  static func older(_ a: Arrival, _ b: Arrival) -> Bool {
    (sentAtMs(a.id) ?? 0, a.id) < (sentAtMs(b.id) ?? 0, b.id)
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

  init(
    id: String, from: String, photo: UIImage? = nil, video: URL? = nil, file: URL? = nil, pending: MediaFile.Kind? = nil
  ) {
    self.id = id
    self.from = from
    self.photo = photo
    self.video = video
    self.file = file
    self.pending = pending
  }
}

@MainActor @Observable
final class Inbox {
  /// When the hub last answered a sync: Settings › Encryption's "Hub Connected".
  var lastSync: Date?
  /// Counts each time Yos are ready for the list (`takeArrivals`): at most every `batchSeconds`, so a burst (twenty
  /// Yos in a second) redraws the list five times, not twenty, and each time with all that came meanwhile.
  private(set) var arrived = 0
  /// Sustained floods alone suppress list motion: more than twelve arrivals in one second.
  private(set) var busy = false
  @ObservationIgnored private var recentArrivals: [ContinuousClock.Instant] = []
  @ObservationIgnored private var lastArrival = ContinuousClock.now

  private func noteArrival() {
    let now = ContinuousClock.now
    lastArrival = now
    recentArrivals.removeAll { now - $0 >= .seconds(1) }
    // Only the latest thirteen are needed to detect the threshold.
    if recentArrivals.count == 13 { recentArrivals.removeFirst() }
    recentArrivals.append(now)
    guard !busy, recentArrivals.count > 12 else { return }
    busy = true
    Perf.mark("flood.start")
    Task { [weak self] in
      while let deadline = self?.lastArrival.advanced(by: .milliseconds(1500)) {
        do { try await ContinuousClock().sleep(until: deadline) } catch { return }
        guard let self else { return }
        guard ContinuousClock.now - self.lastArrival >= .milliseconds(1500) else { continue }
        self.busy = false
        self.recentArrivals.removeAll(keepingCapacity: true)
        Perf.mark("flood.end")
        return
      }
    }
  }
  /// The Yos that came since the list last took them, in order. Who and which only; the photos stay in `media` and go
  /// once seen.
  @ObservationIgnored private var ready: [Arrival] = []
  /// A batch is being gathered: what comes now waits for its end.
  @ObservationIgnored private var gathering = false
  /// Counts Yos as they come, to tell whether more came during a batch.
  @ObservationIgnored private var came = 0
  // Allow the 160 ms count transition to finish between batches.
  static let batchSeconds = 0.2
  /// Group members' names by user id (hex), as their Yos came: who a group's photo is from in the viewer.
  var memberNames: [String: String] = [:]

  /// Who a group's photo or video is from (their user id, hex): its id is the group's id (64 hex, spec 4.9), the
  /// sender's (32 hex), then the rest (`mediaID`).
  static func sender(of arrival: Arrival) -> String? {
    let head = arrival.id.split(separator: "/").first.map(String.init) ?? ""
    return head.count == 96 ? String(head.suffix(32)) : nil
  }
  /// Notifications already taken in, so one tapped and one collected on opening count once: the newest 2000, oldest
  /// first out (a notification and its Yo come within moments of each other).
  @ObservationIgnored private var taken: Set<String> = []
  @ObservationIgnored private var takenOrder: [String] = []
  private static let takenKept = 2000
  /// Photos and videos not yet seen, by sender, oldest first: kept on this iPhone until then (ReceivedMedia). By the
  /// sender name's skeleton: a friend whose spelling changed since ("LEO", then "Leo") still finds theirs. A group's by
  /// its id (an arrival `from` "group:" and the id), whatever it is called now.
  private var unseenMedia: [String: [Arrival]] = [:] {
    didSet { mediaChanges += 1 }
  }
  /// Counts changes of the photos and videos waiting (one came, its download finished, one was seen): the list's
  /// signs follow it.
  private(set) var mediaChanges = 0

  /// A photo or video waiting, wherever it is kept: the viewer takes the downloaded one in place of its placeholder.
  func unseen(id: String) -> Arrival? {
    for kept in unseenMedia.values { if let found = kept.first(where: { $0.id == id }) { return found } }
    return nil
  }

  /// A bar's photos and videos not yet seen, oldest first; nil when none wait.
  func media(for friend: Friend) -> [Arrival]? { unseenMedia[friend.mediaKey] }

  /// Which bar a photo or video is on: a group's by its id, a friend's by the sender's user id, which starts the media's
  /// own id ("user/ms/blob", `mediaID`). Never by a name: two people may have had the same one (Friend.mediaKey).
  private static func mediaKey(_ arrival: Arrival) -> String {
    arrival.from.hasPrefix("group:") ? arrival.from : "user:" + (arrival.id.split(separator: "/").first.map(String.init) ?? "")
  }

  init() {
    Task { @MainActor in
      for arrival in await ReceivedMedia.shared.waiting() { add(arrival) }
    }
  }

  /// A photo or video kept, on its sender's bar until seen, in the order sent: noted as its message comes (`pending`,
  /// the download still to come), then the downloaded one in its place.
  private func add(_ arrival: Arrival) {
    Perf.timed("inbox.media.handoff") {
      let key = Self.mediaKey(arrival)
      var waiting = unseenMedia[key] ?? []
      if let at = waiting.firstIndex(where: { $0.id == arrival.id }) {
        // Downloaded now: in its placeholder's place. Never the other way round (a late placeholder after a restart).
        guard waiting[at].pending != nil, arrival.pending == nil else { return }
        waiting[at] = arrival
      } else {
        waiting.insert(arrival, at: waiting.firstIndex { Arrival.older(arrival, $0) } ?? waiting.endIndex)
      }
      unseenMedia[key] = waiting
    }
  }

  /// A photo or video that will not come (the hub no longer has it, or it does not open): off its bar.
  private func drop(_ id: String) {
    unseenMedia = unseenMedia.compactMapValues { kept in
      let rest = kept.filter { $0.id != id }
      return rest.isEmpty ? nil : rest
    }
  }

  /// Signed out: what waited to be seen is gone (MediaStorage.clearAll deleted it), and Yos not shown yet.
  func forgetMedia() {
    unsent = [:]  // the account's Yos kept to send again: not the next account's
    resending?.cancel()
    resending = nil
    unseenMedia = [:]
    ready = []
    uncounted = [:]
    taken = []
    takenOrder = []
  }


  /// The Yo sound, a few players taking turns: every Yo sounds in full, Yos close together over each other, as the
  /// original's did (none cuts off the one before). Made at launch off the main thread (eight players and the audio
  /// session took 60 ms of the launch there), ready long before the first Yo.
  @ObservationIgnored private let sounds = InboxSounds()

  /// When the Yo sound last played for Yos that came: at most `soundsPerSecond` in any second, so a burst sounds like
  /// many Yos without twenty players fighting over the speaker.
  @ObservationIgnored private var sounded: [ContinuousClock.Instant] = []
  static let soundsPerSecond = 10

  /// The notification's category: "Yo back" right on it, as the original had, sent without opening the app.
  nonisolated static let category = ShareBridge.yoCategory
  nonisolated static let yoBack = ShareBridge.yoBackAction

  func askForNotifications() async {
    let center = UNUserNotificationCenter.current()
    ShareBridge.registerCategories()
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
    if problem != nil {
      let retry: (@MainActor () -> Void)?
      if unsent[friend.key] != nil || delivering != nil {
        retry = { self.resume(accounts: accounts, friends: friends) }
      } else { retry = nil }
      SendFailure.shared.show(name: friend.name, retry: retry)
    }
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

  /// Who a typed name is, asked of the hub (no session, no prekey).
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
    if let groups = try? await Hub.run({ try client.groups() }), client === accounts.client { friends.takeGroups(groups) }
    let typed = friends.all.filter { $0.user == nil && $0.group == nil && !$0.blocked }.prefix(20).map(\.name)
    await identify(Array(typed), accounts: accounts, friends: friends)
  }

  /// What a send to a group of this account alone says (`YoError.GroupEmpty`).
  static let groupEmpty = "Group Is Empty"

  private func sendNow(to friend: Friend, client: YoClient, accounts: AccountStore, friends: Friends) async -> String? {
    if let group = friend.group {
      do {
        let (events, contacts) = try await Hub.run { (try client.sendGroupYo(group: group), client.contacts()) }
        guard client === accounts.client else { return nil }
        await take(events, contacts: contacts, client: client, friends: friends)
        Stats.sentYo(to: friend.name)
        return nil
      } catch YoError.Replaced {
        accounts.replaced(client)
        return "Not Sent"
      } catch YoError.GroupEmpty {
        return Self.groupEmpty  // nobody left in it: the list offers to delete it
      } catch {
        return "Not Sent"
      }
    }
    let target = friend.user ?? friend.name
    do {
      let (events, contacts, waiting) = try await Hub.run {
        try Perf.timed("send.core") { (try client.sendYo(to: target), client.contacts(), client.waiting()) }
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
      // Gone (deleted): the bar says so from now on.
      if let contacts = try? await Hub.run({ client.contacts() }), client === accounts.client {
        friends.markGone(contacts)
      }
      return "Not on Yo"
    } catch YoError.Blocked {
      return "Blocked"
    } catch YoError.Outdated {
      // Their app is too old: the Yo waits in the outbox and goes once they update.
      deliverLater(accounts: accounts, friends: friends)
      return "Needs Update"
    } catch YoError.Replaced {
      accounts.replaced(client)
      return "Not Sent"
    } catch let error as YoError where retryLater(error: error) {
      // Not now (no network, the hub's limits; a first Yo the core cannot keep yet, its friend not looked up): kept
      // here and tried again by itself, a little later each time, until it goes or is refused for good.
      keepAndRetry(friend, accounts: accounts, friends: friends)
      return "Not Sent"
    } catch {
      // Refused for good: why, for the diagnostics (a first Yo that said "Not Sent" in the Simulator once).
      Perf.mark("send.refused \(error)")
      return "Not Sent"
    }
  }

  /// Yos that met "not now" before the core could keep them, by friend: sent again from here (`keepAndRetry`).
  @ObservationIgnored private var unsent: [UUID: (friend: Friend, count: Int)] = [:]
  @ObservationIgnored private var resending: Task<Void, Never>?

  /// Keeps one more Yo for `friend` and sends what is kept again after 2 s, then less and less often (up to 30 s).
  private func keepAndRetry(_ friend: Friend, accounts: AccountStore, friends: Friends) {
    unsent[friend.key, default: (friend, 0)].count += 1
    guard resending == nil else { return }
    resending = Task {
      var pause = 2.0
      while !unsent.isEmpty, !Task.isCancelled {
        try? await Task.sleep(for: .seconds(pause))
        pause = min(pause * 2, 30)
        guard let client = accounts.client else { break }
        // Taken out, each sent again: one that meets "not now" again comes back by itself (`sendNow` keeps it).
        let round = unsent
        unsent = [:]
        for kept in round.values {
          for _ in 0..<kept.count {
            _ = await sendNow(to: kept.friend, client: client, accounts: accounts, friends: friends)
          }
        }
      }
      resending = nil
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
      while let client = accounts.client, !Task.isCancelled, !HubTransport.unsupported {  // too old: nothing goes
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

  /// What the core held for a friend goes now: their new key confirmed (spec 3.4), or unblocked.
  func resume(accounts: AccountStore, friends: Friends) {
    let round = unsent
    unsent = [:]
    Task {
      for kept in round.values {
        for _ in 0..<kept.count { _ = await send(to: kept.friend, accounts: accounts, friends: friends) }
      }
    }
    deliverLater(accounts: accounts, friends: friends)
  }

  /// The outbox being sent again (`deliverLater`), and whether a send asked for it since the last look.
  @ObservationIgnored private var delivering: Task<Void, Never>?
  @ObservationIgnored private var deliverAgain = false

  /// While the list is on screen: waits at the hub for Yos (spec 6.3, the inbox's long wait) and shows each on its
  /// sender's bar. A pause after an error; this phone replaced elsewhere: signed out; this build too old (426): no
  /// more waits.
  func listen(accounts: AccountStore, friends: Friends) async {
    var respelled = false
    var storageTaken = false
    while !Task.isCancelled && !HubTransport.unsupported {
      guard let client = accounts.client else {
        try? await Task.sleep(for: .seconds(1))
        continue
      }
      // In the background iOS gives no turn on the shared state once its time is up (SharedAccess): the wait goes on
      // in front again, rather than asking for a turn every few seconds in vain. Pushes bring Yos meanwhile.
      if UIApplication.shared.applicationState == .background {
        try? await Task.sleep(for: .seconds(1))
        continue
      }
      if !respelled {
        respelled = true
        Task { await respell(client: client, accounts: accounts, friends: friends) }
      }
      do {
        // What the notification service saved as its push came: on the bars at once, before the hub is asked.
        let (saved, known) = try await Hub.run { try Perf.timed("inbox.listen.saved.core") { (try client.savedEvents(), client.contacts()) } }
        if !saved.isEmpty, client === accounts.client {
          await take(saved, contacts: known, client: client, friends: friends)
          fetchMedia(client: client)
        }
        let (events, contacts) = try await Hub.run(reportErrors: false) { try Perf.timed("inbox.listen.sync.core") { (try client.sync(waitS: 25), client.contacts()) } }
        lastSync = .now
        Perf.mark("listen.events \(events.count)")
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
        // Signed out (or in again) meanwhile: the old client is closed and its wait fails with `State`, as expected.
        if client === accounts.client { Diagnostics.recordUnreported(error) }
        try? await Task.sleep(for: .seconds(5))
      }
    }
  }

  /// A silent push (content-available, a delivery that is not urgent) woke the app in the background: one short look
  /// at the hub, as the long wait's, without waiting there. Whether it brought something; nil when it could not ask.
  func syncOnce(accounts: AccountStore, friends: Friends) async -> Bool? {
    // Started by the push itself: the core may still be opening.
    for _ in 0..<100 where accounts.client == nil && accounts.account != nil { try? await Task.sleep(for: .milliseconds(100)) }
    guard let client = accounts.client, !HubTransport.unsupported else { return nil }
    do {
      let (events, contacts) = try await Hub.run(reportErrors: false) { (try client.sync(waitS: 0), client.contacts()) }
      guard client === accounts.client else { return nil }
      await take(events, contacts: contacts, client: client, friends: friends)
      if !events.isEmpty { fetchMedia(client: client) }
      return !events.isEmpty
    } catch YoError.Replaced {
      accounts.replaced(client)
    } catch {
      Diagnostics.recordUnreported(error)
    }
    return nil
  }

  /// What the core handed over while sending a photo or video (the outbox): as any, if `client` is still the
  /// account's (not signed out, or out and in again, meanwhile).
  func handle(
    _ events: [YoEvent], contacts: [YoContact], client: YoClient, accounts: AccountStore, friends: Friends
  ) async {
    guard client === accounts.client else { return }
    await take(events, contacts: contacts, client: client, friends: friends)
  }

  /// When the core opens and each time the app comes to the foreground (spec 4.4): key updates that are due, the
  /// storage's write; what it received on the way, as any.
  @discardableResult
  func maintain(accounts: AccountStore, friends: Friends) async -> Bool {
    guard let client = accounts.client, !HubTransport.unsupported else { return false }
    do {
      let (events, contacts) = try await Hub.run { (try client.maintenance(), client.contacts()) }
      guard client === accounts.client else { return false }  // signed out meanwhile: not for this account
      await take(events, contacts: contacts, client: client, friends: friends)
      return true
    } catch YoError.Replaced {
      accounts.replaced(client)
    } catch {}
    return false
  }

  /// A photo or video shown in the viewer: it will not show again.
  func viewed(_ arrival: Arrival) {
    Task { await ReceivedMedia.shared.consumed(arrival.id) }
  }

  /// What the core handed over: a Yo shows on its sender's bar, a new chat brings its friend into the list; a photo
  /// or video is noted, fetched and waits on the bar until seen; a contact's new key is noted for Contacts. Then the
  /// core may forget this batch, all of it that is kept (another call's batch is its own: the core acknowledges each
  /// as named); what could not be kept comes again after a restart. `contacts`: the core's, read with the events.
  /// What a call besides the sync brought from the inbox (a group made, renamed or left): taken as a sync's.
  func absorb(_ events: [YoEvent], client: YoClient, friends: Friends) async {
    guard !events.isEmpty, let contacts = try? await Hub.run({ client.contacts() }) else { return }
    await take(events, contacts: contacts, client: client, friends: friends)
  }

  private func take(_ events: [YoEvent], contacts: [YoContact], client: YoClient, friends: Friends) async {
    let start = Perf.on ? CACurrentMediaTime() : 0
    defer { if Perf.on { Perf.mark("take \(events.count) " + String(format: "%.1f ms", (CACurrentMediaTime() - start) * 1000)) } }
    var users: [String] = []
    var seenUsers: Set<String> = []
    var changedKeys: Set<String> = []
    for event in events {
      switch event {
      case .message(let user, _, _), .newChat(let user):
        if seenUsers.insert(user).inserted { users.append(user) }
      case .identityChanged(let user):
        if seenUsers.insert(user).inserted { users.append(user) }
        changedKeys.insert(user)
      default: break
      }
    }
    let resolved = friends.reconcile(contacts, users: users, changedKeys: changedKeys)
    for contact in contacts {
      if let name = resolved[contact.user]?.name ?? contact.handles.first ?? contact.name, name != "?", !name.isEmpty {
        memberNames[contact.user] = name
      }
    }
    guard !events.isEmpty else { return }
    let contactsByUser = Perf.timed("inbox.take.contacts.index") {
      Dictionary(contacts.map { ($0.user, $0) }, uniquingKeysWith: { first, _ in first })
    }
    // Resolve metadata once before the synchronous batch, never suspend between plain Yos.
    let needsGroups = events.contains { event in
      switch event { case .groupMessage, .groupChanged: true; default: false }
    }
    let groupsRead = needsGroups ? try? await Hub.run({ try client.groups() }) : nil
    if let groupsRead {
      Perf.timed("inbox.take.groups") { friends.takeGroups(groupsRead) }
    }
    let bars: [String: Friend] = needsGroups
      ? Dictionary(friends.all.compactMap { bar in bar.group.map { ($0, bar) } }, uniquingKeysWith: { a, _ in a })
      : [:]
    func friend(_ user: String) -> Friend {
      resolved[user] ?? Friend(name: contactsByUser[user]?.handles.first ?? contactsByUser[user]?.name ?? "?", color: 0, user: user)
    }
    var kept: [YoEvent] = []
    var mediaNotes: [(YoEvent, YoMedia, String, String, MediaFile.Kind)] = []
    Perf.timed("inbox.take.batch") {
      for event in events {
        switch event {
        case .message(let user, let sentAtMs, let kind):
          // What Yo does not show yet (a text, a link, a place, a kind it does not know) hangs on a Yo: shown as one
          // (spec 5).
          let sender = friend(user)
          received(id: "\(user)/\(sentAtMs)", from: sender.name)
          // On top it goes by the list, in one step with the others and never under a finger (FriendList.rise).
          count(from: sender.name)
          if let (media, kind) = kind.media {
            let id = Self.mediaID(user, sentAtMs, media)
            // Not noted (the disk full): the core keeps the Yo, which comes again after a restart.
            mediaNotes.append((event, media, id, sender.name, kind))
            add(Arrival(id: id, from: sender.name, pending: kind))  // on the bar at once; the download follows
            continue
          }
        case .groupMessage(let group, let user, let sentAtMs, let kind):
          // On the group's bar, its sender's name beside the Yo (spec 4.9). A group not on the list yet comes now.
          guard let bar = bars[group] else { break }
          let from = contactsByUser[user]
          let senderName = resolved[user]?.name ?? from?.handles.first ?? from?.name ?? "?"
          if senderName != "?" {
            memberNames[user] = senderName
          } else if memberNames[user] == nil {
            // Shown as "?" on the group's bar: why, to find it (no name, no id: a hash prefix).
            let roster = groupsRead?.first { $0.id == group }
            UnknownSender.record(place: "list", user: user, contact: from.map(UnknownSender.facts),
              group: (roster?.members.count ?? 0, roster.map { $0.members.contains(user) }),
              friend: friends.all.contains { $0.user == user }, groupsRead: groupsRead != nil)
          }
          received(id: "\(group)/\(user)/\(sentAtMs)", from: bar.name)
          count(from: bar.name)
          if let (media, kind) = kind.media {
            let id = Self.mediaID(group + user, sentAtMs, media)
            // Kept by the group's id: a rename or a bar of the same name never takes it (Friend.mediaKey).
            mediaNotes.append((event, media, id, bar.mediaKey, kind))
            add(Arrival(id: id, from: bar.mediaKey, pending: kind))
            continue
          }
        case .groupChanged:
          break
        case .groupSentElsewhere:
          break
        case .newChat(let user):
          _ = friend(user)  // a friend writing first again (a new phone) is the same friend, by user id
        case .identityChanged:
          break  // reconciled with the batch before publication
        case .profile, .sentElsewhere:
          break
        }
        kept.append(event)
      }
    }
    // Durably note media off main before acknowledging its event. Plain bursts have one main-actor turn.
    if !mediaNotes.isEmpty {
      let written = await ReceivedMedia.shared.expectBatch(mediaNotes.map { ($0.1, $0.2, $0.3, $0.4) })
      kept += mediaNotes.filter { written.contains($0.2) }.map { $0.0 }
      // Not noted: off the bar again until its Yo comes again (after a restart); a download already there stays.
      for note in mediaNotes where !written.contains(note.2) && unseen(id: note.2)?.pending != nil { drop(note.2) }
    }
    // Who sent the newest, for Yo Back from the Action button, Shortcuts or Siri (ShareBridge.noteLast).
    let newest = ShareBridge.yos(
      in: kept, name: { friend($0).name }, group: { bars[$0]?.name ?? "" },
      requests: Set(contacts.filter { !$0.accepted }.map(\.user)))
    if !newest.isEmpty { _ = try? await Hub.run { ShareBridge.noteLast(newest, client: client) } }
    // Taken while not in front (just left, its wait at the hub still running): the notification the notification
    // service would have written, which finds them read then. In front: shown on the bars, so a notification of
    // theirs (the service's, written just as the app opened) goes from Notification Center.
    switch UIApplication.shared.applicationState {
    case .background:
      let groups = (try? await Hub.run { try client.groups() }) ?? []
      let requests = (try? await Hub.run { ShareBridge.requests(of: client) }) ?? []
      await ShareBridge.post(
        ShareBridge.yos(
          in: kept, name: { friend($0).name }, group: { id in groups.first { $0.id == id }?.name ?? "" },
          requests: requests))
    case .active: Task { await collectDelivered() }
    default: break
    }
    // Photos and videos are noted (above) before the core may forget their Yos; fetched from here on.
    _ = try? await Hub.run { [kept] in client.acknowledge(events: kept) }
    fetchMedia(client: client)
  }

  /// A photo's or video's own id: who, when, and which blob (two at the same moment are two).
  private static func mediaID(_ user: String, _ sentAtMs: UInt64, _ media: YoMedia) -> String {
    "\(user)/\(sentAtMs)/" + media.id.map { String(format: "%02x", $0) }.joined()
  }

  /// Fetches the photos and videos noted and not kept yet (also those a broken network left).
  func fetchMedia(client: YoClient) {
    Task { [weak self] in
      await ReceivedMedia.shared.fetchWaiting(client: client) { [weak self] arrival in
        Perf.timed("inbox.media.add") { self?.add(arrival) }
      } dropped: { [weak self] id in
        self?.drop(id)
      }
    }
  }

  /// A Yo notification that arrived while the app is open, or was tapped.
  func received(id: String, from sender: String, senderName: String? = nil) {
    let parts = id.split(separator: "/")
    if parts.count == 3, let senderName, senderName != "?", !senderName.isEmpty {
      memberNames[String(parts[1])] = senderName
    }
    Perf.timed("inbox.arrived") {
      guard taken.insert(id).inserted else { return }
      takenOrder.append(id)
      if takenOrder.count > Self.takenKept {
        let gone = takenOrder.prefix(takenOrder.count - Self.takenKept)
        taken.subtract(gone)
        takenOrder.removeFirst(gone.count)
      }
      show(Arrival(id: id, from: sender))
    }
  }

  /// On opening the app: the Yo notifications waiting in Notification Center come in, oldest first, and leave it, with
  /// the pushes that named nobody ("Yo": the core brings their Yos, if any).
  @ObservationIgnored private var collectingDelivered = false
  func collectDelivered() async {
    guard !collectingDelivered else { return }
    collectingDelivered = true
    defer { collectingDelivered = false }
    let waiting = await Task.detached(priority: .utility) {
      let center = Perf.timed("inbox.notifications.center") { UNUserNotificationCenter.current() }
      let delivered = await center.deliveredNotifications()
      let waiting = Perf.timed("inbox.notifications.decode") {
        delivered.filter { $0.request.content.categoryIdentifier == Self.category }
          .sorted { $0.date < $1.date }.compactMap { notification -> (Arrival, String?)? in
            let info = notification.request.content.userInfo
            guard let sender = info["from"] as? String else { return nil }
            return (Arrival(id: info["id"] as? String ?? notification.request.identifier, from: sender), info["sender"] as? String)
          }
      }
      Perf.timed("inbox.notifications.remove") {
        center.removeDeliveredNotifications(
          withIdentifiers: delivered.map(\.request.identifier))
      }
      try? await center.setBadgeCount(0)
      return waiting
    }.value
    Perf.timed("inbox.notifications.apply") {
      for (arrival, name) in waiting { received(id: arrival.id, from: arrival.from, senderName: name) }
    }
  }

  /// Photos and videos seen (each once): they are gone, from this iPhone too.
  func seen(_ shown: [Arrival]) {
    for arrival in shown {
      Task { await ReceivedMedia.shared.forget(arrival.id) }
      // By id wherever it is kept: the viewer shows it under the bar's name, not under the key it is kept by.
      unseenMedia = unseenMedia.compactMapValues { kept in
        let rest = kept.filter { $0.id != arrival.id }
        return rest.isEmpty ? nil : rest
      }
    }
  }

  /// The Yo sound, once, on a player not busy if there is one (over the ones still playing).
  func playYo() { sounds.play() }

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
    return Perf.timed("inbox.takeArrivals") {
      var batch: [Arrival] = []
      swap(&batch, &ready)  // transfer the buffer without copying elements
      return batch
    }
  }

  /// The sound, and the sender's row takes the Yo (as Messages and Signal show a new message in the list: on the
  /// bar, not in a banner). The first Yo after a quiet moment goes to the list at once; what comes within
  /// `batchSeconds` after it goes together, at the end of them.
  private func show(_ arrival: Arrival) {
    Perf.timed("inbox.show") {
      noteArrival()
      chime()
      ready.append(Arrival(id: arrival.id, from: arrival.from))
      if arrival.photo != nil { unseenMedia[Self.mediaKey(arrival), default: []].append(arrival) }
      came += 1
      guard !gathering else { return }
      gathering = true
      PerfProbe.nextFrame { [weak self = self] in self?.arrived += 1 }
      Task {
        var told = came
        while true {
          try? await Task.sleep(for: .seconds(Self.batchSeconds))
          guard came != told else { break }  // a quiet moment: the next Yo goes at once again
          told = came
          PerfProbe.nextFrame { [weak self = self] in self?.arrived += 1 }
        }
        gathering = false
      }
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

/// Audio session, player preparation and playback stay on the same serial worker.
private final class InboxSounds: @unchecked Sendable {
  private let queue = DispatchQueue(label: "yo.inbox.audio", qos: .userInitiated)
  // Accessed exclusively on queue.
  private var players: [AVAudioPlayer] = []
  private var next = 0

  init() {
    queue.async { [self] in
      Perf.timed("inbox.sound.prepare") {
        guard let url = Bundle.main.url(forResource: "yo", withExtension: "caf") else { return }
        try? AVAudioSession.sharedInstance().setCategory(.ambient)
        players = (0..<8).compactMap { _ in
          let player = try? AVAudioPlayer(contentsOf: url)
          player?.prepareToPlay()
          return player
        }
      }
    }
  }

  func play() {
    queue.async { [self] in
      Perf.timed("inbox.sound.play") {
        guard !players.isEmpty else { return }
        let player = players.first { !$0.isPlaying } ?? players[next % players.count]
        next += 1
        player.currentTime = 0
        player.play()
      }
    }
  }
}
