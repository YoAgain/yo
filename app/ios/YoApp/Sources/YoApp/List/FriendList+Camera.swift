import AVFoundation
import ImageIO
import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import YoCore
import _PhotosUI_SwiftUI

extension FriendList {
  /// Media from the library, shown as soon as its picture is decoded (once, at the size it is sent); the bytes it
  /// goes as are made meanwhile, off the main actor, and a tap awaits them only if they are not ready yet.
  struct Picked: Equatable {
    let image: UIImage
    let thumbnail: UIImage?
    let isVideo: Bool
    let file: Task<MediaFile?, Never>

    static func == (lhs: Self, rhs: Self) -> Bool {
      lhs.file == rhs.file && lhs.image === rhs.image && lhs.thumbnail === rhs.thumbnail
    }

    /// A photo: one decode serves the screen, the bar's thumbnail and the HEIC that goes.
    static func photo(_ data: Data) async -> Picked? {
      await Task.detached(priority: .userInitiated) { () -> Picked? in
        guard let sized = MediaFile.sized(data) else { return nil }
        // The screen's own decoded copy: drawing never waits on the encoder reading `sized` (it held the main
        // thread for the encode's length).
        let shown = UIImage(cgImage: sized).preparingForDisplay() ?? UIImage(cgImage: sized)
        let file = Task.detached(priority: .userInitiated) { MediaFile.photo(sized) }
        return Picked(image: shown, thumbnail: FriendList.scaled(sized, to: 360), isVideo: false, file: file)
      }.value
    }
  }

