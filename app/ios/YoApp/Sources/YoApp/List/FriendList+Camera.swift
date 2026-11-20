import AVFoundation
import ImageIO
import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import YoCore
import _PhotosUI_SwiftUI

extension FriendList {
  /// A photo from the library, decoded and scaled once (a 48-megapixel photo decoded at each redraw stalled the list).
  struct Picked: Equatable {
    let data: Data
    var video: MediaFile? = nil
    let image: UIImage
    let thumbnail: UIImage?

    static func == (lhs: Self, rhs: Self) -> Bool {
      lhs.data == rhs.data && lhs.video?.data == rhs.video?.data
        && lhs.image === rhs.image && lhs.thumbnail === rhs.thumbnail
    }

    static func make(_ data: Data) async -> Picked? {
      await Task.detached(priority: .userInitiated) {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let options: [CFString: Any] = [
          kCGImageSourceCreateThumbnailFromImageAlways: true,
          kCGImageSourceCreateThumbnailWithTransform: true,
          kCGImageSourceThumbnailMaxPixelSize: 2868,
          kCGImageSourceShouldCacheImmediately: true,
        ]
        guard let pixels = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        return Picked(data: data, image: UIImage(cgImage: pixels),
                      thumbnail: FriendList.decodeOutgoingThumbnail(data))
      }.value
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
    Text(picked.isEmpty ? "Tap or hold a name" : "Tap a name to send selected media")
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
        let side = (geometry.size.width - CGFloat(columns - 1)) / CGFloat(columns)
        let rows = (picked.count + columns - 1) / columns
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
        PhotosPicker(selection: $libraryItems, maxSelectionCount: 10, selectionBehavior: .ordered,
                   matching: .any(of: [.images, .videos])) {
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
        // With media picked: back to the live camera; with nothing picked: closed.
        if cameraOn && selecting {
          picked = []
          libraryItems = []
        } else if cameraOn || !sheetUp { cameraSwitched(!cameraOn) }
      }
    } label: {
      Label(cameraOn ? (selecting ? "Clear Selection" : "Close Camera") : "Camera",
            systemImage: cameraOn ? "xmark" : "camera.fill")
    }
    .font(.system(size: 30))
    .background {
      if cameraPrepared { Image(systemName: "xmark").font(.system(size: 30)).hidden() }
    }
  }

  func switchCamera(_ on: Bool) async {
    guard on else { return camera.stop() }
    _ = Self.outgoingThumbnailQueue  // enqueue decode warm-up before camera startup/capture
    // The microphone is asked too where the camera was allowed before videos had sound.
    if Camera.authorized, !Camera.microphoneUndecided { if cameraOn { camera.start() } }
    else if await Camera.allowed() { if cameraOn { camera.start() } } else { cameraSwitched(false) }
    await readZooms()
  }

  func readZooms() async {
    zooms = await camera.zoomChoices()
    zoomMost = await camera.zoomMost()
    zoom = 1
  }

  /// Loads in selection order; a selection superseded by another pick or closed cannot land.
  func load(_ items: [PhotosPickerItem]) {
    picked = []
    guard !items.isEmpty else { return }
    Task {
      var made: [Picked] = []
      for item in items {
        guard libraryItems == items else { return }
        guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
        if item.supportedContentTypes.contains(where: { $0.conforms(to: .movie) }) {
          let url = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).mov")
          do {
            try await Task.detached { try data.write(to: url) }.value
            var source = url
            // Longer than a Yo's video: trimmed; closed, left out; where it cannot cut, refused.
            if let seconds = await MediaFile.length(of: url), !MediaFile.fits(seconds) {
              let result = await VideoTrim.run(url) { videoTrim = $0 }
              Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: url) }
              switch result {
              case .cut(let cut): source = cut
              case .cancelled: continue
              case .failed:
                problem = (productText(text: .videoTooLong, lang: .english) + " · " + productVideoLimitLabel(), "")
                continue
              }
            }
            let image = await Self.outgoingVideoThumbnail(source)
            // Export applies the same limits as camera video and removes the temporary source.
            let video = await MediaFile.video(source)
            if let video, let image {
              made.append(Picked(data: Data(), video: video, image: image, thumbnail: image))
            }
          } catch {
            Task.detached(priority: .utility) { try? FileManager.default.removeItem(at: url) }
          }
        } else if let photo = await Picked.make(data) {
          made.append(photo)
        }
      }
      if libraryItems == items { picked = made }
    }
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
