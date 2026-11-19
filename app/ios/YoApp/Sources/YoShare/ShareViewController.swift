// Tap the Yo bars to send the same media to several friends or groups, then tap Done.
// It sends as the app does, with the app's own state (SharedState.swift: in turns with the app, under the lock in the
// App Group, its key from the shared keychain): made small and plain (MediaFile), sealed, uploaded, its Yo sent, one
// after the other (an extension has little memory, about 120 MB). What the hub cannot take now (no network, "not
// now"), or all of it when the state does not open here (no account, the keychain locked, the app holding it), waits
// in the group's outbox instead and goes as soon as Yo opens (ShareBridge); so does a photo or video there was no
// memory left to make small here (the app makes it). Up to ten photos or one video (Info: NSExtensionActivationRule).
// A video longer than a Yo's (10 s) is offered to Yo's trimmer first (VideoTrimmer); only where that cannot
// be is it refused.
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import YoCore
import os

@objc(YoShareViewController)
final class ShareViewController: UIViewController {
  private var trim: VideoTrim?
  private var appeared = false
  private var presentingTrim = false
  private lazy var model = ShareModel(showTrim: { [weak self] in
    self?.showTrim($0)
  }, done: { [weak self] in
    self?.trim?.finish(.cancelled)
    self?.extensionContext?.completeRequest(returningItems: nil)
  })

  override func viewDidAppear(_ animated: Bool) {
    super.viewDidAppear(animated)
    appeared = true
    presentTrimIfReady()
  }

  override func viewDidDisappear(_ animated: Bool) {
    super.viewDidDisappear(animated)
    appeared = false
    if !presentingTrim { model.cancel() }
  }

  private func showTrim(_ trim: VideoTrim?) {
    self.trim = trim
    if trim == nil, presentingTrim {
      dismiss(animated: true) { self.presentingTrim = false }
    } else {
      presentTrimIfReady()
    }
  }

  private func presentTrimIfReady() {
    guard appeared, !presentingTrim, let trim else { return }
    presentingTrim = true
    let controller = UIHostingController(rootView: VideoTrimmer(trim: trim))
    controller.modalPresentationStyle = .fullScreen
    controller.overrideUserInterfaceStyle = .dark
    present(controller, animated: true)
  }

  @objc private func hostInBackground() { model.cancel() }

