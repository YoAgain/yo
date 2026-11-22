// Photos and videos as they go (spec 5, 6.8): small, upright, without metadata. Shared with YoShare.
import AVFoundation
import Foundation
import ImageIO
import UniformTypeIdentifiers
import VideoToolbox
import YoCore

/// A photo or video ready to be sealed: its bytes and what the Yo says about it.
struct MediaFile: Sendable {
  enum Kind: String, Codable, Sendable { case photo, video }

  let kind: Kind
  let data: Data
  let width: Int
  let height: Int
  let durationMs: UInt32

  static let longestSide = Int(productPhotoLongSide())
  private static let encoding = productEncoding()
  static let quality = encoding.photoQuality
  static let longestVideo = Double(productVideoLimitMs()) / 1000
  /// The most a video's bytes may be: under a blob's 10 MiB (MAX_BLOB_LEN, its tag included) with room to spare.
  static let largestVideo = Int(productVideoLimitBytes())
  static let videoSides = (long: Double(encoding.videoLongSide), short: Double(encoding.videoShortSide))
  static let videoBitRate = Int(encoding.videoBitRate)
  static let audioBitRate = Int(encoding.audioBitRate)

  /// HEIC (every reader decodes it), upright, at most `longestSide`, pixels only (no place, camera or date).
  static func photo(_ data: Data) -> MediaFile? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return failed() }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,  // upright, as it was taken
      kCGImageSourceThumbnailMaxPixelSize: longestSide,
      kCGImageSourceShouldCacheImmediately: true,
    ]
    guard let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary),
      let image = onWhite(thumbnail)
    else { return failed() }
    let out = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(out, UTType.heic.identifier as CFString, 1, nil)
    else { return failed() }
    // Only the quality: no properties, so no metadata.
    CGImageDestinationAddImage(
      destination, image, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
    guard CGImageDestinationFinalize(destination) else { return failed() }
    return MediaFile(kind: .photo, data: out as Data, width: image.width, height: image.height, durationMs: 0)
  }

  /// A photo goes without transparency: see-through pixels laid on white, as Android sends them (a HEIC's alpha is
  /// shown black or dropped by other readers, a JPEG has none).
  static func onWhite(_ image: CGImage) -> CGImage? {
    switch image.alphaInfo {
    case .none, .noneSkipFirst, .noneSkipLast: return image
    default: break
    }
    let space = image.colorSpace.flatMap { $0.model == .rgb ? $0 : nil } ?? CGColorSpace(name: CGColorSpace.sRGB)!
    guard
      let context = CGContext(
        data: nil, width: image.width, height: image.height, bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
    else { return nil }
    let all = CGRect(x: 0, y: 0, width: image.width, height: image.height)
    context.setFillColor(CGColor(gray: 1, alpha: 1))
    context.fill(all)
    context.draw(image, in: all)
    return context.makeImage()
  }

  static func length(of video: URL) async -> Double? {
    guard let duration = try? await AVURLAsset(url: video).load(.duration), duration.isNumeric else { return nil }
    return duration.seconds
  }

  static func fits(_ seconds: Double) -> Bool { seconds <= longestVideo }

  /// HEVC MP4 with stereo AAC, no metadata; explicit settings because no export preset does 720p HEVC. The track's
  /// transform (mirror included) is drawn into the pixels, so readers that ignore transforms show it alike. Deletes
  /// the recording.
  static func video(_ recording: URL) async -> MediaFile? {
    defer { try? FileManager.default.removeItem(at: recording) }
    let out = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mp4")
    defer { try? FileManager.default.removeItem(at: out) }
    let asset = AVURLAsset(url: recording)
    guard let duration = await Diagnostics.attempt({ try await asset.load(.duration) }),
      let pictures = await Diagnostics.attempt({ try await asset.loadTracks(withMediaType: .video) }),
      let picture = pictures.first,
      let (natural, turn, rate) = await Diagnostics.attempt({
        try await picture.load(.naturalSize, .preferredTransform, .nominalFrameRate)
      }),
      var standing = await Diagnostics.attempt({ try await AVVideoComposition.Configuration(for: asset) })
    else { return failed() }
    // Frames as 8-bit SDR (an HDR recording is mapped down).
    standing.colorPrimaries = AVVideoColorPrimaries_ITU_R_709_2
    standing.colorTransferFunction = AVVideoTransferFunction_ITU_R_709_2
    standing.colorYCbCrMatrix = AVVideoYCbCrMatrix_ITU_R_709_2
    let shown = natural.applying(turn)
    let sound = (try? await asset.loadTracks(withMediaType: .audio).first) ?? nil
    let length = CMTimeMinimum(duration, CMTime(seconds: longestVideo, preferredTimescale: 600))
    let size = fitted(CGSize(width: abs(shown.width), height: abs(shown.height)))
    do {
      try await withTaskCancellationHandler {
        try Task.checkCancellation()
        try await write(
          asset, pictures: pictures, standing: AVVideoComposition(configuration: standing), sound: sound, to: out,
          size: size, rate: rate, bitRate: videoBitRate,
          range: CMTimeRange(start: .zero, duration: length))
        try Task.checkCancellation()
      } onCancel: {
        try? FileManager.default.removeItem(at: recording)
        try? FileManager.default.removeItem(at: out)
      }
    } catch {
      Diagnostics.record(error)
      return nil
    }
    guard let data = Diagnostics.attempt({ try Data(contentsOf: out) }), !data.isEmpty, data.count <= largestVideo else {
      return nil
    }
    let ms = UInt32(max(0, min(length.seconds, longestVideo)) * 1000)
    return MediaFile(kind: .video, data: data, width: Int(size.width), height: Int(size.height), durationMs: ms)
  }

  /// Never upscaled; even sides, as the encoder wants.
  private static func fitted(_ natural: CGSize) -> CGSize {
    let long = max(natural.width, natural.height), short = min(natural.width, natural.height)
    guard long > 0, short > 0 else { return CGSize(width: videoSides.long, height: videoSides.short) }
    let scale = min(1, videoSides.long / long, videoSides.short / short)
    func even(_ side: CGFloat) -> CGFloat { max(2, (side * scale / 2).rounded() * 2) }
    return CGSize(width: even(natural.width), height: even(natural.height))
  }

  private static func write(
    _ asset: AVAsset, pictures: [AVAssetTrack], standing: AVVideoComposition, sound: AVAssetTrack?, to out: URL,
    size: CGSize, rate: Float, bitRate: Int, range: CMTimeRange
  ) async throws {
    let reader = try AVAssetReader(asset: asset)
    reader.timeRange = range
    let writer = try AVAssetWriter(outputURL: out, fileType: .mp4)
    writer.shouldOptimizeForNetworkUse = true
    writer.metadata = []
    let sdr: [String: Any] = [
      AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
      AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
      AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
    ]
    // Upright at their own size; the encoder scales them.
    let frames = AVAssetReaderVideoCompositionOutput(
      videoTracks: pictures,
      videoSettings: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange])
    frames.videoComposition = standing
    frames.alwaysCopiesSampleData = false
    guard reader.canAdd(frames) else { throw failure(2) }
    reader.add(frames)
    let video = AVAssetWriterInput(
      mediaType: .video,
      outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.hevc,
        AVVideoWidthKey: size.width,
        AVVideoHeightKey: size.height,
        AVVideoScalingModeKey: AVVideoScalingModeResizeAspect,
        AVVideoColorPropertiesKey: sdr,
        AVVideoCompressionPropertiesKey: [
          AVVideoAverageBitRateKey: bitRate,
          AVVideoExpectedSourceFrameRateKey: rate > 0 ? rate : 30,
          AVVideoMaxKeyFrameIntervalDurationKey: 2,
          AVVideoProfileLevelKey: kVTProfileLevel_HEVC_Main_AutoLevel as String,
        ],
      ])
    video.expectsMediaDataInRealTime = false
    guard writer.canAdd(video) else { throw failure(3) }
    writer.add(video)
    var pairs = [(frames as AVAssetReaderOutput, video)]
    if let sound {
      var layout = AudioChannelLayout()
      layout.mChannelLayoutTag = kAudioChannelLayoutTag_Stereo
      let stereo = Data(bytes: &layout, count: MemoryLayout<AudioChannelLayout>.size)
      // Mono from a microphone without stereo is spread to both sides.
      let samples = AVAssetReaderAudioMixOutput(
        audioTracks: [sound],
        audioSettings: [
          AVFormatIDKey: kAudioFormatLinearPCM,
          AVSampleRateKey: 48_000,
          AVNumberOfChannelsKey: 2,
          AVChannelLayoutKey: stereo,
          AVLinearPCMBitDepthKey: 16,
          AVLinearPCMIsFloatKey: false,
          AVLinearPCMIsBigEndianKey: false,
          AVLinearPCMIsNonInterleaved: false,
        ])
      let audio = AVAssetWriterInput(
        mediaType: .audio,
        outputSettings: [
          AVFormatIDKey: kAudioFormatMPEG4AAC,
          AVSampleRateKey: 48_000,
          AVNumberOfChannelsKey: 2,
          AVChannelLayoutKey: stereo,
          AVEncoderBitRateKey: audioBitRate,
        ])
      audio.expectsMediaDataInRealTime = false
      if reader.canAdd(samples), writer.canAdd(audio) {
        reader.add(samples)
        writer.add(audio)
        pairs.append((samples, audio))
      }
    }
    guard reader.startReading(), writer.startWriting() else {
      throw reader.error ?? writer.error ?? failure(4)
    }
    writer.startSession(atSourceTime: range.start)
    nonisolated(unsafe) let (readerRef, writerRef) = (reader, writer)
    try await withTaskCancellationHandler {
      // Each track pumped on its own queue as the writer asks for more; done when all are.
      await withTaskGroup(of: Void.self) { group in
        for (index, pair) in pairs.enumerated() {
          nonisolated(unsafe) let output = pair.0
          nonisolated(unsafe) let input = pair.1
          group.addTask {
            await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
              let queue = DispatchQueue(label: "yo.media-write.\(index)")
              nonisolated(unsafe) var finished = false  // only on `queue`
              input.requestMediaDataWhenReady(on: queue) {
                while !finished, input.isReadyForMoreMediaData || writerRef.status != .writing {
                  if readerRef.status == .reading, writerRef.status == .writing, let sample = output.copyNextSampleBuffer() {
                    if !input.append(sample) { readerRef.cancelReading() }
                  } else {
                    finished = true
                    if writerRef.status == .writing { input.markAsFinished() }
                    done.resume()
                  }
                }
              }
            }
          }
        }
      }
    } onCancel: {
      readerRef.cancelReading()
      writerRef.cancelWriting()
    }
    try Task.checkCancellation()
    guard reader.status == .completed else {
      writer.cancelWriting()
      throw reader.error ?? writer.error ?? failure(5)
    }
    await writer.finishWriting()
    guard writer.status == .completed else { throw writer.error ?? failure(6) }
  }

  private static func failure(_ code: Int) -> NSError { NSError(domain: "MediaFile", code: code) }

  private static func failed() -> MediaFile? {
    Diagnostics.record(failure(1))
    return nil
  }
}
