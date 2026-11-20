// Yos through the hub (Rust core): sent from the list, and received while the list is on screen (the inbox's long
// wait). A plain Yo shows on its sender's bar for a moment; only photos and videos wait, until seen once.
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
  var photo: UIImage? = nil
  var video: URL? = nil
  var file: URL? = nil
  /// Not downloaded yet: what it will be. On its bar from the moment its message comes; the viewer waits for it.
  var pending: MediaFile.Kind? = nil

  var isVideo: Bool { video != nil || pending == .video }

  /// A photo's or video's id starts with its sender and the time (`Inbox.mediaID`).
  var sentAt: Date? { Self.sentAtMs(id).map { Date(timeIntervalSince1970: Double($0) / 1000) } }

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
}

@MainActor @Observable
final class Inbox {
  /// Settings › Encryption's "Hub Connected".
  var lastSync: Date?
  /// Counts each time Yos are ready for the list (`takeArrivals`): at most every `batchSeconds`, so a burst redraws
  /// the list a few times, not once per Yo.
  private(set) var arrived = 0
  /// Sustained floods alone suppress list motion: more than twelve arrivals in one second.
  private(set) var busy = false
  @ObservationIgnored private var recentArrivals: [ContinuousClock.Instant] = []
  @ObservationIgnored private var lastArrival = ContinuousClock.now