  override func viewDidLoad() {
    super.viewDidLoad()
    overrideUserInterfaceStyle = .dark
    NotificationCenter.default.addObserver(
      self, selector: #selector(hostInBackground), name: .NSExtensionHostDidEnterBackground, object: nil)
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
  enum Feedback: Equatable {
    case sending, sent, queued, failed
  }

  /// One shared photo or video as prepared: made small here, or its original for the app to make (no memory left).
  enum Shared: Sendable {
    case ready(MediaFile)
    case original(URL, MediaFile.Kind)
  }

  let list = ShareBridge.readList()
  let ready = ShareBridge.container != nil
  private(set) var feedback: [String: Feedback] = [:]
  private(set) var sending = false
  /// What was shared: how many photos and videos, for the words above the list.
  private(set) var counts = (photos: 0, videos: 0)
  /// The video is longer than a Yo's (`MediaFile.longestVideo`) and the trimmer could not cut it: refused whole,
  /// nothing goes.
  private(set) var tooLong = false
  private var files: Task<[Shared], Never>?
  private var originals: [URL] = []
  private var cancelled = false
  /// Cancel, for the sending under way off the main actor: the next photo does not go.
  private let stop = Stop()
  let lifetime = SharedAccess.Lifetime()
  private let showTrim: (VideoTrim?) -> Void
  private let done: () -> Void

  init(showTrim: @escaping (VideoTrim?) -> Void, done: @escaping () -> Void) {
    self.showTrim = showTrim
    self.done = done
  }

  var friends: [ShareBridge.Friend]? { list?.friends }

  /// "📷" or "🎥", as the island and the bars say it.
  var mark: String { productMediaSign(kind: counts.videos > 0 ? .video : .photo) }

  /// Makes them as they go (a HEIC of at most 2048 pixels, an HEVC video of at most 10 seconds) while a name is chosen,
  /// one after the other: an extension has little memory. One there is no room for (`room`) is kept as it was shared
  /// and goes to the app, which makes it; a video that would not make here goes to the app as well.
  func prepare(_ providers: [NSItemProvider]) {
    var picked: [(NSItemProvider, MediaFile.Kind)] = []
    let limits = productShareLimits()
    for provider in providers {
      if provider.hasItemConformingToTypeIdentifier(UTType.movie.identifier), counts.videos < Int(limits.videos) {
        counts.videos += 1
        picked.append((provider, .video))
      } else if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier), counts.photos < Int(limits.photos) {
        counts.photos += 1
        picked.append((provider, .photo))
      }
    }
    files = Task {
      var made: [Shared] = []
      for (provider, kind) in picked {
        guard !Task.isCancelled else { break }
        switch kind {
        case .photo:
          if let url = await Self.copyFile(provider, type: .image) {
            guard Self.room(for: .photo) else { made.append(.original(url, .photo)); continue }
            let file = await Task.detached(priority: .userInitiated) {
              // Mapped, not read in: a large original costs no memory of its own.
              (try? Data(contentsOf: url, options: .alwaysMapped)).flatMap(MediaFile.photo)
            }.value
            try? FileManager.default.removeItem(at: url)
            Self.measure("photo made")
            if let file { made.append(.ready(file)) }
          } else if let data = await Self.imageData(provider),
            let file = await Task.detached(priority: .userInitiated, operation: { MediaFile.photo(data) }).value
          {
            made.append(.ready(file))
          }
        case .video:
          guard var url = await Self.copyFile(provider, type: .movie) else { continue }
          // Longer than a Yo's video: Yo's trimmer offers the part to send. Closed, the share closes; where
          // it cannot cut, the video is refused and nothing of this share goes.
          if let seconds = await MediaFile.length(of: url), !MediaFile.fits(seconds), !cancelled {
            let result = await VideoTrim.run(url, show: showTrim)
            try? FileManager.default.removeItem(at: url)
            if case .cut(let cut) = result {
              url = cut
            } else {
              for case .original(let other, _) in made { try? FileManager.default.removeItem(at: other) }
              if case .cancelled = result { cancel() } else { tooLong = true }
              return []
            }
          }
          // MediaFile deletes what it reads: it reads a clone, the original stays for the app should it fail.
          let work = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).\(url.pathExtension)")
          guard Self.room(for: .video), (try? FileManager.default.copyItem(at: url, to: work)) != nil else {
            made.append(.original(url, .video))
            continue
          }
          let file = await MediaFile.video(work)
          Self.measure("video made")
          if let file {
            try? FileManager.default.removeItem(at: url)
            made.append(.ready(file))
          } else {
            made.append(.original(url, .video))
          }
        }
      }
      originals = made.compactMap {
        if case .original(let url, _) = $0 { return url }
        return nil
      }
      if cancelled { removeOriginals() }
      return made
    }
  }

  /// The memory an extension may still use (iOS ends it beyond about 120 MB). Making a photo of 2048 pixels takes
  /// some 50 MB at its peak, an HEVC video of 720p (the hardware encoder, its frames in flight) some 80 MB; with less
  /// left it goes to the app instead. 0: not known (not limited), made here.
  nonisolated private static func room(for kind: MediaFile.Kind) -> Bool {
    let left = os_proc_available_memory()
    measure("before \(kind.rawValue)")
    return left == 0 || left >= (kind == .photo ? 60 : 90) << 20
  }

  nonisolated private static func measure(_ step: String) {
    Logger(subsystem: "app.justyo", category: "share-memory").notice(
      "\(step, privacy: .public): \(os_proc_available_memory() >> 20, privacy: .public) MB left")
  }

  var subtitle: String {
    productShareSubtitle(photos: UInt32(counts.photos), videos: UInt32(counts.videos), lang: .english)
  }

  func title(_ friend: ShareBridge.Friend) -> String {
    switch feedback[friend.to] {
    case .sending: return productSendWords(mark: mark, sending: true, lang: .english)
    case .sent: return productSendWords(mark: mark, sending: false, lang: .english)
    case .queued: return productText(text: .shareQueued, lang: .english)
    case .failed: return productText(text: .notSent, lang: .english)
    case nil: return friend.name
    }
  }

  func canGive(to friend: ShareBridge.Friend) -> Bool {
    productShareCanSend(sending: sending, tooLong: tooLong, hasMedia: counts.photos + counts.videos > 0,
      hasFeedback: feedback[friend.to] != nil)
  }

