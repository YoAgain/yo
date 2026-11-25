import LinkPresentation
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import YoCore

/// Share the invite as a URL so the system offers Copy Link. Through UIActivityViewController with the URL itself:
/// SwiftUI's ShareLink hands a URL over as its archived property list too, which some apps (X) posted as text
/// ("bplist00…https://justyo.co/NAME…").
@MainActor
struct InviteShareLink<Label: View>: View {
  let name: String
  @ViewBuilder var label: () -> Label

  var body: some View {
    Button {
      Share.present([InviteItem(name: name)]) { _ in }
    } label: {
      label()
    }
  }
}

/// The invite link for the share sheet: the URL, the mail subject, and the preview's title ("NAME wants to Yo").
private final class InviteItem: NSObject, UIActivityItemSource {
  let name: String
  let url: URL

  init(name: String) {
    self.name = name
    url = Invite.link(for: name)
  }

  func activityViewControllerPlaceholderItem(_ controller: UIActivityViewController) -> Any { url }

  func activityViewController(_ controller: UIActivityViewController, itemForActivityType type: UIActivity.ActivityType?)
    -> Any?
  { url }

  func activityViewController(_ controller: UIActivityViewController, subjectForActivityType type: UIActivity.ActivityType?)
    -> String
  { "\(name) wants to Yo" }

  func activityViewControllerLinkMetadata(_ controller: UIActivityViewController) -> LPLinkMetadata? {
    let metadata = LPLinkMetadata()
    metadata.originalURL = url
    metadata.url = url
    metadata.title = "\(name) wants to Yo"
    return metadata
  }
}

/// The invite as a picture and a text, from the code page's Share and a tap on its code: the card in the page's own
/// look and "Yo me on Yo: https://justyo.co/NAME". The link goes inside the string, not as a URL item (see
/// `InviteShareLink`: some apps posted a URL's archived property list).
@MainActor
enum InviteShare {
  static func text(name: String) -> String { "Yo me on Yo: \(Invite.link(for: name).absoluteString)" }

  static func present(name: String, completion: @escaping @MainActor () -> Void = {}) async {
    var items: [Any] = [text(name: name)]
    if let image = await ContactCodeExport.render(name: name)?.image { items.insert(InvitePictureItem(name: name, image: image), at: 0) }
    Share.present(items) { _ in completion() }
  }
}

/// The card for the share sheet, with the sheet's header ("NAME wants to Yo" and the card).
private final class InvitePictureItem: NSObject, UIActivityItemSource {
  let name: String
  let image: UIImage

  init(name: String, image: UIImage) {
    self.name = name
    self.image = image
  }

  func activityViewControllerPlaceholderItem(_ controller: UIActivityViewController) -> Any { image }

  func activityViewController(_ controller: UIActivityViewController, itemForActivityType type: UIActivity.ActivityType?)
    -> Any?
  { image }

  func activityViewController(_ controller: UIActivityViewController, subjectForActivityType type: UIActivity.ActivityType?)
    -> String
  { "\(name) wants to Yo" }

  func activityViewControllerLinkMetadata(_ controller: UIActivityViewController) -> LPLinkMetadata? {
    let metadata = LPLinkMetadata()
    metadata.title = "\(name) wants to Yo"
    metadata.imageProvider = NSItemProvider(object: image)
    metadata.iconProvider = NSItemProvider(object: image)
    return metadata
  }
}

/// The code as the page draws it: its modules and whether its centre is cleared for "Yo".
struct InviteCode: Equatable {
  let matrix: QRMatrix
  let cleared: Bool
}

@MainActor
private struct ContactCodeExport {
  let image: UIImage
  let code: InviteCode
  private static var cached: (name: String, export: Self)?

