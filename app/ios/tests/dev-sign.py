"""Check dev identity expansion without credentials, signing, or a phone."""
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile

script = Path(__file__).resolve().parents[1] / "dev/sign.sh"
with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    app = root / "Yo.app"
    app.mkdir()
    bundle = "app.justyo.ios.dev"
    # Deliberately different: an app-id prefix need not be the team identifier.
    prefix, team = "AAAAAAAAAA", "BBBBBBBBBB"
    plistlib.dump({"CFBundleIdentifier": bundle}, (app / "Info.plist").open("wb"))
    profile = {
        "ApplicationIdentifierPrefix": [prefix], "TeamIdentifier": [team],
        "Entitlements": {
            "application-identifier": prefix + ".*",
            "aps-environment": "development",
            "keychain-access-groups": [prefix + ".*"],
            "com.apple.developer.associated-domains": ["*"],
        },
    }
    mock = root / "rcodesign"
    mock.write_text('''#!/usr/bin/env python3
import os, plistlib, sys
from pathlib import Path
args = sys.argv
ent = plistlib.load(open(args[args.index("--entitlements-xml-file") + 1], "rb"))
info = plistlib.load(open(Path(args[-1]) / "Info.plist", "rb"))
expected = "AAAAAAAAAA.app.justyo.ios.dev"
assert ent["application-identifier"] == expected
assert ent["keychain-access-groups"] == [expected, "AAAAAAAAAA.*"]
assert info["YoKeychainAccessGroup"] == expected
assert ent.get("com.apple.developer.devicecheck.appattest-environment") == (os.environ["TEST_ATTEST_ENV"] or None)
''')
    mock.chmod(0o755)
    for environment in ("", "development", "production"):
        if environment:
            profile["Entitlements"]["com.apple.developer.devicecheck.appattest-environment"] = environment
        (root / (bundle + ".mobileprovision")).write_bytes(plistlib.dumps(profile))
        subprocess.run(["bash", str(script), str(app), str(root / "out.ipa"), str(root)],
                       env={**os.environ, "PATH": str(root) + ":" + os.environ["PATH"], "TEST_ATTEST_ENV": environment},
                       check=True, stdout=subprocess.DEVNULL)
print("Dev signing: concrete app identity, app-id prefix and legacy access passed")
