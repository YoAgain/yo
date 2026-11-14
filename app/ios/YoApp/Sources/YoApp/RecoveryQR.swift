// The recovery words as a QR code (on the card in Photos and on screen) and back: from a picture in Photos, or the
// camera. The code holds the 12 words, space-separated, nothing else.
import CoreImage
import CoreImage.CIFilterBuiltins
import PhotosUI
import SwiftUI
import VisionKit

enum RecoveryQR {
  /// The words as a QR code: black on white, sharp at any size.
  static func image(_ words: [String]) -> UIImage? { image(text: words.joined(separator: " ")) }

  /// Any text as a QR code: black on white, sharp at any size.
  static func image(text: String) -> UIImage? {
    let filter = CIFilter.qrCodeGenerator()
    filter.message = Data(text.utf8)
    filter.correctionLevel = "M"
    guard let code = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 12, y: 12)),
      let cg = CIContext().createCGImage(code, from: code.extent)
    else { return nil }
    return UIImage(cgImage: cg)
  }

  /// The text of the first QR code in a picture.
  static func read(_ image: UIImage) -> String? {
    guard let ci = CIImage(image: image),
      let detector = CIDetector(ofType: CIDetectorTypeQRCode, context: nil, options: [CIDetectorAccuracy: CIDetectorAccuracyHigh])
    else { return nil }
    return detector.features(in: ci).compactMap { ($0 as? CIQRCodeFeature)?.messageString }.first
  }
}

/// The QR code of the words, on a white tile.
struct RecoveryQRView: View {
  let words: [String]
  var size: CGFloat = 140

  var body: some View {
    if let image = RecoveryQR.image(words) {
      Image(uiImage: image)
        .interpolation(.none)
        .resizable()
        .scaledToFit()
        .frame(width: size, height: size)
        .padding(10)
        .background(.white, in: .rect(cornerRadius: 14))
        .accessibilityLabel("QR code for your recovery key")
    }
  }
}

/// The camera, reading the first QR code it sees (VisionKit's scanner).
struct QRScanner: UIViewControllerRepresentable {
  let found: (String) -> Void

  static var isAvailable: Bool { DataScannerViewController.isSupported && DataScannerViewController.isAvailable }

  func makeUIViewController(context: Context) -> DataScannerViewController {
    let scanner = DataScannerViewController(
      recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced, isHighlightingEnabled: true)
    scanner.delegate = context.coordinator
    try? scanner.startScanning()
    return scanner
  }

  func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {}

  func makeCoordinator() -> Coordinator { Coordinator(found: found) }

  final class Coordinator: NSObject, DataScannerViewControllerDelegate {
    let found: (String) -> Void
    private var done = false

    init(found: @escaping (String) -> Void) { self.found = found }

    func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
      for item in items {
        if case .barcode(let code) = item, let text = code.payloadStringValue, !done {
          done = true
          found(text)
        }
      }
    }
  }
}

/// The recovery key on screen, frosted until tapped (someone looking over a shoulder sees nothing); tapped again, it
/// frosts over again.
struct FrostedKey: View {
  let words: [String]
  var qrSize: CGFloat = 140
  @State private var shown = false
  @Environment(\.scenePhase) private var phase

  var body: some View {
    VStack(spacing: 16) {
      RecoveryQRView(words: words, size: qrSize)
      WordGrid(words: words)
    }
    .blur(radius: shown ? 0 : 14)
    .overlay {
      if !shown {
        Label("Tap to Show", systemImage: "eye")
          .font(.headline)
          .padding(.horizontal, 18)
          .padding(.vertical, 10)
          .glassEffect()
          .transition(.opacity)
      }
    }
    .contentShape(.rect)
    .onTapGesture { shown.toggle() }
    .animation(.smooth(duration: 0.25), value: shown)
    .accessibilityElement(children: shown ? .contain : .ignore)
    .accessibilityLabel(shown ? "" : "Recovery key, hidden")
    .accessibilityAddTraits(.isButton)
    .accessibilityAction { shown.toggle() }
    // Hidden again when Yo leaves the screen: not in the app switcher's picture, not open on coming back.
    .onChange(of: phase) { _, now in
      if now != .active { shown = false }
    }
  }
}
