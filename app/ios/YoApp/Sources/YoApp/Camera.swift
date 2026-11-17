// The camera of the camera mode (the original's camera "context"): a live picture behind the list, a photo taken
// when a name is tapped, a video while a name is held, front or back. AVFoundation has the camera; SwiftUI has no
// view for its preview, so one small UIViewRepresentable carries the preview layer.
import AVFoundation
import CoreImage
import ImageIO
import SwiftUI

/// The capture session. Configured and run on its own queue, as AVFoundation asks; the session, its inputs and
/// outputs are only touched there, which is what makes the class safe to share.
final class Camera: NSObject, @unchecked Sendable {
  /// The longest video a held name records.
  static let longestVideo = 10.0

  private lazy var session = AVCaptureSession()
  // Display buffers have no connection to the capture session or its configuration lock.
  private var display: PreviewRenderer?
  private var reveal: (@Sendable (Bool) -> Void)?
  private var revealed = false
  /// How the display turns the frames upright (an EXIF orientation's raw value), told only when it changes.
  private var turn: (@Sendable (UInt32) -> Void)?
  private var turned: CGImagePropertyOrientation?

  func display(
    on renderer: PreviewRenderer, reveal: @escaping @Sendable (Bool) -> Void,
    turn: @escaping @Sendable (UInt32) -> Void
  ) {
    previewQueue.async { [self] in
      display = renderer
      self.reveal = reveal
      self.turn = turn
      turned = nil
      revealed = false
    }
  }
  private lazy var photoOutput = AVCapturePhotoOutput()
  private lazy var previewOutput = AVCaptureVideoDataOutput()
  private let previewQueue = DispatchQueue(label: "yo.camera-preview", qos: .userInitiated)
  private lazy var previewContext = CIContext()
  private let previewLock = NSLock()
  // Only the preview worker writes; the main actor reads under the same lock.
  private var latestPreview: UIImage?
  private var previewTime = -Double.infinity

  func previewThumbnail() -> UIImage? {
    previewLock.withLock { latestPreview }
  }

  private lazy var movieOutput = AVCaptureMovieFileOutput()
  private let queue = DispatchQueue(label: "yo.camera", qos: .userInitiated)
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

  /// Never asks permission or starts capture: safe to call after the list settles.
  static var authorized: Bool { AVCaptureDevice.authorizationStatus(for: .video) == .authorized }

  func prepare() async {
    await withCheckedContinuation { continuation in
      queue.async { [self] in
        guard Self.authorized else { return continuation.resume() }
        if !configured { Perf.timed("camera.configure.prepare") { configure() } }
        previewQueue.async { [self] in
          _ = previewContext  // Core Image/Metal setup belongs before the first captured frame too.
          continuation.resume()
        }
      }
    }
  }

  func start() {
    queue.async { [self] in
      if !configured { Perf.timed("camera.configure.first-start") { configure() } }
      if !session.isRunning {
        previewQueue.async { [self] in
          revealed = false
          display?.value.flush()
          reveal?(false)
        }
        Perf.timed("camera.start") { session.startRunning() }
      }
    }
  }

  func stop() {
    queue.async { [self] in
      if movieOutput.isRecording { movieOutput.stopRecording() }
      if session.isRunning { session.stopRunning() }
      // The last frame goes with it: kept, the next open showed it at once, then hid it on start and faded the new
      // feed in (a flicker on the camera button). Now each open fades in from the first new frame only.
      previewQueue.async { [self] in
        revealed = false
        display?.value.flush()
        reveal?(false)
      }
    }
  }

