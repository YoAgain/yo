// The camera of the camera mode (the original's camera "context"): a live picture behind the list, a photo taken
// when a name is tapped, a video while a name is held, front or back. AVFoundation has the camera; SwiftUI has no
// view for its preview, so one small UIViewRepresentable carries the preview layer.
import AVFoundation
import CoreImage
import SwiftUI
import YoCore
import os

/// TEMP, until the turn is seen right on the phone: what each capture was told ("YoMediaDiag" in the device log).
let mediaDiag = Logger(subsystem: "app.justyo", category: "media-diag")

/// The capture session. Configured and run on its own queue, as AVFoundation asks; the session, its inputs and
/// outputs are only touched there, which is what makes the class safe to share.
final class Camera: NSObject, @unchecked Sendable {
  /// The longest video a held name records.
  static let longestVideo = Double(productVideoLimitMs()) / 1000

  private lazy var session = AVCaptureSession()
  // Display buffers have no connection to the capture session or its configuration lock.
  private var display: PreviewRenderer?
  private var reveal: (@Sendable (Bool) -> Void)?
  private var revealed = false

  func display(on renderer: PreviewRenderer, reveal: @escaping @Sendable (Bool) -> Void) {
    previewQueue.async { [self] in
      display = renderer
      self.reveal = reveal
      revealed = false
    }
    queue.async { [self] in
      shownOn = renderer  // read once it is in a window (`previewShown`)
      shown = false
    }
  }

