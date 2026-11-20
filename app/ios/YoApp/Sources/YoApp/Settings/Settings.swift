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
  enum Page: Hashable {
    case account, security, help, status, contacts, code, licenses, appIcon

    /// Each page's height, fixed here and never measured: half for the pages that fit there without scrolling (the
    /// first page and App Icon's five icons), full for every other.
    var fitsHalf: Bool { self == .appIcon }
  }
  @State private var path: [Page] = []
  @State private var showingDeleteAccount = false
  @State private var detent: PresentationDetent = .medium
  /// A push to a full-height page from a half-height one, waiting for the sheet to stand at full height.
  @State private var pendingPath: [Page]?
  /// The first page's room left over at half height, given to the profile card so the gap below the last section is
  /// the list's own bottom margin, as at its top. More rows later take it back by themselves.
  @State private var spare: CGFloat = 0
  @State private var halfHeight: CGFloat?

  private static func detent(for path: [Page]) -> PresentationDetent {
    path.last.map { $0.fitsHalf ? .medium : .large } ?? .medium
  }

  /// Every push and pop goes through here. A page laid out while the sheet grows stays cut at the old height and
  /// fills in late, so a full-height page is pushed only once the sheet has grown (its height changes, below). Going
  /// back to a half-height page pops and shrinks together: that page fits the top half, nothing of it is cut.
  private func navigate(to new: [Page]) {
    let target = Self.detent(for: new)
    if target == .large, detent != .large {
      pendingPath = new
      detent = .large
      // The sheet's own height change pushes; should none come, push anyway.
      Task {
        try? await Task.sleep(for: .milliseconds(400))
        pushPending()
      }
    } else {
      pendingPath = nil
      path = new
      detent = target
    }
  }

  private func pushPending() {
    guard let pending = pendingPath else { return }
    pendingPath = nil
    path = pending
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
              // The avatar takes the spare room up to 96 pt, the rest pads the row above and below it.
              let grow = min(spare, 40)
              HStack(spacing: 14) {
                Avatar(name: account.username, size: 56 + grow)
                Text(account.username)
                  .font(.yo(26 + grow * 0.3, relativeTo: .title2))
                  .lineLimit(1)
                  .minimumScaleFactor(0.5)
                if accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey) { RecoveryBadge() }
                Spacer(minLength: 0)
                YourCodeButton {
                  YourCodePage.prepare(name: account.username)
                  navigate(to: path + [.code])
                }
              }
              .padding(.vertical, 2 + (spare - grow) / 2)
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
      // The room the list leaves empty at half height goes to the profile card (what it already has counted in, so
      // it settles in one step); at Dynamic Type's accessibility sizes the compact row, and the list scrolls.
      .onScrollGeometryChange(for: [CGFloat].self) {
        [$0.containerSize.height, $0.containerSize.height - $0.contentInsets.top - $0.contentInsets.bottom - $0.contentSize.height]
      } action: { _, room in
        // Only at half height: while a pop shrinks the sheet back, the taller moment is not counted.
        guard path.isEmpty, pendingPath == nil, detent == .medium, room[0] > 100,
          room[0] <= (halfHeight ?? room[0]) + 0.5
        else { return }
        halfHeight = room[0]
        let settled = typeSize.isAccessibilitySize ? 0 : max(0, spare + room[1])
        if abs(settled - spare) > 0.5 { spare = settled }
      }
      // No title and no close button: only the system's grabber, to pull the sheet down and away. The pages pushed
      // in show their bar with the back button.
      .toolbar(.hidden, for: .navigationBar)
      .task { await accounts.refreshSupport() }
      .onAppear { currentIconName = UIApplication.shared.alternateIconName }
      .navigationDestination(for: Page.self) { page in
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
    // The sheet stands at its new, full height (UIKit sets the final frame as the growth starts): now the page is
    // pushed and laid out at that height from its first frame.
    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { old, new in
      if new > old { pushPending() }
    }
    .hitchScene("settings root")
    .onChange(of: path) { _, path in
      #if YO_DEV
        HitchMonitor.shared.scene = path.last.map { String(describing: $0) } ?? "settings root"
      #endif
    }
    // Every page scrolls only when it does not fit.
    .scrollBounceBehavior(.basedOnSize)
    .presentationDetents([detent])
    // The grabber on the first page (one height there: it only closes); a page pushed in has its bar instead.
    .presentationDragIndicator(path.isEmpty && pendingPath == nil ? .visible : .hidden)
    // Below full height an iOS 26 sheet is see-through glass: opaque instead.
    .presentationBackground(Color(.systemGroupedBackground))
    .task(id: "\(accounts.account?.username ?? "")/\(accounts.account?.secured == true)/\(accounts.wordsSaved)") {
      let available = await Task.detached(priority: .utility) { RecoveryWords.load() != nil }.value
      guard !Task.isCancelled else { return }
      hasRecoveryKey = available
    }
  }
}