  /// Front to back and back again.
  func flip() {
    queue.async { [self] in
      let next: AVCaptureDevice.Position = input?.device.position == .back ? .front : .back
      let device =
        next == .back ? Self.back() : AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .front)
      guard let device, let new = Diagnostics.attempt({ try AVCaptureDeviceInput(device: device) })
      else { return }
      session.beginConfiguration()
      if let input { session.removeInput(input) }
      if session.canAddInput(new) {
        session.addInput(new)
        input = new
      } else if let input {
        session.addInput(input)
      }
      session.commitConfiguration()
      orient()
      readZooms()
    }
  }

  /// The zooms the camera in use offers (the Camera app's 0.5, 1, 2, 3; one only: no choice, as the front camera).
  func zoomChoices() async -> [Double] {
    await withCheckedContinuation { continuation in queue.async { [self] in continuation.resume(returning: zooms) } }
  }

  /// To one of `zoomChoices`, quickly, as the Camera app goes; under a pinch (`immediate`) straight with the fingers.
  func zoom(to choice: Double, immediate: Bool = false) {
    queue.async { [self] in
      guard let device = input?.device, (Diagnostics.attempt({ try device.lockForConfiguration() })) != nil else { return }
      let factor = min(max(CGFloat(choice) * wide, device.minAvailableVideoZoomFactor), device.maxAvailableVideoZoomFactor)
      if immediate {
        device.cancelVideoZoomRamp()
        device.videoZoomFactor = factor
      } else {
        device.ramp(toVideoZoomFactor: factor, withRate: 32)
      }
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
    if (Diagnostics.attempt({ try device.lockForConfiguration() })) != nil {
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
        orient()
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
      orient()
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
    if let device = Self.back(), let new = Diagnostics.attempt({ try AVCaptureDeviceInput(device: device) }), session.canAddInput(new)
    {
      session.addInput(new)
      input = new
    }
    if session.canAddOutput(photoOutput) { session.addOutput(photoOutput) }
    if photoOutput.isResponsiveCaptureSupported { photoOutput.isResponsiveCaptureEnabled = true }
    if photoOutput.isZeroShutterLagSupported { photoOutput.isZeroShutterLagEnabled = true }
    if session.canAddOutput(movieOutput) { session.addOutput(movieOutput) }
    previewOutput.alwaysDiscardsLateVideoFrames = true
    previewOutput.setSampleBufferDelegate(self, queue: previewQueue)
    if session.canAddOutput(previewOutput) { session.addOutput(previewOutput) }
    movieOutput.maxRecordedDuration = CMTime(seconds: Self.longestVideo, preferredTimescale: 600)
    session.commitConfiguration()
    orient()
    // The photo pipeline set up ahead (same settings as `photo()`): the first shot is as quick as the next ones,
    // its shutter sound at once instead of after a warm-up.
    let ready = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
    ready.photoQualityPrioritization = .speed
    photoOutput.setPreparedPhotoSettingsArray([ready], completionHandler: nil)
    readZooms()
  }

  /// Photos and videos come out upright in portrait (the app is portrait only), and from the front camera mirrored,
  /// as the preview showed them. Set after every change of camera and again before each photo and video: a
  /// connection is rebuilt with the camera, and the new one starts at the sensor's own angle.
  ///
  /// The preview's frames stay as the sensor gives them (no turn, no mirror): the display turns them itself, from
  /// each frame's own shape (`captureOutput`). A turn asked of the preview's connection was not kept for the front
  /// camera, and its selfie showed sideways.
  private func orient() {
    let front = input?.device.position == .front
    for output in [photoOutput, movieOutput] as [AVCaptureOutput] {
      guard let connection = output.connection(with: .video) else { continue }
      if connection.isVideoRotationAngleSupported(90), connection.videoRotationAngle != 90 {
        connection.videoRotationAngle = 90
      }
      if connection.isVideoMirroringSupported {
        connection.automaticallyAdjustsVideoMirroring = false
        connection.isVideoMirrored = front
      }
    }
    if let connection = previewOutput.connection(with: .video) {
      if connection.isVideoRotationAngleSupported(0), connection.videoRotationAngle != 0 {
        connection.videoRotationAngle = 0
      }
      if connection.isVideoMirroringSupported {
        connection.automaticallyAdjustsVideoMirroring = false
        connection.isVideoMirrored = false
      }
    }
  }

  /// The turn that shows a frame upright in portrait, from its shape alone: a landscape frame is the sensor's own
  /// (turned a quarter right; from the front camera also mirrored, as a mirror shows a face), a portrait one is
  /// upright already (from the front camera only mirrored, unless its connection did).
  fileprivate static func upright(width: Int, height: Int, front: Bool, mirrored: Bool) -> CGImagePropertyOrientation {
    if width > height { return front ? (mirrored ? .left : .leftMirrored) : .right }
    return front && !mirrored ? .upMirrored : .up
  }

  /// A photo that would show landscape gets the portrait orientation in its metadata (the pixels are untouched).
  fileprivate static func upright(photo data: Data, front: Bool) -> Data {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil),
      let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
      let width = properties[kCGImagePropertyPixelWidth] as? Int,
      let height = properties[kCGImagePropertyPixelHeight] as? Int, let type = CGImageSourceGetType(source)
    else { return data }
    let tag = properties[kCGImagePropertyOrientation] as? UInt32 ?? 1
    let quarter = (5...8).contains(tag)
    guard quarter ? height > width : width > height else { return data }  // portrait as it shows: kept
    let fixed: CGImagePropertyOrientation = width > height ? (front ? .leftMirrored : .right) : .up
    let out = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(out, type, 1, nil) else { return data }
    let options = [kCGImagePropertyOrientation: fixed.rawValue] as CFDictionary
    guard CGImageDestinationCopyImageSource(destination, source, options, nil) else { return data }
    return out as Data
  }
}

