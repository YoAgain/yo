// "Yo" in the share sheet: the list of friends as in the app (a coloured bar each, the name large in Montserrat, on
// Amethyst); a tap on a name gives the photos or the video to that friend, and the sheet closes. The extension runs
// no core: what was shared waits in the App Group's outbox, made small and plain already (MediaFile), and goes with
// its Yo as soon as Yo opens (ShareBridge). Up to ten photos or one video (Info: NSExtensionActivationRule).
import SwiftUI
import UIKit
import UniformTypeIdentifiers

@objc(YoShareViewController)
final class ShareViewController: UIViewController {
  private lazy var model = ShareModel { [weak self] sent in
    guard let context = self?.extensionContext else { return }
    if sent {
      context.completeRequest(returningItems: nil)
    } else {
      context.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
    }
  }

  override func viewDidLoad() {
    super.viewDidLoad()
    let host = UIHostingController(rootView: ShareView(model: model))
    addChild(host)
    host.view.frame = view.bounds
    host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    view.addSubview(host.view)
    host.didMove(toParent: self)
    let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] }
    model.prepare(providers)
  }
}

@MainActor @Observable
final class ShareModel {
  enum State: Equatable {
    case picking
    case giving(String)
    case given(String, Int)
    case failed(String)
  }

  let list = ShareBridge.readList()
  let ready = ShareBridge.container != nil
  private(set) var state = State.picking
  /// What was shared: how many photos and videos, for the words above the list.
  private(set) var counts = (photos: 0, videos: 0)
  private var files: Task<[MediaFile], Never>?
  private var giving: Task<Void, Never>?
  private var cancelled = false
  private let done: @MainActor (Bool) -> Void

  init(done: @escaping @MainActor (Bool) -> Void) {
    self.done = done
  }

  var friends: [ShareBridge.Friend]? { list?.friends }

  /// Makes them as they go (a JPEG of at most 1600 pixels, a video of at most 10 seconds) while a name is chosen,
  /// one after the other: an extension has little memory.
  func prepare(_ providers: [NSItemProvider]) {
    var picked: [(NSItemProvider, MediaFile.Kind)] = []
    for provider in providers {
      if provider.hasItemConformingToTypeIdentifier(UTType.movie.identifier), counts.videos == 0 {
        counts.videos += 1
        picked.append((provider, .video))
      } else if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier), counts.photos < 10 {
        counts.photos += 1
        picked.append((provider, .photo))
      }
    }
    files = Task {
      var made: [MediaFile] = []
      for (provider, kind) in picked {
        guard !Task.isCancelled else { break }
        switch kind {
        case .photo:
          guard let data = await Self.imageData(provider) else { continue }
          if let file = await Task.detached(priority: .userInitiated, operation: { MediaFile.photo(data) }).value {
            made.append(file)
          }
        case .video:
          guard let copy = await Self.copyMovie(provider) else { continue }
          if let file = await MediaFile.video(copy) { made.append(file) }
        }
      }
      return made
    }
  }

  var subtitle: String {
    switch state {
    case .picking:
      switch (counts.photos, counts.videos) {
      case (0, 1): return "Tap a name to send this video."
      case (1, 0): return "Tap a name to send this photo."
      case (0, 0): return "Nothing to send."
      default: return "Tap a name to send these \(counts.photos + counts.videos)."
      }
    case .giving, .given: return "It goes as soon as you open Yo."
    case .failed: return "This could not be sent."
    }
  }

  /// The bar's words: the name, else what happens to what was shared ("Sent 2 of 3 📷" when some would not open).
  func title(_ friend: ShareBridge.Friend) -> String {
    let mark = counts.videos > 0 ? "🎥" : "📷"
    let all = counts.photos + counts.videos
    switch state {
    case .giving(friend.to): return "Sending \(mark)"
    case .given(friend.to, let put): return put < all ? "Sent \(put) of \(all) \(mark)" : "Sent \(mark)"
    case .failed(friend.to): return "Not Sent"
    default: return friend.name
    }
  }

  func give(to friend: ShareBridge.Friend) {
    guard state == .picking, let files, let owner = list?.owner else { return }
    state = .giving(friend.to)
    giving = Task {
      let made = await files.value
      // Cancelled while they were being made: nothing is left for the app.
      guard !cancelled else { return }
      let put = await Task.detached(priority: .userInitiated) {
        made.reduce(into: 0) { count, file in if ShareBridge.put(file, for: friend, of: owner) { count += 1 } }
      }.value
      guard put > 0 else {
        state = .failed(friend.to)
        try? await Task.sleep(for: .seconds(1.5))
        done(false)
        return
      }
      state = .given(friend.to, put)
      try? await Task.sleep(for: .seconds(1.2))
      done(true)
    }
  }

  /// Cancel: what is being made stops and nothing is given; once given, the sheet only closes.
  func cancel() {
    if case .given = state { return done(true) }
    cancelled = true
    files?.cancel()
    giving?.cancel()
    done(false)
  }

  /// A photo's bytes, whatever form the app sharing it gives: its data, else (only then) an image.
  private static func imageData(_ provider: NSItemProvider) async -> Data? {
    let data: Data? = await withCheckedContinuation { done in
      _ = provider.loadDataRepresentation(for: .image) { data, _ in done.resume(returning: data) }
    }
    if let data { return data }
    guard provider.canLoadObject(ofClass: UIImage.self) else { return nil }
    return await withCheckedContinuation { done in
      _ = provider.loadObject(ofClass: UIImage.self) { image, _ in
        done.resume(returning: (image as? UIImage)?.jpegData(compressionQuality: 0.9))
      }
    }
  }

  /// The video copied out of the sharing app (its file is ours only while the handler runs).
  private static func copyMovie(_ provider: NSItemProvider) async -> URL? {
    await withCheckedContinuation { done in
      _ = provider.loadFileRepresentation(for: .movie, openInPlace: false) { url, _, _ in
        guard let url else { return done.resume(returning: nil) }
        let copy = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).\(url.pathExtension)")
        done.resume(returning: (try? FileManager.default.copyItem(at: url, to: copy)) != nil ? copy : nil)
      }
    }
  }
}

