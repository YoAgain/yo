import SwiftUI
import UIKit
import YoCore

extension FriendList {
  func revealAddBlock() {
    withAnimation(.easeOut(duration: keyboardDuration)) {
      position.scrollTo(id: Self.addBlockBottomID, anchor: .bottom)
    }
  }

  func beginAdding() {
    submitted += 1
    hint = nil
    newName = ""
    swiped = nil
    adding = true
    typing = true
    revealAddBlock()
  }

  /// No friend yet (bots, groups and requests don't count) once storage is read: Group and Invite show under "+".
  var noFriendYet: Bool {
    !friends.hasFriend && accounts.storageRead
  }

  var addRow: some View {
    let open = adding || noFriendYet
    return VStack(spacing: 0) {
      if adding {
        TextField(hint ?? "Name", text: $newName)
          .font(.yo(Metrics.rowFont))
          .foregroundStyle(.white)
          .multilineTextAlignment(.center)
          .textInputAutocapitalization(.words)
          .autocorrectionDisabled()
          .submitLabel(.done)
          .focused($typing)
          .onSubmit(add)
          .onChange(of: newName) { _, value in
            let clean = Username.clean(value)
            if clean != value { newName = clean }
          }
          .frame(height: Metrics.rowHeight)
          .background { addBackground(Color.row(friends.nextColor)) }
          .onAppear { typing = true }
      } else {
        RowButton("+", color: Color.row(friends.nextColor), cameraOn: cameraOn) { _ in
          beginAdding()
        }
        .accessibilityLabel("Add Friend")
      }
      if open {
        HStack(spacing: 0) {
          Button {
            typing = false
            adding = false
            newGroup = true
          } label: {
            addAction("Group", icon: "person.2.badge.plus", color: Color(hex: 0x3498DB))
          }
          if let account = accounts.account {
            InviteShareLink(name: account.username) {
              addAction("Invite", icon: "square.and.arrow.up", color: .amethyst)
            }
          }
        }
        .buttonStyle(.plain)
      }
    }
    .transaction { $0.animation = nil }
    .animation(reduceMotion ? nil : ListMotion.expansion) { content in
      content.frame(height: Metrics.rowHeight * (open ? 2 : 1), alignment: .top).clipped()
        .modifier(ListColorClip())
    }
    .onChange(of: typing) { _, active in
      HitchScene.event("add block", then: "list")
      // Return ends the editing too: while a submitted name's hint shows ("Name…", "No Such User") the field stays,
      // its answer closes it (or brings the keyboard back for another try).
      if !active && hint == nil { adding = false }
    }
  }

  func addAction(_ title: String, icon: String, color: Color) -> some View {
    Label(title, systemImage: icon)
      .font(.yo(22))
      // Half a row each: with Dynamic Type's large sizes the words shrink instead of ending in "Gro…".
      .lineLimit(1)
      .minimumScaleFactor(0.5)
      .foregroundStyle(.white)
      .frame(maxWidth: .infinity)
      .frame(height: Metrics.rowHeight)
      .background { addBackground(color) }
      .contentShape(Rectangle())
  }

  @ViewBuilder func addBackground(_ color: Color) -> some View {
    Color.clear.modifier(ListColor(color: color))
  }

  func add() {
    if hint != nil && newName.isEmpty { return }  // the hint stays a moment, then the bar closes
    let name = Username.clean(newName)
    newName = ""
    guard !name.isEmpty else {
      adding = false
      return
    }
    submitted += 1
    let this = submitted
    hint = "\(name)…"
    Task {
      let problem = await addFriend(name)
      guard submitted == this else { return }  // another name came meanwhile: the field is its
      if case .unreachable(let why) = problem {
        // Nothing added, nothing dropped: the banner says why and the name stays in the field for another try.
        Problems.shared.report(why)
        hint = nil
        if newName.isEmpty { newName = name }
        typing = true
        return
      }
      if case .said(let words) = problem { return say(words) }
      hint = nil
      if newName.isEmpty { adding = false }
    }
  }

