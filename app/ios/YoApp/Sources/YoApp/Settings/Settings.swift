import Foundation
import SwiftUI
import UIKit
import YoCore

/// The iPhone's own appearance, whatever the list forces. SwiftUI applies the list's `.preferredColorScheme(.dark)` as
/// a trait override on the window scene, so the scene, its windows and every sheet see dark; only the screen carries
/// the iPhone's style, and a screen reports no trait changes. It is read again whenever Yo becomes active (Control
/// Center and Settings, where it is switched, both take Yo out of active). A value read once in a sheet's content was
/// kept until the list happened to redraw, so a sheet open across a switch kept the old appearance.
@MainActor @Observable final class SystemAppearance {
  static let shared = SystemAppearance()
  private(set) var scheme: ColorScheme = .light

  private init() {
    update()
    NotificationCenter.default.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) {
      _ in MainActor.assumeIsolated { SystemAppearance.shared.update() }
    }
  }

  func update() {
    guard let screen = UIApplication.shared.connectedScenes.compactMap({ ($0 as? UIWindowScene)?.screen }).first
    else { return }
    let new: ColorScheme = screen.traitCollection.userInterfaceStyle == .dark ? .dark : .light
    if new != scheme { scheme = new }
  }
}

extension View {
  /// The iPhone's own appearance for a sheet over the always dark list, following it while the sheet is open. Each
  /// sheet over such a sheet needs it too: a sheet keeps the appearance it was presented with.
  func systemAppearance() -> some View { modifier(SystemAppearanceModifier()) }
}

private struct SystemAppearanceModifier: ViewModifier {
  func body(content: Content) -> some View {
    content
      .preferredColorScheme(SystemAppearance.shared.scheme)
      .onAppear { SystemAppearance.shared.update() }
  }
}

/// The trailing chevron of a row that is a button but opens a page (its own sheet), as a navigation link's.
struct DisclosureChevron: View {
  var body: some View {
    Image(systemName: "chevron.forward")
      .font(.footnote.weight(.semibold))
      .foregroundStyle(.tertiary)
      .accessibilityHidden(true)
  }
}

/// A button row that opens a page: its label, then what it shows and its chevron at the trailing edge, as a navigation
/// link's row keeps them, at every text size (LabeledContent puts its content under the label at accessibility sizes,
/// the chevron then at the row's leading edge).
struct ChevronRow<Trailing: View, Label: View>: View {
  @ViewBuilder var trailing: Trailing
  @ViewBuilder var label: Label

