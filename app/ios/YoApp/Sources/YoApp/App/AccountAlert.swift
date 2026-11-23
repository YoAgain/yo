// The account's alerts ("Can't Open Your Account", "Server Not Reachable", "Update Not Finished") as a system alert in
// a window of its own above the app's. SwiftUI's `.alert` on the root was not shown when it came up while a sheet was
// open or still appearing (UIKit refuses the presentation and SwiftUI does not try again): the list stayed "Offline"
// with nothing to tap.
import UIKit

@MainActor final class AccountAlert {
  struct Content: Equatable {
    var title: String
    var message: String
    var button: String
  }

  static let shared = AccountAlert()
  private var window: UIWindow?
  private var shown: Content?

  /// What the account needs said, if anything.
  static func content(_ accounts: AccountStore) -> Content? {
    if accounts.broken {
      return accounts.hubDown
        ? Content(
          title: "Server Not Reachable",
          message: "Yo can’t reach its server right now. Your account is fine; try again in a moment.",
          button: "Try Again")
        : Content(
          title: "Can’t Open Your Account",
          message: "Yo can’t open your account on this device. Sign in again to keep using it.\n\n(\(accounts.brokenWhy))",
          button: "Sign In Again")
    }
    if accounts.updateStalled {
      return Content(
        title: "Update Not Finished",
        message: "Yo couldn’t finish updating your account on this device. Nothing is lost. Check your connection and try again.",
        button: "Try Again")
    }
    return nil
  }

  /// Shows what the account needs said (none: the alert goes); its button does what it says and asks again after.
  func update(_ accounts: AccountStore) {
    let content = Self.content(accounts)
    guard content != shown else { return }
    hide()
    guard let content else { return }
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    guard let scene = scenes.first(where: { $0.activationState == .foregroundActive }) ?? scenes.first else { return }
    let window = UIWindow(windowScene: scene)
    window.windowLevel = .alert
    let root = UIViewController()
    root.view.backgroundColor = .clear
    window.rootViewController = root
    window.makeKeyAndVisible()
    let alert = UIAlertController(title: content.title, message: content.message, preferredStyle: .alert)
    let broken = accounts.broken
    alert.addAction(UIAlertAction(title: content.button, style: .default) { [weak self, weak accounts] _ in
      self?.hide()
      guard let accounts else { return }
      Task { @MainActor in
        if broken { await accounts.recover() } else { accounts.retryUpdate() }
        self?.update(accounts)
      }
    })
    root.present(alert, animated: true)
    self.window = window
    shown = content
  }

  private func hide() {
    guard let window else { return }
    window.rootViewController?.presentedViewController?.dismiss(animated: false)
    window.isHidden = true
    window.windowScene?.windows.first { $0 !== window && !$0.isHidden && $0.windowLevel == .normal }?.makeKey()
    self.window = nil
    shown = nil
  }
}
