// Drives Yo Dev in the Simulator by commands from a folder (dev/sim/drive.py writes them): one UI test that runs
// until it is told to stop, so a person or a script can tap through the app step by step and look at each screen.
// The folder is YO_DRIVER_DIR (xcodebuild passes TEST_RUNNER_YO_DRIVER_DIR). A command is `cmd.json`, its answer
// `out.json`, both {"id": n, ...}.
import XCTest

@MainActor final class Driver: XCTestCase {
  private let app = XCUIApplication(bundleIdentifier: "app.justyo.ios.dev")
  private let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")

  /// `sb=true`: SpringBoard; `app=BUNDLE_ID`: another app (`com.apple.Spotlight`, whose search field and keyboard are
  /// its own); else Yo.
  private func chosen(_ c: [String: Any]) -> XCUIApplication {
    if c["sb"] as? Bool == true { return springboard }
    if let id = c["app"] as? String { return XCUIApplication(bundleIdentifier: id) }
    return app
  }

  func testDrive() throws {
    continueAfterFailure = true
    executionTimeAllowance = 24 * 3600
    let dir = URL(fileURLWithPath: ProcessInfo.processInfo.environment["YO_DRIVER_DIR"] ?? "/tmp/yo-driver")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let cmd = dir.appending(path: "cmd.json")
    let out = dir.appending(path: "out.json")
    try? Data(#"{"id":0,"ok":true,"ready":true}"#.utf8).write(to: out, options: .atomic)
    while true {
      guard let data = try? Data(contentsOf: cmd),
        let command = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
      else {
        Thread.sleep(forTimeInterval: 0.05)
        continue
      }
      try? FileManager.default.removeItem(at: cmd)
      var answer: [String: Any]
      do {
        answer = try run(command)
        answer["ok"] = true
      } catch {
        answer = ["ok": false, "error": "\(error)"]
      }
      answer["id"] = command["id"] ?? 0
      if let reply = try? JSONSerialization.data(withJSONObject: answer) { try? reply.write(to: out, options: .atomic) }
      if command["op"] as? String == "quit" {
        try? Data().write(to: dir.appending(path: "quit"))
        return
      }
    }
  }

  struct Failure: Error, CustomStringConvertible {
    let description: String
  }

  private func run(_ c: [String: Any]) throws -> [String: Any] {
    let op = c["op"] as? String ?? ""
    switch op {
    case "account-layout":
      // Start on Account; its rows in order, then Account Security's rows, each recovery screen and Contacts.
      func rowsInOrder(_ ids: [String]) throws -> [XCUIElement] {
        let rows = ids.map { app.descendants(matching: .any).matching(identifier: $0).firstMatch }
        for (id, row) in zip(ids, rows) {
          guard row.waitForExistence(timeout: 5) else { throw Failure(description: "missing \(id)") }
        }
        guard zip(rows, rows.dropFirst()).allSatisfy({ $0.frame.minY < $1.frame.minY }) else {
          throw Failure(description: "row order \(ids)")
        }
        return rows
      }
      let accountIds = ["account.contacts", "account.security", "account.signOut"]
      let securityIds = ["security.accountSync", "security.recoveryKey", "security.passkey"]
      // The recovery screens close with Done (a sheet's toolbar) or Close (the step screens' X).
      func closeScreen() {
        app.buttons["Done"].exists ? app.buttons["Done"].tap() : app.buttons["Close"].tap()
      }
      let account = try rowsInOrder(accountIds)
      account[1].tap()
      // With the red badge the row opens the recovery key's screen itself, and the page waits until it is saved.
      var checked = accountIds
      if app.staticTexts["Keep it safe. It brings your account back."].waitForExistence(timeout: 2) {
        closeScreen()
      } else {
        let security = try rowsInOrder(securityIds)
        checked += securityIds
        // A switch that is off opens its screen; the saved key's own row opens it while the key is on.
        let showKey = app.buttons["security.showKey"]
        for (row, line) in [
          (security[1], "Keep it safe. It brings your account back."), (security[2], "Sign in with Face ID on a new phone."),
        ] {
          let on = (row.value as? String) == "1"
          if on && row == security[2] { continue }
          if on { showKey.tap() } else { row.switches.firstMatch.exists ? row.switches.firstMatch.tap() : row.tap() }
          guard app.staticTexts[line].waitForExistence(timeout: 5) else {
            throw Failure(description: "\(line) screen did not open")
          }
          closeScreen()
        }
        app.navigationBars.buttons.element(boundBy: 0).tap()
      }
      account[0].tap()
      // Contacts lists the contacts only; Blocked is a page of its own.
      guard app.navigationBars["Contacts"].waitForExistence(timeout: 5) else {
        throw Failure(description: "missing Contacts title")
      }
      app.navigationBars.buttons.element(boundBy: 0).tap()
      return ["rows": checked]
    case "camera-add":
      // Begin with camera mode open and the final row visible.
      let add = app.buttons["Add Friend"]
      guard add.waitForExistence(timeout: 5), add.isHittable else {
        throw Failure(description: "Add Friend is missing in camera mode")
      }
      add.tap()
      guard app.textFields["Name"].waitForExistence(timeout: 5),
        app.buttons["Group"].exists,
        !app.buttons["Stop and Send Video"].exists
      else { throw Failure(description: "camera Add Friend did not open the add field") }
      return ["adding": true]
    case "launch":
      app.launchArguments = c["args"] as? [String] ?? []
      app.launchEnvironment = c["env"] as? [String: String] ?? [:]
      app.launch()
      return [:]
    case "activate":
      app.activate()
      return [:]
    case "terminate":
      app.terminate()
      return [:]
    case "home":
      XCUIDevice.shared.press(.home)
      return [:]
    case "rotate":
      // The device turned (an iPad: to=landscape or to=portrait).
      XCUIDevice.shared.orientation = c["to"] as? String == "landscape" ? .landscapeLeft : .portrait
      return [:]
    case "lock":
      // The side button: the screen off and locked (no API names it; XCUIDevice answers to it).
      XCUIDevice.shared.perform(NSSelectorFromString("pressLockButton"))
      return [:]
    case "tree":
      return ["tree": try tree(chosen(c))]
    case "tap", "press", "doubletap", "exists", "wait", "gone", "info", "swipe":
      let target = chosen(c)
      let timeout = c["timeout"] as? Double ?? (op == "exists" ? 0 : 5)
      if op == "gone" {
        let element = query(target, c)
        let end = Date.now.addingTimeInterval(timeout)
        while element.exists && Date.now < end { Thread.sleep(forTimeInterval: 0.1) }
        return ["gone": !element.exists]
      }
      guard var element = try find(target, c, timeout: timeout, required: op != "exists" && op != "wait")
      else { return ["exists": false] }
      // A touch on an element that cannot take it (a bar under a sheet with the same name) fails the test, and XCTest
      // ends the driver: the first match that can take it instead, or an answer.
      if ["tap", "press", "doubletap", "swipe"].contains(op), c["dx"] == nil, !element.isHittable {
        guard c["index"] == nil,
          let other = matches(target, c).allElementsBoundByIndex.first(where: { $0.exists && $0.isHittable })
        else { throw Failure(description: "not hittable: \(c)") }
        element = other
      }
      switch op {
      case "tap":
        if let dx = c["dx"] as? Double, let dy = c["dy"] as? Double {
          element.coordinate(withNormalizedOffset: CGVector(dx: dx, dy: dy)).tap()
        } else {
          element.tap()
        }
      case "doubletap": element.doubleTap()
      case "press": element.press(forDuration: c["seconds"] as? Double ?? 1)
      case "swipe":
        switch c["dir"] as? String {
        case "up": element.swipeUp()
        case "down": element.swipeDown()
        case "left": element.swipeLeft()
        default: element.swipeRight()
        }
      default: break
      }
      return describe(element)
    case "tapxy", "pressxy":
      let point = origin(c["sb"] as? Bool == true ? springboard : app)
        .withOffset(CGVector(dx: c["x"] as? Double ?? 0, dy: c["y"] as? Double ?? 0))
      if op == "tapxy" { point.tap() } else { point.press(forDuration: c["seconds"] as? Double ?? 1) }
      return [:]
    case "drag":
      let o = origin(c["sb"] as? Bool == true ? springboard : app)
      let from = o.withOffset(CGVector(dx: c["x"] as? Double ?? 0, dy: c["y"] as? Double ?? 0))
      let to = o.withOffset(CGVector(dx: c["x2"] as? Double ?? 0, dy: c["y2"] as? Double ?? 0))
      from.press(
        forDuration: c["hold"] as? Double ?? 0.05, thenDragTo: to,
        withVelocity: XCUIGestureVelocity(c["velocity"] as? Double ?? 1000), thenHoldForDuration: c["after"] as? Double ?? 0)
      return [:]
    case "type":
      let text = c["text"] as? String ?? ""
      // Typing with no keyboard up fails the test (and XCTest ends it): an answer instead.
      let app = chosen(c)
      guard app.keyboards.count > 0 else { throw Failure(description: "no keyboard") }
      if let field = try find(app, c, timeout: 0, required: false) { field.typeText(text) } else { app.typeText(text) }
      return [:]
    case "clear":
      guard app.keyboards.count > 0 else { throw Failure(description: "no keyboard") }
      app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: c["n"] as? Int ?? 30))
      return [:]
    case "burst":
      // Fast taps as a finger makes them, without XCTest waiting for the app to be still between them: `n` taps,
      // one every `interval` seconds, at `points` (screen points, taken in turn).
      let points = (c["points"] as? [[Double]] ?? [[c["x"] as? Double ?? 0, c["y"] as? Double ?? 0]])
        .map { CGPoint(x: $0[0], y: $0[1]) }
      let n = c["n"] as? Int ?? 10
      let interval = c["interval"] as? Double ?? 0.1
      let away = (c["away"] as? [Double]).map { CGPoint(x: $0[0], y: $0[1]) }
      try Burst.tap(points, n: n, interval: interval, hold: c["hold"] as? Double ?? 1, away: away)
      return ["taps": n, "seconds": Double(n) * interval]
    case "touches":
      // Fingers as given, in one record (no waiting for the app to be still between them): each path is a list of
      // [x, y, seconds] in screen points, down at the first, moving through the next, lifting at the last. XCTest
      // leaves out time with no finger down: a finger off the screen holds the gap. A fling, then a tap 0.5 s on:
      // paths=[[[200,700,0],[200,620,0.07]],[[600,1200,0.074],[600,1200,0.49]],[[200,450,0.5],[200,450,0.57]]].
      let paths = (c["paths"] as? [[[Double]]] ?? []).filter { $0.count >= 2 }
      try Burst.paths(paths)
      return ["paths": paths.count]
    case "sleep":
      Thread.sleep(forTimeInterval: c["seconds"] as? Double ?? 1)
      return [:]
    case "shot":
      let path = c["path"] as? String ?? "/tmp/yo-driver.png"
      try XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: path))
      return ["path": path]
    case "audit":
      // Xcode's accessibility audit of the screen as it is; every issue reported, none failing the test.
      var issues: [String] = []
      try app.performAccessibilityAudit { issue in
        let element = issue.element.map { "\($0.elementType.rawValue) '\($0.label)'" } ?? "-"
        issues.append("\(issue.auditType.rawValue) \(issue.compactDescription) [\(element)]")
        return true
      }
      return ["issues": issues]
    case "quit":
      return [:]
    default:
      throw Failure(description: "unknown op \(op)")
    }
  }

  private func origin(_ target: XCUIApplication) -> XCUICoordinate {
    target.coordinate(withNormalizedOffset: .zero)
  }

  private static let types: [String: XCUIElement.ElementType] = [
    "any": .any, "button": .button, "text": .staticText, "field": .textField, "secure": .secureTextField,
    "textView": .textView, "cell": .cell, "switch": .switch, "image": .image, "alert": .alert, "sheet": .sheet,
    "navBar": .navigationBar, "link": .link, "menuItem": .menuItem, "other": .other, "toggle": .toggle,
    "key": .key, "scroll": .scrollView, "collection": .collectionView, "window": .window,
  ]

  /// Elements by label (exact or `contains`), identifier, type, and index among the matches.
  private func query(_ target: XCUIApplication, _ c: [String: Any]) -> XCUIElement {
    matches(target, c).element(boundBy: c["index"] as? Int ?? 0)
  }

  private func matches(_ target: XCUIApplication, _ c: [String: Any]) -> XCUIElementQuery {
    let type = Self.types[c["type"] as? String ?? "any"] ?? .any
    var predicates: [NSPredicate] = []
    if let label = c["label"] as? String { predicates.append(NSPredicate(format: "label == %@", label)) }
    if let part = c["contains"] as? String { predicates.append(NSPredicate(format: "label CONTAINS %@", part)) }
    if let id = c["id"] as? String { predicates.append(NSPredicate(format: "identifier == %@", id)) }
    if let value = c["value"] as? String { predicates.append(NSPredicate(format: "value == %@", value)) }
    return target.descendants(matching: type).matching(NSCompoundPredicate(andPredicateWithSubpredicates: predicates))
  }

  private func find(_ target: XCUIApplication, _ c: [String: Any], timeout: Double, required: Bool) throws
    -> XCUIElement?
  {
    guard c["label"] != nil || c["contains"] != nil || c["id"] != nil || c["value"] != nil || c["type"] != nil
    else { return nil }
    let element = query(target, c)
    if element.exists || (timeout > 0 && element.waitForExistence(timeout: timeout)) { return element }
    if required { throw Failure(description: "not found: \(c)") }
    return nil
  }

  /// What an element is now, from one snapshot: a tap that changes the screen (Sign In, Sign Out) may take the element
  /// away between two questions, and asking a gone element fails the test (XCTest then ends the driver). `exists`
  /// and then `frame` did that; a snapshot that cannot be had is an answer instead.
  private func describe(_ e: XCUIElement) -> [String: Any] {
    guard let s = try? e.snapshot() else { return ["exists": false] }
    let f = s.frame
    return [
      "exists": true, "label": s.label, "id": s.identifier, "value": "\(s.value ?? "")",
      "frame": [f.minX, f.minY, f.width, f.height], "enabled": s.isEnabled,
    ]
  }

  /// The screen's elements, one line each, indented: type, label, identifier, value, frame.
  private func tree(_ target: XCUIApplication) throws -> String {
    var lines: [String] = []
    func walk(_ s: XCUIElementSnapshot, _ depth: Int) {
      let f = s.frame
      var line = String(repeating: " ", count: depth) + "\(s.elementType.rawValue)"
      line += Self.types.first { $0.value == s.elementType }.map { ":\($0.key)" } ?? ""
      if !s.label.isEmpty { line += " '\(s.label)'" }
      if !s.identifier.isEmpty { line += " #\(s.identifier)" }
      if let v = s.value as? String, !v.isEmpty { line += " =\(v)" }
      if !s.isEnabled { line += " (disabled)" }
      if s.isSelected { line += " (selected)" }
      line += " [\(Int(f.minX)),\(Int(f.minY)) \(Int(f.width))x\(Int(f.height))]"
      // Only what tells something: an element without words is shown when it has children with words.
      let telling = !s.label.isEmpty || !s.identifier.isEmpty || s.elementType != .other
      if telling { lines.append(line) }
      for child in s.children { walk(child, telling ? depth + 1 : depth) }
    }
    walk(try target.snapshot(), 0)
    return lines.joined(separator: "\n")
  }
}