  /// An invite link's or scanned code's name, once the hub said who it is: an alert asks to add it and send a Yo (only a
  /// Yo, for one already on the list). Never added unverified: without an answer it stays pending (false) for the next
  /// try; what is wrong ("No Such User", "That’s You") the alert says.
  func invited(_ name: String) async -> Bool {
    switch await lookUpFriend(name) {
    case .signedOut: return true
    case .problem(let problem):
      inviting = Invitation(name: name.uppercased(), user: nil, known: false, problem: problem)
    case .known(let friend):
      inviting = Invitation(name: friend.name, user: friend.user, known: true)
    case .unreachable(let why):
      Problems.shared.report(why)  // the hub did not answer: said, and asked again later
      return false
    case .new(let spelled, let user):
      inviting = Invitation(name: spelled, user: user, known: false)
    }
    return true
  }

  /// "Add & Yo" (or "Yo"): the friend on top as `add` puts it, then one Yo as a tap on its bar sends.
  func acceptInvite(_ invitation: Invitation) {
    if !invitation.known {
      withAnimation(.snappy) {
        friends.add(invitation.name, user: invitation.user)
        if !scrolled { position.scrollTo(edge: .top) }
      }
    }
    let skeleton = Username.skeleton(invitation.name)
    guard
      let friend = friends.shown.first(where: {
        !$0.gone && $0.group == nil
          && ((invitation.user != nil && $0.user == invitation.user) || Username.skeleton($0.name) == skeleton)
      })
    else { return }
    if invitation.known {
      withAnimation(.snappy) { position.scrollTo(id: friend.key.uuidString, anchor: .center) }
    }
    send(to: friend, origin: nil)
  }

  /// A name to add, once the hub said who it is: added as its owner spells it, on top; without an answer, not at all. One already
  /// on the list says so on its bar. Returns what to say instead ("That’s You", "No Such User"), or why the hub did not
  /// answer, or nil.
  enum AddProblem { case said(String), unreachable(ProductProblem) }

  func addFriend(_ name: String) async -> AddProblem? {
    switch await lookUpFriend(name) {
    case .signedOut: return nil
    case .problem(let problem): return .said(problem)
    case .unreachable(let why): return .unreachable(why)
    case .known(let known):
      withAnimation(.snappy) {
        position.scrollTo(id: known.key.uuidString, anchor: .center)
      }
      showSent(to: known, productText(text: .alreadyAdded, lang: .english))
    case .new(let spelled, let user):
      withAnimation(.snappy) {
        friends.add(spelled, user: user)
        if !scrolled { position.scrollTo(edge: .top) }
      }
    }
    return nil
  }

  enum Looked {
    /// Signed out meanwhile: not this account's friend.
    case signedOut
    case problem(String)
    case known(Friend)
    /// Not on the list yet: as its owner spells it, with its user id.
    case new(String, user: String)
    /// The hub did not answer (never added unchecked: the name could be nobody's): why.
    case unreachable(ProductProblem)
  }

  func lookUpFriend(_ name: String) async -> Looked {
    if let me = accounts.account?.username, Username.skeleton(name) == Username.skeleton(me) {
      return .problem(ProductText.thatsYou.english)
    }
    let client = accounts.client
    let answer = await inbox.lookUp(name, accounts: accounts)
    guard client === accounts.client else { return .signedOut }
    var user: String?
    var spelled = name
    var unreachable: ProductProblem?
    switch answer {
    case .noSuchUser: return .problem(ProductText.noSuchUser.english)
    case .found(let found, let owners):
      user = found
      spelled = owners ?? spelled
    case .unknown(let why): unreachable = why
    }
    let skeleton = Username.skeleton(spelled)
    if let known = friends.shown.first(where: {
      !$0.gone && (Username.skeleton($0.name) == skeleton || (user != nil && $0.user == user))
    }) {
      return .known(known)
    }
    guard let user else { return .unreachable(unreachable ?? .failed) }
    return .new(spelled, user: user)
  }

  /// A hint in the empty field for a moment; then the bar closes unless something is typed.
  func say(_ text: String) {
    submitted += 1
    let this = submitted
    hint = text
    UIAccessibility.post(notification: .announcement, argument: text)  // a placeholder alone is not read out
    typing = true  // the keyboard stays, to type the name again
    Task {
      try? await Task.sleep(for: .seconds(1.5))
      guard submitted == this else { return }
      hint = nil
      if newName.isEmpty && !typing { adding = false }
    }
  }
}
