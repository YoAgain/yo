// What a photo or video is as it goes (spec 5, 6.8): a HEIC of at most 2048 pixels, an HEVC video of at most 720p and
// 30 seconds (the camera's 10) with stereo AAC sound, neither with a place or a device in it. Without the core: the share extension (YoShare) makes them the same
// way, from this very file.
import AVFoundation
import Foundation
import ImageIO
import UniformTypeIdentifiers
import VideoToolbox
import os

/// A photo or video ready to be sealed: its bytes and what the Yo says about it.
struct MediaFile: Sendable {
  enum Kind: String, Codable, Sendable { case photo, video }

  let kind: Kind
  let data: Data
  let width: Int
  let height: Int
  let durationMs: UInt32

  static let longestSide = 2048
  static let quality = 0.75
  /// The longest video that goes (spec 5): a shared or picked one; the camera records `Camera.longestVideo`.
  static let longestVideo = 30.0
  /// The most a video's bytes may be: under a blob's 10 MiB (MAX_BLOB_LEN, its tag included) with room to spare.
  static let largestVideo = 9_000_000
  /// What a video aims at: 2.5 Mbit/s up to some 21 s, less beyond, so 30 s are some 7 MB too.
  static let videoTarget = 7_000_000
  /// 1280 by 720 at most, either way round.
  static let videoSides = (long: 1280.0, short: 720.0)
  static let videoBitRate = 2_500_000
  static let audioBitRate = 128_000

  /// The picture's bit rate for a video `seconds` long: `videoBitRate`, lowered so it stays near `videoTarget`.
  static func videoBitRate(for seconds: Double) -> Int {
    guard seconds > 0 else { return videoBitRate }
    let fitting = Int(Double(videoTarget) * 8 / seconds) - audioBitRate
    return max(500_000, min(videoBitRate, fitting))
  }

  /// A photo as it goes: a HEIC (no older reader needs a JPEG: both decode HEIC) at most `longestSide` pixels on its longest side, upright, and nothing but its
  /// pixels (no place, no camera, no date).
  static func photo(_ data: Data) -> MediaFile? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { Diagnostics.record(NSError(domain: "MediaFile", code: 1)); return nil }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,  // upright, as it was taken
      kCGImageSourceThumbnailMaxPixelSize: longestSide,
      kCGImageSourceShouldCacheImmediately: true,
    ]
    guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { Diagnostics.record(NSError(domain: "MediaFile", code: 1)); return nil }
    let out = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(out, UTType.heic.identifier as CFString, 1, nil)
    else { Diagnostics.record(NSError(domain: "MediaFile", code: 1)); return nil }
    // Only the quality: no properties, so no metadata.
    CGImageDestinationAddImage(
      destination, image, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
    guard CGImageDestinationFinalize(destination) else { Diagnostics.record(NSError(domain: "MediaFile", code: 1)); return nil }
    return MediaFile(kind: .photo, data: out as Data, width: image.width, height: image.height, durationMs: 0)
  }

  /// A shared video's length in seconds (nil: not readable as a video). A share longer than `longestVideo` (and
  /// the half second a recording may run over) is offered to the system's trimmer, never cut by itself
  /// (VideoTrimmer); where that cannot be, it is refused whole.
  static func length(of video: URL) async -> Double? {
    guard let duration = try? await AVURLAsset(url: video).load(.duration), duration.isNumeric else { return nil }
    return duration.seconds
  }

  static func fits(_ seconds: Double) -> Bool { seconds <= longestVideo + 0.5 }

  /// A video as it goes: HEVC at 2.5 Mbit/s (less for one longer than some 21 s: `videoBitRate(for:)`), at most 1280 by 720 (either way round), with its sound as stereo AAC,
  /// in an MP4 of at most `longestVideo` seconds and `largestVideo` bytes, without metadata. Written with explicit
  /// settings (an export preset has no 720p HEVC). The recording is deleted.
  ///
  /// It goes standing in its pixels: the recording's own turn (its track's transform, as a player shows it: from
  /// the app's camera as `Camera.orient` set it, mirror and all) is drawn into the frames, and what goes has none.
  /// So every reader shows it alike, whether it honours a track's transform or not.
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
    else { Diagnostics.record(NSError(domain: "MediaFile", code: 1)); return nil }
    os.Logger(subsystem: "app.justyo", category: "media-diag").notice(
      "YoMediaDiag file natural=\(natural.width, privacy: .public)x\(natural.height, privacy: .public) transform=[\(turn.a, privacy: .public) \(turn.b, privacy: .public) \(turn.c, privacy: .public) \(turn.d, privacy: .public) \(turn.tx, privacy: .public) \(turn.ty, privacy: .public)]")
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
          size: size, rate: rate, bitRate: videoBitRate(for: length.seconds),
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

  /// The recording's size made to fit 1280 by 720 (never larger), in even pixels as the encoder wants them.
  private static func fitted(_ natural: CGSize) -> CGSize {
    let long = max(natural.width, natural.height), short = min(natural.width, natural.height)
    guard long > 0, short > 0 else { return CGSize(width: videoSides.long, height: videoSides.short) }
    let scale = min(1, videoSides.long / long, videoSides.short / short)
    func even(_ side: CGFloat) -> CGFloat { max(2, (side * scale / 2).rounded() * 2) }
    return CGSize(width: even(natural.width), height: even(natural.height))
  }

  /// Reads the recording and writes it again as it goes: its frames stood upright (`standing`) and scaled into
  /// `size`, its sound as stereo AAC at 48 kHz; no metadata.
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
    // Frames upright at their own size (the encoder scales them), in 8-bit SDR as `standing` renders them.
    let frames = AVAssetReaderVideoCompositionOutput(
      videoTracks: pictures,
      videoSettings: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange])
    frames.videoComposition = standing
    frames.alwaysCopiesSampleData = false
    guard reader.canAdd(frames) else { throw NSError(domain: "MediaFile", code: 2) }
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
    guard writer.canAdd(video) else { throw NSError(domain: "MediaFile", code: 3) }
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
      throw reader.error ?? writer.error ?? NSError(domain: "MediaFile", code: 4)
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
      throw reader.error ?? writer.error ?? NSError(domain: "MediaFile", code: 5)
    }
    await writer.finishWriting()
    guard writer.status == .completed else { throw writer.error ?? NSError(domain: "MediaFile", code: 6) }
  }
}