/// Taps by XCTest's own event synthesis (XCSynthesizedEventRecord: one record, many touches at their offsets), which
/// `XCUIElement.tap` uses too, minus its wait for the app to be idle before and after each tap.
@MainActor enum Burst {
  static func tap(_ points: [CGPoint], n: Int, interval: Double, hold: Double, away: CGPoint?) throws {
    guard let pathClass = NSClassFromString("XCPointerEventPath"),
      let recordClass = NSClassFromString("XCSynthesizedEventRecord"),
      !points.isEmpty
    else { throw Driver.Failure(description: "no event synthesis in this XCTest") }
    typealias InitPath = @convention(c) (AnyObject, Selector, CGPoint, Double) -> Unmanaged<AnyObject>
    typealias Lift = @convention(c) (AnyObject, Selector, Double) -> Void
    typealias InitRecord = @convention(c) (AnyObject, Selector, NSString, Int) -> Unmanaged<AnyObject>
    typealias Add = @convention(c) (AnyObject, Selector, AnyObject) -> Void
    typealias Synthesize =
      @convention(c) (AnyObject, Selector, AnyObject, @escaping @convention(block) (Bool, NSError?) -> Void) -> Void
    func imp<T>(_ object: AnyObject, _ name: String, _: T.Type) throws -> (Selector, T) {
      let selector = NSSelectorFromString(name)
      guard object.responds(to: selector), let method = class_getMethodImplementation(object_getClass(object), selector)
      else { throw Driver.Failure(description: "no \(name)") }
      return (selector, unsafeBitCast(method, to: T.self))
    }
    func alloc(_ cls: AnyClass) -> AnyObject {
      (cls as! NSObject.Type).perform(NSSelectorFromString("alloc"))!.takeUnretainedValue()
    }
    guard let synthesizer = XCUIDevice.shared.perform(NSSelectorFromString("eventSynthesizer"))?.takeUnretainedValue()
    else { throw Driver.Failure(description: "no event synthesizer") }
    let (synthesizeSelector, synthesize) = try imp(synthesizer, "synthesizeEvent:completion:", Synthesize.self)
    final class Done: @unchecked Sendable {
      var done = false
      var error: NSError?
    }
    let done = Done()
    // One record (XCTest performs one gesture at a time, each taking about 0.3 s to start). It leaves out the time
    // when no finger is down: the next tap comes down as the one before lifts, so a tap is held the whole interval
    // (`hold`: a part of it, for faster taps).
    let first = alloc(recordClass)
    let (initRecord, makeRecord) = try imp(first, "initWithName:interfaceOrientation:", InitRecord.self)
    let record = makeRecord(first, initRecord, "burst" as NSString, UIInterfaceOrientation.portrait.rawValue)
      .takeUnretainedValue()
    let (addSelector, add) = try imp(record, "addPointerEventPath:", Add.self)
    for i in 0..<n {
      let raw = alloc(pathClass)
      let (initPath, makePath) = try imp(raw, "initForTouchAtPoint:offset:", InitPath.self)
      let point = points[i % points.count]
      let path = makePath(raw, initPath, point, Double(i) * interval).takeUnretainedValue()
      let (liftSelector, lift) = try imp(path, "liftUpAtOffset:", Lift.self)
      lift(path, liftSelector, Double(i) * interval + hold * interval)
      add(record, addSelector, path)
      // The finger away until the next tap: a touch beyond the screen's edge meanwhile, which nothing gets, keeps
      // that time from being left out.
      if let away, hold < 1 {
        let raw = alloc(pathClass)
        let path = makePath(raw, initPath, away, Double(i) * interval + hold * interval + 0.004).takeUnretainedValue()
        lift(path, liftSelector, Double(i + 1) * interval - 0.004)
        add(record, addSelector, path)
      }
    }
    synthesize(synthesizer, synthesizeSelector, record) { _, error in
      DispatchQueue.main.async {
        done.error = error
        done.done = true
      }
    }
    let end = Date.now.addingTimeInterval(Double(n) * interval + 30)
    while !done.done && Date.now < end { RunLoop.current.run(until: .now + 0.02) }
    if let error = done.error { throw Driver.Failure(description: "\(error)") }
  }

