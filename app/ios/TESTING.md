# Testing Yo in the iOS Simulator

How the app was gone through, screen by screen, in the Simulator on a Mac (`dev/sim.sh`), against the dev hub, with
two Simulators ("Yo Chaos A" and "Yo Chaos B", iPhone 17 Pro, iOS 26.4) and two accounts talking to each other. The
screenshots are not in the repository (names below are from the test run, `shots/`).

## How

```sh
app/ios/dev/sim.sh build                 # core for aarch64-apple-ios-sim, XcodeGen, xcodebuild build-for-testing
app/ios/dev/sim.sh device "Yo Chaos A"   # made once, booted
app/ios/dev/sim.sh install "Yo Chaos A"
app/ios/dev/sim.sh drive "Yo Chaos A" ~/yo-chaos/dA &      # YoDriver, a UI test taking commands from the folder
app/ios/dev/sim/drive.py ~/yo-chaos/dA launch
app/ios/dev/sim/drive.py ~/yo-chaos/dA tap label="Sign Up"
app/ios/dev/sim/drive.py ~/yo-chaos/dA type text=ChaosAnna1
app/ios/dev/sim/drive.py ~/yo-chaos/dA tree                 # the screen's elements
app/ios/dev/sim/drive.py ~/yo-chaos/dA audit                # Xcode's accessibility audit
xcrun simctl io "Yo Chaos A" screenshot shot.png
```

`drive.py` ops: `launch` (`env=YO_HUB:https://127.0.0.1:9` takes the hub away: "offline"), `activate`, `terminate`,
`home`, `tap`/`press`/`doubletap`/`exists`/`wait`/`gone`/`swipe` by `label`, `contains`, `id`, `type`, `index` (`sb=true`
for the system's alerts), `tapxy`/`pressxy`/`drag` by points, `type`, `clear`, `tree`, `audit`, `shot`, `quit`.
Face ID: `xcrun simctl spawn DEV notifyutil -s com.apple.BiometricKit.enrollmentChanged 1` (and `-p` it), then
`notifyutil -p com.apple.BiometricKit_Sim.pearl.match`; without it iOS asks for the passcode, and any is taken.

Not in the Simulator: passkeys (the app id has no team, so `justyo.app` is not associated: error 1004, shown in the
guide), the iCloud Keychain's sync (its items stay on the Simulator, which is enough for the Sign In list), push (no
APNs token), the camera (black; photos come from the library).

## Flows (2026-11-14 night)

| Flow | Result | Shots |
|---|---|---|
| Welcome: logo and 3 cards with pictures of the app, carousel both ways, auto-advance every 4 s, stops on a touch, a swipe, a dot; Reduce Motion: still; largest Dynamic Type | ok after fixes (started on the last card once; pictures; loop) | 30-*, 31-34, 35-axxxl, 70-75, 191 |
| No notification prompt before an account | fixed (was over the welcome) | 01, 36 |
| Name: too short (after a pause, and on Continue with a shake), taken (YoGPT), reserved (Apple), lookalike (Y0GPT), digits first stripped, 16 characters, emoji, accents | ok; 16 characters overflowed the field: fixed | 02-11 |
| Register, YoGPT on the list as "YoGPT" | ok | 12 |
| Yo to YoGPT, its answer ("Yo!") | ok | 13, 14-* |
| Add friend: own name "That’s you", unknown "No Such User" (keyboard stays now), other spelling "chaosben1" becomes "ChaosBen1", offline added as typed | ok | 15-16, 40-41, 162-163 |
| A to B: one Yo, a burst of 10: B counted exactly 11 received | ok | 42-45, 51 |
| Swipe a bar: Block and Remove, one open at a time, a tap closes without sending, not in camera mode | new, ok | 21-23 |
| Long press: Report (Spam, Abuse, Something Else), Thanks for Reporting with Block, Block, Remove | ok | 103-106 |
| Blocked: B gets nothing from A; Blocked list, swipe to Unblock | ok | 107-111 |
| Camera mode: permission, photo from the library, undo ring at the bar's left, sent, B gets "Yo 📷" | ok (a stuck "Sent 📷" once while the library photo was decoded on every redraw: fixed) | 120-134 |
| Viewer: sender and "now" at the top left over the lines (no pill), next, back, hold to pause, swipe down closes, seen once; largest Dynamic Type truncates the name | ok | 141-153 |
| Offline (YO_HUB): a Yo waits in the core's outbox and arrives when back online (B: 8 → 9) | ok | 160-164 |
| Profile: Statistics, Appearance (Negative, Default icons), Media (Keep Sent Photos: permission, denied alert) | denied alert never showed: fixed | 50-51, 170-176 |
| Send Feedback: 11 times, the 11th "You’ve sent a lot of feedback today" | ok | 52-55 |
| Secure Your Account: passkey fails (Simulator), Skip, Recovery Key, Save… (share sheet, Copy), All Set | ok | 81-85 |
| Account: Show Recovery Key (passcode), Create New Recovery Key | ok | 86-92, 177-179 |
| Sign Out unsecured / secured, Sign In list of keychain accounts, sign in from it | new; a stale call signed the new session out: fixed | 56-64 |
| Sign In with the recovery key: wrong word, wrong checksum, numbered and capitalised words | ok; double submission possible: fixed | 94-98 |
| Delete Account: name typed (any case), passcode, deleted; the name free again | ok; re-registering hit the hub's registration limit ("Too many tries") | 180-190 |
| Accessibility audit (list) | contrast of white on the brand's bars (by design), title not Dynamic Type (by design) | – |

## Bugs found and fixed (branch chaos-ios)

- The welcome asked for notifications before anything (now with the list).
- Hidden hints and the bars' invisible touch copies were read by VoiceOver.
- 16-character names did not fit the name field; the cursor was blue on purple.
- A stale long poll of a client signed out meanwhile signed out the account signed in since (`AccountStore.replaced`).
- Friends stayed after the account was taken to another iPhone; an empty list after signing in (YoGPT now).
- The Media page's "Allow Access to Photos" alert never showed (it was on the profile's root).
- The recovery-key sign-in could run twice; its field was not focused.
- A picked photo was decoded at every redraw (the list stopped answering for a while).
- No photo or video capture without a camera (AVFoundation raises); the camera no longer starts after it was closed.
- From the Codex review: Yo Back on a notification did nothing; a collected notification showed twice; the push token
  was not sent again for a new device registration of the same account; answers for a signed-out client changed the
  new account's state; the quiet passkey request after leaving the welcome; dead code.

## Left

- Passkeys, push and the share/notification extensions need a device (or TestFlight).
- Sending a photo while signing out within its 1.5 s, and the shared state's deletion racing an extension's write
  (App Group builds only), from the Codex review: not changed.
- Video recording (no camera in the Simulator).
