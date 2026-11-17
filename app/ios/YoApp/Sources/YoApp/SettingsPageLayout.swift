import SwiftUI

/// Keeps eager page geometry steady while the sheet clips it during growth.
/// Placement does not enlarge the navigation host's viewport: scroll content must be non-lazy.
struct SettingsPageLayout<Content: View>: View {
  let height: CGFloat?
  let content: Content

  init(height: CGFloat?, @ViewBuilder content: () -> Content) {
    self.height = height
    self.content = content()
  }

  var body: some View {
    FullHeightLayout(height: height) { content }
  }
}

private struct FullHeightLayout: Layout {
  let height: CGFloat?

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    proposal.replacingUnspecifiedDimensions()
  }

  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    for subview in subviews {
      subview.place(
        at: bounds.origin, anchor: .topLeading,
        proposal: ProposedViewSize(width: bounds.width, height: height ?? bounds.height))
    }
  }
}

extension EnvironmentValues {
  @Entry var eagerSettingsRows = false
}

/// Inset grouped content without List/Form's viewport-dependent cell creation.
struct SettingsForm<Content: View>: View {
  var profileFirst = false
  var measuredHeight: (CGFloat) -> Void = { _ in }
  @ViewBuilder var content: () -> Content

  var body: some View {
    ScrollView {
      VStack(spacing: 28) {
        Group(sections: content()) { sections in
          ForEach(Array(sections.enumerated()), id: \.element.id) { index, section in
            VStack(alignment: .leading, spacing: 8) {
              if !section.header.isEmpty {
                section.header
                  .font(.footnote)
                  .textCase(nil)
                  .foregroundStyle(.secondary)
                  .padding(.horizontal, 16)
              }
              if profileFirst && index == 0 {
                section.content
              } else {
                VStack(spacing: 0) {
                  ForEach(Array(section.content.enumerated()), id: \.element.id) { row, view in
                    if row > 0 { Divider().padding(.horizontal, 16) }
                    view
                      .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                      .padding(.horizontal, 16)
                      .padding(.vertical, 5)
                  }
                }
                .background(Color(.secondarySystemGroupedBackground), in: .rect(cornerRadius: 26))
              }
              if !section.footer.isEmpty {
                section.footer
                  .font(.footnote)
                  .foregroundStyle(.secondary)
                  .padding(.horizontal, 16)
              }
            }
          }
        }
      }
      .padding(.horizontal, 20)
      .padding(.top, profileFirst ? 0 : 20)
      .padding(.bottom, 20)
      .fixedSize(horizontal: false, vertical: true)
      .onGeometryChange(for: CGFloat.self) {
        $0.size.height
      } action: {
        measuredHeight($0)
      }
    }
    .background(Color(.systemGroupedBackground))
    .buttonStyle(.plain)
    .environment(\.eagerSettingsRows, true)
    .scrollBounceBehavior(.basedOnSize)
  }
}

/// NavigationLink supplies its arrow only inside a List; eager cards supply the same accessory.
struct SettingsNavigationLabel<Content: View>: View {
  @ViewBuilder var content: () -> Content

  var body: some View {
    HStack {
      content().frame(maxWidth: .infinity, alignment: .leading)
      Chevron()
    }
    .frame(minHeight: 44)
    .foregroundStyle(Color(.label))
    .contentShape(Rectangle())
  }
}
