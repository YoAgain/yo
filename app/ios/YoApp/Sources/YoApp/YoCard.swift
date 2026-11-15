// The one place for pictures Yo makes itself: a square in Yo's look (Amethyst, white Montserrat Bold, flat), "Yo" over
// it as its heading and one sentence in the middle whose names stand out: people in Sunflower, groups underlined, so
// each reads as a piece of the sentence. Rendered to JPEG and sent as any photo.
// First use: what happened in a group (GroupNotice).
import SwiftUI

struct YoCard: View {
  /// A piece of the sentence: plain words, a person's name, or a group's.
  enum Piece: ExpressibleByStringLiteral {
    case text(String), user(String), group(String)
    init(stringLiteral value: String) { self = .text(value) }
  }

  /// The sentence in the middle ("LEO added MIA to FAMILY").
  let sentence: [Piece]

  static let side: CGFloat = 1080
  static let userColor = Color(hex: 0xF1C40F)  // sunflower

  /// The sentence as one text: people in Sunflower, a group's name underlined.
  private var text: Text {
    sentence.reduce(Text("")) { all, piece in
      switch piece {
      case .text(let words): Text("\(all)\(words)")
      case .user(let name): Text("\(all)\(Text(name).foregroundStyle(Self.userColor))")
      case .group(let name): Text("\(all)\(Text(name).underline())")
      }
    }
  }

  var body: some View {
    text
      .font(.custom("Montserrat-Bold", size: 116))
      .multilineTextAlignment(.center)
      .lineLimit(5)
      .minimumScaleFactor(0.3)
      .foregroundStyle(.white)
      .padding(.horizontal, 90)
      .padding(.top, 200)
      .padding(.bottom, 90)
      .frame(width: Self.side, height: Self.side)
      .background(Color.amethyst)
      .overlay(alignment: .top) {
        Text("Yo").font(.custom("Montserrat-Bold", size: 96)).foregroundStyle(.white).padding(.top, 70)
      }
  }

  /// The card as JPEG, ready to send as a photo; nil if it could not be drawn.
  @MainActor func jpeg(quality: CGFloat = 0.9) -> Data? {
    let renderer = ImageRenderer(content: self)
    renderer.scale = 1
    return renderer.uiImage?.jpegData(compressionQuality: quality)
  }
}
