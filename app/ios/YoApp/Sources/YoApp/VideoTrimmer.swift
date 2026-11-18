// A shared or picked video longer than a Yo's (`MediaFile.longestVideo`, 10 s): the system's own trimmer
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

  /// Shows the trimmer through `show` and waits for its answer.
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
  /// Presents the system's trimmer for `trim` full-screen.
  func videoTrimming(_ trim: Binding<VideoTrim?>) -> some View {
    fullScreenCover(item: trim) { trim in
      VideoTrimmer(trim: trim).preferredColorScheme(.dark).onDisappear { trim.finish(.cancelled) }
    }
  }
}

/// UIVideoEditorController in SwiftUI: its length held to `MediaFile.longestVideo`.
private struct VideoTrimmer: UIViewControllerRepresentable {
  let trim: VideoTrim

  func makeUIViewController(context: Context) -> YoVideoEditor { YoVideoEditor(trim: trim) }

  func updateUIViewController(_ editor: YoVideoEditor, context: Context) {}
}

/// The system editor is already a navigation controller. Present it directly in the share extension.
final class YoVideoEditor: UIVideoEditorController, UIVideoEditorControllerDelegate, UINavigationControllerDelegate {
  private let trim: VideoTrim

  init(trim: VideoTrim) {
    self.trim = trim
    super.init(nibName: nil, bundle: nil)
    modalPresentationStyle = .fullScreen
    overrideUserInterfaceStyle = .dark
    videoPath = trim.video.path
    videoMaximumDuration = MediaFile.longestVideo
    videoQuality = .typeHigh
    delegate = self
    setNavigationBarHidden(false, animated: false)
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError() }

  override func viewWillLayoutSubviews() {
    // The limit as the bar's title (instead of the system's "Edit"); a prompt above it ran into the sheet's top edge.
    if let item = navigationBar.topItem {
      let title = "Max \(Int(MediaFile.longestVideo)) s"
      if item.title != title { item.title = title }
      if item.prompt != nil { item.prompt = nil }
    }
    super.viewWillLayoutSubviews()
  }

  func videoEditorController(_ editor: UIVideoEditorController, didSaveEditedVideoToPath path: String) {
    trim.finish(.cut(URL(fileURLWithPath: path)))
  }

  func videoEditorController(_ editor: UIVideoEditorController, didFailWithError error: any Error) {
    Diagnostics.record(error)
    trim.finish(.failed)
  }

  func videoEditorControllerDidCancel(_ editor: UIVideoEditorController) { trim.finish(.cancelled) }
}
