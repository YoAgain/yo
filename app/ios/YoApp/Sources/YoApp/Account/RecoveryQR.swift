// The recovery key as a QR code (the key's 64 characters, nothing else), and read back from Photos or the camera.
import CoreImage
import CoreImage.CIFilterBuiltins
import PhotosUI
import SwiftUI
import VisionKit

enum RecoveryQR {
  /// Shared: making a context sets up Metal, tens of milliseconds each time.
  private static let context = CIContext()

  static func image(key: String) -> UIImage? {
    let filter = CIFilter.qrCodeGenerator()
    filter.message = Data(key.utf8)
    filter.correctionLevel = "M"
    guard let code = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 12, y: 12)),
      let cg = context.createCGImage(code, from: code.extent)
    else { return nil }
    return UIImage(cgImage: cg)
  }

  /// White modules on the brand colour; high correction tolerates the card’s small centre logo.
  static func contactImage(text: String, inverted: Bool) -> UIImage? {
    let generator = CIFilter.qrCodeGenerator()
    generator.message = Data(text.utf8)
    generator.correctionLevel = "H"
    guard let qr = generator.outputImage else { return nil }
    let colors = CIFilter.falseColor()
    colors.inputImage = qr
    let amethyst = CIColor(red: 155.0 / 255, green: 89.0 / 255, blue: 182.0 / 255)
    colors.color0 = inverted ? CIColor.white : amethyst
    colors.color1 = inverted ? amethyst : CIColor.white
    guard let output = colors.outputImage?.transformed(by: CGAffineTransform(scaleX: 12, y: 12)),
      let cg = context.createCGImage(output, from: output.extent) else { return nil }
    return UIImage(cgImage: cg)
  }

  static func read(_ image: UIImage) -> String? {
    guard let ci = CIImage(image: image),
      let detector = CIDetector(ofType: CIDetectorTypeQRCode, context: nil, options: [CIDetectorAccuracy: CIDetectorAccuracyHigh])
    else { return nil }
    return detector.features(in: ci).compactMap { ($0 as? CIQRCodeFeature)?.messageString }.first
  }
}

struct RecoveryQRView: View {
  let key: String
  var size: CGFloat = 140

  var body: some View {
    if let image = RecoveryQR.image(key: key) {
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
