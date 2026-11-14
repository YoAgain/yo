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

`drive.py` ops: `burst` (fast taps, see Speed), `launch` (`env=YO_HUB:https://127.0.0.1:9` takes the hub away: "offline"), `activate`, `terminate`,
`home`, `tap`/`press`/`doubletap`/`exists`/`wait`/`gone`/`swipe` by `label`, `contains`, `id`, `type`, `index` (`sb=true`
for the system's alerts and Notification Center), `tapxy`/`pressxy`/`drag` by points (`drag sb=true x=100 y=1 x2=100
y2=700 velocity=600` pulls down Notification Center), `type`, `clear`, `tree`, `audit`, `shot`, `quit`.
Face ID: `xcrun simctl spawn DEV notifyutil -s com.apple.BiometricKit.enrollmentChanged 1` (and `-p` it), then
`notifyutil -p com.apple.BiometricKit_Sim.pearl.match`; without it iOS asks for the passcode, and any is taken.

Push: the Simulator build has the notification service and Yo Dev's App Group. `simctl push` shows the payload as it
is (no notification service runs), and APNs gives the Simulator no token on the Mac mini ("Could not get certificate
from simulator", MobileActivation). So `sim.sh notify "Yo Push"` runs YoNotify's own code in a process of its own
(`YoNotifyProbe`, the same App Group and lock) on the hub's push (`{"aps":{"alert":{"title":"Yo","body":"Yo"},
"sound":"yo.caf","mutable-content":1}}`), prints what it made and pushes that. The sender is a second account driven
by the core over HTTP against the dev hub.

Not in the Simulator: passkeys (the app id has no team, so `justyo.app` is not associated: error 1004, shown in the
guide), the iCloud Keychain's sync (its items stay on the Simulator, which is enough for the Sign In list), a real
push through APNs (see above), the camera (black; photos come from the library).

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
| Camera mode: permission, photo from the library, undo ring at the bar's left, sent, B gets "Yo 📷" | a photo from the library often never went: "Sent 📷" and its undo ring stayed (the 1.5-second task read the list's state from before the tap); fixed with the sending state in an object of its own (`FriendList.Bars`); also decoded at every redraw: fixed | 120-134, 260-269 |
| Notifications (simctl push): banner "Yo from YoGPT", Yo Back from the banner sends one (YoGPT answers with a photo) | ok after Yo Back was implemented | 230-236 |
| Push through the notification service (`sim.sh notify`, 2026-11-14): app closed, a stranger's first Yo "Yo from LeoPush71"; a friend's Yo; three in a row "3 Yos from …"; two senders, a line each; a photo "📷 from …"; Yo Back from Notification Center sends one | ok after fixes: the service's notification had no category (no Yo Back, never taken in on opening) and "Yo" as title and text | push-03, 05, 07-09, 15-17 |
| Push with nothing new (the rest of a burst, a blocked sender's Yo): quiet, no banner, no sound | was one more "Yo" with the sound: fixed | push-06, 11 |
| Yo just after leaving the app (its wait at the hub still running takes it, the service finds nothing) | was "Yo" with no name: the app writes "Yo from …" itself now | push-02, 13 |
| The app holds the state's lock past the wait (10 s): "Yo" with the sound, no crash | ok | push-12 |
| Viewer: sender and "now" at the top left over the lines (no pill), next, back, hold to pause, swipe down closes, seen once; largest Dynamic Type truncates the name | ok | 141-153 |
| Offline (YO_HUB): a Yo waits in the core's outbox and arrives when back online (B: 8 → 9) | ok | 160-164 |
| Profile: Statistics, Appearance (Negative, Default icons), Media (Keep Sent Photos: permission, denied alert) | denied alert never showed: fixed | 50-51, 170-176 |
| Send Feedback: 11 times, the 11th "You’ve sent a lot of feedback today" | ok | 52-55 |
| Secure Your Account: passkey fails (Simulator), Skip, Recovery Key, Save… (share sheet, Copy), All Set | ok | 81-85 |
| Account: Show Recovery Key (passcode), Create New Recovery Key | ok | 86-92, 177-179 |
| Sign Out unsecured / secured, Sign In list of keychain accounts, sign in from it | new; a stale call signed the new session out: fixed | 56-64 |
| Sign In with the recovery key: wrong word, wrong checksum, numbered and capitalised words | ok; double submission possible: fixed | 94-98 |
| Delete Account: name typed (any case), passcode, deleted; the name free again (checkmark) | ok; registering it again was refused by the hub's limit of 10 registrations an hour per address, shared with other tests on the dev hub that night ("Too many tries"), from 01:16 to past 02:10 | 180-190 |
| Hold a name in camera mode (no camera): "● Recording", nothing sent, no crash | ok | 274-275 |
| Offline (YO_HUB): Sign Up, Sign In with the recovery key say "Yo can’t reach its server" | ok | 220-221 |
| Recovery key of a deleted account: "We couldn’t find your account" | ok | – |
| Swipe a bar, then another: the first closes; Remove | ok | 205-207 |
| Your Code (QR) sheet | ok; it encodes https://justyo.co/NAME (opened by the app since, see below) | 200 |
| Accessibility audit (list, profile pages) | contrast of white on the brand's bars (by design), title not Dynamic Type (by design); the audit also flags the profile's black titles in a sheet, a false alarm | – |

## Invite links and the first Yo (2026-11-14)

With `xcrun simctl openurl DEV yo://add/NAME` (the same handler as https://justyo.co/NAME; universal links need the
app's team in its id, which the Simulator build has not: the https link opens Safari there), against the dev hub.

| Flow | Result |
|---|---|
| Signed in, link to someone not on the list: added once, on top; again: "Already Added" on their bar, no second bar | ok |
| Signed in, link to a name nobody has: "No Such User" in Add Friend's field, nothing added | ok |
| Signed in, link to one's own name: "That’s you" | ok |
| Fresh install, link before the welcome ("NAME invited you" over Sign Up), app killed while typing the name, still there after the start; signed up: NAME and YoGPT on the list | ok |
| Signed out, link, Sign In from the keychain list: NAME added (without a link it is not) | ok |
| Link to the name then registered: not added, nothing said | ok |
| First Yo: notifications declined: "First Yo Sent!" with Turn On Notifications (opens Settings) and Not Now; allowed: Awesome; the second Yo: nothing | ok |

## Speed (2026-11-14)

How fast a tap answers and a Yo arrives, measured in the Simulator (iPhone 17 Pro, iOS 26.4, on a Mac mini; the app
built Release: `YO_SIM_CONFIG=Release dev/sim.sh build`) with ten friends on the list, against the dev hub, before and
after the changes of branch `perf`.

- `YO_PERF=1` in the app's environment (`drive.py DIR launch env=YO_PERF:1`) writes timings to `perf.log` in its
  temporary folder (`Perf.swift`, `PerfProbe.swift`): when a finger lifts (the touch's own time), the tap's action and
  haptic, when the change was committed and the frame that shows it, each frame that came late (a hitch), each HTTP
  request, the long wait's answer, the notification service's steps (`YO_PERF=stderr` for `YoNotifyProbe`).
- `drive.py DIR burst` taps fast without XCTest's wait for an idle app: `burst n=50 interval=0.1 hold=0.6
  away=[600,1200] points=[[201,154]]` is 50 taps at 10 a second on the first bar, each held 60 ms, the finger lifted
  for 40 ms (XCTest leaves out time when no finger is down: a touch beyond the screen's edge fills it; without it each
  tap comes down as the one before lifts, and SwiftUI drops some of them).
- `yo-send` (tests) is a second account sending Yos (`--count 20 --every-ms 50`); Time Profiler by `xcrun xctrace record
  --template "Time Profiler" --device UDID --attach PID`.

| Tap, 50 at 10/s | Before | After |
|---|---|---|
| Haptic after the tap's action | 8–9 ms (`.sensoryFeedback`, in SwiftUI's next update) | 0 ms (in the action) |
| Main thread per tap (action to committed) | 11 ms | 5 ms |
| Tap to the frame showing "Sent Yo" (p50) | 30 ms | 28 ms |
| SwiftUI's graph update, all 50 taps (Time Profiler) | 141 ms, the whole list each tap | 9–14 ms, the tapped bar |
| Frames lost, one bar / across five bars | 8 / 5 | 4 / 2 |
| Taps lost (finger lifted between taps) | 1 of 50 | 0 of 50 |

The touch's own time to the action is the Simulator's event delivery (9–20 ms, the same before and after).

| Launch, 5 cold starts | Before | After |
|---|---|---|
| `YoApp.init` to the list on screen | 529–666 ms | 381–449 ms (the Yo sound's eight players made off the main thread: 58 ms of it) |
| Process start to the list's first frame | 1381–1574 ms | 1243–1474 ms |

| A Yo arriving (20 at 20 a second) | |
|---|---|
| Long wait's answer to the Yos handed to the app | 34–58 ms: the core acknowledges them to the hub first (a round trip), and writes the account's storage when the list changed (another). Since branch `fast-receive` both come after the hand-over (`after_sync`, the next wait): `tests/speed.rs` at 40 ms round trip, 62 ms before, 21 ms after (one way back from the hub) |
| Handed to the app to on screen | 31–53 ms p50, mostly SwiftUI's drawing (the Simulator draws the bars' soft edge and glass on the CPU) |

| Notification service (`YoNotifyProbe`, the dev hub over the internet) | |
|---|---|
| App closed: state opened (lock, keychain, 342 KiB) | 8–26 ms |
| Inbox read (a new TLS connection) and acknowledged | 78–160 ms and 34–57 ms; since `fast-receive` the ack goes after the notification (`afterSync`) |
| All, from the push to the notification | 0.14–0.25 s |
| App just left, its wait at the hub running: the lock | free (opened in 9 ms; the app took the Yo, the push went quiet): the app holds it for a step, never through its wait |

The hub: `send_yo` to the push request 1.6 ms p50 (`tests/speed.rs`); its APNs connection is kept open now (to
api.push.apple.com a request on a new connection took 420 ms, on an open one 138 ms).

## Bugs found and fixed (branch chaos-ios)

- The welcome asked for notifications before anything (now with the list).
- Hidden hints and the bars' invisible touch copies were read by VoiceOver.
- 16-character names did not fit the name field; the cursor was blue on purple.
- A stale long poll of a client signed out meanwhile signed out the account signed in since (`AccountStore.replaced`).
- Friends stayed after the account was taken to another iPhone; an empty list after signing in (YoGPT now).
- The Media page's "Allow Access to Photos" alert never showed (it was on the profile's root).
- The recovery-key sign-in could run twice; its field was not focused.
- A photo picked from the library was often never sent: the task that ends its 1.5 seconds read the list's @State
  as it was before the tap, found nothing, and the bar kept "Sent 📷" with its undo ring for good. The bars' sending
  state is now an @Observable object (`FriendList.Bars`), captured by those tasks.
- A picked photo was decoded at every redraw.
- No photo or video capture without a camera (AVFoundation raises); the camera no longer starts after it was closed.
- Statistics did not follow Yos while open, and counted blocked friends.
- From the second Codex review: a backup change could be written through the next account's client; registrations
  were not one at a time across ways in; a queued Yo could go from the next account; overlapping passkey requests
  lost a caller; the old keychain item was deleted even when moving it failed; backup changes could overlap and their
  errors showed nowhere; no way on from the passkey check with a password manager without PRF; revealed words stayed
  revealed in the app switcher; onboarding pages did not scroll at the largest text sizes.
- From the first Codex review: Yo Back on a notification did nothing; a collected notification showed twice; the push token
  was not sent again for a new device registration of the same account; answers for a signed-out client changed the
  new account's state; the quiet passkey request after leaving the welcome; dead code.

## Left

- (Since fixed with the core's `YoContact.gone`: a friend whose account was deleted shows "Not on Yo" on the bar, and
  the name added again is asked of the hub anew. Not yet tried in the Simulator.)
- Media sent or received while signing out within the same second may reach the next account's outbox or list.

- Passkeys, push and the share/notification extensions need a device (or TestFlight).
- Sending a photo while signing out within its 1.5 s, and the shared state's deletion racing an extension's write
  (App Group builds only), from the Codex review: not changed.
- Video recording (no camera in the Simulator).
