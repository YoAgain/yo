// Shared by the library picker and YoShare.
import AVFoundation
import SwiftUI
import UIKit

/// One video waiting for its trim; `finish` answers once, whatever closed the trimmer.
@MainActor
final class VideoTrim: Identifiable {
  enum Result: Sendable {
    /// The part chosen, as a new file (the editor's, in the temporary directory).
    case cut(URL)
    /// Closed without a choice.
    case cancelled
    /// The trimmer could not open or save it.
    case failed
  }

  let video: URL
  private var answer: ((Result) -> Void)?

  private init(video: URL, answer: @escaping (Result) -> Void) {
    self.video = video
    self.answer = answer
  }

  /// Shows the trimmer through `show` and waits for its answer.
  static func run(_ video: URL, show: (VideoTrim?) -> Void) async -> Result {
    let result = await withCheckedContinuation { (done: CheckedContinuation<Result, Never>) in
      show(VideoTrim(video: video) { done.resume(returning: $0) })
    }
    show(nil)
    return result
  }

  func finish(_ result: Result) {
    guard let answer else {
      if case .cut(let url) = result { try? FileManager.default.removeItem(at: url) }
      return
    }
    self.answer = nil
    answer(result)
  }
}

extension View {
  /// Presents the trimmer for `trim` full-screen.
  func videoTrimming(_ trim: Binding<VideoTrim?>) -> some View {
    fullScreenCover(item: trim) { trim in
      VideoTrimmer(trim: trim)
    }
  }
}

struct VideoTrimmer: View {
  let trim: VideoTrim

  private static let frameCount = 12
  private static let stripHeight: CGFloat = 58
  private let limit = MediaFile.longestVideo
  @Environment(\.displayScale) private var displayScale
  @State private var asset: AVURLAsset?
  @State private var player: AVPlayer?
  @State private var duration = 0.0
  @State private var selection = 0.0...0.0
  @State private var position = 0.0
  @State private var playing = false
  @State private var frames: [Int: CGImage] = [:]
  @State private var saving = false
  @State private var dragOrigin: ClosedRange<Double>?
  @State private var dragPart: DragPart = .window
  @State private var resumeAfterDrag = false

  private enum DragPart { case start, window, end }

