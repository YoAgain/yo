import SwiftUI
import UIKit
import UniformTypeIdentifiers
import YoCore

/// The invite: the link as text, the card with the QR code as the share sheet's picture. Nothing is drawn before the
/// share sheet asks for the picture (`InviteCard`), not as the list or a page with the button shows.
@MainActor
struct InviteShareLink<Label: View>: View {
  let name: String
  @ViewBuilder var label: () -> Label

  var body: some View {
    ShareLink(
      item: "Yo me on Yo: \(Invite.link(for: name).absoluteString)",
      preview: SharePreview("\(name) wants to Yo", icon: InviteCard(name: name))
    ) { label() }
  }
}

/// The invite card as the share sheet's picture, drawn when the sheet asks for it (ContactCodeExport).
private struct InviteCard: Transferable {
  let name: String

  struct Unavailable: Error {}

  static var transferRepresentation: some TransferRepresentation {
    DataRepresentation(exportedContentType: .png) { card in
      guard let data = await ContactCodeExport.render(name: card.name)?.image.pngData() else { throw Unavailable() }
      return data
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

  /// The one under way, which another asking for the same name waits for.
  private static var making: (name: String, task: Task<Self?, Never>)?

  /// The QR codes and the check that the card reads back are made off the main thread (Core Image and its QR reader
  /// took a quarter of a second there, right as the list first showed); only the card's layout is drawn on it.
  static func render(name: String) async -> Self? {
    if let cached, cached.name == name { return cached.export }
    if let making, making.name == name { return await making.task.value }
    let task = Task { await make(name: name) }
    making = (name, task)
    let result = await task.value
    if making?.name == name { making = nil }
    if let result { cached = (name, result) }
    return result
  }

  private static func make(name: String) async -> Self? {
    let url = Invite.link(for: name).absoluteString
    func card(inverted: Bool) async -> (image: UIImage, code: UIImage)? {
      guard let code = await Task.detached(priority: .utility, operation: {
        RecoveryQR.contactImage(text: url, inverted: inverted)
      }).value else { return nil }
      let renderer = ImageRenderer(
        content: ContactCodeCard(name: name, code: code, inverted: inverted))
      renderer.scale = 3
      renderer.isOpaque = true
      guard let image = renderer.uiImage else { return nil }
      return (image, code)
    }
    func reads(_ image: UIImage) async -> Bool {
      await Task.detached(priority: .utility) { RecoveryQR.read(image) == url }.value
    }
    if let made = await card(inverted: true), await reads(made.image) { return Self(image: made.image, code: made.code) }
    guard let made = await card(inverted: false), await reads(made.image) else { return nil }
    return Self(image: made.image, code: made.code)
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
    Task { _ = await ContactCodeExport.render(name: name) }
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
      let card = await ContactCodeExport.render(name: name)
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
