// "Yo" in the share sheet: the list of friends as in the app (a coloured bar each, the name large in Montserrat, on
// Amethyst); a tap on a name sends the photos or the video to that friend (or group), from here, right away, and a
// short card says so before the sheet closes, as the island does ("Yo 📷 · lina", "3× 📷 · lina", "No 📷 · lina").
// It sends as the app does, with the app's own state (SharedState.swift: in turns with the app, under the lock in the
// App Group, its key from the shared keychain): made small and plain (MediaFile), sealed, uploaded, its Yo sent, one
// after the other (an extension has little memory, about 120 MB). What the hub cannot take now (no network, "not
// now"), or all of it when the state does not open here (no account, the keychain locked, the app holding it), waits
// in the group's outbox instead and goes as soon as Yo opens (ShareBridge); so does a photo or video there was no
// memory left to make small here (the app makes it). Up to ten photos or one video (Info: NSExtensionActivationRule).
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import YoCore
import os

@objc(YoShareViewController)
final class ShareViewController: UIViewController {
  private lazy var model: ShareModel = ShareModel { [weak self] sent in
    self?.model.lifetime.stop()
    guard let context = self?.extensionContext else { return }
    if sent {
      context.completeRequest(returningItems: nil)
    } else {
      context.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
    }
  }

  override func viewDidDisappear(_ animated: Bool) {
    super.viewDidDisappear(animated)
    model.lifetime.stop()  // the host dismissed the sheet without our Cancel button
  }

  @objc private func hostInBackground() { model.cancel() }

  override func viewDidLoad() {
    super.viewDidLoad()
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
  /// What happened to what was shared, for the card.
  struct Done: Equatable {
    let friend: ShareBridge.Friend
    /// Sent from here.
    var sent: Int
    /// Waiting for the app.
    var queued: Int
    /// Not sent (the hub refused it here); "Send when Yo opens" offers them to the app.
    var failed: Int
  }

  enum State: Equatable {
    case picking
    case giving(String)
    case done(Done)
  }

  /// One shared photo or video as prepared: made small here, or its original for the app to make (no memory left).
  enum Shared: Sendable {
    case ready(MediaFile)
    case original(URL, MediaFile.Kind)
  }

  let list = ShareBridge.readList()
  let ready = ShareBridge.container != nil
  private(set) var state = State.picking
  /// What was shared: how many photos and videos, for the words above the list.
  private(set) var counts = (photos: 0, videos: 0)
  /// The video is longer than a Yo's (`MediaFile.longestVideo`): refused whole, nothing goes.
  private(set) var tooLong = false
  private var files: Task<[Shared], Never>?
  private var giving: Task<Void, Never>?
  /// Those the hub refused here, for "Send when Yo opens".
  private var failed: [Shared] = []
  private var cancelled = false
  /// Cancel, for the sending under way off the main actor: the next photo does not go.
  private let stop = Stop()
  let lifetime = SharedAccess.Lifetime()
  private let done: @MainActor (Bool) -> Void

  init(done: @escaping @MainActor (Bool) -> Void) {
    self.done = done
  }

  var friends: [ShareBridge.Friend]? { list?.friends }

  /// "📷" or "🎥", as the island and the bars say it.
  var mark: String { counts.videos > 0 ? "🎥" : "📷" }

  /// Makes them as they go (a HEIC of at most 2048 pixels, an HEVC video of at most 10 seconds) while a name is chosen,
  /// one after the other: an extension has little memory. One there is no room for (`room`) is kept as it was shared
  /// and goes to the app, which makes it; a video that would not make here goes to the app as well.
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
          guard let url = await Self.copyFile(provider, type: .movie) else { continue }
          // Longer than a Yo's video: never cut, refused, and nothing of this share goes.
          if let seconds = await MediaFile.length(of: url), !MediaFile.fits(seconds) {
            try? FileManager.default.removeItem(at: url)
            for case .original(let other, _) in made { try? FileManager.default.removeItem(at: other) }
            tooLong = true
            return []
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
    switch state {
    case .picking:
      switch (counts.photos, counts.videos) {
      case (0, 1): return "Tap a name to send this video."
      case (1, 0): return "Tap a name to send this photo."
      case (0, 0): return "Nothing to send."
      default: return "Tap a name to send these \(counts.photos + counts.videos)."
      }
    case .giving: return "Sending…"
    case .done(let done) where done.sent > 0: return "Sent."
    case .done(let done) where done.queued > 0: return "Sends when you open Yo."
    case .done: return "This could not be sent."
    }
  }

  /// The bar's words while it goes: "Sending 📷", else the name.
  func title(_ friend: ShareBridge.Friend) -> String {
    state == .giving(friend.to) ? "Sending \(mark)" : friend.name
  }

  func give(to friend: ShareBridge.Friend) {
    guard state == .picking, !tooLong, let files, let owner = list?.owner else { return }
    state = .giving(friend.to)
    giving = Task {
      let made = await files.value
      // Cancelled while they were being made: nothing goes.
      guard !cancelled else { return }
      let stop = stop
      let lifetime = lifetime
      let outcome = await Task.detached(priority: .userInitiated) {
        lifetime.run { Self.send(made, to: friend, owner: owner, stop: stop) }
      }.value
      // Cancelled meanwhile: the sheet is closed already; what went, went.
      guard !cancelled else { return }
      // The app's list shows it as its own ("Sent Yo", the bar on top), as a Yo from the Action button.
      if outcome.sent > 0 { SentNote.note(to: friend.to) }
      failed = outcome.failed
      state = .done(Done(friend: friend, sent: outcome.sent, queued: outcome.queued, failed: outcome.failed.count))
      // Refused: the card stays, with "Send when Yo opens", until that or Cancel.
      guard failed.isEmpty else { return }
      try? await Task.sleep(for: .seconds(1.6))
      done(outcome.sent + outcome.queued > 0)
    }
  }

  /// "Send when Yo opens" on a card that says "No": those refused here wait for the app, which tries them again.
  func sendLater() {
    guard case .done(var shown) = state, !failed.isEmpty, let owner = list?.owner else { return }
    let left = failed
    failed = []
    Task {
      let friend = shown.friend
      let put = await Task.detached(priority: .userInitiated) {
        left.reduce(into: 0) { count, item in if Self.hand(item, to: friend, of: owner) { count += 1 } }
      }.value
      shown.queued += put
      shown.failed = 0
      state = .done(shown)
      try? await Task.sleep(for: .seconds(1.2))
      done(shown.sent + shown.queued > 0)
    }
  }

  struct Outcome: Sendable {
    var sent = 0
    /// Left for the app.
    var queued = 0
    /// Refused here (for good, or the state would not do).
    var failed: [Shared] = []
  }

  /// Sends them from here, one after the other: sealed, uploaded, then its Yo (the core's `media_chat`,
  /// `upload_media`, `send_photo` or `send_video`, as the app's outbox). Without the app's state here (not moved into
  /// the group yet, the keychain locked, another account), without the lock within a few seconds, or once the hub
  /// cannot take one now (no network, "not now"), that one and the rest wait in the group's outbox for the app, as
  /// do the originals not made here. One the hub refuses here is offered to the app on the card.
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
          outcome.failed.append(item)
        }
      }
    }
    // Cancelled: the rest is neither sent nor left for the app.
    guard !stop.isSet else { return outcome }
    for item in later + left where hand(item, to: friend, of: owner) { outcome.queued += 1 }
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

  /// Cancel: what is being made stops and nothing is given; once given, the sheet only closes.
  func cancel() {
    if case .done(let shown) = state { return done(shown.sent + shown.queued > 0) }
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
          note("Sharing to Yo isn’t set up on this iPhone yet.")
        } else if model.tooLong {
          TooLongCard()
            .transition(.scale(scale: 0.9).combined(with: .opacity))
        } else if case .done(let done) = model.state {
          SentCard(done: done, mark: model.mark, later: done.failed > 0 ? { model.sendLater() } : nil)
            .transition(.scale(scale: 0.9).combined(with: .opacity))
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
        ToolbarItem(placement: .topBarLeading) {
          Button("Cancel", systemImage: "xmark") { model.cancel() }
        }
      }
      .animation(.smooth, value: model.state)
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

