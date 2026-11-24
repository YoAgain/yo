import Foundation
import Testing
import WatchConnectivity
@testable import YoWatch

@MainActor private final class FakeSession: WatchSession {
  var activationState: WCSessionActivationState = .activated
  var isReachable = false
  var reply: Bool?
  var silent = false
  var sent: [[String: Any]] = []
  var transfers: [[String: Any]] = []

  func sendMessage(_ message: [String: Any], replyHandler: (([String: Any]) -> Void)?,
                   errorHandler: ((any Error) -> Void)?) {
    sent.append(message)
    if silent {
      return
    } else if let reply {
      replyHandler?(["ok": reply])
    } else {
      errorHandler?(URLError(.notConnectedToInternet))
    }
  }

  func transferUserInfo(_ userInfo: [String: Any]) { transfers.append(userInfo) }
}

@MainActor struct WatchLinkTests {
  private func deliver(_ session: FakeSession) async -> Bool {
    await WatchLink.deliver(to: "friend", back: true, over: session, reachableWait: .zero,
                             replyWait: .milliseconds(100))
  }

  @Test func unreachableQueuesYo() async throws {
    let session = FakeSession()
    #expect(await deliver(session))
    #expect(session.sent.isEmpty)
    #expect(session.transfers.count == 1)
    let message = try #require(session.transfers.first)
    #expect(Set(message.keys) == ["yo", "back", "id", "at"])
    #expect(message["yo"] as? String == "friend")
    #expect(message["back"] as? Bool == true)
    let id = try #require(message["id"] as? String)
    #expect(UUID(uuidString: id) != nil)
    #expect(message["at"] is Double)
  }

  @Test func sendErrorQueuesSameYo() async throws {
    let session = FakeSession()
    session.isReachable = true
    #expect(await deliver(session))
    #expect(session.sent.count == 1)
    #expect(session.transfers.count == 1)
    let sent = try #require(session.sent.first)
    let queued = try #require(session.transfers.first)
    let id = try #require(sent["id"] as? String)
    #expect(queued["id"] as? String == id)
  }

  @Test func silentPhoneQueuesSameYo() async throws {
    let session = FakeSession()
    session.isReachable = true
    session.silent = true
    #expect(await deliver(session))
    #expect(session.sent.count == 1)
    #expect(session.transfers.count == 1)
    let id = try #require(session.sent.first?["id"] as? String)
    #expect(session.transfers.first?["id"] as? String == id)
  }

  @Test(arguments: [true, false]) func reachableReturnsReply(_ ok: Bool) async {
    let session = FakeSession()
    session.isReachable = true
    session.reply = ok
    #expect(await deliver(session) == ok)
    #expect(session.sent.count == 1)
    #expect(session.transfers.isEmpty)
  }

  @Test func inactiveSendsNothing() async {
    let session = FakeSession()
    session.activationState = .notActivated
    #expect(await deliver(session) == false)
    #expect(session.sent.isEmpty)
    #expect(session.transfers.isEmpty)
  }
}
