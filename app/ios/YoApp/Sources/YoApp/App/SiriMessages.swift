// SiriKit's messages domain, handled in the app (AppDelegate's handlerFor): "send a Yo to Mia" (any words become a
// Yo), "read my messages" (the Yo notifications still on screen are the unread ones) and marking them read; CarPlay
// needs all three. Names are the list's own (ShareBridge.readList); the App Shortcuts ("Yo Mia", YoShortcuts.swift)
// stay as they are.
import Intents
import UserNotifications

final class SiriMessages: NSObject, INSendMessageIntentHandling, INSearchForMessagesIntentHandling,
  INSetMessageAttributeIntentHandling, @unchecked Sendable {
  private typealias Friend = ShareBridge.Friend

  // Stays on the phone and lets Siri recognise names that are only on Yo.
  static func teachNames(_ list: ShareBridge.List?) {
    let vocabulary = INVocabulary.shared()
    guard let list else {
      vocabulary.removeAllVocabularyStrings()
      return
    }
    let people = list.friends.filter { !$0.to.hasPrefix("group:") }.map(\.name)
    let groups = list.friends.filter { $0.to.hasPrefix("group:") }.map { String($0.title.drop(while: { $0 == "#" })) }
    vocabulary.setVocabularyStrings(NSOrderedSet(array: people), of: .contactName)
    vocabulary.setVocabularyStrings(NSOrderedSet(array: groups), of: .contactGroupName)
  }

  private func person(_ friend: Friend) -> INPerson {
    INPerson(personHandle: INPersonHandle(value: friend.to, type: .unknown), nameComponents: nil,
      displayName: friend.title, image: nil, contactIdentifier: nil, customIdentifier: friend.to)
  }

  private func groupName(_ friend: Friend) -> INSpeakableString {
    INSpeakableString(vocabularyIdentifier: friend.to, spokenPhrase: friend.title, pronunciationHint: nil)
  }

  private func key(_ name: String) -> String {
    name.trimmingCharacters(in: .whitespacesAndNewlines).drop(while: { $0 == "#" })
      .folding(options: [.caseInsensitive, .diacriticInsensitive], locale: .current)
  }

  private func matches(_ name: String, in friends: [Friend]) -> (friends: [Friend], exact: Bool) {
    let query = key(name)
    guard !query.isEmpty else { return ([], false) }
    let exact = friends.filter { key($0.name) == query || key($0.title) == query }
    if !exact.isEmpty { return (exact, true) }
    return (friends.filter { key($0.name).contains(query) || key($0.title).contains(query) }, false)
  }

  func resolveRecipients(for intent: INSendMessageIntent) async -> [INSendMessageRecipientResolutionResult] {
    guard let list = ShareBridge.readList() else { return [.unsupported(forReason: .noAccount)] }
    guard let recipients = intent.recipients, !recipients.isEmpty else {
      return intent.speakableGroupName == nil ? [.needsValue()] : [.notRequired()]
    }
    return recipients.map { recipient in
      for id in [recipient.customIdentifier, recipient.personHandle?.value].compactMap({ $0 }) {
        if let friend = list.friends.first(where: { $0.to == id }) { return .success(with: person(friend)) }
      }
      // Siri may hand over a contact ("Anna Berg" for "Anna"): its whole name first, then the given name, then the
      // first word.
      let names = [recipient.displayName, recipient.spokenPhrase, recipient.nameComponents?.givenName ?? "",
        recipient.displayName.split(separator: " ").first.map(String.init) ?? ""]
      guard let found = names.lazy.map({ self.matches($0, in: list.friends) }).first(where: { !$0.friends.isEmpty }),
        let first = found.friends.first
      else { return .unsupported(forReason: .noAccount) }
      if found.friends.count > 1 { return .disambiguation(with: found.friends.map(person)) }
      return found.exact ? .success(with: person(first)) : .confirmationRequired(with: person(first))
    }
  }

  func resolveSpeakableGroupName(for intent: INSendMessageIntent) async -> INSpeakableStringResolutionResult {
    guard let name = intent.speakableGroupName else { return .notRequired() }
    guard let list = ShareBridge.readList() else { return .unsupported() }
    let groups = list.friends.filter { $0.to.hasPrefix("group:") }
    if let group = groups.first(where: { $0.to == name.vocabularyIdentifier }) { return .success(with: groupName(group)) }
    let found = matches(name.spokenPhrase, in: groups).friends
    guard let first = found.first else { return .unsupported() }
    return found.count == 1 ? .success(with: groupName(first)) : .disambiguation(with: found.map(groupName))
  }

  func resolveContent(for intent: INSendMessageIntent) async -> INStringResolutionResult { .success(with: "Yo") }

  func handle(intent: INSendMessageIntent) async -> INSendMessageIntentResponse {
    guard let list = ShareBridge.readList() else { return .init(code: .failureRequiringAppLaunch, userActivity: nil) }
    let targets: [String]
    if let group = intent.speakableGroupName {
      guard let id = group.vocabularyIdentifier, id.hasPrefix("group:") else { return .init(code: .failure, userActivity: nil) }
      targets = [id]
    } else {
      let recipients = intent.recipients ?? []
      targets = recipients.compactMap(\.customIdentifier)
      guard targets.count == recipients.count else { return .init(code: .failure, userActivity: nil) }
    }
    guard !targets.isEmpty, Set(targets).isSubset(of: list.friends.map(\.to)) else {
      return .init(code: .failure, userActivity: nil)
    }
    var sent = Set<String>()
    var success = true
    for target in targets where sent.insert(target).inserted {
      do { try await YoSend.send(to: target, owner: list.owner) }
      catch { success = false }
    }
    return .init(code: success ? .success : .failure, userActivity: nil)
  }

  func handle(intent: INSearchForMessagesIntent) async -> INSearchForMessagesIntentResponse {
    guard ShareBridge.readList() != nil else { return .init(code: .failureRequiringAppLaunch, userActivity: nil) }
    let response = INSearchForMessagesIntentResponse(code: .success, userActivity: nil)
    guard !intent.attributes.contains(.read) else {
      response.messages = []
      return response
    }
    let senders = Set((intent.senders ?? []).map { key($0.displayName) })
    let groups = Set((intent.speakableGroupNames ?? []).map { key($0.spokenPhrase) })
    let delivered = await UNUserNotificationCenter.current().deliveredNotifications()
    response.messages = delivered.sorted { $0.date > $1.date }.compactMap { notification in
      let content = notification.request.content
      let info = content.userInfo
      let group = info["group"] as? String
      guard content.categoryIdentifier == ShareBridge.yoCategory,
        let name = info["senderName"] as? String, senders.isEmpty || senders.contains(key(name)),
        groups.isEmpty || group.map({ groups.contains(key($0)) }) == true,
        let sender = CommunicationNotification.sender(userInfo: info)
      else { return nil }
      return INMessage(identifier: notification.request.identifier, conversationIdentifier: content.threadIdentifier,
        content: content.body, dateSent: notification.date, sender: sender, recipients: [CommunicationNotification.me],
        groupName: group.map { INSpeakableString(spokenPhrase: $0) }, messageType: .text)
    }
    return response
  }

  func resolveAttribute(for intent: INSetMessageAttributeIntent) async -> INMessageAttributeResolutionResult {
    intent.attribute == .read ? .success(with: .read) : .unsupported()
  }

  func handle(intent: INSetMessageAttributeIntent) async -> INSetMessageAttributeIntentResponse {
    guard ShareBridge.readList() != nil else { return .init(code: .failureRequiringAppLaunch, userActivity: nil) }
    guard intent.attribute == .read else { return .init(code: .failure, userActivity: nil) }
    UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: intent.identifiers ?? [])
    return .init(code: .success, userActivity: nil)
  }
}
