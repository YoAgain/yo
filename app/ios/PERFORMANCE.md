# Core and UI performance — round 2, 2026-11-16

Measured here: Intel Core Ultra 7 270K Plus, 24 cores. Final release runs and verification were restricted to CPUs 0–17 (six cores outside that affinity). Existing unrelated work continued, so p95 includes contention. No phone measurement, commit, deployment, hub change, or fuzz change.

`tests/tests/core_cost.rs` uses the existing in-process TestHub over loopback HTTP. Each operation has 21 samples; median is sample 11 and nearest-rank p95 sample 20. Peer sends preparing each receive are outside its timer; every receive asserts its exact delivered Yo count. Live ratchets carry forward between fixtures. Restore includes a warm file read, decrypt, decode and client reconstruction; HTTP-client construction and registration are excluded.

Fixtures have 100/500/2000 contact records, 20 real active peer sessions, and both six- and twenty-member groups (including self). Other contacts are synthetic, with distinct random account keys, realistic metadata and bounded resend plaintext caches: eight 96-byte messages at 100/500 contacts, two at 2000. These are **not 2000 independently established sessions**; a device actively messaging all contacts will have a larger session store and needs additional measurement. The 1000 queued items are held for a blocked synthetic recipient so measuring receive does not drain them. State grows modestly during samples as deduplication/session caches fill; fixture sizes below are initial encrypted sizes. Coverage is approximately 250 KiB–2.3 MiB, encompassing the requested 300 KB–2 MB scale.

The default Store writes an adjacent temporary file and renames it atomically on the repository filesystem, matching the app’s atomic replacement model. `/tmp` is tmpfs here and was excluded from the final file measurements. This models process-crash safety, not power-loss guarantees. Optional `YO_COST_FSYNC=1` additionally syncs file and directory; a separate stress run saw file p95 above one second under contention. Those stress figures are not mixed into the comparison below.

Before/after uses the same release executable and harness. Testing-only `YO_COST_DEEP=1` reproduces deep resend-cache snapshots; normal mode shares them until changed. This isolates the change without changing the wire/storage format. The emulation also allocates an Arc per copied cache; it is a close cost baseline rather than an exact old executable. Keys, ratchet histories and scheduler/disk load vary between runs, explaining small fixture-size differences and some flat/regressing rows. Do not interpret every row as an improvement.

```sh
CARGO_BUILD_JOBS=18 YO_COST_DEEP=1 taskset -c 0-17 cargo test --release -p yo-tests --test core_cost -- --ignored --nocapture
CARGO_BUILD_JOBS=18 taskset -c 0-17 cargo test --release -p yo-tests --test core_cost -- --ignored --nocapture
```

All timings below are **milliseconds, median / p95**. Receive100 is the entire burst; divide by 100 for amortized cost per Yo. The phone column is a rough 2–3× **median wall-time proxy**, using the requested per-thread slowdown. It also scales loopback/file waits, so it is not a CPU-only prediction or an actual phone/network latency estimate. iOS builds optimize for size, whereas this host release build optimizes for speed. Flash, keychain, thermal behavior and real network RTT require the phone.

