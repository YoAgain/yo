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
  @State private var showingManageAccount = false
  @State private var preparingNavigation: Task<Void, Never>?
  // Bootstrap at a system detent only until the first device layout arrives.
  @State private var detent: PresentationDetent = .medium
  @State private var rootHeight: CGFloat?
  /// The sheet is growing to .large before a tall page is pushed.
  @State private var growing = false
  /// Only pages with more below get the grabber and the large detent.
  private var expandable: Bool { growing || (path.last.map(Self.expands) ?? false) }
  private static func expands(_ page: Page) -> Bool {
    page == .help || page == .contacts || page == .status || page == .licenses
      || page == .signInSecurity
  }

  private var smallDetent: PresentationDetent {
    return rootHeight.map { .height(max(1, $0 - bottomInset)) } ?? .medium
  }

  // Custom detents exclude the home-indicator inset; the measured root includes both outer gaps.
  private var bottomInset: CGFloat {
    (UIApplication.shared.connectedScenes.first as? UIWindowScene)?.keyWindow?.safeAreaInsets.bottom
      ?? 0
  }

  private var currentIconAsset: String {
    switch currentIconName {
    case "Negative": "IconNegative"
    case "Original": "IconOriginal"
    case "OG2014": "IconOG2014"
    case "Lilac2015": "IconLilac2015"
    default: "IconStandard"
    }
  }

  private var codeButton: some View {
    YourCodeButton { navigate(to: path + [.code]) }
  }

  var body: some View {
    NavigationStack(path: Binding(get: { path }, set: { navigate(to: $0) })) {
      ScrollView {
        VStack(spacing: 20) {
          if let account = accounts.account {
            VStack(spacing: 0) {
              HStack(spacing: 12) {
                Button {
                  navigate(to: path + [.account])
                } label: {
                  HStack(spacing: 14) {
                    Avatar(name: account.username, size: 76)
                    HStack(spacing: 6) {
                      Text(account.username)
                        .font(.yo(26, relativeTo: .title2))
                        .lineLimit(1)
                        .minimumScaleFactor(0.5)
                      if recoveryNeedsAttention { RecoveryBadge() }
                    }
                    Spacer(minLength: 0)
                  }
                  .frame(maxWidth: .infinity, alignment: .leading)
                  .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                codeButton
                Button {
                  navigate(to: path + [.account])
                } label: {
                  Chevron().frame(minWidth: 16, minHeight: 44).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Account")
              }
              .padding(.vertical, 20)
            }
            .padding(.horizontal, 16)
            .background(Color(.secondarySystemGroupedBackground), in: .rect(cornerRadius: 26))
            compactCard {
              NavigationLink(value: Page.appIcon) {
                HStack {
                  Label {
                    Text("App Icon")
                  } icon: {
                    AppIconPicture(asset: currentIconAsset)
                  }
                  .labelStyle(Tile(.amethyst))
                  Spacer()
                  Chevron()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
              }
              Divider()
              Toggle(
                isOn: Binding(
                  get: { leftHanded },
                  set: {
                    leftHanded = $0
                    accounts.storeLeftHanded($0)
                  }
                )
              ) {
                Label("Left-Handed", systemImage: "hand.point.up.left.fill").labelStyle(Tile(.blue))
              }
            }
            compactCard {
              // The tutorial card on top, How to Use and Contact Us, then About & Licenses.
              NavigationLink(value: Page.help) {
                HStack {
                  Label("Help & About", systemImage: "questionmark.circle.fill").labelStyle(
                    Tile(.amethyst))
                  SupportBadge()
                  Spacer()
                  Chevron()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
              }
              .foregroundStyle(Color(.label))
              Divider()
              // Opens the list's own "+" field, so one learns where friends are added.
              Button {
                listAction(.addContact)
              } label: {
                HStack {
                  Label("Add Friends", systemImage: "person.fill.badge.plus").labelStyle(
                    Tile(.green))
                  Spacer()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
              }
            }
          }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 20)
        // Intrinsic content height depends on width/content, never the animated sheet height.
        .fixedSize(horizontal: false, vertical: true)
        .onGeometryChange(for: CGFloat.self) {
          $0.size.height
        } action: { height in
          guard height > 0, rootHeight.map({ abs($0 - height) > 1 }) ?? true else { return }
          rootHeight = height
          if path.isEmpty { detent = smallDetent }
        }
      }
      .task { await accounts.refreshSupport() }
      .onAppear { currentIconName = UIApplication.shared.alternateIconName }
      .scrollBounceBehavior(.basedOnSize)
      // Keep the 44 pt bar installed for pushes, but lay the root out from the sheet edge.
      // Include the home-indicator area so the two measured 20 pt gaps are physical gaps.
      // Only this root ignores insets; pushed pages retain their navigation safe area.
      .ignoresSafeArea(.container, edges: [.top, .bottom])

      .navigationDestination(for: Page.self) { page in
        switch page {
        case .account:
          if let account = accounts.account {
            AccountPage(
              account: account, hasRecoveryKey: hasRecoveryKey,
              dismissSettings: { dismiss() },
              openSecurity: { navigate(to: path + [.signInSecurity]) },
              showingDelete: $showingDeleteAccount, showingManage: $showingManageAccount)
          }
        case .signInSecurity:
          if let account = accounts.account {
            fullPage {
              AccountPage(
                account: account, hasRecoveryKey: hasRecoveryKey, securityOnly: true,
                dismissSettings: { dismiss() },
                showingDelete: $showingDeleteAccount, showingManage: $showingManageAccount)
            }
          }
        case .appIcon:
          AppIconPage(sheetHeight: (rootHeight ?? 430) - bottomInset)
        case .help:
          HelpPage()
        case .status:
          fullPage { StatusPage() }
        case .contacts:
          fullPage { ContactSecurityPage() }
        case .licenses:
          LicensesPage()  // opens at once; its text loads in the page
        case .code:
          if let account = accounts.account {
            YourCodePage(name: account.username, sheetHeight: (rootHeight ?? 430) - bottomInset)
          }
        }
      }
      .navigationTitle("Yodentity")
      .navigationBarTitleDisplayMode(.inline)
      // A hidden bar changes the destination's safe area during the push animation.
      .toolbar(.visible, for: .navigationBar)
      .toolbarBackground(.hidden, for: .navigationBar)
      .toolbar {
        ToolbarItem(placement: .principal) {
          Color.clear.frame(width: 1, height: 1).accessibilityHidden(true)
        }
      }

    }
    .modifier(RoundTripOverlay())
    .hitchScene("settings root")
    .onDisappear { preparingNavigation?.cancel() }
    .presentationDragIndicator(expandable ? .visible : .hidden)
    .presentationDetents(expandable ? [smallDetent, .large] : [smallDetent], selection: $detent)
    // A sheet below full height is see-through glass on iOS 26: the list behind showed through and turned the
    // cards light. Opaque, the same at every height.
    .presentationBackground(Color(.systemGroupedBackground))
    .task(
      id:
        "\(accounts.account?.username ?? "")/\(accounts.account?.secured == true)/\(accounts.wordsSaved)"
    ) {
      let available = await Task.detached(priority: .utility) { RecoveryWords.load() != nil }.value
      guard !Task.isCancelled else { return }
      hasRecoveryKey = available
    }
  }

  private func fullPage<Content: View>(@ViewBuilder content: () -> Content) -> some View {
    content()
  }

  private func navigate(to next: [Page]) {
    preparingNavigation?.cancel()
    if next.count > path.count, next.last == .code, let name = accounts.account?.username {
      YourCodePage.prepare(name: name)
    }
    // A tall page pushed from a half sheet slid in drawn at the old height above empty sheet, even after
    // the sheet had grown. So the sheet grows first, and that is the motion: the page then appears in place.
    if next.count > path.count, next.last.map(Self.expands) ?? false, detent != .large {
      growing = true
      withAnimation(.smooth(duration: 0.35)) { detent = .large }
      preparingNavigation = Task {
        try? await Task.sleep(for: .milliseconds(500))
        guard !Task.isCancelled else { return }
        show(next, animated: false)
      }
      return
    }
    show(next)
  }

  private func show(_ next: [Page], animated: Bool = true) {
    var transaction = Transaction(animation: animated ? .smooth(duration: 0.35) : nil)
    transaction.disablesAnimations = !animated
    withTransaction(transaction) {
      #if YO_DEV
        HitchMonitor.shared.scene = next.last.map { String(describing: $0) } ?? "settings root"
      #endif
      growing = false
      if !(next.last.map(Self.expands) ?? false) { detent = smallDetent }
      path = next
    }
  }

  private var recoveryNeedsAttention: Bool {
    let passkeyOn = accounts.account?.byPasskey == true && accounts.account?.passkeyAuth == true
    let keyOn = accounts.account?.secured == true && accounts.wordsSaved && hasRecoveryKey
    return !passkeyOn && !keyOn
  }

  private func compactCard<Content: View>(@ViewBuilder content: () -> Content) -> some View {
    VStack(spacing: 10) { content() }
      .padding(.horizontal, 16)
      .padding(.vertical, 12)
      .background(Color(.secondarySystemGroupedBackground), in: .rect(cornerRadius: 26))
      .buttonStyle(.plain)
  }

}
