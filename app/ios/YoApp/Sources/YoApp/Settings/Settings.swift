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
  @Environment(\.dynamicTypeSize) private var typeSize
  @Environment(AccountStore.self) private var accounts

  @State private var hasRecoveryKey = true
  @State private var currentIconName = UIApplication.shared.alternateIconName
  enum Page: Hashable, Identifiable {
    case account, security, help, status, contacts, code, licenses, appIcon

    var id: Self { self }

    /// The pages that fit at half height (Account, its Account Security and App Icon's five icons) are pushed inside
    /// the half sheet; every other opens as a sheet of its own at full height, so the half sheet never changes height.
    var opensOwnSheet: Bool { !(self == .account || self == .security || self == .appIcon) }
  }
  @State private var path: [Page] = []
  /// The full-height page open as its own sheet on top, with its own stack (About & Licenses and Status push in it).
  @State private var sheet: Page?
  @State private var showingDeleteAccount = false

  /// Every push goes through here: a half page joins the path, a full page opens its own sheet instead.
  private func navigate(to new: [Page]) {
    if new.count > path.count, let page = new.last, page.opensOwnSheet {
      sheet = page
    } else {
      path = new
    }
  }

  private var currentIconAsset: String {
    AppIconPage.icons.first { $0.name == currentIconName }?.image ?? "IconStandard"
  }

  var body: some View {
    NavigationStack(path: Binding(get: { path }, set: { navigate(to: $0) })) {
      Form {
        if let account = accounts.account {
          Section {
            NavigationLink(value: Page.account) {
              HStack(spacing: 14) {
                Avatar(name: account.username, size: 56)
                Text(account.username)
                  .font(.yo(26, relativeTo: .title2))
                  .lineLimit(1)
                  .minimumScaleFactor(0.5)
                if accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey) { RecoveryBadge() }
                Spacer(minLength: 0)
                YourCodeButton {
                  YourCodePage.prepare(name: account.username)
                  sheet = .code
                }
              }
              .padding(.vertical, 2)
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
      .listSectionSpacing(.compact)
      // It fits at half height, so it never scrolls: always shown from its top (with Dynamic Type's largest sizes,
      // where it cannot fit, it scrolls again).
      .scrollDisabled(!typeSize.isAccessibilitySize)
      // No title and no close button: only the system's grabber, to pull the sheet down and away. The pages pushed
      // in show their bar with the back button.
      .toolbar(.hidden, for: .navigationBar)
      .task { await accounts.refreshSupport() }
      .onAppear { currentIconName = UIApplication.shared.alternateIconName }
      .navigationDestination(for: Page.self) { destination($0) }
    }
    .sheet(item: $sheet) { page in
      NavigationStack {
        destination(page)
          .toolbar {
            ToolbarItem(placement: .topBarLeading) {
              Button(role: .close) { sheet = nil }
            }
          }
          .navigationDestination(for: Page.self) { destination($0) }
      }
      .scrollBounceBehavior(.basedOnSize)
      .presentationDetents([.large])
      .presentationBackground(Color(.systemGroupedBackground))
    }
    .hitchScene("settings root")
    .onChange(of: path) { _, path in
      #if YO_DEV
        HitchMonitor.shared.scene = path.last.map { String(describing: $0) } ?? "settings root"
      #endif
    }
    // Every page scrolls only when it does not fit.
    .scrollBounceBehavior(.basedOnSize)
    // Always half height: a full page opens as a sheet of its own on top.
    .presentationDetents([.medium])
    // The grabber on the first page; a page pushed in has its bar instead.
    .presentationDragIndicator(path.isEmpty ? .visible : .hidden)
    // Below full height an iOS 26 sheet is see-through glass: opaque instead.
    .presentationBackground(Color(.systemGroupedBackground))
    .task(id: "\(accounts.account?.username ?? "")/\(accounts.account?.secured == true)/\(accounts.wordsSaved)") {
      let available = await Task.detached(priority: .utility) { RecoveryWords.load() != nil }.value
      guard !Task.isCancelled else { return }
      hasRecoveryKey = available
    }
  }

  private func destination(_ page: Page) -> some View {
    Group {
      switch page {
      case .account:
        if let account = accounts.account {
          AccountPage(
            account: account, hasRecoveryKey: hasRecoveryKey, dismissSettings: { dismiss() },
            showingDelete: $showingDeleteAccount)
        }
      case .security:
        if let account = accounts.account { AccountSecurityPage(account: account, hasRecoveryKey: hasRecoveryKey) }
      case .appIcon: AppIconPage()
      case .help: HelpPage()
      case .status: StatusPage()
      case .contacts: ContactSecurityPage()
      case .licenses: LicensesPage()
      case .code:
        if let account = accounts.account { YourCodePage(name: account.username) }
      }
    }
    .toolbar(.visible, for: .navigationBar)
    .navigationBarTitleDisplayMode(.inline)
    // The list's compact top under the inline title, as on the first page.
    .listSectionSpacing(.compact)
  }
}