  static func cachedCode(name: String) -> InviteCode? {
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

  /// The picture is read back before it is used; should "Yo" in the centre keep a code from reading, it goes without.
  private static func make(name: String) async -> Self? {
    let url = Invite.link(for: name).absoluteString
    guard let matrix = await Task.detached(priority: .utility, operation: { RecoveryQR.contactMatrix(text: url) }).value
    else { return nil }
    for cleared in [true, false] {
      let code = InviteCode(matrix: matrix, cleared: cleared)
      let renderer = ImageRenderer(content: InvitePicture(name: name, code: code))
      renderer.scale = 3
      renderer.isOpaque = true
      guard let image = renderer.uiImage else { continue }
      if await Task.detached(priority: .utility, operation: { RecoveryQR.read(image) == url }).value {
        return Self(image: image, code: code)
      }
    }
    return nil
  }
}

/// The page's look, shared with the picture so the two never drift: the name's row, "wants to Yo", the code's card.
@MainActor
enum InviteLayout {
  /// The website's palette (the bars' colours without wisteria), picked by the name's bytes: a name keeps its colour.
  static func color(_ name: String) -> Color { Color.rows[name.utf8.reduce(0) { $0 + Int($1) } % 7] }

  /// The name as large as its row allows, measured once in Montserrat at 100 pt (as justyo.co/NAME does).
  static func nameSize(_ name: String, width: CGFloat, max: CGFloat) -> CGFloat {
    let font = UIFont(name: "Montserrat-Bold", size: 100) ?? .boldSystemFont(ofSize: 100)
    let measured = (name as NSString).size(withAttributes: [.font: font]).width
    guard measured > 0 else { return max }
    return Swift.max(14, Swift.min(max, (width * 100 / measured).rounded(.down)))
  }

  static func wantsSize(nameSize: CGFloat) -> CGFloat { Swift.min(28, Swift.max(20, (nameSize * 0.34).rounded())) }

  /// The white around the code, 18 pt for a 260 pt code.
  static func padding(side: CGFloat) -> CGFloat { (side * 18 / 260).rounded() }
}

/// The name as a full-width coloured row, white Montserrat on one line.
struct InviteNameRow: View {
  let name: String
  let size: CGFloat
  let inset: CGFloat

  var body: some View {
    Text(name)
      .font(.custom("Montserrat-Bold", fixedSize: size))
      .lineLimit(1)
      .minimumScaleFactor(0.5)
      .foregroundStyle(.white)
      .padding(.horizontal, 16)
      .padding(.vertical, inset)
      .frame(maxWidth: .infinity)
      .background(InviteLayout.color(name))
  }
}

struct InviteWants: View {
  let size: CGFloat

  var body: some View {
    Text("wants to Yo")
      .font(.custom("Montserrat-Bold", fixedSize: size))
      .lineLimit(1)
      .foregroundStyle(.white)
  }
}

/// Square amethyst modules and finder eyes on a white square card, "Yo" in the cleared centre (justyo.co/NAME's).
struct QRCard: View {
  let code: InviteCode
  let side: CGFloat

  var body: some View {
    Canvas { context, size in
      let matrix = code.matrix
      let count = matrix.count
      let module = size.width / CGFloat(count)
      let mid = Double(count - 1) / 2
      func eye(_ row: Int, _ column: Int) -> Bool {
        (row < 7 && column < 7) || (row < 7 && column >= count - 7) || (row >= count - 7 && column < 7)
      }
      func clear(_ row: Int, _ column: Int) -> Bool {
        code.cleared && abs(Double(row) - mid) < Double(count) * 0.13 && abs(Double(column) - mid) < Double(count) * 0.19
      }
      var fill = Path()
      for row in 0..<count {
        for column in 0..<count where matrix[row, column] && !eye(row, column) && !clear(row, column) {
          fill.addRect(CGRect(
            x: CGFloat(column) * module + module * 0.06, y: CGFloat(row) * module + module * 0.06,
            width: module * 0.88, height: module * 0.88))
        }
      }
      var rings = Path()
      for (row, column) in [(0, 0), (0, count - 7), (count - 7, 0)] {
        let x = CGFloat(column) * module
        let y = CGFloat(row) * module
        rings.addRect(CGRect(x: x + module / 2, y: y + module / 2, width: module * 6, height: module * 6))
        fill.addRect(CGRect(x: x + module * 2, y: y + module * 2, width: module * 3, height: module * 3))
      }
      context.fill(fill, with: .color(.amethyst))
      context.stroke(rings, with: .color(.amethyst), lineWidth: module)
      if code.cleared {
        context.draw(
          Text("Yo").font(.custom("Montserrat-Bold", fixedSize: module * 6.4)).foregroundStyle(Color.amethyst),
          at: CGPoint(x: size.width / 2, y: size.height / 2))
      }
    }
    .frame(width: side, height: side)
    .padding(InviteLayout.padding(side: side))
    .background(.white)
    .environment(\.layoutDirection, .leftToRight)
  }
}

/// The shared picture: the page without its buttons, on amethyst with a little margin; 843 × 1200 at 3×.
@MainActor
private struct InvitePicture: View {
  let name: String
  let code: InviteCode

