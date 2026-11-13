// Drives Yo Dev in the Simulator by commands from a folder (dev/sim/drive.py writes them): one UI test that runs
// until it is told to stop, so a person or a script can tap through the app step by step and look at each screen.
// The folder is YO_DRIVER_DIR (xcodebuild passes TEST_RUNNER_YO_DRIVER_DIR). A command is `cmd.json`, its answer
// `out.json`, both {"id": n, ...}.
import XCTest

@MainActor final class Driver: XCTestCase {
  private let app = XCUIApplication(bundleIdentifier: "app.justyo.ios.dev")
  private let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")

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
    case "tree":
      return ["tree": try tree(c["sb"] as? Bool == true ? springboard : app)]
    case "tap", "press", "doubletap", "exists", "wait", "gone", "info", "swipe":
      let target = c["sb"] as? Bool == true ? springboard : app
      let timeout = c["timeout"] as? Double ?? (op == "exists" ? 0 : 5)
      if op == "gone" {
        let element = query(target, c)
        let end = Date.now.addingTimeInterval(timeout)
        while element.exists && Date.now < end { Thread.sleep(forTimeInterval: 0.1) }
        return ["gone": !element.exists]
      }
      let element = try find(target, c, timeout: timeout, required: op != "exists" && op != "wait")
      guard let element else { return ["exists": false] }
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
      let o = origin(app)
      let from = o.withOffset(CGVector(dx: c["x"] as? Double ?? 0, dy: c["y"] as? Double ?? 0))
      let to = o.withOffset(CGVector(dx: c["x2"] as? Double ?? 0, dy: c["y2"] as? Double ?? 0))
      from.press(
        forDuration: c["hold"] as? Double ?? 0.05, thenDragTo: to,
        withVelocity: XCUIGestureVelocity(c["velocity"] as? Double ?? 1000), thenHoldForDuration: c["after"] as? Double ?? 0)
      return [:]
    case "type":
      let text = c["text"] as? String ?? ""
      // Typing with no keyboard up fails the test (and XCTest ends it): an answer instead.
      guard app.keyboards.count > 0 else { throw Failure(description: "no keyboard") }
      if let field = try find(app, c, timeout: 0, required: false) { field.typeText(text) } else { app.typeText(text) }
      return [:]
    case "clear":
      guard app.keyboards.count > 0 else { throw Failure(description: "no keyboard") }
      app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: c["n"] as? Int ?? 30))
      return [:]
    case "sleep":
      Thread.sleep(forTimeInterval: c["seconds"] as? Double ?? 1)
      return [:]
    case "shot":
      let path = c["path"] as? String ?? "/tmp/yo-driver.png"
      try XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: path))
      return ["path": path]
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
    let type = Self.types[c["type"] as? String ?? "any"] ?? .any
    var predicates: [NSPredicate] = []
    if let label = c["label"] as? String { predicates.append(NSPredicate(format: "label == %@", label)) }
    if let part = c["contains"] as? String { predicates.append(NSPredicate(format: "label CONTAINS %@", part)) }
    if let id = c["id"] as? String { predicates.append(NSPredicate(format: "identifier == %@", id)) }
    if let value = c["value"] as? String { predicates.append(NSPredicate(format: "value == %@", value)) }
    let matches = target.descendants(matching: type).matching(NSCompoundPredicate(andPredicateWithSubpredicates: predicates))
    return matches.element(boundBy: c["index"] as? Int ?? 0)
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

  /// What an element is now (gone after the tap: only that; asking a gone element fails the test).
  private func describe(_ e: XCUIElement) -> [String: Any] {
    guard e.exists else { return ["exists": false] }
    let f = e.frame
    return [
      "exists": e.exists, "label": e.label, "id": e.identifier, "value": "\(e.value ?? "")",
      "frame": [f.minX, f.minY, f.width, f.height], "hittable": e.exists && e.isHittable, "enabled": e.isEnabled,
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
