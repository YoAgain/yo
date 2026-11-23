import SwiftUI

struct BarActivity {
  let text: String
  let icon: String
  let accessibility: String
}

/// A bar that slides sideways under a horizontal swipe and shows its actions underneath, as Mail's list does
/// (`.swipeActions` exists only inside `List`): a friend's Block and Trash, a group's Leave alone (no group blocking).
/// A vertical move stays the list's scroll.
struct SwipeActions<Content: View>: View {
  @Environment(\.rowMetrics) private var rowMetrics
  @Binding var open: Bool
  let enabled: Bool
  var request: ((Bool) -> Void)? = nil
  let mirrored: Bool
  let activity: BarActivity
  let began: () -> Void
  let block: (() -> Void)?
  let removeTitle: String
  let remove: () -> Void
  @ViewBuilder let content: Content
  @State private var dragged: CGFloat?

  private var button: CGFloat { 128 * rowMetrics.fontScale }  // room for the row-sized words with air on both sides
  private var width: CGFloat { (block == nil ? 1 : 2) * button }

  var body: some View {
    let width = width
    let resting: CGFloat = enabled && open ? -width : 0
    let moved = enabled ? resting + (dragged ?? 0) : 0
    let direction: CGFloat = mirrored ? -1 : 1
    let revealing = moved > 0
    let offset: CGFloat = request != nil || revealing ? moved
      : moved < -width ? -width + (moved + width) / 3 : moved
    content
      .overlay {
        // Open: a tap on the bar closes it (the bar does nothing else then).
        if open { Color.clear.contentShape(.rect).onTapGesture { close() } }
      }
      .offset(x: direction * offset)
      .background(alignment: mirrored ? .leading : .trailing) {
        if request != nil, offset != 0 {
          Text(offset > 0 ? "No" : "Yo")
            .font(.yo(rowMetrics.font))
            .foregroundStyle(.white)
            .frame(width: abs(offset), height: rowMetrics.height)
            .background(Color(hex: offset > 0 ? 0xE74C3C : 0x2ECC71))
            .frame(maxWidth: .infinity, alignment: (offset > 0) != mirrored ? .leading : .trailing)
        } else if offset > 0 {
          Label(activity.text, systemImage: activity.icon)
            .font(.yo(rowMetrics.font))
            .foregroundStyle(.white)
            .lineLimit(1)
            .minimumScaleFactor(0.5)
            .padding(.horizontal, 24)
            .frame(width: max(280, offset), height: rowMetrics.height)
            .background(Color.amethyst)
            .frame(maxWidth: .infinity, maxHeight: .infinity,
              alignment: mirrored ? .trailing : .leading)
            .accessibilityLabel(activity.accessibility)
            .allowsHitTesting(false)
            .transition(.identity)
        } else if offset < 0 {
          HStack(spacing: 0) {
            if mirrored { action(removeTitle, .alizarin) { remove() } }
            if let block { action("Block", Color(.systemGray)) { close(); block() } }  // not a bar's own colour
            if !mirrored { action(removeTitle, .alizarin) { remove() } }
          }
          .frame(width: max(width, -offset))
          .transition(.identity)
        }
      }
      .gesture(
        HorizontalPan(enabled: enabled) { x in
          if dragged == nil { began() }
          dragged = x * direction
        } ended: { x, velocity in
          let end = resting + x * direction
          let speed = velocity * direction
          if let request, abs(end) > 100 || (abs(end) > 20 && abs(speed) > 700 && end * speed > 0) {
            request(end < 0)
          }
          withAnimation(.spring(duration: 0.35, bounce: 0.2)) {
            open = request == nil && end < 0 && (speed < -400 || (speed < 400 && end < -width / 2))
            dragged = nil
          }
        }
      )
      .onChange(of: enabled) { _, on in
        // Only an open bar closes: an animated close on every bar put the camera switch itself in an animation.
        if !on && open { close() }
      }
      .accessibilityActions {
        if enabled && request == nil {
          if let block { Button("Block") { block() } }
          Button(removeTitle) { remove() }
        }
      }
  }

  private func close() {
    withAnimation(.snappy(duration: 0.3)) { open = false }
  }

  private func action(_ title: String, _ color: Color, _ run: @escaping () -> Void) -> some View {
    Button(action: run) {
      Text(title).font(.yo(rowMetrics.font))
      .lineLimit(1).minimumScaleFactor(0.6)
      .foregroundStyle(.white)
      .padding(.horizontal, 16)
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(color)
    }
    .buttonStyle(.plain)
  }
}
