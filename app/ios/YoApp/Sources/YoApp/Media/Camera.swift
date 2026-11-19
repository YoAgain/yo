// The camera mode: a live preview behind the list, a photo on tap, a video while a name is held. SwiftUI has no
// preview view, so one UIViewRepresentable carries the preview layer.
import AVFoundation
import CoreImage
import SwiftUI

/// The session, its inputs and outputs are only touched on `queue`, which is what makes the class safe to share.
final class Camera: NSObject, @unchecked Sendable {

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
      shownOn = renderer
      shown = false
    }
  }

  /// Only while the preview layer is in a window can the rotation coordinator read its angle.
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
  private var microphone: AVCaptureDeviceInput?
  private var photos: [Int64: CheckedContinuation<Data?, Never>] = [:]
  private var movie: CheckedContinuation<URL?, Never>?
  /// A video that hit the length limit before the name was let go; `stopVideo()` hands it over then.
  private var finishedAlone: URL?
  /// While true, `stopVideo()` waits for the file even when the limit stopped it and AVFoundation is still writing.
  private var videoActive = false
  /// The preview angle is only right while the layer is in a window: the last one read there is kept
  /// (`levelPreview`), so a preview going away does not turn the frames.
  private var rotation: AVCaptureDevice.RotationCoordinator?
  private var rotationWatch: NSKeyValueObservation?
  private var shownOn: PreviewRenderer?
  private var shown = false
  private var levelPreview: CGFloat?
  /// The zooms as the Camera app names them (0.5, 1, 2, 3); `wide` is the device factor shown as "1".
  private var zooms: [Double] = [1]
  private var wide: CGFloat = 1
  /// The most this camera zooms, counted as the choices are (1 = the wide lens).
  private var most: Double = 1

  /// All back lenses as one virtual device where there are several (it switches as it zooms).
  private static func back() -> AVCaptureDevice? {
    [.builtInTripleCamera, .builtInDualWideCamera, .builtInDualCamera, .builtInWideAngleCamera].lazy
      .compactMap { AVCaptureDevice.default($0, for: .video, position: .back) }.first
  }

  /// False when the camera is refused; a refused microphone only leaves videos silent.
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

  static var microphoneUndecided: Bool { AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined }

  /// Never asks permission or starts capture: safe to call after the list settles.
  static var authorized: Bool { AVCaptureDevice.authorizationStatus(for: .video) == .authorized }

  func prepare() async {
    await withCheckedContinuation { continuation in
      queue.async { [self] in
        guard Self.authorized else { return continuation.resume() }
        if !configured { Perf.timed("camera.configure.prepare") { configure() } }
        previewQueue.async { [self] in
          _ = previewContext  // Core Image/Metal setup before the first frame
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
        addMicrophone()
        session.commitConfiguration()
      }
      if !session.isRunning {
        previewQueue.async { [self] in
          revealed = false
          display?.value.flush()
          reveal?(false)
        }
        allowHaptics()
        Perf.timed("camera.start") { session.startRunning() }
        allowHaptics()  // running, the session has set the audio session up again
      }
    }
  }

  func stop() {
    queue.async { [self] in
      if movieOutput.isRecording { movieOutput.stopRecording() }
      if session.isRunning { session.stopRunning() }
      // A kept last frame flickered on the next open; each open fades in from its first new frame only.
      previewQueue.async { [self] in
        revealed = false
        display?.value.flush()
        reveal?(false)
      }
    }
  }

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
      coordinate()  // inside the configuration, so the new camera's rotation comes with it
      session.commitConfiguration()
      readZooms()
    }
  }

  func zoomChoices() async -> [Double] {
    await withCheckedContinuation { continuation in queue.async { [self] in continuation.resume(returning: zooms) } }
  }

  /// Read from the device as it is now (its most may change once it runs).
  func zoomMost() async -> Double {
    await withCheckedContinuation { continuation in
      queue.async { [self] in
        if let device = input?.device { most = Double(device.maxAvailableVideoZoomFactor / wide) }
        continuation.resume(returning: most)
      }
    }
  }

  /// Ramps as the Camera app does; `immediate` follows a pinch.
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

  private func readZooms() {
    guard let device = input?.device, device.position == .back else {
      (zooms, wide) = ([1], 1)
      most = Double(input?.device.maxAvailableVideoZoomFactor ?? 1)
      return
    }
    let switches = device.virtualDeviceSwitchOverVideoZoomFactors.map { CGFloat(truncating: $0) }
    let ultraWide = device.constituentDevices.contains { $0.deviceType == .builtInUltraWideCamera }
    wide = ultraWide ? (switches.first ?? 1) : 1
    // Each lens, plus 2× on the wide one (as the Camera app offers it).
    var factors = Set([1] + switches + [2 * wide])
    factors = factors.filter { $0 >= device.minAvailableVideoZoomFactor && $0 <= device.maxAvailableVideoZoomFactor }
    zooms = factors.sorted().map { (Double($0 / wide) * 10).rounded() / 10 }
    most = Double(device.maxAvailableVideoZoomFactor / wide)
    if (Diagnostics.attempt({ try device.lockForConfiguration() })) != nil {
      device.videoZoomFactor = max(wide, device.minAvailableVideoZoomFactor)
      device.unlockForConfiguration()
    }
  }

  /// Nil when the camera is not running.
  func photo() async -> Data? {
    await withCheckedContinuation { continuation in
      queue.async { [self] in
        // No camera in the Simulator: capturing without one raises.
        guard session.isRunning, photoOutput.connection(with: .video) != nil else {
          return continuation.resume(returning: nil)
        }
        orient()
        let settings = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
        settings.photoQualityPrioritization = .speed
        photos[settings.uniqueID] = continuation
        photoOutput.capturePhoto(with: settings, delegate: self)
      }
    }
  }

  func startVideo() {
    queue.async { [self] in
      guard session.isRunning, movieOutput.connection(with: .video) != nil, !movieOutput.isRecording, !videoActive
      else { return }
      orient()  // the whole video keeps this rotation
      allowHaptics()
      finishedAlone = nil
      videoActive = true
      let file = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mov")
      keepLens(true)
      movieOutput.startRecording(to: file, recordingDelegate: self)
    }
  }

  /// While filming, the lens it started on stays and zooms on its own: a camera of several lenses switched between
  /// them as the drawn finger zoomed back, and the picture jumped. Apple advises the same for recording.
  private func keepLens(_ keep: Bool) {
    guard let device = input?.device, device.isVirtualDevice,
      (Diagnostics.attempt { try device.lockForConfiguration() }) != nil
    else { return }
    device.setPrimaryConstituentDeviceSwitchingBehavior(keep ? .locked : .auto, restrictedSwitchingBehaviorConditions: [])
    device.unlockForConfiguration()
  }

  /// A running microphone mutes every haptic of the app (taps, holds, the zoom's tick) unless allowed; the capture
  /// session sets the audio session up as it starts and records, so this is said again each time.
  private func allowHaptics() {
    let audio = AVAudioSession.sharedInstance()
    guard !audio.allowHapticsAndSystemSoundsDuringRecording else { return }
    _ = Diagnostics.attempt { try audio.setAllowHapticsAndSystemSoundsDuringRecording(true) }
  }

  /// Nil when none was recording or it failed.
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
          movie = continuation  // stopped by the limit, still being written: the delegate hands it over
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
    // Music or a call keeps playing while the camera is open.
    session.configuresApplicationAudioSessionToMixWithOthers = true
    allowHaptics()
    addMicrophone()
    if session.canAddOutput(photoOutput) { session.addOutput(photoOutput) }
    compensateNot()
    if photoOutput.isResponsiveCaptureSupported { photoOutput.isResponsiveCaptureEnabled = true }
    if photoOutput.isZeroShutterLagSupported { photoOutput.isZeroShutterLagEnabled = true }
    if session.canAddOutput(movieOutput) { session.addOutput(movieOutput) }
    previewOutput.alwaysDiscardsLateVideoFrames = true
    previewOutput.setSampleBufferDelegate(self, queue: previewQueue)
    if session.canAddOutput(previewOutput) { session.addOutput(previewOutput) }
    movieOutput.maxRecordedDuration = CMTime(seconds: MediaFile.longestVideo, preferredTimescale: 600)
    coordinate()  // before the first run: a data output rotating its frames is slow to set up later
    session.commitConfiguration()
    // Prepared ahead (same settings as `photo()`) so the first shot is as quick as the next.
    let ready = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
    ready.photoQualityPrioritization = .speed
    photoOutput.setPreparedPhotoSettingsArray([ready], completionHandler: nil)
    readZooms()
  }

  /// Inside a configuration; nothing until the microphone is allowed.
  private func addMicrophone() {
    guard microphone == nil, AVCaptureDevice.authorizationStatus(for: .audio) == .authorized,
      let device = AVCaptureDevice.default(for: .audio),
      let new = Diagnostics.attempt({ try AVCaptureDeviceInput(device: device) }), session.canAddInput(new)
    else { return }
    session.addInput(new)
    if new.isMultichannelAudioModeSupported(.stereo) { new.multichannelAudioMode = .stereo }
    microphone = new
  }

  /// iOS 26 rotates newer cameras' stills to the older cameras' orientation by default; the coordinator's angles
  /// count from the sensor's own.
  private func compensateNot() {
    if photoOutput.isCameraSensorOrientationCompensationSupported,
      photoOutput.isCameraSensorOrientationCompensationEnabled
    {
      photoOutput.isCameraSensorOrientationCompensationEnabled = false
    }
  }

  private func coordinate() {
    rotationWatch?.invalidate()
    rotationWatch = nil
    guard let device = input?.device else { return }
    if rotation?.device != device { levelPreview = nil }
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

  /// The one place that decides rotation (newer front cameras give portrait frames). The preview's data output
  /// rotates its pixels; photo and video get the capture angle as metadata (EXIF, track transform). The front
  /// camera mirrors all three, so what goes is what the preview showed. Connections are rebuilt per camera.
  private func orient() {
    guard let rotation, let front = input.map({ $0.device.position == .front }) else { return }
    let capture = rotation.videoRotationAngleForHorizonLevelCapture
    // Out of a window the coordinator says 0 for the preview: keep the last angle read in one.
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
}

extension Camera: AVCaptureVideoDataOutputSampleBufferDelegate {
  func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer,
                     from connection: AVCaptureConnection) {
    guard let buffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
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
    if !done, let error { Diagnostics.record(error) }
    if !done { try? FileManager.default.removeItem(at: file) }
    queue.async { [self] in
      keepLens(false)
      if let movie {
        self.movie = nil
        videoActive = false
        movie.resume(returning: done ? file : nil)
      } else if done {
        finishedAlone = file
      } else {
        videoActive = false
      }
    }
  }
}

/// Only the preview queue calls the renderer; the main actor owns the layer but never calls it, so capture never
/// hops to the main actor. The layer is carried for the rotation coordinator alone.
final class PreviewRenderer: @unchecked Sendable {
  let value: AVSampleBufferVideoRenderer
  let layer: CALayer
  @MainActor init(_ layer: AVSampleBufferDisplayLayer) {
    value = layer.sampleBufferRenderer
    self.layer = layer
  }
}

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
    var shown: ((Bool) -> Void)?
    override init(frame: CGRect) {
      super.init(frame: frame)
      layer.addSublayer(feed)
    }

    /// First frame: the feed fades in (250 ms). Gone: hidden at once.
    func reveal(_ ready: Bool) {
      feed.removeAllAnimations()
      guard ready else {
        feed.opacity = 0
        return
      }
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
