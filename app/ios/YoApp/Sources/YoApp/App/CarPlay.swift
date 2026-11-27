// CarPlay: the list, as on the phone and the watch. A tap sends Yo (YoSend, as Siri does). Its order and words are
// the bars' own: a core list model of its own (YoList), fed the list the app publishes (ShareInbox.publish), the Yos
// taken wherever (ShareBridge.RecentYo) and the Yos sent from outside the list (IntentSent), so it moves and says
// "Yo from MIA" or "Sent Yo" also while the iPhone is locked and the app's list does not run. Signed out: a line to
// open Yo on the iPhone. CarPlay as a communication app (Entitlements.plist) allows this one list template; Siri
// still reads and sends Yos in the car (SiriMessages).
import CarPlay
import YoCore

@MainActor
final class CarPlayList {
  static let shared = CarPlayList()
  private var controller: CPInterfaceController?
  private var list: ShareBridge.List?
  private let model = YoList()
  private var rows: [ListRow] = []
  private var active = false
  private var ticking: Task<Void, Never>?
  /// The Yos and sends the model was told (or that were older than the list when CarPlay came).
  private var told: Set<String> = []
  /// Sends from here the model was told at once: their notes (IntentSent) are not told again.
  private var ownSends: [(to: String, at: Date)] = []
  private var listening = false
  private let template: CPListTemplate = {
    let template = CPListTemplate(title: "Yo", sections: [])
    template.emptyViewTitleVariants = ["Open Yo on your iPhone"]
    return template
  }()

  private var now: UInt64 { DeleteNotice.now }

  func connect(_ controller: CPInterfaceController) {
    self.controller = controller
    listen()
    list = ShareBridge.readList()
    // What came or went before the list was written last is in its order already.
    let written = ShareBridge.friendsFile.flatMap {
      try? FileManager.default.attributesOfItem(atPath: $0.path)[.modificationDate] as? Date
    } ?? .distantPast
    for yo in ShareBridge.readRecentYos() where Date(timeIntervalSince1970: Double(yo.sentAtMs) / 1000) <= written {
      told.insert(yo.id)
    }
    for note in IntentSent.peek() where note.at <= written { told.insert(Self.key(note)) }
    take(model.friends(now: now, rows: friends))
    catchUp()
    controller.setRootTemplate(template, animated: false, completion: nil)
  }

  func disconnect() {
    activate(false)
    controller = nil
  }

  /// On the car's screen (not behind Maps or another app): the Yos there count as seen, as on a bar in view.
  func activate(_ on: Bool) {
    active = on
    take(model.foreground(now: now, active: on))
    take(model.rowsVisible(now: now, ids: on ? rows.map(\.id) : []))
  }

  func publish(_ list: ShareBridge.List?) {
    self.list = list
    take(model.friends(now: now, rows: friends))
  }

  private var friends: [ListFriend] {
    guard let list, !list.owner.isEmpty else { return [] }
    return list.friends.map { ListFriend(id: $0.to, name: $0.name, request: false, group: $0.to.hasPrefix("group:")) }
  }

  /// Yos taken by the notification service or the app, and Yos sent from Siri, a control or the watch.
  private func listen() {
    guard !listening else { return }
    listening = true
    CFNotificationCenterAddObserver(
      CFNotificationCenterGetDarwinNotifyCenter(), Unmanaged.passUnretained(self).toOpaque(),
      { _, _, _, _, _ in Task { @MainActor in CarPlayList.shared.catchUp() } },
      ShareBridge.lastYoChanged as CFString, nil, .deliverImmediately)
    IntentSent.listen()
    NotificationCenter.default.addObserver(forName: IntentSent.changed, object: nil, queue: .main) { _ in
      MainActor.assumeIsolated { CarPlayList.shared.catchUp() }
    }
  }

  private static func key(_ note: IntentSent) -> String { "sent/\(note.to)/\(note.at.timeIntervalSince1970)" }