  private func noteArrival() {
    let now = ContinuousClock.now
    lastArrival = now
    recentArrivals.removeAll { now - $0 >= .seconds(1) }
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
  /// The Yos that came since the list last took them, in order (the photos stay in `unseenMedia`).
  @ObservationIgnored private var ready: [Arrival] = []
  @ObservationIgnored private var gathering = false
  @ObservationIgnored private var came = 0
  // Allow the 160 ms count transition to finish between batches.
  static let batchSeconds = 0.2
  /// Group members' names by user id, as their Yos came: who a group's photo is from in the viewer.
  var memberNames: [String: String] = [:]

  /// Who a group's photo or video is from (their user id, hex): its id is the group's id (64 hex, spec 4.9), the
  /// sender's (32 hex), then the rest (`mediaID`).
  static func sender(of arrival: Arrival) -> String? {
    let head = arrival.id.split(separator: "/").first.map(String.init) ?? ""
    return head.count == 96 ? String(head.suffix(32)) : nil
  }
  /// Notifications already taken in, so one tapped and one collected on opening count once (the newest 2000).
  @ObservationIgnored private var taken: Set<String> = []
  @ObservationIgnored private var takenOrder: [String] = []
  private static let takenKept = 2000
  /// Photos and videos not yet seen, by `mediaKey`, oldest first: kept on this iPhone until then (ReceivedMedia).
  private var unseenMedia: [String: [Arrival]] = [:] {
    didSet { mediaChanges += 1 }
  }
  /// Counts changes of the photos and videos waiting: the list's signs follow it.
  private(set) var mediaChanges = 0

  func unseen(id: String) -> Arrival? {
    for kept in unseenMedia.values { if let found = kept.first(where: { $0.id == id }) { return found } }
    return nil
  }

  func media(for friend: Friend) -> [Arrival]? { unseenMedia[friend.mediaKey] }

  /// A group's by its id, a friend's by the sender's user id, which starts the media's id. Never by a name: two
  /// people may have had the same one (Friend.mediaKey).
  private static func mediaKey(_ arrival: Arrival) -> String {
    arrival.from.hasPrefix("group:") ? arrival.from : "user:" + (arrival.id.split(separator: "/").first.map(String.init) ?? "")
  }

  init() {
    // Only once Yo is in front: a launch in the background (Action button, silent push) sounds nothing.
    Launch.whenActive { [sounds] in sounds.prepare() }
    Launch.whenActive { [weak self] in
      Task { @MainActor in
        for arrival in await ReceivedMedia.shared.waiting() { self?.add(arrival) }
      }
    }
  }

  /// Noted as its message comes (`pending`), then the downloaded one in its place, in the order sent.
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

  /// A photo or video that will not come (gone at the hub, or it does not open).
  private func drop(_ id: String) {
    unseenMedia = unseenMedia.compactMapValues { kept in
      let rest = kept.filter { $0.id != id }
      return rest.isEmpty ? nil : rest
    }
  }

  /// Signed out: what waited to be seen is gone (MediaStorage.clearAll deleted it).
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

  /// A few players taking turns, so Yos close together sound over each other, as the original's did. Made off the
  /// main thread once Yo is in front (the session and players took a quarter of a second at launch).
  @ObservationIgnored private let sounds = InboxSounds()

  /// At most `soundsPerSecond` in any second: a burst sounds like many Yos without players fighting over the speaker.
  @ObservationIgnored private var sounded: [ContinuousClock.Instant] = []
  static let soundsPerSecond = 10

  /// "Yo back" right on the notification, as the original had, sent without opening the app.
  nonisolated static let category = ShareBridge.yoCategory
  nonisolated static let yoBack = ShareBridge.yoBackAction

  func askForNotifications() async {
    let center = UNUserNotificationCenter.current()
    ShareBridge.registerCategories()
    _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
  }

  // MARK: The hub

  /// A Yo through the hub (spec 4.7): nil when it went, else what the bar says instead. A friend's Yos go in the order
  /// tapped (a burst waits here, not in threads blocked on the core); Yos to other friends go meanwhile.
  func send(to friend: Friend, accounts: AccountStore, friends: Friends) async -> String? {
    let key = Username.skeleton(friend.name)
    let before = lastSend[key]
    let tapped = accounts.client  // a Yo waiting here never goes from the next account
    let account = accounts.account?.userID
    let this = Task { () async -> String? in
      _ = await before?.value
      var client = tapped
      // Tapped right after a cold start: the Yo waits up to 5 s for the core to open, same account only.
      if client == nil, let account {
        let soon = await accounts.clientSoon(seconds: 5) { accounts.account?.userID == account }
        if accounts.account?.userID == account { client = soon }
      }
      guard let client, client === accounts.client else { return productText(text: .notSent, lang: .english) }
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

  @ObservationIgnored private var lastSend: [String: Task<String?, Never>] = [:]

  enum Lookup {
    /// The name as its owner writes it ("YoGPT").
    case found(user: String, name: String?)
    case noSuchUser
    /// No answer (offline).
    case unknown
  }

  func lookUp(_ name: String, accounts: AccountStore) async -> Lookup {
    guard let client = accounts.client else { return .unknown }
    do {
      guard let found = try await Hub.run({ try client.whois(name: name) }) else { return .noSuchUser }
      return .found(user: found.user, name: found.username)
    } catch {
      return .unknown
    }
  }

  /// Friends added by a typed name while offline, asked now so each shows as its owner writes it.
  func identify(_ names: [String], accounts: AccountStore, friends: Friends) async {
    for name in names {
      if case .found(let user, let spelled) = await lookUp(name, accounts: accounts) {
        friends.found(name, user: user, spelled: spelled)
      }
    }
  }

  /// Once per opening: the core's spellings, and up to 20 friends known only by a typed name looked up.
  private func respell(client: YoClient, accounts: AccountStore, friends: Friends) async {
    if let contacts = try? await Hub.run({ client.contacts() }), client === accounts.client {
      friends.spell(contacts)
      friends.markGone(contacts)
    }
    if let groups = try? await Hub.run({ try client.groups() }), client === accounts.client { friends.takeGroups(groups) }
    let typed = friends.all.filter { $0.user == nil && $0.group == nil && !$0.blocked }.prefix(20).map(\.name)
    await identify(Array(typed), accounts: accounts, friends: friends)
  }

  static let groupEmpty = productText(text: .groupEmpty, lang: .english)

  private func sendNow(to friend: Friend, client: YoClient, accounts: AccountStore, friends: Friends) async -> String? {
    if let group = friend.group {
      do {
        let (events, contacts) = try await Hub.run { (try client.send(target: .group(id: group), kind: .yo), client.contacts()) }
        guard client === accounts.client else { return nil }
        await take(events, contacts: contacts, client: client, friends: friends)
        Stats.sentYo(to: friend.name)
        return nil
      } catch YoError.Replaced {
        accounts.replaced(client)
        return productText(text: .notSent, lang: .english)
      } catch YoError.GroupEmpty {
        return Self.groupEmpty  // nobody left in it: the list offers to delete it
      } catch {
        return productText(text: .notSent, lang: .english)
      }
    }
    let target = friend.user ?? friend.name
    do {
      let (events, contacts, waiting) = try await Hub.run {
        try Perf.timed("send.core") { (try client.send(target: .contact(id: target), kind: .yo), client.contacts(), client.waiting()) }
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
      if let contacts = try? await Hub.run({ client.contacts() }), client === accounts.client {
        friends.markGone(contacts)
      }
      return productText(text: .notOnYo, lang: .english)
    } catch YoError.Blocked {
      return productText(text: .blocked, lang: .english)
    } catch YoError.Outdated {
      // Their app is too old: the Yo waits in the outbox until they update.
      deliverLater(accounts: accounts, friends: friends)
      return productText(text: .needsUpdate, lang: .english)
    } catch YoError.Replaced {
      accounts.replaced(client)
      return productText(text: .notSent, lang: .english)
    } catch let error as YoError where retryLater(error: error) {
      // Not now (no network, the hub's limits, a first Yo the core cannot keep yet): kept here and retried.
      keepAndRetry(friend, accounts: accounts, friends: friends)
      return productText(text: .notSent, lang: .english)
    } catch {
      Perf.mark("send.refused \(error)")
      return productText(text: .notSent, lang: .english)
    }
  }

  /// Yos that met "not now" before the core could keep them, by friend.
  @ObservationIgnored private var unsent: [UUID: (friend: Friend, count: Int)] = [:]
  @ObservationIgnored private var resending: Task<Void, Never>?

  /// Sends what is kept again after 2 s, then less and less often (up to 30 s).
  private func keepAndRetry(_ friend: Friend, accounts: AccountStore, friends: Friends) {
    unsent[friend.key, default: (friend, 0)].count += 1
    guard resending == nil else { return }
    resending = Task {
      var pause = 2.0
      while !unsent.isEmpty, !Task.isCancelled {
        try? await Task.sleep(for: .seconds(pause))
        pause = min(pause * 2, 30)
        guard let client = accounts.client else { break }
        // One that meets "not now" again comes back by itself (`sendNow` keeps it).
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

  /// Yos the hub did not take yet (its per-sender rate, spec 6.6, or no network): sent again from the core's outbox
  /// after a second, then less and less often (up to 30 s), until none waits.
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

  @ObservationIgnored private var delivering: Task<Void, Never>?
  @ObservationIgnored private var deliverAgain = false

  /// While the list is on screen: the inbox's long wait at the hub (spec 6.3). Replaced elsewhere: signed out; this
  /// build too old (426): no more waits.
  func listen(accounts: AccountStore, friends: Friends) async {
    var respelled = false
    var storageTaken = false
    // After a failure (also one in the background) or while the hub counts as unreachable: a short look first, so
    // "Offline" goes as soon as the hub answers, not only at the end of a 25 s wait.
    var failed = false
    while !Task.isCancelled && !HubTransport.unsupported {
      guard let client = accounts.client else {
        try? await Task.sleep(for: .seconds(1))
        continue
      }
      // In the background iOS gives no turn on the shared state once its time is up (SharedAccess): wait for the
      // front rather than ask in vain. Pushes bring Yos meanwhile.
      if UIApplication.shared.applicationState == .background {
        await nap(1)
        continue
      }
      if !respelled {
        respelled = true
        Task { await respell(client: client, accounts: accounts, friends: friends) }
      }
      retryNow = false
      do {
        // What the notification service saved: on the bars before the hub is asked.
        let (saved, known) = try await Hub.run { try Perf.timed("inbox.listen.saved.core") { (try client.savedEvents(), client.contacts()) } }
        if !saved.isEmpty, client === accounts.client {
          await take(saved, contacts: known, client: client, friends: friends)
          fetchMedia(client: client)
        }
        let wait: UInt64 = failed || Availability.shared.remote ? 0 : 25
        let (events, contacts) = try await Hub.run(reportErrors: false) { try Perf.timed("inbox.listen.sync.core") { (try client.sync(waitS: wait), client.contacts()) } }
        failed = false
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
        let again = failed
        failed = true
        // Back in front or on another network (`reconnect`): the wait from before was cut off on purpose, asked
        // again at once, without the retry wait it had grown to.
        if retryNow {
          retryNow = false
          continue
        }
        // Signed out meanwhile: the old client's wait fails with `State`, as expected.
        if client === accounts.client { Diagnostics.recordUnreported(error) }
        // The first failure is tried again soon, before "Offline" shows; later ones every 5 s.
        await nap(again ? 5 : 1)
      }
    }
  }

  @ObservationIgnored private var retryNow = false
  @ObservationIgnored private var napping: Task<Void, Never>?

  /// Back in front after being away, or on another network (`Reconnect`): the inbox's wait from before is cut off
  /// (HubTransport.reconnect) and asked anew at once; a retry wait under way ends now.
  func reconnect() {
    retryNow = true
    napping?.cancel()
  }

  /// A wait `reconnect` can end early.
  private func nap(_ seconds: Double) async {
    let nap = Task<Void, Never> { try? await Task.sleep(for: .seconds(seconds)) }
    napping = nap
    await withTaskCancellationHandler { await nap.value } onCancel: { nap.cancel() }
    napping = nil
  }

  /// An extension wrote the shared state (`StateChanged`): its saved Yos and the stored friends, without the hub.
  func takeSaved(accounts: AccountStore, friends: Friends) async {
    guard let client = accounts.client else { return }
    guard let (saved, known) = try? await Hub.run({ (try client.savedEvents(), client.contacts()) }),
      client === accounts.client
    else { return }
    if !saved.isEmpty {
      await take(saved, contacts: known, client: client, friends: friends)
      fetchMedia(client: client)
    }
    await accounts.takeStorage(into: friends)
  }

  /// A silent push woke the app: one look at the hub without waiting. Whether it brought something; nil if not asked.
  func syncOnce(accounts: AccountStore, friends: Friends) async -> Bool? {
    // The core may still be opening. An old install signs in again only in the foreground (`AccountStore.updating`).
    guard !accounts.updating else { return nil }
    guard let client = await accounts.clientSoon(seconds: 10, while: { accounts.account != nil }),
      !HubTransport.unsupported
    else { return nil }
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

  /// What the core handed over while the outbox sent, if `client` is still the account's.
  func handle(
    _ events: [YoEvent], contacts: [YoContact], client: YoClient, accounts: AccountStore, friends: Friends
  ) async {
    guard client === accounts.client else { return }
    await take(events, contacts: contacts, client: client, friends: friends)
  }

  /// When the core opens and on each foreground (spec 4.4): due key updates and the storage's write.
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

  func viewed(_ arrival: Arrival) {
    Task { await ReceivedMedia.shared.consumed(arrival.id) }
  }

  /// What a call besides the sync brought from the inbox (a group made, renamed or left): taken as a sync's.
  func absorb(_ events: [YoEvent], client: YoClient, friends: Friends) async {
    guard !events.isEmpty, let contacts = try? await Hub.run({ client.contacts() }) else { return }
    await take(events, contacts: contacts, client: client, friends: friends)
  }

  /// What the core handed over, onto the bars; then the core may forget what was kept (the rest comes again after a
  /// restart). `contacts`: the core's, read with the events.
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
          // A kind Yo does not show yet (text, link, place) is shown as a Yo (spec 5).
          let sender = friend(user)
          received(id: "\(user)/\(sentAtMs)", from: sender.name)
          count(from: sender.name)
          if let (media, kind) = kind.media {
            let id = Self.mediaID(user, sentAtMs, media)
            // Not noted (the disk full): the core keeps the Yo, which comes again after a restart.
            mediaNotes.append((event, media, id, sender.name, kind))
            add(Arrival(id: id, from: sender.name, pending: kind))  // on the bar at once; the download follows
            continue
          }
        case .groupMessage(let group, let user, let sentAtMs, let kind):
          guard let bar = bars[group] else { break }
          let from = contactsByUser[user]
          let senderName = resolved[user]?.name ?? from?.handles.first ?? from?.name ?? "?"
          if senderName != "?" {
            memberNames[user] = senderName
          } else if memberNames[user] == nil {
            // Shown as "?" on the group's bar: recorded to find why.
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
        case .newChat(let user):
          _ = friend(user)
        case .identityChanged:
          break  // reconciled with the batch before publication
        case .profile:
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
    // Taken in the background (the wait still running): the notification the service would have written. In front:
    // the service's notifications written as the app opened go from Notification Center.
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

  /// Who, when, and which blob (two at the same moment are two).
  private static func mediaID(_ user: String, _ sentAtMs: UInt64, _ media: YoMedia) -> String {
    "\(user)/\(sentAtMs)/" + media.id.map { String(format: "%02x", $0) }.joined()
  }

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

  /// On opening: the Yo notifications in Notification Center come in, oldest first, and leave it.
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

  func seen(_ shown: [Arrival]) {
    for arrival in shown {
      Task { await ReceivedMedia.shared.forget(arrival.id) }
      unseenMedia = unseenMedia.compactMapValues { kept in
        let rest = kept.filter { $0.id != arrival.id }
        return rest.isEmpty ? nil : rest
      }
    }
  }

  func playYo() { sounds.play() }

  /// Statistics, written a second after the first Yo, once per sender.
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

  func takeArrivals() -> [Arrival] {
    return Perf.timed("inbox.takeArrivals") {
      var batch: [Arrival] = []
      swap(&batch, &ready)  // transfer the buffer without copying elements
      return batch
    }
  }

  /// The first Yo after a quiet moment goes to the list at once; what comes within `batchSeconds` goes together.
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
  // Only on `queue`.
  private var players: [AVAudioPlayer] = []
  private var prepared = false
  static let atOnce = Int(productSoundLimit())

  func prepare() {
    queue.async { [self] in prepareNow() }
  }

  private func prepareNow() {
    guard !prepared else { return }
    prepared = true
    Perf.timed("inbox.sound.prepare") {
      guard let url = Bundle.main.url(forResource: "yo", withExtension: "caf") else { return }
      try? AVAudioSession.sharedInstance().setCategory(.ambient)
      try? AVAudioSession.sharedInstance().setActive(true)  // ready before the first Yo, not woken by it
      players = (0..<Self.atOnce).compactMap { _ in
        let player = try? AVAudioPlayer(contentsOf: url)
        player?.prepareToPlay()
        return player
      }
    }
  }

  func play() {
    queue.async { [self] in
      prepareNow()
      Perf.timed("inbox.sound.play") {
        guard let slot = productSoundSlot(playing: players.map(\.isPlaying)) else { return }
        players[Int(slot)].play()
      }
    }
  }
}