| Contacts / queued | Initial KiB before → after | Operation | Before | After | Rough phone median ms |
| --- | --- | --- | --- | --- | --- |
| 100 / 0 | 248 → 247 | save | 0.513 / 0.679 | 0.526 / 0.653 | 1.1–1.6 |
| 100 / 0 | 248 → 247 | restore | 0.732 / 0.804 | 0.777 / 0.874 | 1.6–2.3 |
| 100 / 0 | 248 → 247 | send1 | 5.163 / 6.517 | 3.860 / 6.555 | 7.7–11.6 |
| 100 / 0 | 248 → 247 | group6 | 10.148 / 12.400 | 9.405 / 10.825 | 18.8–28.2 |
| 100 / 0 | 248 → 247 | group20 | 22.703 / 25.037 | 21.045 / 23.181 | 42.1–63.1 |
| 100 / 0 | 248 → 247 | receive1 | 9.419 / 11.745 | 7.191 / 10.214 | 14.4–21.6 |
| 100 / 0 | 248 → 247 | receive100 | 52.927 / 101.706 | 45.549 / 48.852 | 91.1–136.6 |
| 100 / 1000 | 767 → 766 | save | 1.437 / 2.152 | 1.813 / 2.034 | 3.6–5.4 |
| 100 / 1000 | 767 → 766 | restore | 2.402 / 3.238 | 2.298 / 2.855 | 4.6–6.9 |
| 100 / 1000 | 767 → 766 | send1 | 5.449 / 19.531 | 7.090 / 8.639 | 14.2–21.3 |
| 100 / 1000 | 767 → 766 | group6 | 15.979 / 102.860 | 12.154 / 15.647 | 24.3–36.5 |
| 100 / 1000 | 767 → 766 | group20 | 30.232 / 94.308 | 24.975 / 26.635 | 50.0–74.9 |
| 100 / 1000 | 767 → 766 | receive1 | 11.575 / 84.289 | 9.374 / 12.492 | 18.7–28.1 |
| 100 / 1000 | 767 → 766 | receive100 | 63.993 / 91.756 | 54.127 / 63.489 | 108.3–162.4 |
| 500 / 0 | 1191 → 1185 | save | 22.472 / 68.233 | 2.159 / 2.634 | 4.3–6.5 |
| 500 / 0 | 1191 → 1185 | restore | 4.469 / 4.959 | 3.932 / 4.062 | 7.9–11.8 |
| 500 / 0 | 1191 → 1185 | send1 | 22.220 / 93.575 | 8.868 / 10.738 | 17.7–26.6 |
| 500 / 0 | 1191 → 1185 | group6 | 38.983 / 93.335 | 18.076 / 20.799 | 36.2–54.2 |
| 500 / 0 | 1191 → 1185 | group20 | 44.955 / 193.052 | 30.423 / 33.358 | 60.8–91.3 |
| 500 / 0 | 1191 → 1185 | receive1 | 22.662 / 28.240 | 14.633 / 16.549 | 29.3–43.9 |
| 500 / 0 | 1191 → 1185 | receive100 | 91.583 / 126.662 | 64.315 / 73.703 | 128.6–192.9 |
| 500 / 1000 | 1421 → 1415 | save | 3.398 / 5.240 | 2.892 / 3.055 | 5.8–8.7 |
| 500 / 1000 | 1421 → 1415 | restore | 5.039 / 6.083 | 3.143 / 3.274 | 6.3–9.4 |
| 500 / 1000 | 1421 → 1415 | send1 | 11.341 / 20.261 | 10.105 / 10.881 | 20.2–30.3 |
| 500 / 1000 | 1421 → 1415 | group6 | 29.463 / 38.149 | 22.158 / 24.190 | 44.3–66.5 |
| 500 / 1000 | 1421 → 1415 | group20 | 37.899 / 51.629 | 38.164 / 41.118 | 76.3–114.5 |
| 500 / 1000 | 1421 → 1415 | receive1 | 27.817 / 37.549 | 19.885 / 27.583 | 39.8–59.7 |
| 500 / 1000 | 1421 → 1415 | receive100 | 89.000 / 189.568 | 80.328 / 88.155 | 160.7–241.0 |
| 2000 / 0 | 2101 → 2078 | save | 9.894 / 14.006 | 4.794 / 8.613 | 9.6–14.4 |
| 2000 / 0 | 2101 → 2078 | restore | 9.947 / 11.397 | 8.488 / 8.834 | 17.0–25.5 |
| 2000 / 0 | 2101 → 2078 | send1 | 27.268 / 32.718 | 15.255 / 19.342 | 30.5–45.8 |
| 2000 / 0 | 2101 → 2078 | group6 | 51.301 / 60.929 | 23.810 / 26.269 | 47.6–71.4 |
| 2000 / 0 | 2101 → 2078 | group20 | 87.939 / 107.024 | 42.332 / 55.912 | 84.7–127.0 |
| 2000 / 0 | 2101 → 2078 | receive1 | 42.659 / 52.067 | 46.727 / 57.277 | 93.5–140.2 |
| 2000 / 0 | 2101 → 2078 | receive100 | 254.418 / 400.742 | 127.121 / 152.735 | 254.2–381.4 |
| 2000 / 1000 | 2331 → 2308 | save | 8.633 / 10.799 | 8.406 / 9.593 | 16.8–25.2 |
| 2000 / 1000 | 2331 → 2308 | restore | 10.944 / 11.832 | 9.535 / 10.404 | 19.1–28.6 |
| 2000 / 1000 | 2331 → 2308 | send1 | 26.762 / 29.614 | 23.668 / 36.411 | 47.3–71.0 |
| 2000 / 1000 | 2331 → 2308 | group6 | 44.863 / 55.480 | 49.351 / 70.413 | 98.7–148.1 |
| 2000 / 1000 | 2331 → 2308 | group20 | 81.348 / 106.170 | 59.448 / 87.767 | 118.9–178.3 |
| 2000 / 1000 | 2331 → 2308 | receive1 | 37.362 / 41.637 | 37.533 / 43.986 | 75.1–112.6 |
| 2000 / 1000 | 2331 → 2308 | receive100 | 323.158 / 356.533 | 185.547 / 243.028 | 371.1–556.6 |