extension CGImagePropertyOrientation {
  /// The same turn for a layer: in UIKit's coordinates (y down) a positive angle turns clockwise.
  var layerTransform: CGAffineTransform {
    let mirror = CGAffineTransform(scaleX: -1, y: 1)
    return switch self {
    case .upMirrored: mirror
    case .right: CGAffineTransform(rotationAngle: .pi / 2)
    case .left: CGAffineTransform(rotationAngle: -.pi / 2)
    case .leftMirrored: CGAffineTransform(rotationAngle: .pi / 2).concatenating(mirror)
    case .rightMirrored: CGAffineTransform(rotationAngle: -.pi / 2).concatenating(mirror)
    case .down: CGAffineTransform(rotationAngle: .pi)
    case .downMirrored: CGAffineTransform(rotationAngle: .pi).concatenating(mirror)
    default: .identity
    }
  }

  var quarterTurn: Bool { [.left, .right, .leftMirrored, .rightMirrored].contains(self) }
}

extension Camera: AVCaptureVideoDataOutputSampleBufferDelegate {
  func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer,
                     from connection: AVCaptureConnection) {
    guard let buffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
    // Upright whatever the camera and whatever the connection did: decided by each frame's own shape.
    let front = connection.inputPorts.first?.sourceDevicePosition == .front
    let orientation = Self.upright(
      width: CVPixelBufferGetWidth(buffer), height: CVPixelBufferGetHeight(buffer), front: front,
      mirrored: connection.isVideoMirrored)
    if orientation != turned {
      turned = orientation
      turn?(orientation.rawValue)
    }
    if let renderer = display {
      let display = renderer.value
      if display.status == .failed { display.flush() }
      if display.isReadyForMoreMediaData {
        display.enqueue(sampleBuffer)
        if !revealed { revealed = true; reveal?(true) }
      }
    }
    let now = CMSampleBufferGetPresentationTimeStamp(sampleBuffer).seconds
    guard now - previewTime >= 0.1 else { return }
    previewTime = now
    let frame = CIImage(cvPixelBuffer: buffer).oriented(orientation)
    let scale = 360 / max(frame.extent.width, frame.extent.height)
    let small = frame.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
    guard let pixels = previewContext.createCGImage(small, from: small.extent) else { return }
    let image = UIImage(cgImage: pixels)
    previewLock.withLock { latestPreview = image }
  }
}

