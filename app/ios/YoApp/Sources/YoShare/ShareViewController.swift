// "Yo" in the share sheet: the list of friends as in the app (a coloured bar each, the name large in Montserrat, on
// Amethyst); a tap on a name sends the photos or the video to that friend, from here, and the sheet closes. It sends
// as the app does, with the app's own state (SharedState.swift: in turns with the app, under the lock in the App
// Group): made small and plain (MediaFile), sealed, uploaded, its Yo sent, one after the other (an extension has
// little memory). What the hub cannot take now (no network, "not now") waits in the group's outbox instead and goes
// as soon as Yo opens (ShareBridge). Up to ten photos or one video (Info: NSExtensionActivationRule).
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import YoCore

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
    /// Sent: how many of them.
    case given(String, Int)
    /// Waiting for the app (some or all): how many were sent or wait.
    case queued(String, Int)
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
  /// Cancel, for the sending under way off the main actor: the next photo does not go.
  private let stop = Stop()
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
    case .giving: return "Sending…"
    case .given: return "Sent."
    case .queued: return "It goes as soon as you open Yo."
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
    case .queued(friend.to, let put): return put < all ? "\(put) of \(all) \(mark) Waiting" : "Waiting \(mark)"
    case .failed(friend.to): return "Not Sent"
    default: return friend.name
    }
  }

  func give(to friend: ShareBridge.Friend) {
    guard state == .picking, let files, let owner = list?.owner else { return }
    state = .giving(friend.to)
    giving = Task {
      let made = await files.value
      // Cancelled while they were being made: nothing goes.
      guard !cancelled else { return }
      let stop = stop
      let outcome = await Task.detached(priority: .userInitiated) {
        Self.send(made, to: friend, owner: owner, stop: stop)
      }.value
      // Cancelled meanwhile: the sheet is closed already; what went, went.
      guard !cancelled else { return }
      switch outcome {
      case .sent(let sent) where sent > 0:
        state = .given(friend.to, sent)
      case .queued(let count) where count > 0:
        state = .queued(friend.to, count)
      default:
        state = .failed(friend.to)
        try? await Task.sleep(for: .seconds(1.5))
        done(false)
        return
      }
      try? await Task.sleep(for: .seconds(1.2))
      done(true)
    }
  }

  enum Outcome: Sendable {
    case sent(Int)
    /// Some or all left for the app: how many were sent or wait.
    case queued(Int)
  }

  /// Sends them from here, one after the other: sealed, uploaded, then its Yo (the core's `media_chat`,
  /// `upload_media`, `send_photo` or `send_video`, as the app's outbox). Without the app's state here (not moved into
  /// the group yet, the keychain locked, another account), without the lock within a few seconds, or once the hub
  /// cannot take one now (no network, "not now"), that one and the rest wait in the group's outbox for the app, as
  /// before. One the hub refuses for good (blocked, not on Yo) is not sent.
  nonisolated private static func send(
    _ made: [MediaFile], to friend: ShareBridge.Friend, owner: String, stop: Stop
  ) -> Outcome {
    var sent = 0
    var left = made[...]
    let client = Guest.open(within: 5)
    if let client, ownerOf(client) == owner {
      // Yos that came while sending: saved for the app, which has not seen them, and their push's notification
      // service will find them read. Their notification is written here (`notify`).
      var received: [YoEvent] = []
      defer { notify(received, client: client) }
      while let file = left.first, !stop.isSet {
        do {
          let sealed = try sealMedia(
            plain: file.data, width: UInt16(clamping: file.width), height: UInt16(clamping: file.height))
          let chat = try client.mediaChat(to: friend.to)
          received += chat.events
          try client.uploadMedia(media: sealed.media, ciphertext: sealed.ciphertext, fetches: chat.fetches)
          // Once the hub has the blob the Yo goes: offline it waits in the core's own outbox, sent by the app later.
          switch file.kind {
          case .photo: received += try client.sendPhoto(to: friend.to, media: sealed.media)
          case .video: received += try client.sendVideo(to: friend.to, media: sealed.media, durationMs: file.durationMs)
          }
          sent += 1
        } catch let error as YoError where mediaRetryLater(error: error) {
          break
        } catch {
          // Refused for good (or the state would not do): this one is not sent; the next may be.
        }
        left = left.dropFirst()
      }
    }
    // Cancelled: the rest is neither sent nor left for the app.
    guard !left.isEmpty, !stop.isSet else { return .sent(sent) }
    let put = left.reduce(into: 0) { count, file in if ShareBridge.put(file, for: friend, of: owner) { count += 1 } }
    return put > 0 ? .queued(sent + put) : .sent(sent)
  }

  /// A notification for the Yos received while sending, as the notification service writes it.
  nonisolated private static func notify(_ events: [YoEvent], client: YoClient) {
    let yos = ShareBridge.yos(in: events) { ShareBridge.name(of: $0, client: client) }
    guard !yos.isEmpty else { return }
    Task { await ShareBridge.post(yos) }
  }

  /// The account the state is of, as the list names it (its user id's bytes in base64).
  nonisolated private static func ownerOf(_ client: YoClient) -> String? {
    Hub.bytes(client.userId())?.base64EncodedString()
  }

  /// Cancel: what is being made stops and nothing is given; once given, the sheet only closes.
  func cancel() {
    if case .given = state { return done(true) }
    if case .queued = state { return done(true) }
    cancelled = true
    stop.set()
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

/// A flag set once, read from any thread.
final class Stop: @unchecked Sendable {
  private let lock = NSLock()
  private var value = false

  var isSet: Bool { lock.withLock { value } }
  func set() { lock.withLock { value = true } }
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
