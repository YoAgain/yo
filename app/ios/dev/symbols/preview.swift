// Renders the four controls (dark Control Center circles) from the symbols compiled into a bundle:
//   actool YoApp/Resources/Symbols.xcassets --compile <b.bundle>/Contents/Resources --platform macosx --minimum-deployment-target 14.0
//   swift dev/symbols/preview.swift <b.bundle> controls.png 25 medium
import AppKit
let a = CommandLine.arguments
let bundle = Bundle(path: a[1])!
let pt = CGFloat(Double(a[3])!)
let weights: [String: NSFont.Weight] = ["regular": .regular, "medium": .medium, "semibold": .semibold, "bold": .bold]
let items: [(String, String?, String)] = [("yo.wordmark", nil, "Send Yo"), ("yo.back", nil, "Yo Back"), ("camera.fill", "sys", "Yo Camera"), ("yo.letter", nil, "Open Yo")]
let s: CGFloat = 3, d: CGFloat = 64, gapX: CGFloat = 30, padX: CGFloat = 28, padY: CGFloat = 26, labelH: CGFloat = 34
let W = padX * 2 + d * 4 + gapX * 3, H = padY * 2 + d + labelH
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(W * s), pixelsHigh: Int(H * s), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = NSSize(width: W, height: H)
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let bg = NSGradient(starting: NSColor(calibratedRed: 0.13, green: 0.12, blue: 0.17, alpha: 1), ending: NSColor(calibratedRed: 0.07, green: 0.07, blue: 0.09, alpha: 1))!
bg.draw(in: NSRect(x: 0, y: 0, width: W, height: H), angle: -90)
for (i, it) in items.enumerated() {
  let x = padX + CGFloat(i) * (d + gapX), y = padY + labelH
  let c = NSRect(x: x, y: y, width: d, height: d)
  NSColor(white: 1, alpha: 0.16).setFill(); NSBezierPath(ovalIn: c).fill()
  let cfg = NSImage.SymbolConfiguration(pointSize: pt, weight: weights[a[4]]!).applying(NSImage.SymbolConfiguration(paletteColors: [.white]))
  let base = it.1 == nil ? NSImage(symbolName: it.0, bundle: bundle, variableValue: 0)! : NSImage(systemSymbolName: it.0, accessibilityDescription: nil)!
  let img = base.withSymbolConfiguration(cfg)!
  let r = NSRect(x: c.midX - img.size.width / 2, y: c.midY - img.size.height / 2, width: img.size.width, height: img.size.height)
  img.draw(in: r)
  let p = NSMutableParagraphStyle(); p.alignment = .center
  (it.2 as NSString).draw(in: NSRect(x: x - 15, y: padY - 6, width: d + 30, height: 30), withAttributes: [.font: NSFont.systemFont(ofSize: 11, weight: .semibold), .foregroundColor: NSColor.white, .paragraphStyle: p])
  print(it.0, img.size)
}
NSGraphicsContext.current = nil
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
