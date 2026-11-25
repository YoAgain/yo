import SwiftUI
import WatchKit

struct FriendList: View {
  let link: WatchLink
  private struct Feedback {
    let id = UUID()
    let text: String
  }
  @State private var feedback: [String: Feedback] = [:]
  @State private var overlayTarget: String?

  var body: some View {
    NavigationStack {
      Group {
        if link.state.friends.isEmpty {
          Text("Open Yo on your iPhone")
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
          GeometryReader { geometry in
            ScrollView {
              LazyVStack(spacing: 0) {
                ForEach(link.state.friends, id: \.to) { friend in
                  Button { send(to: friend.to, back: false) } label: {
                    Text(feedback[friend.to]?.text ?? friend.title)
                      .font(.yo(20))
                      .foregroundStyle(.white)
                      .lineLimit(1)
                      .minimumScaleFactor(0.6)
                      .padding(.horizontal, 4)
                      .frame(maxWidth: .infinity)
                      .frame(height: max(44, geometry.size.width * 0.28))
                      .background(Color.row(friend.color))
                      .contentShape(Rectangle())
                  }
                  .buttonStyle(.plain)
                }
              }
            }
          }
          .ignoresSafeArea(edges: .horizontal)
        }
      }
      .overlay {
        if let to = overlayTarget, let result = feedback[to],
          !link.state.friends.contains(where: { $0.to == to }) {
          Text(result.text)
            .font(.yo(20))
            .foregroundStyle(.white)
            .padding()
            .background(Color.amethyst, in: Capsule())
            .allowsHitTesting(false)
        }
      }
      // The system's title: on the clock's line, in the app's tint (amethyst).
      .navigationTitle("Yo")
    }
    .onOpenURL { url in
      guard url.scheme == "yo", url.host == "back" else { return }
      link.requestYoBack(to: nil)
    }
    .onChange(of: link.yoBackRequest?.id, initial: true) {
      guard let request = link.yoBackRequest else { return }
      link.yoBackRequest = nil
      send(to: request.to, back: true)
    }
  }

  private func send(to: String, back: Bool) {
    WKInterfaceDevice.current().play(.click)
    Task {
      let sent = await link.send(to: to, back: back)
      WKInterfaceDevice.current().play(sent ? .success : .failure)
      let result = Feedback(text: sent ? "Sent Yo!" : "Not Sent")
      feedback[to] = result
      if back { overlayTarget = to }
      try? await Task.sleep(for: .seconds(2))
      if feedback[to]?.id == result.id {
        feedback[to] = nil
        if overlayTarget == to { overlayTarget = nil }
      }
    }
  }
}
