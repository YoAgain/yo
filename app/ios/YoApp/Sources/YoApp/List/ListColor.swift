import SwiftUI

/// Bounds come from the actual row subtree: scrolling, expansion and reordering have one layout.
struct ListColorEntry {
  let id: UUID
  let bounds: Anchor<CGRect>
  let color: Color
  let opacity: Double
  /// Under the finger in camera mode: the picture shows through it clearer.
  var pressed = false
  var clips: [Anchor<CGRect>] = []
}

struct ListColorKey: PreferenceKey {
  static var defaultValue: [ListColorEntry] { [] }
  static func reduce(value: inout [ListColorEntry], nextValue: () -> [ListColorEntry]) {
    value.append(contentsOf: nextValue())
  }
}

/// Carry local clipping into the shared colour plane during expanding action rows.
struct ListColorClip: ViewModifier {
  func body(content: Content) -> some View {
    content.transformAnchorPreference(key: ListColorKey.self, value: .bounds) { entries, bounds in
      for index in entries.indices { entries[index].clips.append(bounds) }
    }
  }
}

extension EnvironmentValues {
  @Entry var sharedListColors = false
}

struct ListColor: ViewModifier {
  @Environment(\.sharedListColors) private var shared
  @State private var id = UUID()
  let color: Color
  var opacity: Double = 1
  var pressed = false

  func body(content: Content) -> some View {
    if shared {
      content.anchorPreference(key: ListColorKey.self, value: .bounds) {
        [ListColorEntry(id: id, bounds: $0, color: color, opacity: opacity, pressed: pressed)]
      }
    } else {
      content.background(color.opacity(pressed ? opacity * 0.45 : opacity))
    }
  }
}

/// Only the shared preview is clipped; the row colours and foreground never have wipe masks.
struct CameraWipeShape: Shape {
  var progress: Double
  var left: Bool
  var animatableData: Double {
    get { progress }
    set { progress = newValue }
  }
  func path(in rect: CGRect) -> Path {
    let width = rect.width * progress
    return Path(CGRect(x: left ? 0 : rect.width - width, y: 0,
                       width: max(0, width), height: rect.height))
  }
}