The component table is the **after** run. Session crypto measures synchronous libsignal futures, including sealed-envelope crypto and in-memory store work. JSON measures serde_json encode/decode. Seal includes state ChaCha20-Poly1305 plus TLS framing and plaintext wiping; restore decrypt is included there. File measures atomic replacement writes; restore reads are in Other. Other includes rollback snapshots, record export/import, bookkeeping, signing, group work, allocation, loopback HTTP and hub wait. Components partition each sample; independently computed medians/p95 do **not** add up to total quantiles.

| Contacts / queued | Operation | Session crypto | JSON | State seal / framing | File write | Other |
| --- | --- | --- | --- | --- | --- | --- |
| 100 / 0 | save | 0.000 / 0.000 | 0.116 / 0.136 | 0.270 / 0.280 | 0.123 / 0.226 | 0.013 / 0.020 |
| 100 / 0 | restore | 0.000 / 0.000 | 0.218 / 0.235 | 0.182 / 0.188 | 0.000 / 0.000 | 0.376 / 0.459 |
| 100 / 0 | send1 | 0.367 / 0.390 | 0.281 / 0.309 | 0.604 / 0.635 | 0.610 / 0.730 | 1.913 / 4.610 |
| 100 / 0 | group6 | 1.688 / 1.736 | 0.467 / 0.495 | 1.169 / 1.436 | 0.922 / 1.026 | 5.000 / 6.359 |
| 100 / 0 | group20 | 6.229 / 6.362 | 0.550 / 0.686 | 1.693 / 1.938 | 1.136 / 1.275 | 11.552 / 13.530 |
| 100 / 0 | receive1 | 0.656 / 0.761 | 0.735 / 0.816 | 2.324 / 2.660 | 1.114 / 1.347 | 2.156 / 5.048 |
| 100 / 0 | receive100 | 15.831 / 16.234 | 1.431 / 1.897 | 3.993 / 4.852 | 1.902 / 2.386 | 22.415 / 25.885 |
| 100 / 1000 | save | 0.000 / 0.000 | 0.459 / 0.514 | 0.877 / 0.924 | 0.429 / 0.536 | 0.050 / 0.060 |
| 100 / 1000 | restore | 0.000 / 0.000 | 0.728 / 0.851 | 0.578 / 1.114 | 0.000 / 0.000 | 0.981 / 1.253 |
| 100 / 1000 | send1 | 0.377 / 0.397 | 0.971 / 1.207 | 1.920 / 2.367 | 0.977 / 1.358 | 2.899 / 3.761 |
| 100 / 1000 | group6 | 1.694 / 1.759 | 1.462 / 1.638 | 2.873 / 3.159 | 1.518 / 2.016 | 4.574 / 7.901 |
| 100 / 1000 | group20 | 6.294 / 6.582 | 1.487 / 1.594 | 2.887 / 3.505 | 1.506 / 2.032 | 12.619 / 14.017 |
| 100 / 1000 | receive1 | 0.622 / 0.772 | 1.457 / 1.553 | 2.850 / 2.923 | 1.534 / 1.679 | 2.713 / 6.299 |
| 100 / 1000 | receive100 | 15.894 / 16.261 | 2.404 / 3.253 | 5.963 / 6.555 | 2.537 / 4.033 | 28.343 / 32.035 |
| 500 / 0 | save | 0.000 / 0.000 | 0.811 / 0.970 | 0.930 / 1.038 | 0.336 / 0.687 | 0.038 / 0.049 |
| 500 / 0 | restore | 0.000 / 0.000 | 1.242 / 1.347 | 0.940 / 0.959 | 0.000 / 0.000 | 1.754 / 1.843 |
| 500 / 0 | send1 | 0.199 / 0.440 | 1.828 / 2.572 | 2.763 / 3.213 | 1.241 / 1.711 | 2.497 / 3.828 |
| 500 / 0 | group6 | 1.679 / 1.775 | 2.841 / 3.956 | 4.769 / 5.442 | 2.078 / 3.005 | 6.455 / 9.226 |
| 500 / 0 | group20 | 6.340 / 6.828 | 3.221 / 3.460 | 5.281 / 5.945 | 2.234 / 2.328 | 13.335 / 15.844 |
| 500 / 0 | receive1 | 0.670 / 0.783 | 3.254 / 3.502 | 5.135 / 5.340 | 2.226 / 2.558 | 3.030 / 4.481 |
| 500 / 0 | receive100 | 15.949 / 16.818 | 5.113 / 6.366 | 10.731 / 12.167 | 3.520 / 5.253 | 32.800 / 35.372 |
| 500 / 1000 | save | 0.000 / 0.000 | 1.128 / 1.312 | 1.184 / 1.249 | 0.443 / 0.537 | 0.050 / 0.068 |
| 500 / 1000 | restore | 0.000 / 0.000 | 1.429 / 1.521 | 0.681 / 0.717 | 0.000 / 0.000 | 1.031 / 1.105 |
| 500 / 1000 | send1 | 0.367 / 0.391 | 2.251 / 2.387 | 3.567 / 3.787 | 1.370 / 1.632 | 2.420 / 3.179 |
| 500 / 1000 | group6 | 1.761 / 1.853 | 4.059 / 4.753 | 6.479 / 7.228 | 2.522 / 3.082 | 6.272 / 7.921 |
| 500 / 1000 | group20 | 6.308 / 6.625 | 4.743 / 5.503 | 8.029 / 9.431 | 2.698 / 4.228 | 15.775 / 18.206 |
| 500 / 1000 | receive1 | 0.740 / 0.798 | 4.450 / 5.266 | 7.904 / 8.697 | 2.549 / 6.887 | 4.231 / 6.501 |
| 500 / 1000 | receive100 | 16.552 / 16.971 | 6.329 / 8.992 | 11.496 / 13.545 | 4.003 / 5.973 | 42.077 / 46.535 |
| 2000 / 0 | save | 0.000 / 0.000 | 2.352 / 3.060 | 1.750 / 3.261 | 0.633 / 2.164 | 0.063 / 0.085 |
| 2000 / 0 | restore | 0.000 / 0.000 | 3.146 / 3.311 | 1.722 / 1.749 | 0.000 / 0.000 | 3.670 / 3.851 |
| 2000 / 0 | send1 | 0.196 / 0.398 | 4.206 / 4.621 | 4.407 / 6.213 | 2.201 / 3.441 | 4.731 / 6.747 |
| 2000 / 0 | group6 | 0.800 / 0.916 | 6.628 / 7.609 | 5.633 / 6.422 | 2.634 / 3.608 | 7.580 / 8.638 |
| 2000 / 0 | group20 | 2.995 / 7.086 | 6.375 / 7.098 | 7.772 / 11.010 | 3.739 / 7.710 | 19.765 / 27.854 |
| 2000 / 0 | receive1 | 0.619 / 1.078 | 8.132 / 10.908 | 8.927 / 11.098 | 25.781 / 29.317 | 7.481 / 10.503 |
| 2000 / 0 | receive100 | 16.393 / 17.086 | 10.516 / 13.476 | 18.586 / 20.309 | 5.408 / 6.729 | 76.236 / 90.598 |
| 2000 / 1000 | save | 0.000 / 0.000 | 3.330 / 3.808 | 3.446 / 3.881 | 1.430 / 2.411 | 0.086 / 0.109 |
| 2000 / 1000 | restore | 0.000 / 0.000 | 3.456 / 4.033 | 1.962 / 2.001 | 0.000 / 0.000 | 4.090 / 4.419 |
| 2000 / 1000 | send1 | 0.385 / 0.413 | 6.358 / 8.697 | 7.317 / 12.535 | 3.735 / 5.045 | 5.693 / 9.757 |
| 2000 / 1000 | group6 | 1.803 / 2.685 | 11.615 / 19.756 | 13.284 / 23.571 | 6.413 / 10.086 | 14.590 / 26.716 |
| 2000 / 1000 | group20 | 2.980 / 7.002 | 12.479 / 15.343 | 15.639 / 17.691 | 4.549 / 7.137 | 24.753 / 44.381 |
| 2000 / 1000 | receive1 | 0.625 / 0.787 | 8.983 / 10.123 | 14.693 / 15.936 | 3.335 / 4.357 | 10.306 / 12.786 |
| 2000 / 1000 | receive100 | 17.409 / 19.143 | 13.361 / 18.907 | 21.766 / 32.069 | 7.794 / 10.732 | 119.927 / 163.926 |

