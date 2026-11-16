#!/usr/bin/env bash
# Check the synced keychain item's actual Codable fields on Linux, without an iOS keychain.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out="$root/target/account-auth-swift"
mkdir -p "$out"
python3 - "$here/../YoApp/Sources/YoApp/Account.swift" "$out/main.swift" <<'PY'
from pathlib import Path
import sys
source = Path(sys.argv[1]).read_text()
start = source.index('  struct Item:', source.index('enum SyncedAccount'))
end = source.index('    /// An item of before', start)
Path(sys.argv[2]).write_text('import Foundation\nenum SyncedAccount {\n' + source[start:end] + '''  }
}
let old = Data(#"{"username":"TEST","userID":"AQ==","accountKey":"Ag==","byPasskey":true,"byCode":true,"version":3}"#.utf8)
let decoder = JSONDecoder()
var item = try decoder.decode(SyncedAccount.Item.self, from: old)
precondition(item.codeAuth == nil && item.passkeyAuth == nil, "older items still decode and need migration")
for passkey in [true, false] {
  item.codeAuth = true
  item.passkeyAuth = passkey
  let data = try JSONEncoder().encode(item)
  let restored = try decoder.decode(SyncedAccount.Item.self, from: data)
  precondition(restored == item)
  precondition(restored.codeAuth == true && restored.passkeyAuth == passkey, "auth flags survive keychain storage")
}
print("Synced account: legacy decoding and both passkey auth states passed")
''')
PY
swiftc -module-cache-path "$out/modules" -swift-version 6 "$out/main.swift" -o "$out/check"
"$out/check"
