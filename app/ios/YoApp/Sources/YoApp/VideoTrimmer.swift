// A shared or picked video longer than a Yo's (`MediaFile.longestVideo`, 30 s): the system's own trimmer
// (UIVideoEditorController) offers the part to send, at most that long. Compiled into the app (the library picker)
// and YoShare (the share sheet; Sources/YoShare links to this file).
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

  /// Whether the system's trimmer can take this video at all.
  static func possible(_ video: URL) -> Bool { UIVideoEditorController.canEditVideo(atPath: video.path) }

  /// Shows the trimmer through `show` (setting what a `.videoTrimming` sheet presents) and waits for its answer.
  static func run(_ video: URL, show: (VideoTrim?) -> Void) async -> Result {
    guard possible(video) else { return .failed }
    let result = await withCheckedContinuation { (done: CheckedContinuation<Result, Never>) in
      show(VideoTrim(video: video) { done.resume(returning: $0) })
    }
    show(nil)
    return result
  }

  func finish(_ result: Result) {
    answer?(result)
    answer = nil
  }
}

extension View {
  /// Presents the system's trimmer for `trim`; swiping it away counts as Cancel.
  func videoTrimming(_ trim: Binding<VideoTrim?>) -> some View {
    sheet(item: trim) { trim in
      // Gone without an answer (swiped away): cancelled. After an answer this does nothing.
      VideoTrimmer(trim: trim).ignoresSafeArea().onDisappear { trim.finish(.cancelled) }
    }
  }
}

/// UIVideoEditorController in SwiftUI: its length held to `MediaFile.longestVideo`.
private struct VideoTrimmer: UIViewControllerRepresentable {
  let trim: VideoTrim

  func makeUIViewController(context: Context) -> UIVideoEditorController {
    let editor = UIVideoEditorController()
    editor.videoPath = trim.video.path
    editor.videoMaximumDuration = MediaFile.longestVideo
    editor.videoQuality = .typeHigh  // made small afterwards (MediaFile.video), as any other
    editor.delegate = context.coordinator
    return editor
  }

  func updateUIViewController(_ editor: UIVideoEditorController, context: Context) {}

  func makeCoordinator() -> Coordinator { Coordinator(trim: trim) }

  @MainActor
  final class Coordinator: NSObject, UIVideoEditorControllerDelegate, UINavigationControllerDelegate {
    let trim: VideoTrim
    init(trim: VideoTrim) { self.trim = trim }

    // The editor may report a save twice: `finish` answers the first.
    func videoEditorController(_ editor: UIVideoEditorController, didSaveEditedVideoToPath editedVideoPath: String) {
      trim.finish(.cut(URL(fileURLWithPath: editedVideoPath)))
    }

    func videoEditorController(_ editor: UIVideoEditorController, didFailWithError error: any Error) {
      Diagnostics.record(error)
      trim.finish(.failed)
    }

    func videoEditorControllerDidCancel(_ editor: UIVideoEditorController) { trim.finish(.cancelled) }
  }
}
