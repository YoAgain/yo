# Core

One Rust core runs the whole client ([client](client), [wire](wire), [bindings](bindings), [product](product)); the
apps draw and pass HTTP and storage through. This page lists everything the apps send about themselves, and what the
hub counts.

## Telemetry

Everything the iOS app (`app/ios`), the Android app (`app/android`) and the core send on their own, and what the hub
(`hub/src`) and the admin (`admin/`) keep. Yos, photos and videos are end-to-end encrypted and not covered here.

Summary:

- Crash and error reports go out **on by default**: Settings › Status › **Send Crashes** (iOS `crashReports.on`,
  Android prefs `crashes/enabled`). One switch covers both. Switched off, reports wait on the phone and nothing is
  sent; switched on again, the backlog goes out.
- Reports are unsigned and go over the app's anonymous connection pool (`yo_wire::api::anonymous`): no device id,
  user id or push token. The hub stores no IP for them.
- Support messages are plain text to the hub, not end-to-end encrypted, and belong to the account.
- Hub counters are counts per minute, hour or day. They hold no ids.
- Only the operator sees any of it, on the admin page (Cloudflare Access plus its own login, read-only on the hub
  database).

### 1. Crash reports

`POST /v1/diagnostics/crash`, unsigned, a JSON object of strings, at most 60 KiB. The hub accepts only the keys listed
below (`hub/src/support.rs`).

- **iOS**: MetricKit `MXCrashDiagnostic`, delivered by iOS (usually the next day). Each crash is queued once, deduped by
  payload hash, and sent when the app comes to the foreground.
- **Android**: uncaught JVM exceptions (own handler, then Android's), plus native crashes and ANRs from
  `ApplicationExitInfo` (API 30+). Sent at app start.
- **Old apps**: `POST /v1/crash` (`api::Feedback`: text and app version). The hub maps it into the same table.

| Field | Example | Purpose | Retention | Linked to account? |
|---|---|---|---|---|
| `uuid` | `3F2A…-…` (random per report) | hub drops a re-sent duplicate | `diagnostic_seen`, kept with its group | no |
| `app` | `ios/202611191257`, `android/202611160001` | build | latest value per group | no |
| `os` | `iPhone OS 26.1 (23B85)`, `Android 35` | OS version | latest value per group | no |
| `model` | `iPhone17,1`; Android always `Android` | hardware model (iOS only) | latest value per group | no |
| `exception` | iOS `1/0/11` (type/code/signal); Android `java.lang.IllegalStateException`, `Managed crash`, `5/11` (exit reason/status) | crash kind; Android sends a class name only if it starts with `java.`/`kotlin.` | latest value per group | no |
| `termination` | iOS `Namespace SIGNAL, Code 11` (≤2000 chars); Android `Uncaught` / `OS exit` | why the OS ended the app | latest value per group | no |
| `frames` | `YoApp +0x1c2f4c` (≤25, crashed thread); Android `app.justyo.android.Repository.flush:212` | where it crashed | latest value per group | no |
| `details` | iOS: the MetricKit call-stack tree cut down to binary names and offsets (≤40 KB); Android: same as `frames` | all threads, for symbolication | latest value per group | no |

**Stripped on the phone**:

- iOS keeps only `binaryName` (file name, no path), offsets and the thread tree. Exception messages, the
  Objective-C exception reason, VM region info and every other MetricKit string are dropped.
- Android keeps only frames of `app.justyo.android.*` and `uniffi.yo.*` (class, method, line). Native frames are kept
  only for a fixed list of binaries (`libyo.so`, `libc.so`, `libart.so`, …). Messages, thread names, paths and memory
  are never kept.

**Hub**:

- Table `diagnostics`, one row per group. A group's `signature` is the hash of (kind, exception, frames, termination,
  app).
- Each row has `count`, `first_seen`, `last_seen` (ms), the latest `app`/`os`/`model`/`payload`, and `done` (operator
  marked it handled).
- Rows are **kept until deleted by hand**; there is no automatic expiry. `diagnostic_days` (count per day and kind)
  is kept 14 days.
- Seen by the operator only: admin › Errors.

**On the phone**: iOS keeps up to 500 report rows in UserDefaults (`diagnostics.reports`), sent ones too, shown under
Status › Crash Reports. Android keeps the last 16 (`crashes/history`).

### 2. Error reports (not crashes)

`POST /v1/errors`, unsigned, same 60 KiB JSON object and the same table (`kind = 'error'`). A group's `signature` is
the hash of (kind, domain, code, place, app). Sent with the same switch.

