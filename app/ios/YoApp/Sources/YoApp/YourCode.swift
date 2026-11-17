import SwiftUI
import UIKit
import YoCore

@MainActor
struct InviteShareLink<Label: View>: View {
  let name: String
  @ViewBuilder var label: () -> Label
  @State private var card: ContactCodeExport?

  var body: some View {
    Group {
      if let card {
        ShareLink(
          item: "Yo me on Yo: \(Invite.link(for: name).absoluteString)",
          preview: SharePreview("\(name) wants to Yo", icon: Image(uiImage: card.image))
        ) { label() }
      } else {
        label().disabled(true)
      }
    }
    .task(id: name) {
      card = ContactCodeExport.render(name: name)
    }
  }
}

/// One renderer for Your Code and every invite's preview.
@MainActor
private struct ContactCodeExport {
  let image: UIImage
  let code: UIImage
  private static var cached: (name: String, export: Self)?

  static func cachedCode(name: String) -> UIImage? {
    cached.flatMap { $0.name == name ? $0.export.code : nil }
  }

  static func render(name: String) -> Self? {
    if let cached, cached.name == name { return cached.export }
    let url = Invite.link(for: name).absoluteString
    func card(inverted: Bool) -> UIImage? {
      guard let code = RecoveryQR.contactImage(text: url, inverted: inverted) else { return nil }
      let renderer = ImageRenderer(
        content: ContactCodeCard(name: name, code: code, inverted: inverted))
      renderer.scale = 3
      renderer.isOpaque = true
      return renderer.uiImage
    }
    var usedFallback = false
    var candidate = card(inverted: true)
    if candidate.flatMap({ RecoveryQR.read($0) }) != url {
      usedFallback = true
      candidate = card(inverted: false)
    }
    guard let image = candidate, RecoveryQR.read(image) == url else { return nil }
    guard let code = RecoveryQR.contactImage(text: url, inverted: !usedFallback) else { return nil }
    let result = Self(image: image, code: code)
    cached = (name, result)
    return result
  }
}

/// Fixed point dimensions keep the export at 1080 × 1350 when rendered at 3×.
@MainActor
private struct ContactCodeCard: View {
  let name: String
  let code: UIImage
  var inverted = true

  var body: some View {
    VStack(spacing: 0) {
      Text("\(name) wants to Yo")
        .font(.custom("Montserrat-Bold", size: 32))
        .multilineTextAlignment(.center)
        .lineLimit(2)
        .minimumScaleFactor(0.3)
        .padding(.horizontal, 24)
        .padding(.top, 24)
      Spacer(minLength: 8)
      Image(uiImage: code)
        .resizable()
        .interpolation(.none)
        .frame(width: 216, height: 216)
        .overlay {
          Text("Yo")
            .font(.custom("Montserrat-Bold", size: 24))
            .foregroundStyle(.white)
            .frame(width: 44, height: 44)
            .background(Color.amethyst)
        }
        // At least four modules even for the smallest (21-module) QR.
        .padding(42)
        .background(inverted ? Color.amethyst : .white)
      Spacer(minLength: 8)
      Text(Invite.link(for: name).absoluteString)
        .font(.custom("Montserrat-Bold", size: 40))
        .lineLimit(1)
        .minimumScaleFactor(0.1)
        .frame(width: 300, height: 40)
        .padding(.top, 8)
        .padding(.bottom, 16)
    }
    .foregroundStyle(.white)
    .frame(width: 360, height: 450)
    .background(Color.amethyst)
    .environment(\.layoutDirection, .leftToRight)
  }
}

/// A full-sheet invite; sharing retains the fixed card layout.
@MainActor
struct YourCodePage: View {
  let name: String
  let sheetHeight: CGFloat
  @State private var image: UIImage?
  @State private var loaded = false

  init(name: String, sheetHeight: CGFloat) {
    self.name = name
    self.sheetHeight = sheetHeight
    _image = State(initialValue: ContactCodeExport.cachedCode(name: name))
  }

  static func prepare(name: String) {
    _ = ContactCodeExport.render(name: name)
  }

  var body: some View {
    let side = max(1, min(260, sheetHeight - 210))
    let contentHeight = max(1, sheetHeight - 70)
    return Group {
      VStack(spacing: 0) {
        Text("\(name) wants to Yo")
          .font(.custom("Montserrat-Bold", size: 28))
          .lineLimit(1)
          .minimumScaleFactor(0.3)
        Spacer(minLength: 8)
        if let code = image {
          Image(uiImage: code)
            .resizable()
            .interpolation(.none)
            .frame(width: side, height: side)
            .overlay {
              Text("Yo")
                .font(.custom("Montserrat-Bold", size: side / 9))
                .foregroundStyle(.white)
                .frame(width: side / 5, height: side / 5)
                .background(Color.amethyst)
            }
            .accessibilityLabel(
              "Contact QR code for \(name). \(Invite.link(for: name).absoluteString)")
        } else if !loaded {
          ProgressView().tint(.white).frame(width: side, height: side)
        } else {
          ContentUnavailableView(
            "Unable to Create Code", systemImage: "qrcode",
            description: Text("Go back and try again."))
        }
        Spacer(minLength: 8)
        Text(Invite.link(for: name).absoluteString)
          .font(.custom("Montserrat-Bold", size: 28))
          .lineLimit(1)
          .minimumScaleFactor(0.1)
      }
      .foregroundStyle(.white)
      .padding(.horizontal, 20)
      .padding(.vertical, 16)
      .frame(maxWidth: .infinity)
      .frame(height: contentHeight)
    }
    // Opened from the full-height Account page too: the colour fills the whole page, the card centred in it.
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(Color.amethyst.ignoresSafeArea())
    .toolbarBackground(.hidden, for: .navigationBar)
    .toolbarColorScheme(.dark, for: .navigationBar)
    .navigationTitle("Your Code")
    .navigationBarTitleDisplayMode(.inline)
    .toolbar(.visible, for: .navigationBar)
    .toolbar {
      ToolbarItem(placement: .topBarTrailing) {
        InviteShareLink(name: name) {
          Label("Share Your Code", systemImage: "square.and.arrow.up")
        }
        .disabled(image == nil)
      }
    }

    .tint(.white)
    .task(id: name) {
      let card = ContactCodeExport.render(name: name)
      image = card?.code
      loaded = true
    }
  }

}

struct YourCodeButton: View {
  var action: () -> Void

  var body: some View {
    Button(action: action) {
      Image(systemName: "qrcode")
        .font(.system(size: 22))
        .frame(width: 44, height: 44)
        .background(Color(uiColor: .secondarySystemFill), in: .circle)
    }
    .buttonStyle(.borderless)
    .accessibilityLabel("Show Your Code")
  }
}