  var body: some View {
    NavigationStack {
      VStack(spacing: 12) {
        ZStack {
          Color.black
          if let player {
            VideoPreview(player: player)
              .accessibilityHidden(true)
            Button(action: togglePlayback) {
              Color.clear.contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(playing ? "Pause video" : "Play video")
            .disabled(saving)
          } else if !saving {
            ProgressView()
          }
        }
        if duration > 0 {
          filmstrip
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Selected video range")
            .accessibilityValue(rangeLabel)
            .accessibilityHint("Swipe up or down to move the selection by one second.")
            .accessibilityAdjustableAction { direction in
              switch direction {
              case .increment: moveWindow(from: selection, by: 1)
              case .decrement: moveWindow(from: selection, by: -1)
              @unknown default: return
              }
              cueSelection()
            }
            .disabled(saving)
          Text(rangeLabel)
            .font(.footnote.monospacedDigit())
            .foregroundStyle(.secondary)
            .accessibilityHidden(true)
        }
      }
      .padding(.horizontal, 16)
      .padding(.bottom, 8)
      .background(.black)
      .navigationTitle("Max \(Int(limit)) s")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Cancel", systemImage: "xmark") { trim.finish(.cancelled) }
            .labelStyle(.iconOnly)
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("Use") {
            player?.pause()
            player?.replaceCurrentItem(with: nil)
            player = nil
            playing = false
            saving = true
          }
          .buttonStyle(.glassProminent)
          .disabled(asset == nil || duration == 0 || saving)
        }
      }
      .overlay(alignment: .center) {
        if saving {
          ProgressView("Saving…").padding().background(.regularMaterial, in: .rect(cornerRadius: 12))
        }
      }
    }
    .preferredColorScheme(.dark)
    .task { await prepare() }
    .task(id: saving) { if saving { await exportSelection() } }
    .task(id: player) {
      guard let player else { return }
      while !Task.isCancelled {
        if playing, dragOrigin == nil {
          let currentTime = player.currentTime().seconds
          if currentTime.isFinite {
            position = min(max(currentTime, selection.lowerBound), selection.upperBound)
            if currentTime >= min(selection.upperBound, time(selection.upperBound).seconds) {
              position = selection.upperBound
              _ = await player.seek(to: time(selection.lowerBound), toleranceBefore: .zero, toleranceAfter: .zero)
              if playing, dragOrigin == nil, !Task.isCancelled { player.play() }
            }
          }
        }
        try? await Task.sleep(for: .milliseconds(33))
      }
    }
    .onDisappear {
      player?.pause()
      player?.replaceCurrentItem(with: nil)
      trim.finish(.cancelled)
    }
  }

  private var filmstrip: some View {
    GeometryReader { geometry in
      let width = geometry.size.width
      let cellWidth = width / CGFloat(Self.frameCount)
      let left = CGFloat(selection.lowerBound / duration) * width
      let right = CGFloat(selection.upperBound / duration) * width
      let windowWidth = right - left
      let handleWidth = min(13, windowWidth / 3)
      let innerWidth = windowWidth - 2 * handleWidth
      let playheadWidth = min(2, innerWidth)
      let progress = (min(max(position, selection.lowerBound), selection.upperBound) - selection.lowerBound)
        / (selection.upperBound - selection.lowerBound)
      let playheadOffset = handleWidth + CGFloat(progress) * (innerWidth - playheadWidth)

      HStack(spacing: 0) {
        ForEach(0..<Self.frameCount, id: \.self) { index in
          Color(white: 0.16)
            .frame(width: cellWidth, height: Self.stripHeight)
            .overlay {
              if let frame = frames[index] {
                Image(decorative: frame, scale: 1).resizable().scaledToFill()
              }
            }
            .clipped()
        }
      }
      .frame(width: width, height: Self.stripHeight)
      .overlay(alignment: .leading) { Color.black.opacity(0.55).frame(width: left) }
      .overlay(alignment: .trailing) { Color.black.opacity(0.55).frame(width: max(0, width - right)) }
      .clipShape(.rect(cornerRadius: 7))
      .overlay(alignment: .leading) {
        RoundedRectangle(cornerRadius: 7)
          .strokeBorder(.yellow, lineWidth: 3)
          .frame(width: windowWidth, height: Self.stripHeight)
          .overlay(alignment: .leading) { handle(width: handleWidth) }
          .overlay(alignment: .trailing) { handle(width: handleWidth) }
          .overlay(alignment: .leading) {
            Rectangle().fill(.white)
              .frame(width: playheadWidth)
              .offset(x: playheadOffset)
          }
          .clipShape(.rect(cornerRadius: 7))
          .offset(x: left)
      }
      .task(id: width > 0 && !saving) {
        if let asset, width > 0, !saving {
          await loadFrames(from: asset, duration: duration, cellWidth: cellWidth)
        }
      }
      .contentShape(Rectangle())
      .gesture(DragGesture(minimumDistance: 0)
        .onChanged { value in
          guard width > 0, !saving else { return }
          if dragOrigin == nil {
            let handleHit = min(22, (right - left) / 3)
            let x = value.startLocation.x
            guard x >= left - 10, x <= right + 10 else { return }
            dragPart = x <= left + handleHit ? .start : x >= right - handleHit ? .end : .window
            dragOrigin = selection
            resumeAfterDrag = playing
            player?.pause()
          }
          guard let origin = dragOrigin else { return }
          let delta = Double(value.translation.width / width) * duration
          let wasAtLimit = selection.upperBound - selection.lowerBound >= limit - 0.0001
          let minimum = min(1, duration)
          switch dragPart {
          case .start:
            let end = origin.upperBound
            selection = max(0, end - limit, min(end - minimum, origin.lowerBound + delta))...end
          case .end:
            let start = origin.lowerBound
            selection = start...min(duration, start + limit, max(start + minimum, origin.upperBound + delta))
          case .window:
            moveWindow(from: origin, by: delta)
          }
          if !wasAtLimit, selection.upperBound - selection.lowerBound >= limit - 0.0001 {
            UISelectionFeedbackGenerator().selectionChanged()
          }
          let target = dragPart == .end ? selection.upperBound : selection.lowerBound
          if abs(target - position) >= 0.1 {
            position = target
            player?.currentItem?.forwardPlaybackEndTime = .invalid
            player?.seek(to: time(target), toleranceBefore: time(0.1), toleranceAfter: time(0.1))
          }
        }
        .onEnded { _ in
          guard dragOrigin != nil else { return }
          dragOrigin = nil
          playing = resumeAfterDrag
          cueSelection()
        })
    }
    .frame(height: Self.stripHeight)
    .environment(\.layoutDirection, .leftToRight)
  }

  private func handle(width: Double) -> some View {
    Color.yellow.overlay {
      Capsule().fill(.black.opacity(0.75)).frame(width: min(2, width), height: 17)
    }
    .frame(width: width)
  }

  private var rangeLabel: String {
    let length = (selection.upperBound - selection.lowerBound).formatted(
      .number.precision(.fractionLength(0...1))
    )
    return "\(timestamp(selection.lowerBound)) – \(timestamp(selection.upperBound)) · \(length) s"
  }

  private func timestamp(_ seconds: Double) -> String {
    let total = Int(seconds.rounded())
    return String(format: "%d:%02d", total / 60, total % 60)
  }

  private func moveWindow(from range: ClosedRange<Double>, by delta: Double) {
    let length = min(limit, range.upperBound - range.lowerBound)
    let start = min(max(0, duration - length), max(0, range.lowerBound + delta))
    selection = start...min(duration, start + length)
  }

  private func togglePlayback() {
    guard let player else { return }
    playing.toggle()
    if playing {
      if position >= selection.upperBound - 0.01 { cueSelection() } else { player.play() }
    } else {
      player.pause()
    }
  }

  private func cueSelection() {
    guard let player else { return }
    position = selection.lowerBound
    player.currentItem?.forwardPlaybackEndTime = time(selection.upperBound)
    player.seek(to: time(selection.lowerBound), toleranceBefore: .zero, toleranceAfter: .zero)
    if playing { player.play() }
  }

  private func time(_ seconds: Double) -> CMTime { CMTime(seconds: seconds, preferredTimescale: 600) }

  private func prepare() async {
    let source = AVURLAsset(url: trim.video)
    do {
      let seconds = try await source.load(.duration).seconds
      let tracks = try await source.loadTracks(withMediaType: .video)
      try Task.checkCancellation()
      guard seconds.isFinite, seconds > 0, !tracks.isEmpty else {
        trim.finish(.failed)
        return
      }

      asset = source
      duration = seconds
      selection = 0...min(limit, seconds)
      player = AVPlayer(playerItem: AVPlayerItem(asset: source))
      playing = true
      cueSelection()
    } catch {
      guard !Task.isCancelled else { return }
      Diagnostics.record(error)
      trim.finish(.failed)
    }
  }

  @MainActor
  private func loadFrames(from asset: AVURLAsset, duration: Double, cellWidth: CGFloat) async {
    let generator = AVAssetImageGenerator(asset: asset)
    generator.appliesPreferredTrackTransform = true
    generator.maximumSize = CGSize(width: ceil(cellWidth * displayScale), height: ceil(Self.stripHeight * displayScale))
    let spacing = duration / Double(Self.frameCount)
    let tolerance = time(max(0.25, spacing / 2))
    generator.requestedTimeToleranceBefore = tolerance
    generator.requestedTimeToleranceAfter = tolerance
    let times = (0..<Self.frameCount).map {
      CMTime(seconds: (Double($0) + 0.5) * spacing, preferredTimescale: 600_000)
    }
    for await result in generator.images(for: times) {
      guard !Task.isCancelled, !saving else { return }
      if case .success(let requestedTime, let image, _) = result {
        for index in times.indices where times[index] == requestedTime {
          frames[index] = image
        }
      }
    }
  }

  private func exportSelection() async {
    guard let asset else { return }
    let start = time(selection.lowerBound)
    let length = CMTimeMinimum(time(limit), time(selection.upperBound) - start)
    let range = CMTimeRange(start: start, duration: length)
    for preset in [AVAssetExportPresetPassthrough, AVAssetExportPresetHighestQuality] {
      guard !Task.isCancelled else { return }
      guard let session = AVAssetExportSession(asset: asset, presetName: preset),
            let type = [AVFileType.mov, .mp4].first(where: session.supportedFileTypes.contains)
      else { continue }
      let url = FileManager.default.temporaryDirectory
        .appendingPathComponent(UUID().uuidString)
        .appendingPathExtension(type == .mov ? "mov" : "mp4")
      session.timeRange = range
      do {
        try await session.export(to: url, as: type)
        try Task.checkCancellation()
        trim.finish(.cut(url))
        return
      } catch {
        try? FileManager.default.removeItem(at: url)
        if !Task.isCancelled { Diagnostics.record(error) }
      }
    }
    if !Task.isCancelled { trim.finish(.failed) }
  }
}

private struct VideoPreview: UIViewRepresentable {
  let player: AVPlayer

  func makeUIView(context: Context) -> PlayerView { PlayerView() }

  func updateUIView(_ view: PlayerView, context: Context) {
    view.playerLayer.player = player
    view.playerLayer.videoGravity = .resizeAspect
  }

  final class PlayerView: UIView {
    override class var layerClass: AnyClass { AVPlayerLayer.self }
    var playerLayer: AVPlayerLayer { layer as! AVPlayerLayer }
  }
}
