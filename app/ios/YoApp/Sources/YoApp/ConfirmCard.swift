import SwiftUI

/// A short question as a small card from the bottom (like an action sheet, as Signal shows it): the title, an
/// optional line, the action, Cancel. Used where iOS 26's confirmation dialog would float as a popover.
struct ConfirmCard: View {
  let title: String
  let message: String?
  let action: String
  let cancel: String
  let run: () -> Void
  let dismiss: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 18) {
      VStack(alignment: .leading, spacing: 6) {
        Text(title).font(.headline)
        if let message, !message.isEmpty {
          Text(message).font(.subheadline).foregroundStyle(.secondary)
        }
      }
      .padding(.horizontal, 8)
      VStack(spacing: 10) {
        button(action, color: .red) {
          dismiss()
          run()
        }
        button(cancel, color: Color(.label)) { dismiss() }
      }
    }
    .padding(.horizontal, 20)
    .padding(.top, 26)
    // Down to the screen's bottom edge, clear of the home indicator, like the system's action sheet.
    .padding(.bottom, 34)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(Color(.systemBackground), in: ConcentricRectangle(corners: .concentric, isUniform: true))
    .padding(.horizontal, 6)
    .padding(.bottom, 6)
  }

  private func button(_ title: String, color: Color, run: @escaping () -> Void) -> some View {
    Button(action: run) {
      Text(title)
        .font(.body.weight(.medium))
        .foregroundStyle(color)
        .frame(maxWidth: .infinity, minHeight: 52)
        .background(Color(.tertiarySystemFill), in: .capsule)
        .contentShape(.capsule)
    }
    .buttonStyle(.plain)
  }
}

extension View {
  /// Asks with a ConfirmCard while `item` is set; `run` gets the item it was asked about. Drawn inside the
  /// current screen or sheet (an overlay sliding up from the bottom), so the sheet underneath stays.
  func confirmCard<Item>(
    _ item: Binding<Item?>, title: @escaping (Item) -> String, message: ((Item) -> String)? = nil,
    action: @escaping (Item) -> String, cancel: String = "Cancel", run: @escaping (Item) -> Void
  ) -> some View {
    overlay {
      ZStack(alignment: .bottom) {
        if let value = item.wrappedValue {
          Color.black.opacity(0.25)
            .ignoresSafeArea()
            .onTapGesture { withAnimation(.smooth(duration: 0.3)) { item.wrappedValue = nil } }
            .transition(.opacity)
          ConfirmCard(
            title: title(value), message: message?(value), action: action(value), cancel: cancel,
            run: { run(value) },
            dismiss: { withAnimation(.smooth(duration: 0.3)) { item.wrappedValue = nil } })
          .transition(.move(edge: .bottom))
        }
      }
      .ignoresSafeArea(.container, edges: .bottom)
      .animation(.smooth(duration: 0.3), value: item.wrappedValue != nil)
    }
  }
}