  var body: some View {
    let size = InviteLayout.nameSize(name, width: 281 - 32, max: 56)
    VStack(spacing: 0) {
      InviteNameRow(name: name, size: size, inset: 10)
      InviteWants(size: InviteLayout.wantsSize(nameSize: size)).padding(.top, 10)
      QRCard(code: code, side: 200).padding(.top, 18)
    }
    .frame(width: 281, height: 400)
    .background(Color.amethyst)
    .environment(\.layoutDirection, .leftToRight)
  }
}

@MainActor
struct YourCodePage: View {
  let name: String
  @State private var code: InviteCode?
  @State private var loaded = false
  @State private var sharing = false
  @State private var copyCount = 0
  @State private var copied = false

  init(name: String) {
    self.name = name
    _code = State(initialValue: ContactCodeExport.cachedCode(name: name))
  }

  static func prepare(name: String) {
    Task { _ = await ContactCodeExport.render(name: name) }
  }

  private func share() {
    guard !sharing else { return }
    sharing = true
    Task { await InviteShare.present(name: name) { sharing = false } }
  }

  var body: some View {
    GeometryReader { proxy in
      let width = proxy.size.width
      let height = proxy.size.height
      // Everything on one screen, the iPhone SE's too: the name at most an eighth of the height, the code what is left.
      let gap = min(36, max(14, height * 0.04))
      let nameSize = InviteLayout.nameSize(name, width: width - 32, max: min(100, height * 0.13))
      let wantsSize = InviteLayout.wantsSize(nameSize: nameSize)
      let around = nameSize * 1.22 + gap + gap * 0.5 + wantsSize * 1.22 + gap * 0.8 + gap + 50 + 16
      let side = max(120, min(260, (width - 40) * 260 / 296, (height - around) * 260 / 296))
      let cardWidth = side + 2 * InviteLayout.padding(side: side)
      VStack(spacing: 0) {
        InviteNameRow(name: name, size: nameSize, inset: gap * 0.5)
        InviteWants(size: wantsSize).padding(.top, gap * 0.5)
        Group {
          if let code {
            Button(action: share) {
              QRCard(code: code, side: side)
            }
            .buttonStyle(QRPressStyle())
            .disabled(sharing)
            .accessibilityLabel("Share QR code as picture")
          } else if !loaded {
            ProgressView().tint(.white).frame(width: cardWidth, height: cardWidth)
          } else {
            ContentUnavailableView(
              "Unable to Create Code", systemImage: "qrcode",
              description: Text("Go back and try again."))
            .frame(height: cardWidth)
          }
        }
        .padding(.top, gap * 0.8)
        HStack {
          Button {
            let url = Invite.link(for: name)
            UIPasteboard.general.items = [[UTType.url.identifier: url, UTType.utf8PlainText.identifier: url.absoluteString]]
            copied = true
            copyCount += 1
          } label: {
            Label(copied ? "Copied" : "Link", systemImage: copied ? "checkmark" : "doc.on.doc")
              .frame(maxWidth: .infinity)
          }
          Button(action: share) {
            Label("Share", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity)
          }
          .disabled(sharing)
        }
        .buttonStyle(.glassProminent)
        .tint(.white)
        .foregroundStyle(.black)
        .controlSize(.large)
        .frame(width: cardWidth)
        .padding(.top, gap)
      }
      .foregroundStyle(.white)
      .padding(.vertical, 8)
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
      code = await ContactCodeExport.render(name: name)?.code
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
