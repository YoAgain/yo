#if YO_DEV
  import SwiftUI
  import UIKit

  /// Dev only: whether HDR text and colour render brighter than white on this iPhone. Left SDR, right HDR.
  struct HDRTestPage: View {
    var body: some View {
      HStack(spacing: 0) {
        half(title: "SDR", hdr: false)
        half(title: "HDR", hdr: true)
      }
      .ignoresSafeArea()
      .navigationTitle("HDR Test")
      .navigationBarTitleDisplayMode(.inline)
    }

    private func half(title: String, hdr: Bool) -> some View {
      VStack(spacing: 24) {
        Text(title).font(.headline).foregroundStyle(.gray)
        // SwiftUI (iOS 26): a colour pushed past white.
        Text("Yo!!!!!")
          .font(.custom("Montserrat-Bold", size: 44))
          .foregroundStyle(hdr ? Color.white.exposureAdjust(2) : Color.white)
        Text("Yo!!!!!")
          .font(.custom("Montserrat-Bold", size: 44))
          .foregroundStyle(hdr ? Color(red: 0.82, green: 0.65, blue: 0.92).exposureAdjust(2) : Color(red: 0.82, green: 0.65, blue: 0.92))
        // A plain patch, the easiest to compare.
        Rectangle()
          .fill(hdr ? Color.white.exposureAdjust(2) : Color.white)
          .frame(width: 120, height: 120)
        // UIKit: a colour with linear exposure (iOS 26).
        UIKitPatch(hdr: hdr).frame(width: 120, height: 60)
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(Color(white: 0.5))
      .allowedDynamicRange(hdr ? .high : .standard)
    }
  }

  private struct UIKitPatch: UIViewRepresentable {
    let hdr: Bool
    func makeUIView(context: Context) -> UIView {
      let view = UIView()
      view.backgroundColor = hdr ? UIColor(red: 1, green: 1, blue: 1, alpha: 1, linearExposure: 2) : .white
      return view
    }
    func updateUIView(_ view: UIView, context: Context) {}
  }
#endif