| Field | Example | Purpose | Retention | Linked to account? |
|---|---|---|---|---|
| `uuid` | random | dedupe | as above | no |
| `domain` | `HTTP`, `NSURLErrorDomain`, `YoCore.YoError`, `NotificationFallback`, `UnknownSender`, `StateMigration`, `IntentSend`, `LiveActivity`, `com.apple.AuthenticationServices.AuthorizationError` | error type | as above | no |
| `code` | `500`, `-1001`, `Refused/409`, `Save`, `hubStatus/503`, `unknownGroupSender contact=no members=4 inRoster=yes friend=no` | the enum case or number, never its values or message | as above | no |
| `place` | `YoApp/Shared/Hub.swift:240 request(...)`, `HubTransport.request POST /v1/inbox`, `NotificationService`, `ForegroundInbox` | code location; routes are cut to two segments, so no ids | as above | no |
| `count` | `"37"` (1…100000) | phone-side count for one kind and day | adds to the group's `count` | no |
| `details` | `2026-11-19: 37`, `day=20411 count=37`, `2026-11-19: 2 · user=3fa91c` | day and count; for `UnknownSender` also a **6-hex (24-bit) SHA-256 prefix of the other person's user id** | latest value per group | no; the prefix points loosely at the sender, not the reporter |
| `severity` | `expected` | known to happen (offline, 401 re-sign, 429, locked before first unlock); counted but hidden in admin | as above | no |
| `app`, `os`, `model` | as for crashes; iOS `os` is `Version 26.1 (Build 23B85)` | build, OS, model | latest value per group | no |

**What sends errors**:

- **iOS, at once**: any caught error at `Diagnostics.record`/`attempt` (network errors the core retries are not sent);
  HTTP answers ≥400 that are not expected; transport errors; an answer too large (`HubTransport 413`).
- **iOS, once per kind and day** (`CountedErrors`): app and extensions append to a file in the App Group, and the
  app sends one report per kind and day. This covers expected HTTP statuses and offline URL errors, notification
  fallbacks, unknown senders, state database failures, Siri/Shortcut sends, Live Activities and passkey cancels.
- **Android**: once per day, counted notification fallbacks (`pushSyncFailed`, `unknownSender`,
  `unknownGroupSender`) and state database failures.
- **Hub itself**: a failed push (APNs/FCM). It is written as an error with `domain: push`, `code` the reason,
  `place` the service, `app: hub` and `details` the service's answer with the token taken out. It is also counted as
  `push-fail` (section 5).

### 3. Support messages and user reports

Signed requests from the account, so **linked to the account**.

`POST /v1/support` (and the older `/v1/feedback`), `api::Feedback`. Plain text to the hub, **not end-to-end
encrypted** (spec 1). Limit `feedback_per_user_day`; at most 1000 messages per thread.

| Field | Example | Purpose | Retention | Linked to account? |
|---|---|---|---|---|
| `text` | `the widget shows ?` (1–2000 bytes UTF-8) | the message | kept for good | yes (`support_messages.user_id`) |
| `app_version` | iOS `1.2 (34)`; Android `VERSION_NAME` (≤64 bytes) | which build | with the message | yes |
| `created_at` | ms, set by the hub | order | with the message | yes |
| `read_at`, `done_at` | ms | operator's answers read by the user; user's message handled by the operator | with the message | yes |

- The operator's answers are in the same table (`from_team = 1`), read by the app with `GET /v1/support`.
- When the account is deleted, its messages stay but `user_id` becomes all zeros (`NOBODY`): no longer linked.
- Admin › Support shows each thread with the user's name.

`POST /v1/report` (report a user) stores reporter id, reported id, reason (1 spam, 2 abuse, 3 other) and `created_at`.
It holds no content, keeps one row per pair, and is deleted with either account. Linked: yes, to both.

### 4. Per-request metadata

What the hub sees on every request, and what it keeps.

| Field | Example | Purpose | Stored? | Linked to account? |
|---|---|---|---|---|
| `Yo-App` header | `ios/202611191257` | too old → `426`; on a signed request kept per device | `devices.app`, updated when it changes; anonymous requests: only checked | yes, per device |
| `Yo-Device`, `Yo-Time`, signature | device id (b64), ms | authenticate the request; replay check | the signature only in an in-memory replay cache for the time window | yes (it is the device) |
| last seen | ms of the first signed request after 24 h | active users, retention | `devices.last_seen_at`, written at most once a day | yes, per device |
| push token and platform | APNs/FCM token, `1` | wake the phone | `devices.push_token`, `push_platform`; cleared when the service says it is dead | yes |
| device created | ms | admin's device list | `devices.created_at` | yes |
| account created | ms | sign-ups | `users.created_at` | yes |
| IP address | `203.0.113.7` (`CF-Connecting-IP` behind Cloudflare, IPv6 cut to /64) | rate limits (registration, anonymous bytes per hour, …) | **no**: in-memory buckets only, dropped after a day idle and on restart; never logged | no |
| User-Agent | the system default (CFNetwork/Darwin, okhttp) | none | not read | no |
| request body hash, path | | signature | no | |

- Sealed Yos (`POST /v1/inbox/{user}`), blobs and reports run on a separate anonymous connection pool, so no
  connection carries both kinds of request.
- A stored envelope keeps its `received_at` until fetched or until `envelope_ttl_days`.
- Cloudflare, in front of the hub, sees IPs under its own terms; that is outside this repository.