  /// A picked video as a file of its own (not its bytes in memory), copied out before the picker removes it.
  struct PickedMovie: Transferable {
    let url: URL
    static var transferRepresentation: some TransferRepresentation {
      FileRepresentation(contentType: .movie) { SentTransferredFile($0.url) } importing: { received in
        let ending = received.file.pathExtension.isEmpty ? "mov" : received.file.pathExtension
        let copy = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).\(ending)")
        try FileManager.default.copyItem(at: received.file, to: copy)
        return Self(url: copy)
      }
    }
  }

  /// What a library item gave before its turn in the selection's order.
  enum Fetched: Sendable {
    case photo(Picked)
    case movie(URL)
  }

  /// `image` at most `side` pixels long, for the bar's thumbnail.
  nonisolated static func scaled(_ image: CGImage, to side: Int) -> UIImage? {
    let scale = min(1, CGFloat(side) / CGFloat(max(image.width, image.height)))
    let size = CGSize(width: max(1, (CGFloat(image.width) * scale).rounded()),
                      height: max(1, (CGFloat(image.height) * scale).rounded()))
    let format = UIGraphicsImageRendererFormat()
    format.scale = 1
    return UIGraphicsImageRenderer(size: size, format: format).image { _ in
      UIImage(cgImage: image).draw(in: CGRect(origin: .zero, size: size))
    }
  }

  var cameraLook: Double {
    let progress = Double(edgeOffset)
    return cameraOn ? 1 - progress : progress
  }

  var navigation: some View {
    GeometryReader { geometry in
      ZStack {
        Color.amethyst.ignoresSafeArea()
        listNavigation
          .environment(\.sharedListColors, true)
          .allowsHitTesting(!edgeActive && !edgeSettling)
      }
      .gesture(CameraEdgePan(
        left: cameraOn != leftHanded,
        enabled: !edgeSettling && viewing == nil && recordingFor == nil && !sheetUp,
        touched: {
          if !cameraOn && Camera.authorized { camera.start() }
        },
        released: {
          if !cameraOn && !edgeActive { camera.stop() }
        },
        changed: { x in
          if !edgeActive {
            #if YO_DEV
            HitchMonitor.shared.scene = "edge wipe"
            #endif
            edgeActive = true
            swiped = nil
          }
          let direction: CGFloat = cameraOn != leftHanded ? 1 : -1
          var transaction = Transaction()
          transaction.disablesAnimations = true
          withTransaction(transaction) {
            edgeOffset = min(1, max(0, x * direction / geometry.size.width))
          }
        }, ended: { x, velocity, cancelled in
          guard edgeActive else { return }
          let direction: CGFloat = cameraOn != leftHanded ? 1 : -1
          let complete = !cancelled && (x * direction > geometry.size.width * 0.4
            || velocity * direction >= 500)
          if complete { Haptic.tap() }
          edgeSettling = true
          withAnimation(reduceMotion ? nil : .easeOut(duration: 0.25), completionCriteria: .removed) {
            edgeOffset = complete ? 1 : 0
          } completion: {
            var transaction = Transaction()
            transaction.disablesAnimations = true
            withTransaction(transaction) {
              if complete { cameraSwitched(!cameraOn) }
              edgeOffset = 0
              edgeActive = false
              edgeSettling = false
            }
            HitchScene.event("edge wipe", then: cameraOn ? "camera" : "list")
            if !cameraOn { camera.stop() }
          }
        }))
    }
    .ignoresSafeArea(.container)
  }

  /// In camera mode what to do, in a glass capsule over the controls.
  var title: some View {
    Text(libraryLoading ? "Loading selected media…" : picked.isEmpty ? "Tap or hold a name" : "Tap a name to send selected media")
      .font(.system(size: 15, weight: .semibold))
      .foregroundStyle(.white)
      .lineLimit(1)
      .fixedSize()
      .padding(.horizontal, 18)
      .frame(height: Metrics.titleHeight)
      .glassEffect(.regular, in: .capsule)
      .allowsHitTesting(false)  // the bars under it still take photos
      .transition(.opacity.combined(with: .scale(scale: 0.9)))
  }

  @ViewBuilder var cameraBackdrop: some View {
    if cameraOn, picked.count > 1 {
      GeometryReader { geometry in
        let columns = picked.count <= 4 ? 2 : 3
        let rows = (picked.count + columns - 1) / columns
        let side = max(0, min((geometry.size.width - CGFloat(columns - 1)) / CGFloat(columns),
          (geometry.size.height - CGFloat(rows - 1)) / CGFloat(rows)))
        VStack(spacing: 1) {
          ForEach(0..<rows, id: \.self) { row in
            HStack(spacing: 1) {
              ForEach(0..<columns, id: \.self) { column in
                let index = row * columns + column
                if index < picked.count {
                  Image(uiImage: picked[index].image)
                    .resizable().scaledToFill()
                    .frame(width: side, height: side)
                    .clipped()
                } else {
                  Color.black.frame(width: side, height: side)
                }
              }
            }
          }
        }
        .frame(width: geometry.size.width, height: geometry.size.height)
      }
      .background(.black)
    } else if cameraOn, let item = picked.first {
      Image(uiImage: item.image).resizable().scaledToFill()
    } else {
      CameraPreview(camera: camera)
    }
  }

  @ViewBuilder var leadingControl: some View {
    ZStack {
      settingsButton.opacity(cameraOn ? 0 : 1).allowsHitTesting(!cameraOn).accessibilityHidden(cameraOn)
      if cameraOn || cameraPrepared {
        // `.current`: the item as it is stored, never transcoded first (`.automatic` took seconds for a large photo).
        PhotosPicker(selection: $libraryItems, maxSelectionCount: 10, selectionBehavior: .ordered,
                     matching: .any(of: [.images, .videos]), preferredItemEncoding: .current) {
          Label("Photo Library", systemImage: "photo.on.rectangle")
        }
        .opacity(cameraOn ? 1 : 0).allowsHitTesting(cameraOn).accessibilityHidden(!cameraOn)
      }
    }
  }

  /// Media from the library chosen (or still loading): the taps send it, the camera's own controls are gone.
  var selecting: Bool { !libraryItems.isEmpty || !picked.isEmpty }

  /// A sheet over the list (Settings and what opens from it, New Group, members): the camera does not open under it.
  var sheetUp: Bool { settings || newGroup || membersOf != nil }

  var flipButton: some View {
    Button {
      Haptic.tap(.light)
      camera.flip()
      Task { await readZooms() }
    } label: { Label("Switch Camera", systemImage: "arrow.triangle.2.circlepath") }
  }

  var cameraButton: some View {
    Button {
      // At once, no wipe from the side.
      var transaction = Transaction()
      transaction.disablesAnimations = true
      withTransaction(transaction) {
        // Closed in one press, with media picked or not; the selection goes with it.
        if cameraOn && selecting {
          picked = []
          libraryItems = []
          cameraSwitched(false)
        } else if cameraOn || !sheetUp { cameraSwitched(!cameraOn) }
      }
    } label: {
      Label(cameraOn ? "Close Camera" : "Camera",
            systemImage: cameraOn ? "xmark" : "camera.fill")
    }
    .font(.system(size: 30))
    .background {
      if cameraPrepared { Image(systemName: "xmark").font(.system(size: 30)).hidden() }
    }
  }

  func switchCamera(_ on: Bool) async {
    guard on else { return camera.stop() }
    let refusedBefore = Camera.refused
    _ = Self.outgoingThumbnailQueue  // enqueue decode warm-up before camera startup/capture
    // The microphone is asked too where the camera was allowed before videos had sound.
    if Camera.authorized, !Camera.microphoneUndecided { if cameraOn { camera.start() } }
    else if await Camera.allowed() { if cameraOn { camera.start() } } else {
      cameraSwitched(false)
      // Turned off in Settings before: say so (it closed without a word). A "Don't Allow" just given is respected.
      cameraRefused = refusedBefore
    }
    await readZooms()
  }

  func readZooms() async {
    zooms = await camera.zoomChoices()
    zoomMost = await camera.zoomMost()
    zoom = 1
  }

  /// Fetches and decodes the items side by side (at most three at once), shows them in selection order as each is
  /// ready; a video's export runs on after its first frame shows. A selection superseded by another pick or closed
  /// cannot land.
  func load(_ items: [PhotosPickerItem]) {
    picked = []
    libraryLoading = !items.isEmpty
    guard !items.isEmpty else { return }
    Perf.mark("pick.start \(items.count)")
    Task {
      await withTaskGroup(of: (Int, Fetched?).self) { group in
        var next = 0
        func add() {
          guard next < items.count else { return }
          let (index, item) = (next, items[next])
          next += 1
          group.addTask {
            if item.supportedContentTypes.contains(where: { $0.conforms(to: .movie) }) {
              return (index, (try? await item.loadTransferable(type: PickedMovie.self)).map { .movie($0.url) })
            }
            guard let data = try? await item.loadTransferable(type: Data.self) else { return (index, nil) }
            Perf.mark("pick.fetched \(index) \(data.count)")
            return (index, await Picked.photo(data).map { .photo($0) })
          }
        }
        for _ in 0..<3 { add() }
        var ready: [Int: Fetched?] = [:]
        var shown = 0
        var made: [Picked] = []
        while let (index, fetched) = await group.next() {
          add()
          guard libraryItems == items else {
            group.cancelAll()
            if case .movie(let url) = fetched { Self.removeLater(url) }
            continue
          }
          ready[index] = fetched
          // In order: a later item waits for the earlier ones (and a long video's trimmer, one at a time).
          while let one = ready.removeValue(forKey: shown) {
            shown += 1
            guard libraryItems == items else {
              if case .movie(let url) = one { Self.removeLater(url) }
              continue
            }
            switch one {
            case .photo(let photo): made.append(photo)
            case .movie(let url): if let video = await pickedVideo(url) { made.append(video) }
            case nil: break
            }
            if libraryItems == items {
              if picked.isEmpty && !made.isEmpty { Perf.mark("pick.shown \(made.count)") }
              picked = made
            }
          }
        }
      }
      if libraryItems == items {
        libraryLoading = false
        Perf.mark("pick.ready \(picked.count)")
      }
    }
  }

  nonisolated static func removeLater(_ url: URL) {
    Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: url) }
  }

  /// A picked video: trimmed first where it is longer than a Yo's (closed, left out; where it cannot cut, refused),
  /// shown with its first frame, and exported to a Yo's limits in the background (which removes the source).
  func pickedVideo(_ url: URL) async -> Picked? {
    var source = url
    if let seconds = await MediaFile.length(of: url), !MediaFile.fits(seconds) {
      let result = await VideoTrim.run(url) { videoTrim = $0 }
      Self.removeLater(url)
      switch result {
      case .cut(let cut): source = cut
      case .cancelled: return nil
      case .failed:
        problem = (productText(text: .videoTooLong, lang: .english) + " · " + productVideoLimitLabel(), "")
        return nil
      }
    }
    guard let image = await Self.outgoingVideoThumbnail(source) else {
      Self.removeLater(source)
      return nil
    }
    let export = source
    return Picked(image: image, thumbnail: image, isVideo: true,
                  file: Task(priority: .userInitiated) { await MediaFile.video(export) })
  }

  func cameraSwitched(_ on: Bool) {
    cameraOn = on
    // A list at its top stays there: the first bar never moves on the switch.
    if !scrolled {
      var instant = Transaction(); instant.disablesAnimations = true
      withTransaction(instant) { position.scrollTo(edge: .top) }
    }
    if on { hintNeeded = true }
    HitchScene.event("edge wipe", then: on ? "camera" : "list")
    publish(listModel.camera(now: listNow, on: on))
    if !on {
      sightings.reset()  // a code still held up asks again at the next opening
      // Only the session closes: selection and pending batches stay until Clear Selection or Undo.
      if recordingFor != nil {  // a hold the camera closed under: no video waits
        recordingChanged(nil)
        Task {
          if let file = await camera.stopVideo() { Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: file) } }
          flushRise()
        }
      }
    }
    Task { await switchCamera(on) }
  }
}

