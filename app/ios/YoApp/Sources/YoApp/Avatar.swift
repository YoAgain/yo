import SwiftUI
import UIKit
import YoCore

/// A picture for someone without one, as Signal makes it: the first letter of the name on a colour of the list's,
/// the same for the same name on every phone.
struct Avatar: View {
  let name: String
  var size: CGFloat = 44

  var body: some View {
    let seed = Username.skeleton(name).unicodeScalars.reduce(0) { $0 &+ Int($1.value) }
    Circle()
      .fill(Color.row(seed))
      .frame(width: size, height: size)
      .overlay {
        Text(name.first.map { String($0).uppercased() } ?? "?")
          .font(.yo(size * 0.55))
          .foregroundStyle(.white)
      }
      .accessibilityHidden(true)
  }
}
