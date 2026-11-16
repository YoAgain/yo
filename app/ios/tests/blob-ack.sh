#!/usr/bin/env bash
# Exercise the production ack journal on Linux, including a failed ack across the NSE/app file handoff.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out="$root/target/blob-ack-swift"
mkdir -p "$out"
python3 - "$here/../YoApp/Sources/YoApp/Prefetch.swift" "$out/main.swift" <<'PY'
from pathlib import Path
import sys
source = Path(sys.argv[1]).read_text()
helpers = source[source.index('  private static func make('):source.index('  private static func mark(')]
journal = source[source.index('  private struct Ack:'):source.index('  // MARK: The notification service')]
Path(sys.argv[2]).write_text('''import Foundation
import Glibc
struct YoMedia { let id: Data }
enum ShareBridge { static let container: URL? = URL(fileURLWithPath: CommandLine.arguments[1]) }
final class YoClient {
  var fail = false
  var calls = 0
  var whileAcking: (() -> Void)?
  func ackMedia(id: Data) throws {
    calls += 1
    whileAcking?()
    if fail { throw CocoaError(.fileWriteUnknown) }
  }
}
enum Prefetch {
''' + next(line + '\n' for line in source.splitlines() if 'private static var directory:' in line) + helpers + journal + '''}
let directory = ShareBridge.container!
try? FileManager.default.removeItem(at: directory)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
let media = YoMedia(id: Data(repeating: 9, count: 16))
let file = directory.appending(path: "prefetched.media")
let appFile = directory.appending(path: "received.media")
let client = YoClient()
precondition(Prefetch.prepareAck(media, file: file))
precondition(Prefetch.retryAcks(client: client))
precondition(client.calls == 0, "never acknowledge before storage")
try Data("photo".utf8).write(to: file, options: .atomic)
client.fail = true
client.whileAcking = {
  precondition(Prefetch.prepareAck(media, file: file), "in-flight ack must not hold the storage journal lock")
}
precondition(Prefetch.retryAcks(client: client))
precondition(client.calls == 1)
// The file moves from the service to the app; viewing can delete that file before the next retry.
precondition(Prefetch.prepareAck(media, file: appFile))
try FileManager.default.moveItem(at: file, to: appFile)
Prefetch.stored(media)
try FileManager.default.removeItem(at: appFile)
client.fail = false
precondition(!Prefetch.retryAcks(client: client))
precondition(client.calls == 2, "persisted stored marker permits retry after viewing")
precondition(Prefetch.prepareAck(media, file: appFile))
precondition(!Prefetch.retryAcks(client: client))
precondition(client.calls == 2, "app must not repeat the service's successful ack")
// A process can end after the atomic write but before recording the stored marker.
let interrupted = YoMedia(id: Data(repeating: 8, count: 16))
precondition(Prefetch.prepareAck(interrupted, file: file))
try Data("photo".utf8).write(to: file, options: .atomic)
let restarted = YoClient()
precondition(!Prefetch.retryAcks(client: restarted))
precondition(restarted.calls == 1, "recover the written file after process death")
try FileManager.default.removeItem(at: directory)
print("Blob ack journal: storage ordering, failed ack, file handoff, viewing and duplicate suppression passed")
''')
PY
swiftc -module-cache-path "$out/modules" -swift-version 6 "$out/main.swift" -o "$out/check"
"$out/check" "$out/files"