  /// The preview's layer is in a window now, or leaves it: only in one can the coordinator read how it stands.
  func previewShown(_ now: Bool) {
    queue.async { [self] in
      shown = now
      if now { coordinate() }
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
  /// The microphone, for the videos' sound: in the session once it is allowed (photos are taken without it).
  private var microphone: AVCaptureDeviceInput?
  private var photos: [Int64: CheckedContinuation<Data?, Never>] = [:]
  private var movie: CheckedContinuation<URL?, Never>?
  /// A video that ended at the length limit before the name was let go; `stopVideo()` hands it over then.
  private var finishedAlone: URL?
  /// A video was started and not yet handed over: while true, `stopVideo()` waits for its file, even when the length
  /// limit stopped it and AVFoundation is still finishing the file.
  private var videoActive = false
  /// AVFoundation's own answer to how the camera in use stands (`orient`); made again with each camera and each
  /// preview layer. Its preview angle is only right while that layer is in a window (`shown`): the last one read
  /// then is kept (`levelPreview`), so a preview going away does not turn the frames.
  private var rotation: AVCaptureDevice.RotationCoordinator?
  private var rotationWatch: NSKeyValueObservation?
  private var shownOn: PreviewRenderer?
  private var shown = false
  private var levelPreview: CGFloat?
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

  /// Asks for the camera once, then for the microphone (videos have sound); false when the camera is refused. A
  /// refused microphone only leaves the videos silent.
  static func allowed() async -> Bool {
    let camera =
      switch AVCaptureDevice.authorizationStatus(for: .video) {
      case .authorized: true
      case .notDetermined: await AVCaptureDevice.requestAccess(for: .video)
      default: false
      }
    if camera, AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
      _ = await AVCaptureDevice.requestAccess(for: .audio)
    }
    return camera
  }

  /// The microphone not asked yet (`allowed` asks it).
  static var microphoneUndecided: Bool { AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined }

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
      if microphone == nil, AVCaptureDevice.authorizationStatus(for: .audio) == .authorized {
        session.beginConfiguration()
        addMicrophone()  // allowed since the session was set up
        session.commitConfiguration()
      }
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
      compensateNot()
      coordinate()  // inside the configuration: the new camera's turn comes with it, not after
      session.commitConfiguration()
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
        diagnose("photo", photoOutput)
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
      orient()  // the turn of the whole video, decided here: kept while it records
      diagnose("video", movieOutput)
      finishedAlone = nil
      videoActive = true
      let file = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mov")
      movieOutput.startRecording(to: file, recordingDelegate: self)
    }
  }

  /// Ends the video and returns its file (its turn in the track, as recorded); nil when none was recording or it
  /// failed.
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
    // Music or a call playing on keeps playing while the camera is open (the microphone would stop it otherwise).
    session.configuresApplicationAudioSessionToMixWithOthers = true
    addMicrophone()
    if session.canAddOutput(photoOutput) { session.addOutput(photoOutput) }
    compensateNot()
    if photoOutput.isResponsiveCaptureSupported { photoOutput.isResponsiveCaptureEnabled = true }
    if photoOutput.isZeroShutterLagSupported { photoOutput.isZeroShutterLagEnabled = true }
    if session.canAddOutput(movieOutput) { session.addOutput(movieOutput) }
    previewOutput.alwaysDiscardsLateVideoFrames = true
    previewOutput.setSampleBufferDelegate(self, queue: previewQueue)
    if session.canAddOutput(previewOutput) { session.addOutput(previewOutput) }
    movieOutput.maxRecordedDuration = CMTime(seconds: Self.longestVideo, preferredTimescale: 600)
    coordinate()  // the preview's turn before the session first runs: a data output turning its frames sets up long
    session.commitConfiguration()
    // The photo pipeline set up ahead (same settings as `photo()`): the first shot is as quick as the next ones,
    // its shutter sound at once instead of after a warm-up.
    let ready = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
    ready.photoQualityPrioritization = .speed
    photoOutput.setPreparedPhotoSettingsArray([ready], completionHandler: nil)
    readZooms()
  }

  /// The microphone into the session, in stereo where the iPhone records stereo; nothing until it is allowed.
  /// Inside a configuration.
  private func addMicrophone() {
    guard microphone == nil, AVCaptureDevice.authorizationStatus(for: .audio) == .authorized,
      let device = AVCaptureDevice.default(for: .audio),
      let new = Diagnostics.attempt({ try AVCaptureDeviceInput(device: device) }), session.canAddInput(new)
    else { return }
    session.addInput(new)
    if new.isMultichannelAudioModeSupported(.stereo) { new.multichannelAudioMode = .stereo }
    microphone = new
  }

  /// Stills in the camera's own orientation, as video is (iOS 26 turns a newer camera's stills to the older
  /// cameras' by default): the coordinator's angles count from there.
  private func compensateNot() {
    if photoOutput.isCameraSensorOrientationCompensationSupported,
      photoOutput.isCameraSensorOrientationCompensationEnabled
    {
      photoOutput.isCameraSensorOrientationCompensationEnabled = false
    }
  }

  /// A coordinator for the camera now in use and the preview's layer, watched for the preview's angle (the capture
  /// angle is read as each photo or video starts).
  private func coordinate() {
    rotationWatch?.invalidate()
    rotationWatch = nil
    guard let device = input?.device else { return }
    if rotation?.device != device { levelPreview = nil }  // each camera stands its own way
    let coordinator = AVCaptureDevice.RotationCoordinator(device: device, previewLayer: shown ? shownOn?.layer : nil)
    rotation = coordinator
    let current = ObjectIdentifier(coordinator)
    rotationWatch = coordinator.observe(\.videoRotationAngleForHorizonLevelPreview) { [weak self] _, _ in
      guard let self else { return }
      queue.async {
        guard self.rotation.map(ObjectIdentifier.init) == current else { return }
        self.orient()
      }
    }
    orient()
  }

  /// The one place that decides how pictures stand. AVFoundation's rotation coordinator knows how the camera in use
  /// is mounted (not every one lies landscape: newer front cameras give portrait frames) and how the phone is held:
  ///
  /// - the preview gets the angle that shows it level in its layer: the frames come turned (a data output turns its
  ///   pixels), and the display shows them as they are;
  /// - photo and video get the angle that stands them level as the phone is held: written beside the pixels (EXIF,
  ///   the track's transform), honoured by everything that reads them. A video's is set as it starts and kept.
  ///
  /// From the front camera all three are mirrored, as a mirror shows a face: what goes is what the preview showed.
  /// A connection is rebuilt with each camera, so this runs after every change of camera too.
  private func orient() {
    guard let rotation, let front = input.map({ $0.device.position == .front }) else { return }
    let capture = rotation.videoRotationAngleForHorizonLevelCapture
    // Out of a window the coordinator says 0 for the preview: the last angle read in one stays (before any, the
    // phone's own).
    if shown, rotation.previewLayer != nil { levelPreview = rotation.videoRotationAngleForHorizonLevelPreview }
    let preview = levelPreview ?? capture
    func set(_ output: AVCaptureOutput, _ angle: CGFloat) {
      guard let connection = output.connection(with: .video) else { return }
      if connection.isVideoMirroringSupported {
        connection.automaticallyAdjustsVideoMirroring = false
        if connection.isVideoMirrored != front { connection.isVideoMirrored = front }
      }
      if connection.isVideoRotationAngleSupported(angle), connection.videoRotationAngle != angle {
        connection.videoRotationAngle = angle
      }
    }
    set(previewOutput, preview)
    set(photoOutput, capture)
    if !movieOutput.isRecording { set(movieOutput, capture) }
  }

  /// TEMP: what a capture was told, to check the coordinator on the phone.
  private func diagnose(_ what: String, _ output: AVCaptureOutput) {
    let connection = output.connection(with: .video)
    let format = input.map { CMVideoFormatDescriptionGetDimensions($0.device.activeFormat.formatDescription) }
    mediaDiag.notice(
      "YoMediaDiag \(what, privacy: .public) front=\(self.input?.device.position == .front, privacy: .public) sensor=\(format?.width ?? 0, privacy: .public)x\(format?.height ?? 0, privacy: .public) capture=\(self.rotation?.videoRotationAngleForHorizonLevelCapture ?? -1, privacy: .public) preview=\(self.rotation?.videoRotationAngleForHorizonLevelPreview ?? -1, privacy: .public) shown=\(self.shown, privacy: .public) angle=\(connection?.videoRotationAngle ?? -1, privacy: .public) mirrored=\(connection?.isVideoMirrored ?? false, privacy: .public)")
  }
}