  /// Finger paths ([x, y, seconds] each, the first down, the last up), synthesized as one record.
  static func paths(_ paths: [[[Double]]]) throws {
    guard let pathClass = NSClassFromString("XCPointerEventPath"),
      let recordClass = NSClassFromString("XCSynthesizedEventRecord"),
      !paths.isEmpty
    else { throw Driver.Failure(description: "no event synthesis in this XCTest") }
    typealias InitPath = @convention(c) (AnyObject, Selector, CGPoint, Double) -> Unmanaged<AnyObject>
    typealias Move = @convention(c) (AnyObject, Selector, CGPoint, Double) -> Void
    typealias Lift = @convention(c) (AnyObject, Selector, Double) -> Void
    typealias InitRecord = @convention(c) (AnyObject, Selector, NSString, Int) -> Unmanaged<AnyObject>
    typealias Add = @convention(c) (AnyObject, Selector, AnyObject) -> Void
    typealias Synthesize =
      @convention(c) (AnyObject, Selector, AnyObject, @escaping @convention(block) (Bool, NSError?) -> Void) -> Void
    func imp<T>(_ object: AnyObject, _ name: String, _: T.Type) throws -> (Selector, T) {
      let selector = NSSelectorFromString(name)
      guard object.responds(to: selector), let method = class_getMethodImplementation(object_getClass(object), selector)
      else { throw Driver.Failure(description: "no \(name)") }
      return (selector, unsafeBitCast(method, to: T.self))
    }
    func alloc(_ cls: AnyClass) -> AnyObject {
      (cls as! NSObject.Type).perform(NSSelectorFromString("alloc"))!.takeUnretainedValue()
    }
    guard let synthesizer = XCUIDevice.shared.perform(NSSelectorFromString("eventSynthesizer"))?.takeUnretainedValue()
    else { throw Driver.Failure(description: "no event synthesizer") }
    let (synthesizeSelector, synthesize) = try imp(synthesizer, "synthesizeEvent:completion:", Synthesize.self)
    final class Done: @unchecked Sendable {
      var done = false
      var error: NSError?
    }
    let done = Done()
    let first = alloc(recordClass)
    let (initRecord, makeRecord) = try imp(first, "initWithName:interfaceOrientation:", InitRecord.self)
    let record = makeRecord(first, initRecord, "touches" as NSString, UIInterfaceOrientation.portrait.rawValue)
      .takeUnretainedValue()
    let (addSelector, add) = try imp(record, "addPointerEventPath:", Add.self)
    var last = 0.0
    for points in paths {
      let raw = alloc(pathClass)
      let (initPath, makePath) = try imp(raw, "initForTouchAtPoint:offset:", InitPath.self)
      let path = makePath(raw, initPath, CGPoint(x: points[0][0], y: points[0][1]), points[0][2]).takeUnretainedValue()
      let (moveSelector, move) = try imp(path, "moveToPoint:atOffset:", Move.self)
      let (liftSelector, lift) = try imp(path, "liftUpAtOffset:", Lift.self)
      for point in points.dropFirst() { move(path, moveSelector, CGPoint(x: point[0], y: point[1]), point[2]) }
      lift(path, liftSelector, points[points.count - 1][2])
      last = max(last, points[points.count - 1][2])
      add(record, addSelector, path)
    }
    synthesize(synthesizer, synthesizeSelector, record) { _, error in
      DispatchQueue.main.async {
        done.error = error
        done.done = true
      }
    }
    let end = Date.now.addingTimeInterval(last + 30)
    while !done.done && Date.now < end { RunLoop.current.run(until: .now + 0.02) }
    if let error = done.error { throw Driver.Failure(description: "\(error)") }
  }
}