  func give(to friend: ShareBridge.Friend) {
    guard canGive(to: friend), let files, let owner = list?.owner else { return }
    sending = true
    feedback[friend.to] = .sending
    Task {
      defer {
        sending = false
        if cancelled { removeOriginals() }
      }
      let made = await files.value
      guard !cancelled else { return }
      let stop = stop
      let lifetime = lifetime
      let outcome = await Task.detached(priority: .userInitiated) {
        lifetime.run { Self.send(made, to: friend, owner: owner, stop: stop) }
      }.value
      if outcome.sent > 0 { SentNote.note(to: friend.to) }
      if outcome.failed > 0 || made.isEmpty {
        feedback[friend.to] = .failed
      } else {
        feedback[friend.to] = outcome.queued > 0 ? .queued : .sent
      }
    }
  }

  struct Outcome: Sendable {
    var sent = 0
    /// Left for the app.
    var queued = 0
    /// Refused here (for good, or the state would not do).
    var failed = 0
  }

  /// Sends them from here, one after the other: sealed, uploaded, then its Yo (the core's `media_chat`,
  /// `upload_media`, `send_photo` or `send_video`, as the app's outbox). Without the app's state here (not moved into
  /// the group yet, the keychain locked, another account), without the lock within a few seconds, or once the hub
  /// cannot take one now (no network, "not now"), that one and the rest wait in the group's outbox for the app, as
  /// do the originals not made here. A refusal is shown on the recipient’s bar.
  nonisolated private static func send(
    _ made: [Shared], to friend: ShareBridge.Friend, owner: String, stop: Stop
  ) -> Outcome {
    var outcome = Outcome()
    var later: [Shared] = []
    var left = made[...]
    let client = Guest.open(within: 5)
    if let client, ownerOf(client) == owner {
      // Yos that came while sending: saved for the app, which has not seen them, and their push's notification
      // service will find them read. Their notification is written here (`notify`).
      var received: [YoEvent] = []
      defer { notify(received, client: client) }
      while let item = left.first, !stop.isSet {
        left = left.dropFirst()
        guard case .ready(let file) = item else { later.append(item); continue }
        do {
          let sealed = try sealMedia(
            plain: file.data, width: UInt16(clamping: file.width), height: UInt16(clamping: file.height))
          let chat = try client.mediaChat(toTarget: friend.to)
          received += chat.events
          try client.uploadMedia(media: sealed.media, ciphertext: sealed.ciphertext, fetches: chat.fetches)
          // Once the hub has the blob the Yo goes: offline it waits in the core's own outbox, sent by the app later.
          switch file.kind {
          case .photo: received += try client.sendPhoto(toTarget: friend.to, media: sealed.media)
          case .video: received += try client.sendVideo(toTarget: friend.to, media: sealed.media, durationMs: file.durationMs)
          }
          outcome.sent += 1
        } catch let error as YoError where retryLater(error: error) {
          later.append(item)
          break
        } catch {
          Diagnostics.record(error)
          outcome.failed += 1
        }
      }
    }
    // Cancelled: the rest is neither sent nor left for the app.
    guard !stop.isSet else { return outcome }
    for item in later + left {
      guard !stop.isSet else { break }
      if hand(item, to: friend, of: owner) { outcome.queued += 1 } else { outcome.failed += 1 }
    }
    return outcome
  }

  /// One into the group's outbox for the app.
  nonisolated private static func hand(_ item: Shared, to friend: ShareBridge.Friend, of owner: String) -> Bool {
    switch item {
    case .ready(let file): ShareBridge.put(file, for: friend, of: owner)
    case .original(let url, let kind): ShareBridge.putOriginal(url, kind: kind, for: friend, of: owner)
    }
  }

  /// A notification for the Yos received while sending, as the notification service writes it.
  nonisolated private static func notify(_ events: [YoEvent], client: YoClient) {
    let yos = ShareBridge.yos(
        in: events, name: { ShareBridge.name(of: $0, client: client) },
        group: { id in (try? client.groups())?.first { $0.id == id }?.name ?? "" }, requests: ShareBridge.requests(of: client))
    guard !yos.isEmpty else { return }
    ShareBridge.noteLast(yos, client: client)
    Task { await ShareBridge.post(yos) }
  }

