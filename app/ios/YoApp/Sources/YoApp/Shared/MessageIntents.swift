// A Yo notification as a message from its sender (communication notification, look "E" of 2026-11-23): the sender's
// name as title, an Amethyst circle with their initial (a group's: its own), so Focus, Siri announce and CarPlay see a
// person. Made from the notification's userInfo (ShareBridge.yo), so any place that hands one to iOS can apply it. The
// picture is drawn here, nothing is fetched; the donation stays on the phone.
import CoreText
import Intents
import UIKit
import UserNotifications

enum CommunicationNotification {
  static var me: INPerson {
    INPerson(personHandle: INPersonHandle(value: "me", type: .unknown), nameComponents: nil, displayName: nil,
      image: nil, contactIdentifier: nil, customIdentifier: nil, isMe: true)
  }

  static func sender(userInfo: [AnyHashable: Any]) -> INPerson? {
    guard let user = userInfo["senderUser"] as? String, let name = userInfo["senderName"] as? String else { return nil }
    return INPerson(personHandle: INPersonHandle(value: user, type: .unknown), nameComponents: nil, displayName: name,
      image: YoAvatar.image(for: name, group: false), contactIdentifier: nil, customIdentifier: user)
  }

  static func intent(userInfo: [AnyHashable: Any], body: String) -> INSendMessageIntent? {
    guard let sender = sender(userInfo: userInfo) else { return nil }
    let group = userInfo["group"] as? String
    let intent = INSendMessageIntent(recipients: group == nil ? [me] : [me, sender],
      outgoingMessageType: .outgoingMessageText, content: body, speakableGroupName: group.map { INSpeakableString(spokenPhrase: $0) },
      conversationIdentifier: userInfo["user"] as? String, serviceName: nil, sender: sender, attachments: nil)
    if let group { intent.setImage(YoAvatar.image(for: group, group: true), forParameterNamed: \.speakableGroupName) }
    return intent
  }

  /// The plain title and body still work if iOS can't decorate the notification.
  static func updated(_ content: UNNotificationContent) -> UNNotificationContent {
    guard let intent = intent(userInfo: content.userInfo, body: content.body) else { return content }
    let interaction = INInteraction(intent: intent, response: nil)
    interaction.direction = .incoming
    let donated = DispatchSemaphore(value: 0)
    interaction.donate { _ in donated.signal() }
    _ = donated.wait(timeout: .now() + 0.5)
    do {
      return try content.updating(from: intent)
    } catch {
      Diagnostics.record(error)
      return content
    }
  }
}

enum YoAvatar {
  static func initial(of name: String, group: Bool) -> String {
    name.first { $0.isLetter || $0.isNumber }?.uppercased() ?? (group ? "#" : "Y")
  }

  private final class Cache: @unchecked Sendable {
    let lock = NSLock()
    var images: [String: Data] = [:]
  }
  private static let cache = Cache()

  static func image(for name: String, group: Bool) -> INImage? {
    let letter = initial(of: name, group: group)
    let data = cache.lock.withLock {
      if let data = cache.images[letter] { return data }
      let data = render(letter)
      if cache.images.count >= 64 { cache.images.removeAll(keepingCapacity: true) }
      cache.images[letter] = data
      return data
    }
    return INImage(imageData: data)
  }

  private static let font: UIFont = {
    if let font = UIFont(name: "Montserrat-Bold", size: 100) { return font }
    if let url = Bundle.main.url(forResource: "Montserrat-Bold", withExtension: "ttf") {
      CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
    }
    return UIFont(name: "Montserrat-Bold", size: 100) ?? .boldSystemFont(ofSize: 100)
  }()

  private static func render(_ letter: String) -> Data {
    let size = CGSize(width: 192, height: 192)
    let format = UIGraphicsImageRendererFormat()
    format.scale = 1
    format.opaque = false
    return UIGraphicsImageRenderer(size: size, format: format).pngData { renderer in
      let context = renderer.cgContext
      context.setFillColor(UIColor(red: 0x9B/255, green: 0x59/255, blue: 0xB6/255, alpha: 1).cgColor)
      context.fillEllipse(in: CGRect(origin: .zero, size: size))
      // Core Text draws up from the baseline. Centre the ink, including accents.
      context.translateBy(x: 0, y: size.height)
      context.scaleBy(x: 1, y: -1)
      context.textMatrix = .identity
      context.textPosition = .zero
      let line = CTLineCreateWithAttributedString(NSAttributedString(string: letter,
        attributes: [.font: font, .foregroundColor: UIColor.white]))
      let ink = CTLineGetImageBounds(line, context)
      context.textPosition = CGPoint(x: size.width / 2 - ink.midX, y: size.height / 2 - ink.midY)
      CTLineDraw(line, context)
    }
  }
}
