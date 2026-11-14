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
  /// The zooms the camera in use offers, as the Camera app names them (0.5, 1, 2, 3); and the device's factor that
  /// is "1" (the wide camera's, behind the ultra wide one of a multi-camera device).
  private var zooms: [Double] = [1]
  private var wide: CGFloat = 1

  /// The back cameras, all lenses as one device where the iPhone has several (it switches between them as it zooms),
  /// else the wide one.
  private static func back() -> AVCaptureDevice? {
    [.builtInTripleCamera, .builtInDualWideCamera, .builtInDualCamera, .builtInWideAngleCamera].lazy
      .compactMap { AVCaptureDevice.default($0, for: .video, position: .back) }.first
  }

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
      let device =
        next == .back ? Self.back() : AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .front)
      guard let device, let new = try? AVCaptureDeviceInput(device: device)
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
      readZooms()
    }
  }

  /// The zooms the camera in use offers (the Camera app's 0.5, 1, 2, 3; one only: no choice, as the front camera).
  func zoomChoices() async -> [Double] {
    await withCheckedContinuation { continuation in queue.async { [self] in continuation.resume(returning: zooms) } }
  }

  /// To one of `zoomChoices`, quickly, as the Camera app goes.
  func zoom(to choice: Double) {
    queue.async { [self] in
      guard let device = input?.device, (try? device.lockForConfiguration()) != nil else { return }
      let factor = min(max(CGFloat(choice) * wide, device.minAvailableVideoZoomFactor), device.maxAvailableVideoZoomFactor)
      device.ramp(toVideoZoomFactor: factor, withRate: 32)
      device.unlockForConfiguration()
    }
  }

  /// The choices of the camera now in use, and its zoom back at "1".
  private func readZooms() {
    guard let device = input?.device, device.position == .back else {
      (zooms, wide) = ([1], 1)
      return
    }
    let switches = device.virtualDeviceSwitchOverVideoZoomFactors.map { CGFloat(truncating: $0) }
    let ultraWide = device.constituentDevices.contains { $0.deviceType == .builtInUltraWideCamera }
    wide = ultraWide ? (switches.first ?? 1) : 1
    // Each lens, and 2 on the wide one where no lens is 2 (as the Camera app offers it).
    var factors = Set([1] + switches + [2 * wide])
    factors = factors.filter { $0 >= device.minAvailableVideoZoomFactor && $0 <= device.maxAvailableVideoZoomFactor }
    zooms = factors.sorted().map { (Double($0 / wide) * 10).rounded() / 10 }
    if (try? device.lockForConfiguration()) != nil {
      device.videoZoomFactor = max(wide, device.minAvailableVideoZoomFactor)
      device.unlockForConfiguration()
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
    if let device = Self.back(), let new = try? AVCaptureDeviceInput(device: device), session.canAddInput(new)
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
    readZooms()
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