extension Camera: AVCaptureVideoDataOutputSampleBufferDelegate {
  func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer,
                     from connection: AVCaptureConnection) {
    guard let buffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
    // The frames come level and mirrored as they show (`orient`): shown and kept as they are.
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
    let frame = CIImage(cvPixelBuffer: buffer)
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
    let data = photo.fileDataRepresentation()  // its turn in its EXIF, as `orient` set it
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
/// The layer goes along for the rotation coordinator alone, which reads it on its own.
final class PreviewRenderer: @unchecked Sendable {
  let value: AVSampleBufferVideoRenderer
  let layer: CALayer
  @MainActor init(_ layer: AVSampleBufferDisplayLayer) {
    value = layer.sampleBufferRenderer
    self.layer = layer
  }
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
    view.shown = { [camera] now in camera.previewShown(now) }
    camera.display(
      on: PreviewRenderer(view.feed),
      reveal: { [weak view] ready in
        DispatchQueue.main.async {
          guard let view else { return }
          view.reveal(ready)
        }
      })
    return view
  }

  func updateUIView(_ view: PreviewView, context: Context) {}

  final class PreviewView: UIView {
    let feed = AVSampleBufferDisplayLayer()
    /// Told as the view enters a window and as it leaves: the camera reads the preview's angle from its layer.
    var shown: ((Bool) -> Void)?
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
    override func willMove(toWindow newWindow: UIWindow?) {
      super.willMove(toWindow: newWindow)
      if newWindow == nil { shown?(false) }
    }
    override func didMoveToWindow() {
      super.didMoveToWindow()
      if window != nil { shown?(true) }
    }
    override func layoutSubviews() {
      super.layoutSubviews()
      CATransaction.begin()
      CATransaction.setDisableActions(true)
      feed.frame = bounds
      CATransaction.commit()
    }
  }
}