At 2000 contacts / 1000 queued, receive100 Other fell from 270.245 / 301.158 to 119.927 / 163.926 ms. Full-state rollback cloning/allocation was the cheap fix supported by the profile. Resend caches now share immutable snapshots and copy only on mutation; tests cover independent rollback snapshots and unchanged JSON (including old numeric-array decoding). The rest of State still clones, including the outbox. JSON, state sealing and that remaining bookkeeping are still significant; there is no claim of sub-frame core completion.

No save or ack was removed or deferred. Existing inbox batches already share a persistence step; a full 100-envelope page is saved before ack and further fetch, while sync signing/finalization have their own saves. Sending still persists queued plaintext and advanced sessions before transport. Individual-envelope rollback and failure behavior remain intact. SQLite and a transaction journal remain future work. The measurements do not justify changing the hub tonight.

UI changes: landing values live in per-row observable slots, read inside LiveBar; unchanged bars have an explicit main-actor Equatable boundary, so parent arrival haptics/order changes can reuse them. Settling no longer observes a parent landing dictionary. Actual row moves still require list diff/layout. Stats dictionary reads/mutations/defaults serialization for counts and timestamps run on one ordered utility queue; existing AppStorage counters continue publishing live updates. Reset uses the same queue. Inbox resolves contacts by dictionary and friends once per sender per take instead of repeating linear/skeleton searches for each event.

