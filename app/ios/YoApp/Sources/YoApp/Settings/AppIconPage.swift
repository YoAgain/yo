import SwiftUI
import UIKit
import YoCore

@MainActor
struct AppIconPage: View {
  @Environment(AccountStore.self) private var accounts
  @State private var current = UIApplication.shared.alternateIconName
  @State private var changing = false
  @State private var iconError: String?
  @Environment(\.scenePhase) private var scenePhase
  static let icons: [(name: String?, image: String, label: String)] = [
    (nil, "IconStandard", "Yo"), ("Negative", "IconNegative", "Negative Yo"),
    ("Original", "IconOriginal", "Full Color"), ("OG2014", "IconOG2014", "OG 2014"),
    ("Lilac2015", "IconLilac2015", "2015"),
  ]

  var body: some View {
    let iconSide: CGFloat = 64
    // Scrolls only where it must.
    return ScrollView {
      VStack(spacing: 24) {
        ForEach(0..<2) { row in
          HStack(alignment: .top, spacing: 12) {
            ForEach(0..<3) { column in
              let index = row * 3 + column
              if index < Self.icons.count {
                let icon = Self.icons[index]
                Button {
                  guard !changing, current != icon.name else { return }
                  let client = accounts.client
                  changing = true
                  Task {
                    defer { changing = false }
                    do {
                      try await UIApplication.shared.setAlternateIconName(icon.name)
                      current = UIApplication.shared.alternateIconName
                      if current == icon.name { accounts.storeAppIcon(icon.name, by: client) }
                    } catch { iconError = error.localizedDescription }
                  }
                } label: {
                  VStack(spacing: 10) {
                    picture(icon.image, side: iconSide)
                      .padding(5)
                      .overlay {
                        RoundedRectangle(cornerRadius: 20)
                          .stroke(current == icon.name ? Color.accentColor : .clear, lineWidth: 3)
                      }
                    Text(icon.label).font(.subheadline).foregroundStyle(Color(.label))
                  }
                  .frame(maxWidth: .infinity)
                }
                .buttonStyle(.plain)
                .disabled(changing)
                .accessibilityAddTraits(current == icon.name ? .isSelected : [])
              } else {
                Color.clear.frame(maxWidth: .infinity)
              }
            }
          }
        }
      }
      .padding(20)
    }
    .onAppear { current = UIApplication.shared.alternateIconName }
    .onChange(of: scenePhase) { _, phase in
      if phase == .active { current = UIApplication.shared.alternateIconName }
    }
    .scrollBounceBehavior(.basedOnSize)
    .navigationTitle("App Icon")
    .navigationBarTitleDisplayMode(.inline)
    .toolbar(.visible, for: .navigationBar)
    .problemAlert("Icon Couldn’t Be Changed", $iconError)
  }

  private func picture(_ asset: String, side: CGFloat) -> some View {
    AppIconPicture(asset: asset)
      .frame(width: side, height: side)
      .clipShape(.rect(cornerRadius: 14)).accessibilityHidden(true)
  }
}

/// These are loose alternate-icon PNGs in the main bundle, not asset-catalog images.
struct AppIconPicture: View {
  let asset: String

  var body: some View {
    if let url = Bundle.main.url(forResource: asset + "@3x", withExtension: "png"),
      let image = UIImage(contentsOfFile: url.path)
    {
      Image(uiImage: image).renderingMode(.original).resizable().scaledToFit()
    }
  }
}