  /// The account the state is of, as the list names it (its user id's bytes in base64).
  nonisolated private static func ownerOf(_ client: YoClient) -> String? {
    Hub.bytes(client.userId())?.base64EncodedString()
  }

  func cancel() {
    guard !cancelled else { return }
    cancelled = true
    stop.set()
    lifetime.stop()
    files?.cancel()
    if !sending { removeOriginals() }
    done()
  }

  private func removeOriginals() {
    for url in originals { try? FileManager.default.removeItem(at: url) }
    originals.removeAll()
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

  /// The photo or video copied out of the sharing app (its file is ours only while the handler runs); nil when it
  /// gives no file (a photo then comes as data, `imageData`).
  private static func copyFile(_ provider: NSItemProvider, type: UTType) async -> URL? {
    await withCheckedContinuation { done in
      _ = provider.loadFileRepresentation(for: type, openInPlace: false) { url, _, _ in
        guard let url else { return done.resume(returning: nil) }
        let copy = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).\(url.pathExtension)")
        done.resume(returning: (try? FileManager.default.copyItem(at: url, to: copy)) != nil ? copy : nil)
      }
    }
  }
}

/// A Yo sent from outside the list, for the app's list to show as its own ("Sent Yo", the bar on top): the same note
/// in the App Group and the same Darwin notification as the intents write (IntentSent in YoIntents.swift, which this
/// target does not compile; keep the two alike).
private struct SentNote: Codable {
  let to: String
  var back = false
  let at: Date

  static func note(to: String) {
    guard let file = ShareBridge.container?.appending(path: "intent-sent.json") else { return }
    let old = (try? Data(contentsOf: file)).flatMap { try? JSONDecoder().decode([SentNote].self, from: $0) } ?? []
    guard let data = try? JSONEncoder().encode(Array((old + [SentNote(to: to, at: .now)]).suffix(20))) else { return }
    try? data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    CFNotificationCenterPostNotification(
      CFNotificationCenterGetDarwinNotifyCenter(), CFNotificationName((ShareBridge.group + ".intentSent") as CFString),
      nil, nil, true)
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
          note(productText(text: .shareUnavailable, lang: .english))
        } else if model.tooLong {
          TooLongCard()
            .transition(.scale(scale: 0.9).combined(with: .opacity))
        } else if let friends = model.friends {
          if friends.isEmpty {
            note(productText(text: .shareAddFriend, lang: .english))
          } else {
            ScrollView {
              LazyVStack(spacing: 0) {
                ForEach(friends, id: \.to) { friend in
                  Button {
                    model.give(to: friend)
                  } label: {
                    Text(model.title(friend))
                      .font(.yo(Metrics.rowFont))
                      .foregroundStyle(model.feedback[friend.to] == .failed ? Color.alizarin : .white)
                      .lineLimit(1)
                      .minimumScaleFactor(0.6)
                      .accessibilityLabel(friend.name)
                      .accessibilityValue(model.feedback[friend.to] == nil ? "" : model.title(friend))
                      .padding(.horizontal, 16)
                      .frame(maxWidth: .infinity, minHeight: Metrics.rowHeight)
                      .background(Color.row(friend.color))
                      .contentShape(.rect)
                  }
                  .buttonStyle(BarPress())
                  .disabled(!model.canGive(to: friend))
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
            if model.ready, model.friends?.isEmpty == false, !model.tooLong {
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
        ToolbarItem(placement: .confirmationAction) {
          Button("Done") { model.cancel() }
        }
      }
      .animation(.smooth, value: model.feedback)
      .animation(.smooth, value: model.tooLong)
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

/// An over-limit video the trimmer could not open or save.
struct TooLongCard: View {
  var body: some View {
    HStack(spacing: 10) {
      Text(verbatim: productText(text: .videoTooLong, lang: .english)).foregroundStyle(Color.alizarin).fixedSize()
      Text(verbatim: "·").foregroundStyle(.white.opacity(0.5)).fixedSize()
      Text(verbatim: productVideoLimitLabel()).foregroundStyle(.white).fixedSize()
    }
    .font(.yo(22, relativeTo: .title2))
    .padding(.horizontal, 24)
    .frame(minHeight: 60)
    .background(.black, in: .capsule)
    .accessibilityElement(children: .combine)
    .padding(24)
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