Received-media manifest encode, ChaCha sealing, atomic write and migration-marker defaults update now run on a serial utility queue. Inbox awaits success before considering media kept; duplicate concurrent expects await the same result. Download completion and retry writers use that queue too. Sign-out drains writes before deletion, and generation checks stop stale completions from changing a new account. This is awaited persistence, not fire-and-forget acknowledgement.

Cold media manifest load/reconciliation, first keychain lookup and legacy migration still run synchronously once (or retry while locked); they are **remaining startup work**, not eliminated by the per-Yo write change. Other main-actor work still includes arrival bookkeeping, array snapshot copy-on-write, list diff/layout and small media-index scans. Phone validation must measure those, confirm row-body counts and missed frames at 100/500/2000 friends, and exercise media save failure, duplicate arrivals, background/locked operation, sign-out during a write and statistics updates/reset. Use the existing 300-run README procedure. No physical-device or UI-regression results are claimed.

Validation: cargo test --workspace -- --test-threads=8 passed (329 tests; four explicitly ignored measurements/corpus writers). Clippy --workspace --all-targets -- -D warnings passed. Fresh aarch64 iOS Rust core rebuild and Swift 6 / iOS 26 xtool dev build passed, including the final relink (existing SDK/cache, unused-variable and groupName isolation warnings remain). The ordinary workspace run skips the explicitly ignored corpus-writing test, so fuzz/ is untouched.

---

Previous-round audit, retained as historical estimates (superseded above):

# Static hot-path audit

