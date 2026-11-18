// A sheet as tall as its content: one detent, nothing to pull up or down. Only content taller than 90 % of the
// screen falls back to the full height and scrolls.
import SwiftUI
import UIKit

extension View {
  /// On the sheet's root, outside its NavigationStack: one detent from the height its page reports.
  func fittedSheet() -> some View { modifier(FittedSheet()) }

  /// On the page that sizes the sheet (inside the stack): its content's height, the bar and the bottom inset added.
  func sheetFit(contentHeight: CGFloat) -> some View { modifier(SheetFit(contentHeight: contentHeight)) }

  /// On a page pushed inside a fitted sheet that needs the full height while it is shown.
  func sheetGrows() -> some View { modifier(SheetGrows()) }
}

/// What a fitted sheet's pages tell its root: the height they need, or that a pushed page wants it all.
private struct FittedSheetHandle {
  var fit: (CGFloat) -> Void = { _ in }
  var grow: (Bool) -> Void = { _ in }
}

extension EnvironmentValues {
  @Entry fileprivate var fittedSheet = FittedSheetHandle()
}

private struct FittedSheet: ViewModifier {
  @State private var height: CGFloat = 0
  @State private var grown = 0

  private var detent: PresentationDetent {
    let screen =
      UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.screen.bounds.height }.first ?? 800
    return grown > 0 || height <= 0 || height > screen * 0.9 ? .large : .height(height)
  }

  func body(content: Content) -> some View {
    content
      .environment(
        \.fittedSheet,
        FittedSheetHandle(
          fit: { new in if abs(new - height) > 1 { height = new } },
          grow: { on in grown = max(0, grown + (on ? 1 : -1)) }))
      .scrollBounceBehavior(.basedOnSize)
      .presentationDetents([detent])
      .presentationDragIndicator(.hidden)
  }
}

private struct SheetFit: ViewModifier {
  let contentHeight: CGFloat
  @Environment(\.fittedSheet) private var sheet
  @State private var insets: CGFloat = 0

  func body(content: Content) -> some View {
    content
      .onGeometryChange(for: CGFloat.self) { $0.safeAreaInsets.top + $0.safeAreaInsets.bottom } action: {
        insets = $0
      }
      .onChange(of: contentHeight + insets, initial: true) { _, total in
        if contentHeight > 0 { sheet.fit(total) }
      }
  }
}

private struct SheetGrows: ViewModifier {
  @Environment(\.fittedSheet) private var sheet

  func body(content: Content) -> some View {
    content
      .onAppear { sheet.grow(true) }
      .onDisappear { sheet.grow(false) }
  }
}
