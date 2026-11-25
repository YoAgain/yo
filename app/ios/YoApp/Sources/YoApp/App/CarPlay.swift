// CarPlay: the list, as on the phone and the watch. A tap sends Yo (YoSend, as Siri does) and the row says "Sent Yo!"
// for a moment; the list follows the app's (ShareInbox.publish), newest first. Signed out: a line to open Yo on the
// iPhone. CarPlay as a communication app (Entitlements.plist) allows this one list template; Siri still reads and
// sends Yos in the car (SiriMessages).
import CarPlay

@MainActor
final class CarPlayList {
  static let shared = CarPlayList()
  private var controller: CPInterfaceController?
  private var list: ShareBridge.List?
  private var sent: [String: String] = [:]  // to → the row's text for a moment ("Sent Yo!", "No Yo")
  private let template: CPListTemplate = {
    let template = CPListTemplate(title: "Yo", sections: [])
    template.emptyViewTitleVariants = ["Open Yo on your iPhone"]
    return template
  }()

  func connect(_ controller: CPInterfaceController) {
    self.controller = controller
    list = ShareBridge.readList()
    render()
    controller.setRootTemplate(template, animated: false, completion: nil)
  }

  func disconnect() { controller = nil }

  func publish(_ list: ShareBridge.List?) {
    self.list = list
    render()
  }

  private func render() {
    guard controller != nil else { return }
    let friends = list.map { $0.owner.isEmpty ? [] : $0.friends } ?? []
    let items = friends.map { friend in
      let item = CPListItem(text: sent[friend.to] ?? friend.title, detailText: nil)
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

  private func send(_ friend: ShareBridge.Friend) async {
    guard let owner = list?.owner, !owner.isEmpty, sent[friend.to] == nil else { return }
    let ok = (try? await YoSend.send(to: friend.to, owner: owner)) != nil
    sent[friend.to] = ok ? "Sent Yo!" : "No Yo"
    render()
    try? await Task.sleep(for: .seconds(2))
    sent[friend.to] = nil
    render()
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
}
