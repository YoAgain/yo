import SwiftUI

extension Color {
  private init(rgb: Int) {
    self.init(.sRGB, red: Double(rgb >> 16 & 255) / 255, green: Double(rgb >> 8 & 255) / 255,
              blue: Double(rgb & 255) / 255, opacity: 1)
  }

  static let amethyst = Color(rgb: 0x9B59B6)
  static let rows = [0x1ABC9C, 0x2ECC71, 0x3498DB, 0x34495E, 0x16A085, 0xF1C40F, 0x2980B9, 0x8E44AD]
    .map { Color(rgb: $0) }

  static func row(_ index: Int) -> Color { rows[(index % rows.count + rows.count) % rows.count] }
}

extension Font {
  static func yo(_ size: CGFloat) -> Font { .custom("Montserrat-Bold", fixedSize: size) }
}