  var body: some View {
    HStack(spacing: 8) {
      label
      Spacer(minLength: 0)
      trailing
    }
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
    case account, security, help, status, contacts, code, licenses, appIcon, look, devices, scan

    var id: Self { self }

    /// The pages that fit at half height (Account, its Account Security and App Icon's five icons) are pushed inside
    /// the half sheet; every other opens as a sheet of its own at full height, so the half sheet never changes height.
    var opensOwnSheet: Bool { !(self == .account || self == .security || self == .appIcon || self == .look || self == .devices) }
  }
  @State private var path: [Page] = []
  /// The full-height page open as its own sheet on top, with its own stack (About & Legal and Status push in it).
  @State private var sheet: Page?
  @State private var showingDeleteAccount = false
  @State private var linking = DeviceLinking.on == true
  @State private var deviceCount: Int?

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
              let name = Text(account.username).font(.yo(26, relativeTo: .title2)).lineLimit(1)
              let badge = accounts.recoveryNeedsAttention(hasRecoveryKey: hasRecoveryKey)
              // This account's own code, for a friend to scan (scanning is in Add Friends and Devices › Add Device).
              let code = YourCodeButton {
                YourCodePage.prepare(name: account.username)
                sheet = .code
              }
              // As iOS Settings' Apple Account row: the picture, the name beside it, the code button at the end. A
              // long name shrinks to fit the one line.
              HStack(spacing: 14) {
                Avatar(name: account.username, size: 56)
                name.minimumScaleFactor(0.5)
                if badge { RecoveryBadge() }
                Spacer(minLength: 0)
                code
              }
              .padding(.vertical, 2)
            }
          }
          // Look, and Devices (only on a hub that adds them, `DeviceLinking`; pushed in the sheet).
          Section {
            NavigationLink(value: Page.look) {
              Label("Look", systemImage: "paintbrush.fill").labelStyle(Tile(.amethyst))
            }
            .accessibilityIdentifier("settings.look")
            if linking {
              NavigationLink(value: Page.devices) {
                LabeledContent {
                  if let deviceCount { Text("\(deviceCount)") }
                } label: {
                  Label("Devices", systemImage: "iphone").labelStyle(Tile(.blue))
                }
              }
              .accessibilityIdentifier("settings.devices")
            }
          }
          Section {
            // Opens the list's own "+" field, so one learns where friends are added.
            Button {
              listAction(.addContact)
            } label: {
              ChevronRow {
                DisclosureChevron()
              } label: {
                Label("Add Friends", systemImage: "person.fill.badge.plus").labelStyle(Tile(.green))
              }
            }
            .tint(Color(.label))
            // A full page: only its own sheet, no push (a link would also push it into the half sheet).
            Button {
              sheet = .help
            } label: {
              ChevronRow {
                DisclosureChevron()
              } label: {
                Label("About", systemImage: "questionmark.circle.fill").labelStyle(Tile(.amethyst))
              }
            }
            .tint(Color(.label))
          }
        }
      }
      .listSectionSpacing(.compact)
      // It fits at its height, so it never scrolls: always shown from its top (with Dynamic Type's largest sizes,
      // where it cannot fit, it scrolls again).
      .scrollDisabled(!typeSize.isAccessibilitySize)
      // The title "Yodentity" and no close button: the system's grabber pulls the sheet down and away. The bar stays
      // (see-through), so a push or pop never changes its height and the pushed page's bar does not jump in (hidden
      // on the root, the pushed page's back button and title dropped into place after the push).
      .toolbar(.visible, for: .navigationBar)
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        // The root's own title, in a bar laid out as a pushed page's, so a push or pop never moves it.
        ToolbarItem(placement: .principal) { Text("Yodentity").font(.headline) }
      }
      .toolbarBackground(.hidden, for: .navigationBar)
      .scrollEdgeEffectHidden(!typeSize.isAccessibilitySize, for: .top)
      .task { await accounts.refreshSupport() }
      .task(id: path) {
        guard path.isEmpty else { return }
        linking = await DeviceLinking.check()
        if linking { deviceCount = try? await accounts.ownDevices().count }
      }
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
      // The scanner and the own code are half-height cards; every other full page at full height.
      .presentationDetents(page == .scan || page == .code ? [.medium] : [.large])
      .presentationBackground(Color(.systemGroupedBackground))
      .systemAppearance().appObjects()
    }
    .hitchScene("settings root")
    .onChange(of: path) { _, path in
      #if YO_DEV
        HitchMonitor.shared.scene = path.last.map { String(describing: $0) } ?? "settings root"
      #endif
    }
    // Every page scrolls only when it does not fit.
    .scrollBounceBehavior(.basedOnSize)
    // Just the height of the groups of rows (and the empty bar above them), on any screen: a fraction of the screen would
    // cut them off on a small iPhone and leave space under them on a large one. A full page opens as a sheet of its
    // own on top. With larger text the rows no longer fit: the full height, scrolling.
    .presentationDetents(typeSize > .large ? [.large] : [.custom(SettingsDetent.self)])
    // The grabber on every page: toggled with the path, it moved the bar mid-push (the pushed page's back button and
    // title began at the grabber's height, then dropped into place).
    .presentationDragIndicator(.visible)
    // Below full height an iOS 26 sheet is see-through glass: opaque instead.
    .presentationBackground(Color(.systemGroupedBackground))
    .task(id: "\(accounts.account?.username ?? "")/\(accounts.account?.secured == true)/\(accounts.wordsSaved)") {
      let available = await Task.detached(priority: .utility) { RecoveryWords.load() != nil }.value
      guard !Task.isCancelled else { return }
      hasRecoveryKey = available
    }
  }

  /// Look: the app icon and the bars' sides.
  private var lookPage: some View {
    Form {
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
      } footer: {
        Text("Left-Handed swaps the bars’ sides and moves Settings to the bottom right.")
      }
    }
    .navigationTitle("Look")
  }

  private func destination(_ page: Page) -> some View {
    Group {
      switch page {
      case .account:
        if let account = accounts.account {
          AccountPage(
            account: account, hasRecoveryKey: hasRecoveryKey, dismissSettings: { dismiss() },
            openSheet: { sheet = $0 }, showingDelete: $showingDeleteAccount)
        }
      case .security:
        if let account = accounts.account { AccountSecurityPage(account: account, hasRecoveryKey: hasRecoveryKey) }
      case .appIcon: AppIconPage()
      case .look: lookPage
      case .devices: DevicesPage()
      case .scan: ScanPage(close: nil, done: { sheet = nil; dismiss() })
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

/// The height of the settings root's groups of rows, as far as the screen allows.
private struct SettingsDetent: CustomPresentationDetent {
  static func height(in context: Context) -> CGFloat? { min(context.maxDetentValue, 520) }
}