  /// Tells the model what it was not told yet, oldest first.
  func catchUp() {
    guard controller != nil, let owner = list?.owner, !owner.isEmpty else { return }
    enum Event { case yo(ShareBridge.RecentYo), sent(IntentSent) }
    var events: [(Date, Event)] = ShareBridge.readRecentYos()
      .filter { $0.owner == owner && !told.contains($0.id) }
      .map { (Date(timeIntervalSince1970: Double($0.sentAtMs) / 1000), .yo($0)) }
    for note in IntentSent.peek() where !told.contains(Self.key(note)) {
      told.insert(Self.key(note))
      // Sent from here: told as it went.
      if let own = ownSends.firstIndex(where: { $0.to == note.to && abs($0.at.timeIntervalSince(note.at)) < 10 }) {
        ownSends.remove(at: own)
        continue
      }
      events.append((note.at, .sent(note)))
    }
    for (_, event) in events.sorted(by: { $0.0 < $1.0 }) {
      switch event {
      case .yo(let yo):
        told.insert(yo.id)
        let group = yo.bar.hasPrefix("group:")
        if group { take(model.senderNamed(now: now, user: yo.senderUser, name: yo.senderName)) }
        // A photo or video as a Yo: it is seen on the iPhone, never here, so no sign stays on the bar.
        take(model.received(now: now, id: yo.bar, kind: .yo, token: yo.id, sender: group ? yo.senderUser : nil))
      case .sent(let note):
        take(model.sendFinished(now: now, id: note.to, kind: .yo))
      }
    }
  }

  private func take(_ snapshot: ListSnapshot) {
    var snapshot = snapshot
    if active, snapshot.rows.map(\.id) != rows.map(\.id) {
      // Bars came or went: the visible ones are all of them.
      snapshot = model.rowsVisible(now: now, ids: snapshot.rows.map(\.id))
    }
    rows = snapshot.rows
    ticking?.cancel()
    if let next = snapshot.nextTick {
      let wait = next > now ? next - now : 0
      ticking = Task { [weak self] in
        try? await Task.sleep(for: .milliseconds(wait))
        guard !Task.isCancelled, let self else { return }
        self.take(self.model.tick(now: self.now))
      }
    }
    render()
  }

  private func render() {
    guard controller != nil else { return }
    let byID = Dictionary((list?.friends ?? []).map { ($0.to, $0) }, uniquingKeysWith: { a, _ in a })
    let items = rows.compactMap { row -> CPListItem? in
      guard let friend = byID[row.id] else { return nil }
      let item = CPListItem(text: Self.words(row, friend), detailText: nil)
      item.handler = { [weak self] _, done in
        Task { @MainActor in
          await self?.send(friend)
          done()
        }
      }
      return item
    }
    template.updateSections([CPListSection(items: items)])
  }

  /// What the bar says, on one line.
  private static func words(_ row: ListRow, _ friend: ShareBridge.Friend) -> String {
    switch row.line {
    case .arrival(let count, let name):
      return productArrivalLines(count: Int64(max(1, count)), name: name, lang: .english)[0]
    case .senders(let senders):
      let flow = listSenderFlow(senders: senders, lang: .english)
      let none = ListFlowWidths(prefix: 0, tokens: flow.tokens.map { _ in 0 }, separator: 0, comma: 0,
        more: flow.more.map { _ in 0 })
      return listFlowTwoLines(flow: flow, widths: none, room: 1).filter { !$0.isEmpty }.joined(separator: " ")
    case .title:
      if row.title == friend.name { return friend.title }
      if row.title == BarSendText.sentYo {
        return productSendWords(mark: productCounted(count: Int64(row.count)), sending: false, lang: .english)
      }
      return row.title
    }
  }

  private func send(_ friend: ShareBridge.Friend) async {
    guard let owner = list?.owner, !owner.isEmpty else { return }
    // Noted before it goes: its note (IntentSent) may come before the send returns.
    let started = Date.now
    ownSends.append((friend.to, started))
    if (try? await YoSend.send(to: friend.to, owner: owner)) != nil {
      take(model.sendFinished(now: now, id: friend.to, kind: .yo))
    } else {
      ownSends.removeAll { $0.to == friend.to && $0.at == started }
      take(model.sendFailed(now: now, id: friend.to, text: BarSendText.notSent))
    }
  }
}

@objc(YoCarPlayScene)
final class YoCarPlayScene: UIResponder, CPTemplateApplicationSceneDelegate {
  func templateApplicationScene(
    _ scene: CPTemplateApplicationScene, didConnect interfaceController: CPInterfaceController
  ) {
    MainActor.assumeIsolated { CarPlayList.shared.connect(interfaceController) }
  }

  func templateApplicationScene(
    _ scene: CPTemplateApplicationScene, didDisconnectInterfaceController interfaceController: CPInterfaceController
  ) {
    MainActor.assumeIsolated { CarPlayList.shared.disconnect() }
  }

  func sceneDidBecomeActive(_ scene: UIScene) {
    MainActor.assumeIsolated { CarPlayList.shared.activate(true) }
  }

  func sceneWillResignActive(_ scene: UIScene) {
    MainActor.assumeIsolated { CarPlayList.shared.activate(false) }
  }
}
