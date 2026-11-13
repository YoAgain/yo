// What a photo or video is as it goes (spec 5, 6.8): a JPEG of at most 1600 pixels, a video of at most 10 seconds
// and 8 MB, neither with a place or a device in it. Without the core: the share extension (YoShare) makes them the same
// way, from this very file.
import AVFoundation
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// A photo or video ready to be sealed: its bytes and what the Yo says about it.
struct MediaFile: Sendable {
  enum Kind: String, Codable, Sendable { case photo, video }

  let kind: Kind
  let data: Data
  let width: Int
  let height: Int
  let durationMs: UInt32

  static let longestSide = 1600
  static let quality = 0.7
  static let longestVideo = 10.0
  static let largestVideo: Int64 = 8_000_000

  /// A photo as it goes: a JPEG at most `longestSide` pixels on its longest side, upright, and nothing but its
  /// pixels (no place, no camera, no date).
  static func photo(_ data: Data) -> MediaFile? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,  // upright, as it was taken
      kCGImageSourceThumbnailMaxPixelSize: longestSide,
      kCGImageSourceShouldCacheImmediately: true,
    ]
    guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
    let out = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(out, UTType.jpeg.identifier as CFString, 1, nil)
    else { return nil }
    // Only the quality: no properties, so no metadata.
    CGImageDestinationAddImage(
      destination, image, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
    guard CGImageDestinationFinalize(destination) else { return nil }
    return MediaFile(kind: .photo, data: out as Data, width: image.width, height: image.height, durationMs: 0)
  }

  /// A video as it goes: an MP4 of at most `longestVideo` seconds and `largestVideo` bytes, 960 by 540 at most,
  /// without metadata. The recording is deleted.
  static func video(_ recording: URL) async -> MediaFile? {
    defer { try? FileManager.default.removeItem(at: recording) }
    let asset = AVURLAsset(url: recording)
    guard let export = AVAssetExportSession(asset: asset, presetName: AVAssetExportPreset960x540),
      let duration = try? await asset.load(.duration)
    else { return nil }
    let length = CMTimeMinimum(duration, CMTime(seconds: longestVideo, preferredTimescale: 600))
    export.timeRange = CMTimeRange(start: .zero, duration: length)
    export.metadata = []
    export.metadataItemFilter = .forSharing()  // no place, no device
    export.fileLengthLimit = largestVideo
    export.shouldOptimizeForNetworkUse = true
    let out = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mp4")
    defer { try? FileManager.default.removeItem(at: out) }
    do {
      try await export.export(to: out, as: .mp4)
    } catch {
      return nil
    }
    guard let data = try? Data(contentsOf: out), !data.isEmpty, data.count <= largestVideo + 1_000_000 else {
      return nil
    }
    var size = CGSize(width: 540, height: 960)
    if let track = try? await AVURLAsset(url: out).loadTracks(withMediaType: .video).first,
      let (natural, transform) = try? await track.load(.naturalSize, .preferredTransform)
    {
      let turned = natural.applying(transform)
      size = CGSize(width: abs(turned.width), height: abs(turned.height))
    }
    let ms = UInt32(max(0, min(length.seconds, longestVideo)) * 1000)
    return MediaFile(kind: .video, data: data, width: Int(size.width), height: Int(size.height), durationMs: ms)
  }
}