extension Camera: AVCapturePhotoCaptureDelegate {
  func photoOutput(_ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto, error: Error?) {
    if let error { Diagnostics.record(error) }
    let front = output.connection(with: .video)?.inputPorts.first?.sourceDevicePosition == .front
    let data = photo.fileDataRepresentation().map { Self.upright(photo: $0, front: front) }
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
    if !done, let error { Diagnostics.record(error) }
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

/// AVFoundation's renderer is used only by the serial preview worker. The main actor
/// owns its layer but never calls renderer methods, so capture never needs a main-actor hop.
final class PreviewRenderer: @unchecked Sendable {
  let value: AVSampleBufferVideoRenderer
  init(_ value: AVSampleBufferVideoRenderer) { self.value = value }
}

/// Persistent, session-independent surface. A first buffer reveals only the feed, never the wipe.
struct CameraPreview: UIViewRepresentable {
  let camera: Camera

  func makeUIView(context: Context) -> PreviewView {
    let view = PreviewView()
    // No black card before the first frame: the rows' colours show through until the feed fades in.
    view.backgroundColor = .clear
    view.feed.videoGravity = .resizeAspectFill
    view.feed.opacity = 0
    camera.display(
      on: PreviewRenderer(view.feed.sampleBufferRenderer),
      reveal: { [weak view] ready in
        DispatchQueue.main.async {
          guard let view else { return }
          view.reveal(ready)
        }
      },
      turn: { [weak view] raw in
        DispatchQueue.main.async {
          guard let view, let orientation = CGImagePropertyOrientation(rawValue: raw) else { return }
          view.turn = orientation
        }
      })
    return view
  }

  func updateUIView(_ view: PreviewView, context: Context) {}

  final class PreviewView: UIView {
    let feed = AVSampleBufferDisplayLayer()
    /// The frames' turn to upright (the sensor's landscape frames a quarter right; a selfie mirrored as well).
    var turn = CGImagePropertyOrientation.up {
      didSet { if turn != oldValue { setNeedsLayout() } }
    }
    /// A plain blur over the first moments (no white tint): hides focus and exposure settling while the edge between
    /// the clear window and the colour rows stays sharp. Before the first frame there is nothing over the window.
    private let milk = UIVisualEffectView(effect: nil)
    /// Holds the blur part-way in: a blur without the material's milky wash.
    private var blur: UIViewPropertyAnimator?
    override init(frame: CGRect) {
      super.init(frame: frame)
      layer.addSublayer(feed)
      milk.frame = bounds
      milk.autoresizingMask = [.flexibleWidth, .flexibleHeight]
      milk.isUserInteractionEnabled = false
      addSubview(milk)
    }

    private func clearBlur() {
      blur?.stopAnimation(true)
      blur = nil
      milk.effect = nil
      milk.alpha = 1
    }

    /// First frame: the feed fades in blurred (250 ms), then the blur clears (450 ms, after the camera had a moment
    /// to focus). Gone: back to the start, at once.
    func reveal(_ ready: Bool) {
      feed.removeAllAnimations()
      milk.layer.removeAllAnimations()
      clearBlur()
      guard ready else {
        feed.opacity = 0
        return
      }
      // Test: no blur ("milk") over the first moments: with the see-through rows it was too much.
      milk.alpha = 0
      let fade = CABasicAnimation(keyPath: "opacity")
      fade.fromValue = 0
      fade.toValue = 1
      fade.duration = 0.25
      fade.timingFunction = CAMediaTimingFunction(name: .easeOut)
      feed.opacity = 1
      feed.add(fade, forKey: "fade")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func layoutSubviews() {
      super.layoutSubviews()
      CATransaction.begin()
      CATransaction.setDisableActions(true)
      // Turned about its centre: a quarter turn lays it out with width and height swapped, then turns it to fill.
      feed.setAffineTransform(.identity)
      let size = turn.quarterTurn ? CGSize(width: bounds.height, height: bounds.width) : bounds.size
      feed.bounds = CGRect(origin: .zero, size: size)
      feed.position = CGPoint(x: bounds.midX, y: bounds.midY)
      feed.setAffineTransform(turn.layerTransform)
      CATransaction.commit()
    }
  }
}