struct ShareView: View {
  let model: ShareModel

  var body: some View {
    NavigationStack {
      Group {
        if !model.ready {
          note("Sharing to Yo isn’t set up on this iPhone yet.")
        } else if let friends = model.friends {
          if friends.isEmpty {
            note("Add a friend in Yo first.")
          } else {
            ScrollView {
              LazyVStack(spacing: 0) {
                ForEach(friends, id: \.to) { friend in
                  Button {
                    model.give(to: friend)
                  } label: {
                    Text(model.title(friend))
                      .font(.yo(Metrics.rowFont))
                      .foregroundStyle(.white)
                      .lineLimit(1)
                      .padding(.horizontal, 16)
                      .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
                      .background(Color.row(friend.color))
                      .contentShape(.rect)
                  }
                  .buttonStyle(BarPress())
                  .disabled(model.state != .picking)
                }
              }
            }
            .scrollBounceBehavior(.basedOnSize)
          }
        } else {
          note("Open Yo and sign in first.")
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background { Color.amethyst.ignoresSafeArea() }
      .toolbarBackgroundVisibility(.hidden, for: .navigationBar)
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .principal) {
          VStack(spacing: 0) {
            Text("Yo").font(.yo(Metrics.titleFont, relativeTo: .largeTitle))
            if model.ready, model.friends?.isEmpty == false {
              Text(model.subtitle)
                .font(.system(size: 11, weight: .semibold))
                .opacity(0.85)
                .fixedSize()
                .contentTransition(.opacity)
            }
          }
          .foregroundStyle(.white)
          .accessibilityAddTraits(.isHeader)
        }
        ToolbarItem(placement: .topBarLeading) {
          Button("Cancel", systemImage: "xmark") { model.cancel() }
        }
      }
      .animation(.smooth, value: model.state)
    }
    .preferredColorScheme(.dark)  // white on purple, as the app's list
  }

  private func note(_ text: String) -> some View {
    Text(text)
      .font(.yo(20, relativeTo: .title3))
      .foregroundStyle(.white)
      .multilineTextAlignment(.center)
      .padding(32)
  }
}

/// The app's press on a bar (RowPress): a light veil and a hair smaller.
struct BarPress: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .overlay(.white.opacity(configuration.isPressed ? 0.18 : 0))
      .scaleEffect(configuration.isPressed ? 0.985 : 1)
      .animation(.smooth(duration: 0.15), value: configuration.isPressed)
  }
}
