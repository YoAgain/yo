// The camera of the camera mode (the original's camera "context"): a live picture behind the list, a photo taken
// when a name is tapped, a video while a name is held, front or back. AVFoundation has the camera; SwiftUI has no
// view for its preview, so one small UIViewRepresentable carries the preview layer.
import AVFoundation
import SwiftUI

/// The capture session. Configured and run on its own queue, as AVFoundation asks; the session, its inputs and
/// outputs are only touched there, which is what makes the class safe to share.
final class Camera: NSObject, @unchecked Sendable {
  /// The longest video a held name records.
  static let longestVideo = 10.0

  let session = AVCaptureSession()
  private let photoOutput = AVCapturePhotoOutput()
  private let movieOutput = AVCaptureMovieFileOutput()
  private let queue = DispatchQueue(label: "yo.camera")
  private var configured = false
  private var input: AVCaptureDeviceInput?
  private var photos: [Int64: CheckedContinuation<Data?, Never>] = [:]
  private var movie: CheckedContinuation<URL?, Never>?
  /// A video that ended at the length limit before the name was let go; `stopVideo()` hands it over then.
  private var finishedAlone: URL?
  /// A video was started and not yet handed over: while true, `stopVideo()` waits for its file, even when the length
  /// limit stopped it and AVFoundation is still finishing the file.
  private var videoActive = false

  /// Asks for the camera once; false when it is refused.
  static func allowed() async -> Bool {
    switch AVCaptureDevice.authorizationStatus(for: .video) {
    case .authorized: true
    case .notDetermined: await AVCaptureDevice.requestAccess(for: .video)
    default: false
    }
  }

  func start() {
    queue.async { [self] in
      if !configured { configure() }
      if !session.isRunning { session.startRunning() }
    }
  }

  func stop() {
    queue.async { [self] in
      if movieOutput.isRecording { movieOutput.stopRecording() }
      if session.isRunning { session.stopRunning() }
    }
  }

  /// Front to back and back again.
  func flip() {
    queue.async { [self] in
      let next: AVCaptureDevice.Position = input?.device.position == .back ? .front : .back
      guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: next),
        let new = try? AVCaptureDeviceInput(device: device)
      else { return }
      session.beginConfiguration()
      if let input { session.removeInput(input) }
      if session.canAddInput(new) {
        session.addInput(new)
        input = new
      } else if let input {
        session.addInput(input)
      }
      mirrorFrontCamera()
      session.commitConfiguration()
    }
  }

  /// A JPEG of what the camera sees now; nil when the camera is not running.
  func photo() async -> Data? {
    await withCheckedContinuation { continuation in
      queue.async { [self] in
        // Running with a camera (none in the Simulator: capturing without one raises).
        guard session.isRunning, photoOutput.connection(with: .video) != nil else {
          return continuation.resume(returning: nil)
        }
        let settings = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
        settings.photoQualityPrioritization = .speed  // the shutter at once: a Yo is a snapshot
        photos[settings.uniqueID] = continuation
        photoOutput.capturePhoto(with: settings, delegate: self)
      }
    }
  }

  /// Starts a video; `stopVideo()` ends it and hands over the file. It ends by itself at `longestVideo`.
  func startVideo() {
    queue.async { [self] in
      guard session.isRunning, movieOutput.connection(with: .video) != nil, !movieOutput.isRecording, !videoActive
      else { return }
      finishedAlone = nil
      videoActive = true
      let file = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mov")
      movieOutput.startRecording(to: file, recordingDelegate: self)
    }
  }

  /// Ends the video and returns its file; nil when none was recording or it failed.
  func stopVideo() async -> URL? {
    await withCheckedContinuation { continuation in
      queue.async { [self] in
        if movieOutput.isRecording {
          movie = continuation
          movieOutput.stopRecording()
        } else if let file = finishedAlone {
          finishedAlone = nil
          videoActive = false
          continuation.resume(returning: file)
        } else if videoActive {
          movie = continuation  // stopped by the limit, the file still being finished: the delegate hands it over
        } else {
          continuation.resume(returning: nil)
        }
      }
    }
  }

  private func configure() {
    configured = true
    session.beginConfiguration()
    session.sessionPreset = .high  // photo and video from one session
    if let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back),
      let new = try? AVCaptureDeviceInput(device: device), session.canAddInput(new)
    {
      session.addInput(new)
      input = new
    }
    if session.canAddOutput(photoOutput) { session.addOutput(photoOutput) }
    if photoOutput.isResponsiveCaptureSupported { photoOutput.isResponsiveCaptureEnabled = true }
    if photoOutput.isZeroShutterLagSupported { photoOutput.isZeroShutterLagEnabled = true }
    if session.canAddOutput(movieOutput) { session.addOutput(movieOutput) }
    movieOutput.maxRecordedDuration = CMTime(seconds: Self.longestVideo, preferredTimescale: 600)
    mirrorFrontCamera()
    session.commitConfiguration()
  }

  /// Photos and videos come out upright in portrait (the app is portrait only), and from the front camera mirrored,
  /// as the preview showed them.
  private func mirrorFrontCamera() {
    let front = input?.device.position == .front
    for output in [photoOutput, movieOutput] as [AVCaptureOutput] {
      guard let connection = output.connection(with: .video) else { continue }
      if connection.isVideoRotationAngleSupported(90) { connection.videoRotationAngle = 90 }
      if connection.isVideoMirroringSupported {
        connection.automaticallyAdjustsVideoMirroring = false
        connection.isVideoMirrored = front
      }
    }
  }
}

extension Camera: AVCapturePhotoCaptureDelegate {
  func photoOutput(_ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto, error: Error?) {
    let data = photo.fileDataRepresentation()
    let id = photo.resolvedSettings.uniqueID
    queue.async { [self] in
      photos.removeValue(forKey: id)?.resume(returning: data)
    }
  }
}

extension Camera: AVCaptureFileOutputRecordingDelegate {
  func fileOutput(
    _ output: AVCaptureFileOutput, didFinishRecordingTo file: URL, from connections: [AVCaptureConnection],
    error: Error?
  ) {
    // Reaching the longest length reports an "error" but the file is complete.
    let done = (error as NSError?)?.userInfo[AVErrorRecordingSuccessfullyFinishedKey] as? Bool ?? (error == nil)
    if !done { try? FileManager.default.removeItem(at: file) }  // a failed recording leaves nothing behind
    queue.async { [self] in
      if let movie {
        self.movie = nil
        videoActive = false
        movie.resume(returning: done ? file : nil)
      } else if done {
        finishedAlone = file  // stopped by the length limit while the name is still held
      } else {
        videoActive = false
      }
    }
  }
}

/// The live picture, filling its frame.
struct CameraPreview: UIViewRepresentable {
  let session: AVCaptureSession

  func makeUIView(context: Context) -> PreviewView {
    let view = PreviewView()
    view.preview.session = session
    view.preview.videoGravity = .resizeAspectFill
    return view
  }

  func updateUIView(_ view: PreviewView, context: Context) {}

  final class PreviewView: UIView {
    override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
    var preview: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
  }
}
