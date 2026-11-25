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

  /// The modules of a Yo code (error correction H, which carries the cleared centre with "Yo"), drawn by `QRCard`.
  static func contactMatrix(text: String) -> QRMatrix? {
    let generator = CIFilter.qrCodeGenerator()
    generator.message = Data(text.utf8)
    generator.correctionLevel = "H"
    guard let qr = generator.outputImage, let cg = context.createCGImage(qr, from: qr.extent) else { return nil }
    let width = cg.width
    guard width > 0, cg.height == width else { return nil }
    var pixels = [UInt8](repeating: 255, count: width * width)
    let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
      guard let bitmap = CGContext(
        data: buffer.baseAddress, width: width, height: width, bitsPerComponent: 8, bytesPerRow: width,
        space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue)
      else { return false }
      bitmap.interpolationQuality = .none
      bitmap.draw(cg, in: CGRect(x: 0, y: 0, width: width, height: width))
      return true
    }
    guard drawn else { return nil }
    // One pixel a module, in a white quiet zone: the top left finder's corner is the first dark one on the diagonal.
    var margin = 0
    while margin < width / 2, pixels[margin * width + margin] > 127 { margin += 1 }
    let count = width - 2 * margin
    guard count >= 21 else { return nil }
    var bits = [Bool](repeating: false, count: count * count)
    for row in 0..<count {
      for column in 0..<count { bits[row * count + column] = pixels[(row + margin) * width + column + margin] < 128 }
    }
    return QRMatrix(count: count, bits: bits)
  }

  static func read(_ image: UIImage) -> String? {
    guard let ci = CIImage(image: image),
      let detector = CIDetector(ofType: CIDetectorTypeQRCode, context: nil, options: [CIDetectorAccuracy: CIDetectorAccuracyHigh])
    else { return nil }
    return detector.features(in: ci).compactMap { ($0 as? CIQRCodeFeature)?.messageString }.first
  }
}

/// A QR code's modules, row by row (true = dark).
struct QRMatrix: Sendable, Equatable {
  let count: Int
  let bits: [Bool]

  subscript(row: Int, column: Int) -> Bool { bits[row * count + column] }
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
