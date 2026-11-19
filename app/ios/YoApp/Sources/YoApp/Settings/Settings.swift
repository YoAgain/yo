import Foundation
import SwiftUI
import UIKit
import YoCore

/// The iPhone's own appearance, whatever the app's window was set to (the list forces dark): read from the screen.
@MainActor enum SystemAppearance {
  static var scheme: ColorScheme {
    let screen = (UIApplication.shared.connectedScenes.first as? UIWindowScene)?.screen
    return screen?.traitCollection.userInterfaceStyle == .dark ? .dark : .light
  }
}

enum SettingKey {
  /// The bars' sides swapped: "Yo!!" at the left end, the photo sign and the undo ring at the right.
  static let leftHanded = "settings.leftHanded"
}

@MainActor
struct SettingsView: View {
  enum ListAction { case addContact }
  var listAction: (ListAction) -> Void

  @AppStorage(SettingKey.leftHanded) private var leftHanded = false
  @Environment(\.dismiss) private var dismiss
  @Environment(AccountStore.self) private var accounts

  @State private var hasRecoveryKey = true
  @State private var currentIconName = UIApplication.shared.alternateIconName
  enum Page: Hashable {
    case account, help, status, contacts, signInSecurity, code, licenses, appIcon
  }
  @State private var path: [Page] = []
  @State private var showingDeleteAccount = false
  /// The first page's own height (its rows and the bar above them): the sheet opens that high, no empty bottom.
  @State private var fit: CGFloat?
  @State private var detent: PresentationDetent = .large

  private var currentIconAsset: String {
    AppIconPage.icons.first { $0.name == currentIconName }?.image ?? "IconStandard"
  }

  var body: some View {
    NavigationStack(path: $path) {
      Form {
        if let account = accounts.account {
          Section {
            NavigationLink(value: Page.account) {
              HStack(spacing: 14) {
                Avatar(name: account.username, size: 76)
                Text(account.username)
                  .font(.yo(26, relativeTo: .title2))
                  .lineLimit(1)
                  .minimumScaleFactor(0.5)
                if accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey) { RecoveryBadge() }
                Spacer(minLength: 0)
                YourCodeButton {
                  YourCodePage.prepare(name: account.username)
                  path.append(.code)
                }
              }
              .padding(.vertical, 8)
            }
          }
          Section {
            NavigationLink(value: Page.appIcon) {
              Label {
                Text("App Icon")
              } icon: {
                AppIconPicture(asset: currentIconAsset)
              }
              .labelStyle(Tile(.amethyst))
            }
            Toggle(isOn: Binding(get: { leftHanded }, set: { leftHanded = $0; accounts.storeLeftHanded($0) })) {
              Label("Left-Handed", systemImage: "hand.point.up.left.fill").labelStyle(Tile(.blue))
            }
          }
          Section {
            NavigationLink(value: Page.help) {
              HStack {
                Label("Help & About", systemImage: "questionmark.circle.fill").labelStyle(Tile(.amethyst))
                SupportBadge()
              }
            }
            // Opens the list's own "+" field, so one learns where friends are added.
            Button {
              listAction(.addContact)
            } label: {
              Label("Add Friends", systemImage: "person.fill.badge.plus").labelStyle(Tile(.green))
            }
            .tint(Color(.label))
          }
        }
      }
      .onScrollGeometryChange(for: CGFloat.self) { geometry in
        geometry.contentSize.height + geometry.contentInsets.top + geometry.contentInsets.bottom
      } action: { _, height in
        guard path.isEmpty, height > 0, abs(height - (fit ?? 0)) > 1 else { return }
        let fitted = detent != .large || fit == nil
        fit = height
        if fitted { detent = .height(height) }
      }
      .task { await accounts.refreshSupport() }
      .onAppear { currentIconName = UIApplication.shared.alternateIconName }
      .navigationDestination(for: Page.self) { page in
        switch page {
        case .account:
          if let account = accounts.account {
            AccountPage(
              account: account, hasRecoveryKey: hasRecoveryKey, dismissSettings: { dismiss() },
              openSecurity: { path.append(.signInSecurity) }, showingDelete: $showingDeleteAccount)
          }
        case .signInSecurity:
          if let account = accounts.account {
            AccountPage(
              account: account, hasRecoveryKey: hasRecoveryKey, securityOnly: true, dismissSettings: { dismiss() },
              showingDelete: $showingDeleteAccount)
          }
        case .appIcon: AppIconPage()
        case .help: HelpPage()
        case .status: StatusPage()
        case .contacts: ContactSecurityPage()
        case .licenses: LicensesPage()
        case .code:
          if let account = accounts.account { YourCodePage(name: account.username) }
        }
      }
      .navigationTitle("Yodentity")
      .navigationBarTitleDisplayMode(.inline)
    }
    .hitchScene("settings root")
    .onChange(of: path) { _, path in
      #if YO_DEV
        HitchMonitor.shared.scene = path.last.map { String(describing: $0) } ?? "settings root"
      #endif
      // A page with more to read takes the full height; back on the first page, its own height again.
      if !path.isEmpty { detent = .large } else if let fit { detent = .height(fit) }
    }
    // As high as the first page, with the grabber to pull it up; the pages pushed in take the full height.
    .presentationDetents(fit.map { [.height($0), .large] } ?? [.large], selection: $detent)
    .presentationDragIndicator(.visible)
    // Below full height an iOS 26 sheet is see-through glass: opaque instead.
    .presentationBackground(Color(.systemGroupedBackground))
    .task(id: "\(accounts.account?.username ?? "")/\(accounts.account?.secured == true)/\(accounts.wordsSaved)") {
      let available = await Task.detached(priority: .utility) { RecoveryWords.load() != nil }.value
      guard !Task.isCancelled else { return }
      hasRecoveryKey = available
    }
  }
}