Costs below are **unmeasured estimates**, not device results. At 120 Hz the entire frame has 8.33 ms;
at 60 Hz, 16.67 ms. Size, keychain availability, device storage and list length dominate the ranges.
No phone/signing configuration was available. No core/, hub/ or fuzz/ changes were made.

| Path (Sources/YoApp) | Estimated cost / scaling | Result |
| --- | --- | --- |
| Friends.swift:380 save | JSON encode + defaults serialization, roughly 0.1–5 ms for hundreds of friends, potentially more | Moved snapshot encoding/defaults write to ordered utility queue. Only UI cache; core writes retain original durability. A crash can lose latest local ordering/preferences as it already could with async defaults. |
| Friends.swift:212 promote | Two array mutations each formerly triggered full save/mirror, O(friends) | One local array mutation and one publication/save now. |
| Inbox.swift:504 group sender counts | O(group members) + animated invalidation per Yo, roughly sub-ms CPU plus 1–several ms layout per visible group | Gathered until next display callback. Observable SenderRun per bar; LiveBar reads it in its own body (FriendList.swift:1383), so count updates do not invalidate the list parent. |
| FriendList.swift:1197 arrivals / 1259 settle / 408 ForEach | Whole parent depends on landed dictionary, lifts/order and visibility; O(friends), often 1–several ms layout, larger lists unknown | Existing 100 ms arrival batching retained; instrumented list.arrived. Row-local group invalidation fixed, but landed/ordering can still re-evaluate parent rows. Full landing-state migration needs UI regression coverage. |
| Inbox.swift:468 take | O(events × contacts/friends), Unicode skeleton and linear lookups; sub-ms to several ms for 100 events with large lists | Remains main-actor bookkeeping. Core/network waits use Hub.run. Existing whole-take elapsed mark includes await time, so is NOT a main-thread stall measurement. |
| Inbox.swift:591 collectDelivered | Notification service hop + sorting O(notifications log notifications); typical <1 ms local CPU, large pending set several ms | Async service calls already yield. Per-take collection remains; not a blocking network call. |
| Stats.swift:15 / 34 / 44 | Main-thread defaults dictionary copies/serialization O(friends), ~0.1–several ms per outgoing Yo; incoming already batches once/second | Remains; needs measured contribution before changing live AppStorage behavior. |
| Media.swift:581 / 646 / 662 | Main-actor manifest load, directory scan, JSON, ChaCha sealing, atomic disk write; roughly 1–20+ ms, cold keychain can be tens of ms | Remaining photo blocker. Instrumented media.manifest.load/save. Not deferred: expect must persist before core acknowledge. Moving all manifest writers needs a serialized storage owner and error propagation, not a detached fire-and-forget write. |
| Media.swift:77 ManifestSeal | First keychain lookup, commonly 1–20+ ms, sometimes much longer; symmetric key cached after | Remains part of media startup path; captured by manifest spans. |
| Hub.swift:91 / 301, SharedState.swift:122 | Full sealed-state encode/seal/write (300 KB–2 MB), estimated 1–30+ ms per save plus lock contention | Already off-main via dedicated concurrent Hub queue. Preserved synchronous writes before hub commits/acks. Debouncing would violate crash safety without durable replay/journal, so deliberately not done. Can still limit burst throughput. |
| Account.swift:327 openOnce | Core restore, file I/O, keychain, decode: ~5–100+ ms | Already inside Hub.run. Legacy migration in AccountStore init (Account.swift:229) can access keychain on main once; not moved in this patch. Cold-start confirmation needed. |
| Inbox.swift:171 makeSounds; FriendList.swift:2304 fitted | Audio initialization previously documented ~60 ms; photo decode potentially 5–50+ ms | Already detached. Playback remains on main, capped at ten sounds/second. Photo display/GPU decoding still needs device confirmation. |

The changes reduce known repeated work; they do not establish zero missed deadlines. In particular, received
media persistence and parent landing-state invalidations remain candidates. PerfProbe's longest excess
callback interval is not a sampled stack trace. Its overlapping signpost names indicate correlation, not
proof that a named scope alone caused the interval. Warm/operator actions and presentation timing require
a physical device. See README for the resumable 300-run procedure and measurement limitations.
