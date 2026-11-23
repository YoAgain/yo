// The brand guide (yo-lore/press-kit): Flat UI colours, white Montserrat Bold; glass only for what floats over the list.
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
    0xF1C40F,  // sunflower
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
  static let rowHeight: CGFloat = 89  // the original's (YoContextView.m)
  static let rowFont: CGFloat = 36  // the original's (YOCell.m)
  /// A group's senders, two lines in one bar: the second, fixed size (Android's too: 24 of 36 in an 89 bar).
  static let rowSmallFont: CGFloat = 24
  /// The title: as tall as the glass buttons beside it, its "Yo" as large as fits.
  static let titleHeight: CGFloat = 44
  static let titleFont: CGFloat = 30
}

/// The bars' sizes on a wider screen (iPad, a Mac window): FriendList measures and hands them down.
struct RowMetrics: Equatable {
  var height: CGFloat
  var font: CGFloat
  var smallFont: CGFloat
  static let base = RowMetrics(height: Metrics.rowHeight, font: Metrics.rowFont, smallFont: Metrics.rowSmallFont)

  /// Phone widths keep the original sizes; wider windows grow up to 1.35×.
  static func scale(width: CGFloat) -> CGFloat { min(1.35, 1 + max(0, width - 440) * 0.001) }

  var fontScale: CGFloat { font / Self.base.font }
  var baseHeight: CGFloat { Self.base.height * fontScale }
}

extension EnvironmentValues {
  @Entry var rowMetrics: RowMetrics = .base
}