/// The camera's zoom: a glass circle with the factor ("1×"); each tap the next choice. Hidden with one choice.
struct CameraZoom: View {
  let choices: [Double]
  @Binding var chosen: Double
  /// Under a pinch the number follows the fingers at once (no rolling digits).
  var pinching = false
  /// Under a pinch at most twenty updates a second, as the Camera app; the last one always lands.
  @State private var shown: Double?
  @State private var shownAt = Date.distantPast

  var body: some View {
    if choices.count > 1 {
      Button {
        Haptic.tap(.light)
        let next = choices.first { $0 > chosen + 0.001 } ?? choices[0]
        withAnimation(.snappy(duration: 0.2)) { chosen = next }
      } label: {
        Text(Self.label(pinching ? (shown ?? chosen) : chosen) + "×")
          .font(.system(size: 13, weight: .bold).monospacedDigit())
          .foregroundStyle(.white)
          .contentTransition(pinching ? .identity : .numericText())
          .transaction { if pinching { $0.animation = nil } }
          // A fixed square, so the glass stays a circle whatever the number ("1.7×" shrinks to fit).
          .lineLimit(1)
          .minimumScaleFactor(0.8)
          .padding(6)
          .frame(width: 44, height: 44)
          .glassEffect(.regular.interactive(), in: .circle)
          .contentShape(.circle)
      }
      .onChange(of: chosen) {
        guard pinching else { shown = nil; return }
        if Date.now.timeIntervalSince(shownAt) >= 0.05 { shown = chosen; shownAt = .now }
      }
      .onChange(of: pinching) { if !pinching { shown = nil } }
      .accessibilityLabel("Zoom \(Self.label(chosen))×")
      .accessibilityHint("Changes the zoom")
      .buttonStyle(.plain)
      .transition(.opacity)
    }
  }

  /// "0.5", "1", "2", "2.5": no ".0".
  static func label(_ choice: Double) -> String {
    choice == choice.rounded() ? String(Int(choice)) : String(format: "%.1f", choice)
  }
}
