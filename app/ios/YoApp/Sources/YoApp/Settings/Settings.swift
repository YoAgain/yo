import Foundation
import OSLog
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
  /// The page in view and its height (`SheetFit`): the sheet's one detent. Half height until the first page is
  /// measured; a page that does not tell its height takes the full one.
  @State private var fit: (page: UUID, detent: PresentationDetent)?

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
      .listSectionSpacing(.compact)
      // It fits, so it holds still.
      .scrollBounceBehavior(.basedOnSize)
      .sheetFit()
      // No title on the first page: its rows start at the sheet's top. The pages pushed in keep their bar, inline.
      .toolbar(.hidden, for: .navigationBar)
      .navigationBarTitleDisplayMode(.inline)
      .task { await accounts.refreshSupport() }
      .onAppear { currentIconName = UIApplication.shared.alternateIconName }
      .navigationDestination(for: Page.self) { page in
        Group {
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
        .toolbar(.visible, for: .navigationBar)
        .navigationBarTitleDisplayMode(.inline)
      }
    }
    .hitchScene("settings root")
    .onChange(of: path) { _, path in
      #if YO_DEV
        HitchMonitor.shared.scene = path.last.map { String(describing: $0) } ?? "settings root"
      #endif
    }
    // One detent at a time, so no grabber: each page as high as its rows, a long one the full height (a height
    // detent stops there). Swiping down still closes it.
    .presentationDetents([fit?.detent ?? .medium])
    .environment(\.sheetFit) { page, detent in
      if let detent { fit = (page, detent) } else if fit?.page == page { fit = (page, .large) }
    }
    .presentationDragIndicator(.hidden)
    // Below full height an iOS 26 sheet is see-through glass: opaque instead.
    .presentationBackground(Color(.systemGroupedBackground))
    .task(id: "\(accounts.account?.username ?? "")/\(accounts.account?.secured == true)/\(accounts.wordsSaved)") {
      let available = await Task.detached(priority: .utility) { RecoveryWords.load() != nil }.value
      guard !Task.isCancelled else { return }
      hasRecoveryKey = available
    }
  }
}

extension EnvironmentValues {
  /// A page of the Yodentity sheet telling its height (`SheetFit`); nil: it left.
  @Entry var sheetFit: @MainActor (UUID, PresentationDetent?) -> Void = { _, _ in }
}

extension View {
  /// The sheet as high as this scrolling page: its content with the list's own margins and the bar above it (a
  /// height detent leaves out the home indicator's strip, so not the bottom inset).
  func sheetFit() -> some View { modifier(SheetFit()) }
}

/// Tells the sheet this page's height while it is in view: when it shows, when its content changes.
struct SheetFit: ViewModifier {
  @Environment(\.sheetFit) private var report
  @State private var page = UUID()
  @State private var height: CGFloat?
  @State private var shown = false

  func body(content: Content) -> some View {
    content
      .onScrollGeometryChange(for: CGFloat.self) { geometry in
        geometry.contentSize.height + geometry.contentInsets.top
      } action: { _, height in
        guard height > 0, abs(height - (self.height ?? 0)) > 1 else { return }
        self.height = height
        #if YO_DEV
          Logger(subsystem: "app.justyo", category: "settings").notice("sheet page height \(height, privacy: .public)")
        #endif
        if shown { report(page, .height(height)) }
      }
      .onAppear {
        shown = true
        if let height { report(page, .height(height)) }
      }
      .onDisappear {
        shown = false
        report(page, nil)
      }
  }
}
