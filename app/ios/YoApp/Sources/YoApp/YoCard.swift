// The one place for pictures Yo makes itself: a square in Yo's look (Amethyst, white Montserrat Bold, flat, the "Yo"
// mark), with a title, an optional line over it and an optional SF Symbol. Rendered to JPEG and sent as any photo.
// First use: what happened in a group (GroupNotice).
import SwiftUI

struct YoCard: View {
  /// The large words in the middle ("LEO added MIA").
  let title: String
  /// Small over them ("FAMILY"), with `symbol` before it.
  var subtitle: String? = nil
  var symbol: String? = nil

  static let side: CGFloat = 1080

  var body: some View {
    VStack(spacing: 48) {
      if subtitle != nil || symbol != nil {
        HStack(spacing: 20) {
          if let symbol { Image(systemName: symbol).font(.system(size: 54, weight: .bold)) }
          if let subtitle {
            Text(subtitle).font(.custom("Montserrat-Bold", size: 60)).lineLimit(1).minimumScaleFactor(0.4)
          }
        }
        .opacity(0.8)
      }
      Text(title)
        .font(.custom("Montserrat-Bold", size: 120))
        .multilineTextAlignment(.center)
        .lineLimit(4)
        .minimumScaleFactor(0.3)
    }
    .foregroundStyle(.white)
    .padding(90)
    .frame(width: Self.side, height: Self.side)
    .background(Color.amethyst)
    .overlay(alignment: .bottom) {
      Text("Yo").font(.custom("Montserrat-Bold", size: 64)).foregroundStyle(.white).padding(.bottom, 56)
    }
  }

  /// The card as JPEG, ready to send as a photo; nil if it could not be drawn.
  @MainActor func jpeg(quality: CGFloat = 0.9) -> Data? {
    let renderer = ImageRenderer(content: self)
    renderer.scale = 1
    return renderer.uiImage?.jpegData(compressionQuality: quality)
  }
}
