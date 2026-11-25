import SwiftUI
import UIKit
import UniformTypeIdentifiers
import YoCore

/// Share the invite as a URL so the system offers Copy Link.
@MainActor
struct InviteShareLink<Label: View>: View {
  let name: String
  @ViewBuilder var label: () -> Label

  var body: some View {
    ShareLink(
      item: Invite.link(for: name),
      subject: Text("\(name) wants to Yo"), message: Text("Yo me on Yo"),
      preview: SharePreview("\(name) wants to Yo")
    ) { label() }
  }
}

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

@MainActor
private struct ContactCodeExport {
  let image: UIImage
  let code: UIImage
  private static var cached: (name: String, export: Self)?

  static func cachedCode(name: String) -> UIImage? {
    cached.flatMap { $0.name == name ? $0.export.code : nil }
  }

  /// The render under way, shared by another request for the same name.
  private static var making: (name: String, task: Task<Self?, Never>)?

  /// QR codes and the read-back check run off the main thread (a quarter of a second there as the list first showed).
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
    for inverted in [true, false] {
      if let made = await card(inverted: inverted), await reads(made.image) { return Self(image: made.image, code: made.code) }
    }
    return nil
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

@MainActor
struct YourCodePage: View {
  let name: String
  @State private var image: UIImage?
  @State private var loaded = false
  @State private var copyCount = 0
  @State private var copied = false

  init(name: String) {
    self.name = name
    _image = State(initialValue: ContactCodeExport.cachedCode(name: name))
  }

  static func prepare(name: String) {
    Task { _ = await ContactCodeExport.render(name: name) }
  }

  var body: some View {
    GeometryReader { proxy in
      let side = max(1, min(260, proxy.size.width - 40, proxy.size.height - 210))
      VStack(spacing: 0) {
        Text("\(name) wants to Yo")
          .font(.custom("Montserrat-Bold", size: 28))
          .lineLimit(1)
          .minimumScaleFactor(0.3)
        Spacer(minLength: 8)
        if let code = image {
          ShareLink(item: InviteCard(name: name), preview: SharePreview("\(name) wants to Yo", image: Image(uiImage: code))) {
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
          }
          .buttonStyle(QRPressStyle())
          .accessibilityLabel("Share QR code as picture")
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
        HStack {
          Button {
            let url = Invite.link(for: name)
            UIPasteboard.general.items = [[UTType.url.identifier: url, UTType.utf8PlainText.identifier: url.absoluteString]]
            copied = true
            copyCount += 1
          } label: {
            Label(copied ? "Copied" : "Copy Link", systemImage: copied ? "checkmark" : "doc.on.doc")
              .frame(maxWidth: .infinity)
          }
          InviteShareLink(name: name) {
            Label("Share", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity)
          }
        }
        .buttonStyle(.glass)
        .controlSize(.large)
        .padding(.top, 12)
      }
      .foregroundStyle(.white)
      .padding(.horizontal, 20)
      .padding(.vertical, 16)
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(Color.amethyst.ignoresSafeArea())
    .toolbarBackground(.hidden, for: .navigationBar)
    .toolbarColorScheme(.dark, for: .navigationBar)
    .navigationTitle("Your Code")
    .navigationBarTitleDisplayMode(.inline)
    .toolbar(.visible, for: .navigationBar)
    .sensoryFeedback(.success, trigger: copyCount)
    .task(id: copyCount) {
      guard copied else { return }
      try? await Task.sleep(for: .milliseconds(1500))
      if !Task.isCancelled { copied = false }
    }
    .tint(.white)
    .task(id: name) {
      let card = await ContactCodeExport.render(name: name)
      image = card?.code
      loaded = true
    }
  }
}

private struct QRPressStyle: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label.scaleEffect(configuration.isPressed ? 0.96 : 1)
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