### 5. Hub counters

All counters are counts with no user, device, envelope, blob or group in them (spec 6.6).

**`events`**: (kind, minute, n), in memory and flushed once a minute. Granularity: minute (UTC epoch minutes).
Retention: **kept for good**. Linked to account: no.

| Kind | Counted when |
|---|---|
| `yo-in` | a Yo accepted into a person's inbox (sealed: the hub knows the recipient only) |
| `yo-in-bot` | a Yo accepted into a bot's inbox (every device names `yogpt/` or `cli/`) |
| `yo-out` | a Yo fetched and acknowledged by a device |
| `latency-1s`, `-5s`, `-60s`, `-slow` | the time from accepted to acknowledged, one bucket per acked Yo |
| `media-in`, `media-in-bytes` | a blob uploaded, and its bytes |
| `media-out` | a blob fetched to its end |
| `group-create` | a group created |
| `push-ok`, `push-fail` | APNs/FCM took the push, or failed (a dead token is not counted) |
| `http`, `http-4xx`, `http-429`, `http-5xx` | every answer, and its class |
| `account-gone` | an account deleted (by its device or by the operator) |

**Other counters**:

- **`deliveries_hourly`** (hour, n): `yo-in` + `yo-in-bot` per hour, kept 8 days. Not linked.
- **Live stream** `/v1/admin/live`: each `yo-in`/`media-in` the moment it is counted, as `{"k":"yo"}` or `{"k":"media"}`.
  Nothing is stored. Not linked.
- **`diagnostic_days`** (day, kind, count): crash and error reports per day, kept 14 days. Not linked.
- **`blob_quota`** (user, day, count, bytes): each user's uploads per day, for the quota, kept 2 days. Linked: yes.

**Admin, derived from the hub database** (`admin/users.py`, read-only):

- active users today/7/30 days and D1/D7/D30 retention, from `last_seen_at`;
- sign-ups per day, from `users.created_at`;
- devices per build, total and seen today, from `devices.app`.

**Admin, kept in its own state dir** (`admin/admin.py`). All counts, not linked:

| File | Content | Retention |
|---|---|---|
| `active-days.json` | {day: users seen that day} per hub | 60 days |
| `version-days.json` | per day [current, update, must, unknown] device counts, and devices per build | per day for good; per hour 48 hours |
| `queue.json` | Yos and bytes waiting for offline phones | per hour 48 hours; per day 400 days |

The admin's users table (name, devices, last seen, push platform, build, backup) is a live read of the hub database,
not a copy.

### 6. Other automatic traffic

| What | Sent | Content | Stored at the hub? |
|---|---|---|---|
| Connection warm-up | iOS: `HEAD /` on the anonymous pool every 20 s while the app is in the foreground | nothing but `Yo-App` | no |
| Round trip probe | iOS: a tap on the Account title, and every ~1 s while Status is open; Android: a tap on Status | a sealed Yo to the `PINGTEST` bot, which echoes it | as any Yo: counted in `events` (`yo-in-bot`, `yo-out`, latency) |
| Hub key | iOS Status: `GET /.well-known/yo-hub-key` | none | no |
| MetricKit | iOS subscribes for **crash diagnostics only**; metric payloads (launch time, hangs, battery) are not read or sent | | |
| Hitch monitor | `YO_DEV` builds only; a local file of the last 100 frame hitches, shown in Status | never sent | no |
| Perf marks, `Lat` | developer runs only (`Perf.on`, `Lat.on`), to the device log | never sent | no |
| Data usage | bytes sent and received, counted on the phone for Status | never sent | no |
| Passkeys | a cancel is a counted, expected error; other failures an error report (section 2) | | as section 2 |

### Proposed (not built): daily crash summary

To get crash rates rather than crash counts, the apps would send, only while Send Crashes is on, once a day:
`POST /v1/diagnostics/summary`, unsigned, on the anonymous pool, with no identifier (no uuid, no device id).

| Field | Example | Purpose |
|---|---|---|
| `sessions` | `14` | foreground sessions on the last full day |
| `crashed_sessions` | `1` | of those, ended by a crash |
| `crashed_today` | `true` | any crash on that day |
| `crashed_last_7_days` | `false` | any crash in the last 7 days |
| `crashed_last_30_days` | `true` | any crash in the last 30 days |
| `app` | `ios/202611191257` | build |
| `os` | `iPhone OS 26.1` | OS version |

The hub would add each summary into per-day counters by app and OS (sums of sessions and crashed sessions, number of
summaries, number with each flag true) and keep no single row.

**Admin**:

- **Crash rate per session** for a range = Σ`crashed_sessions` / Σ`sessions`, per build or OS.
- **Crash-free devices (7 / 30 days)** = 1 − (summaries of one day with `crashed_last_7_days` (or `_30_days`) true) /
  (summaries of that day). Each phone sends one summary a day, so one day's summaries stand for that day's active
  devices, with no id needed to dedupe.
- Phones that did not open the app that day are left out. A crash on the last day is in the next day's summary.
