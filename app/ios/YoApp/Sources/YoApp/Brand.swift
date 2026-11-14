// The look of the brand guide (yo-lore/press-kit): Flat UI colours, white Montserrat Bold, upper and lower case as the
// name "Yo". The original's unbroken list of bars from edge to edge; glass only for what floats over it.
import SwiftUI

/// Usable as `Color.amethyst` and as a shape style (`.background(.amethyst)`), defined once.
extension ShapeStyle where Self == Color {
  static var amethyst: Color { Color(hex: 0x9B59B6) }
  static var alizarin: Color { Color(hex: 0xE74C3C) }
}

extension Color {
  /// The bars' colours, in the order of the 2015 press screens: wild on purpose, the brand's mark.
  static let rows: [Color] = [
    0x1ABC9C,  // turquoise
    0x2ECC71,  // emerald
    0x3498DB,  // peter river
    0x34495E,  // wet asphalt
    0x16A085,  // green sea
    0xE67E22,  // carrot: white on sunflower (1.7:1) could not be read
    0x2980B9,  // belize hole
    0x8E44AD,  // wisteria
  ].map(Color.init(hex:))

  static func row(_ index: Int) -> Color { rows[index % rows.count] }

  init(hex: UInt32) {
    self.init(
      red: Double(hex >> 16 & 0xFF) / 255,
      green: Double(hex >> 8 & 0xFF) / 255,
      blue: Double(hex & 0xFF) / 255
    )
  }
}

extension Font {
  /// The brand's typeface, bundled and declared in Info.plist (UIAppFonts).
  static func yo(_ size: CGFloat, relativeTo style: Font.TextStyle = .title) -> Font {
    .custom("Montserrat-Bold", size: size, relativeTo: style)
  }
}

enum Metrics {
  /// The friends: one unbroken list of coloured bars from edge to edge, as the original's.
  static let rowHeight: CGFloat = 76
  static let rowFont: CGFloat = 30
  /// The title: as tall as the glass buttons beside it, its "Yo" as large as fits.
  static let titleHeight: CGFloat = 44
  static let titleFont: CGFloat = 30
}