/// What happened, as the island says it (YoLiveActivity): a black pill, "Yo 📷" in Yo's purple ("3× 📷"; "No 📷" in
/// red when it did not go; faded while it waits for the app), then the friend's name in their bar's colour ("#Name"
/// for a group). Under it what waits for the app, or "Send when Yo opens" for what the hub refused here.
struct SentCard: View {
  let done: ShareModel.Done
  let mark: String
  let later: (() -> Void)?

  private var shown: Int { done.sent > 0 ? done.sent : done.queued }
  private var word: String {
    if shown == 0 { return "No \(mark)" }
    return shown == 1 ? "Yo \(mark)" : "\(shown)× \(mark)"
  }
  private var name: String { done.friend.to.hasPrefix("group:") ? "#" + done.friend.name : done.friend.name }

  var body: some View {
    VStack(spacing: 18) {
      HStack(spacing: 10) {
        Text(verbatim: word)
          .foregroundStyle(shown == 0 ? Color.alizarin : .amethyst)
          .opacity(done.sent == 0 && done.queued > 0 ? 0.5 : 1)
          .fixedSize()  // "Yo" never shrinks
          .contentTransition(.numericText(value: Double(shown)))
        Text(verbatim: "·").foregroundStyle(.white.opacity(0.5)).fixedSize()
        Text(verbatim: name)
          .foregroundStyle(Color.row(done.friend.color))
          .lineLimit(1)
          .truncationMode(.tail)
      }
      .font(.yo(28, relativeTo: .title))
      .padding(.horizontal, 26)
      .frame(minHeight: 64)
      .background(.black, in: .capsule)
      .accessibilityElement(children: .combine)
      if done.queued > 0 {
        Text(done.sent > 0 ? "The rest sends when you open Yo" : "Sends when you open Yo")
          .font(.yo(17, relativeTo: .headline))
          .foregroundStyle(.white)
          .multilineTextAlignment(.center)
      }
      if let later {
        // White with a black label: no purple on purple.
        Button(action: later) {
          Text("Send when Yo opens").font(.yo(17, relativeTo: .headline))
        }
        .buttonStyle(.glassProminent)
        .tint(.white)
        .foregroundStyle(.black)
      }
    }
    .padding(24)
  }
}

/// A video longer than a Yo's, refused as the island says "No": "Video too long" in red, "max 10 s" beside it.
struct TooLongCard: View {
  var body: some View {
    HStack(spacing: 10) {
      Text(verbatim: "Video too long").foregroundStyle(Color.alizarin).fixedSize()
      Text(verbatim: "·").foregroundStyle(.white.opacity(0.5)).fixedSize()
      Text(verbatim: "max \(Int(MediaFile.longestVideo)) s").foregroundStyle(.white).fixedSize()
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
