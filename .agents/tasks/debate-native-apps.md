# Switchboard native apps for iOS and Android

## Outcome

The task: give Switchboard a proper human interface on the phone, instead of WhatsApp. You decided on fully native apps, SwiftUI on iOS and Kotlin with Jetpack Compose on Android.

Decision: the hub gets a second front door for the apps, next to the bots' MCP. It's a small REST/JSON API plus one WebSocket for live events, and both front doors call the same session service the hub already has. Each app is a thin native client generated from one OpenAPI contract.

With the apps you can:
- see your sessions
- start one with pickers for project, machine and agent
- watch the agent's screen update about once a second
- read the worktree diff
- send follow-ups
- stop a session
- approve or deny permission requests

Phones pair with the hub by scanning a QR code. Approving a request needs a signature from a device key that requires Face ID, fingerprint or a passcode, so a stolen login token can deny but never approve.

Push goes directly from the hub: APNs on iOS, Firebase Cloud Messaging on Android. It carries no command text, only a generic alert and ids.

iOS comes first, because your paid Apple account is ready. Android follows to parity, after you create a free Firebase project and run two short tests. Live Activities, widgets and a full terminal emulator are deferred.

## Rationale

The criteria: one developer maintaining two native apps and a Rust backend, everything that leaves your machines masked, and approvals that fail closed.

**One API, many front doors.** The hub already has a typed session service: projects, machines, start, list, get with long-polling, read, prompt, decide, answer, stop and notify (`src/hub/service.rs`). MCP for bots (step 7) and the app API are both thin adapters over it. They share policy, idempotency, masking, audit and the mute state, and neither duplicates logic.

**REST and a WebSocket, not gRPC.** The app makes a handful of calls plus one event stream. gRPC would add heavy client libraries on both platforms and make debugging harder, for no gain at this size.

**Code-first OpenAPI.** The Rust hub is the source of truth, so the spec is generated from Rust types (utoipa), committed, and checked in CI. CI also compiles the generated Swift and Kotlin clients, so a contract break fails the build rather than the app.

The spec is built from dedicated public types, never the hub's internal view types. Today's internal types expose things like the worktree path and raw tool input (`src/hub/service.rs`), which would otherwise become public contract.

Schema-first was considered. It would mean defining every type twice, once in the spec and once in Rust.

**Two separate apps, no shared code layer.** The business logic lives in the hub. What's left in each app is UI, networking from the generated client, secure key storage and push. Kotlin Multiplatform or a shared Rust core over FFI would add an extra build toolchain for very little shared logic. What the apps share is the spec, test fixtures and behaviour tests.

**Pairing by QR, with a confirmation step.** The admin command shows a QR code holding the hub URL and a single-use, short-lived code. The phone creates its keys in hardware (Secure Enclave on iOS, the hardware-backed Android Keystore on Android) and registers them. Both sides then show a short fingerprint derived from the registered keys, and you confirm in the admin terminal. No token is issued before that.

Not everyone agreed. One view was that the QR alone is enough, since it only appears in an admin terminal. Confirmation won because it closes the window where someone photographs the QR before you scan it, and pairing is rare enough that one extra step costs almost nothing.

Passkeys were deferred, because they need associated-domain setup on both platforms.

**Approvals are proven by signature.** A normal biometric prompt inside the app doesn't stop someone who has stolen an access token from approving through the API directly. So an approve must carry a signature from a second device key that requires user authentication. The signature covers the hub, the device, the approval id, the decision, a nonce and an expiry, and the hub consumes it atomically.

On iOS that key requires user presence, Face ID or passcode, not the current biometric set, so enrolling a new fingerprint doesn't silently invalidate it. Android uses the equivalent strong-biometric-or-device-credential setting. The exact Android option names are unverified.

Some situations disable approving on that device but leave viewing and denying working:
- there's no passcode
- hardware keys aren't available
- the passcode fallback isn't enforced at the key level on that device

Deny only needs the normal token, so it can run straight from a notification. Approve always opens the app.

**Push carries nothing sensitive.** Notification text passes through Apple's and Google's servers, so it's generic, for example "Approval needed · session #3", plus ids. Even masked, it never includes the project name, tool name, command or output. The app fetches the details after it opens.

Push is only advisory. After reconnecting, the app reconciles pending approvals from the hub. When an approval closes, its notification is withdrawn or collapsed. Push follows the existing mute state, and each event notifies through one front door only, so the app and WhatsApp don't both fire.

**Android push uses Firebase, not self-hosted UnifiedPush.** For one user, creating a free Firebase project is less work than running an ntfy server and installing a distributor app on the phone. The payloads are opaque, so Google learns little. FCM's service-account key and the APNs signing key are hub secrets: they're stored under the hub's account, never in the repo.

**The live view is plain-text snapshots.** Herdr 0.9.1 has no raw terminal output stream. Its subscribable events are only agent status changes, output-pattern matches and scroll changes. So the runner polls the agent's visible screen as text while someone is watching, about once a second, slowing down when nothing changes and stopping when nobody is watching. It sends a full snapshot only when the screen changes. The whole snapshot is masked before truncation and before it leaves the machine.

ANSI colour is left out because escape sequences can split a secret past the masking patterns. The apps render selectable monospace text, not a terminal emulator.

## Scope and acceptance criteria

**In scope.**
- Hub event and protocol additions.
- The runner's watch and diff operations.
- Device pairing and auth.
- The app API.
- The events WebSocket.
- The OpenAPI spec with generated clients.
- APNs and FCM sending.
- The iOS app.
- The Android app to parity.

**Out of scope for v1.** Live Activities, widgets, ANSI colour or a terminal emulator, passkeys, offline actions, multiple users and app store release. TestFlight and direct installs are enough.

**App features in v1.**
- Pairing.
- A sessions list with status and pending approvals.
- Session detail: status, approvals, follow-up prompt, stop and mute.
- A start sheet with project, machine and agent pickers.
- A live screen.
- A diff view.
- Push for "approval needed" and "finished".
- A devices screen with revoke.

**Acceptance criteria.**
1. Pairing works only with a fresh, single-use code and confirmed fingerprints. Expired, reused or unconfirmed codes issue no token. Every pairing is audited and announced to the devices already paired.
2. A stolen access token can list, read and deny, but an approve without a valid, unexpired, single-use device signature is rejected. Approvals still work when the signature uses the passcode fallback, where the device enforces it.
3. Revoking a device rejects its requests and closes its WebSocket immediately.
4. Starting from the app creates exactly one session per tap, using a request id per tap.
5. The live screen updates within about 2 seconds of a change, stops polling when no one watches, and never shows an unmasked secret. A test feeds a token through the screen and checks that it's masked.
6. The diff covers staged, unstaged and untracked files within a size cap, masked before truncation, with external diff drivers disabled and no arbitrary paths.
7. Push reaches a physical iPhone, and an Android phone once Firebase exists, with generic text only. Deny from the notification works. Approve opens the app. Closed approvals withdraw their notification, and missed pushes are reconciled on open.
8. Muting a session silences push and WhatsApp alike, and each event notifies through only one front door.
9. CI regenerates the OpenAPI spec, fails on drift, and compiles both generated clients.

## Implementation plan

New code is marked as proposed. Existing code is referenced by path.

**1. Hub events and runner operations.**
- Extend the hub's event type with session changes (id and revision), approval closed, and screen frames.
- Extend the runner protocol (`src/proto/mod.rs`) with:
  - watch screen and unwatch, held by a 30-second lease
  - worktree diff
- New runner modules (proposed): a screen poller with the lease in `src/runner/screen.rs`, and diffs in `src/runner/diff.rs`.
- The diff operation also fills the stale, dirty and unmerged flags that step 6 left out of the session list.

**2. Early spikes, before the apps grow.**
- Android key signing with the authentication-bound key, including whether the passcode fallback is enforced by the key itself.
- Generating and compiling the Swift and Kotlin clients from a stub spec.
- Whether the token-refresh key can be used from a notification action on a locked phone after the first unlock.

**3. Hub device auth and the app API** (proposed modules):
- Device pairing, challenge signing, token issuing and revocation, in `src/auth/device.rs`.
- Routes under `/app/v1`, in `src/hub/app/`.
- The events WebSocket at `/app/v1/events`.
- Public API types and the OpenAPI generation.
- A store migration for devices, push tokens and nonces.
- An admin command, `switchboard admin pair-device --name`.
- App principals and the bots' OAuth or xAI principals stay separate, and each is refused on the other's routes.

**4. iOS app** (proposed `apps/ios/`, SwiftUI): the generated API client, device keys in the Secure Enclave, and the pairing, sessions, detail, start sheet, live screen, diff and devices screens.

**5. Push** (proposed `src/push/`):
- APNs over HTTP/2 with a token-based signing key.
- FCM HTTP v1 with a service-account key.
- Collapse ids so closed approvals replace their notification.
- The single-front-door rule, applied using the mute state.

Push to a real phone needs a public HTTPS hub: either step 8's deploy or a tunnel.

**6. Android app** (proposed `apps/android/`, Kotlin and Compose): the same modules as iOS, with Keystore keys and FCM, after the Firebase project exists.

**Dependencies.**
- Step 1 comes before the live screen and diff screens.
- Step 3 comes before any app screen.
- Step 5 needs public HTTPS.
- Step 7 (MCP and OAuth) can go in parallel, because it sits on the same service.
- Step 8's HTTPS and signing groundwork should be pulled forward to unblock push.

**Local development.**
- Run the hub and runner on the Mac.
- The iOS simulator reaches the hub on localhost, and the Android emulator on 10.0.2.2.
- Physical phones go through a tunnel.
- `xcrun simctl push` can deliver test notifications to the simulator (unverified).

## Testing strategy

All planned; nothing has been built.
- **axum integration tests in the hub:**
  - pairing, including expired, reused and unconfirmed codes
  - signature checks for approvals: wrong device, wrong decision, a replayed nonce, an expired challenge
  - revocation closing sockets
  - events, reconnects and snapshot reconciliation
  - watch leases
  - two devices deciding the same approval at once
- **Masking tests** on screen frames, diffs and push payloads.
- **Runner tests** for the diff operation's limits and path refusal.
- **CI:** spec drift and compiling both generated clients.
- **iOS:** XCUITest smoke tests on the simulator, then TestFlight on your iPhone for pairing, approvals, push and notification deny.
- **Android:** instrumented tests on the emulator, then a real phone for Keystore keys and FCM.

## Constraints and open questions

**Settled.**
- Native SwiftUI and Kotlin.
- REST and WebSocket with code-first OpenAPI.
- Separate apps.
- QR pairing with fingerprint confirmation.
- Signed approvals.
- Generic push text.
- FCM on Android.
- Plain-text masked snapshots.
- iOS first.

**Prerequisites you need to do.**
- In the Apple developer portal: create the app's bundle id and an APNs key (a `.p8` file). The key is stored on the hub only.
- Create a free Firebase project for Android push. This blocks only Android push.
- Install Android Studio with its SDK, which isn't on this Mac yet; Xcode 27 is installed. This blocks only Android work.

**Open questions.**

| Question | Kind | Steps affected |
|---|---|---|
| Android key-level enforcement of the passcode fallback, and the exact key options on both platforms | Blocks Android approvals only; viewing and denying work without it | 2, 6 |
| Whether a notification action can refresh its token on a locked phone | Non-blocking: if not, notification deny fails and the approval's deadline denies anyway | 2, 5 |
| A public HTTPS hub for push to real devices | Blocks push to real devices only | 5 |
| Exact generator tools for the Swift and Kotlin clients | Non-blocking, settled by the spike | 2, 3 |

**Ready now.** Steps 1 and 3 and the iOS app without push.

**Not ready.** Push to real devices, until the hub has public HTTPS. Android, until Android Studio, the Firebase project and the key spike are in place.

## Verification status

- **Code inspected.** The repository at the commits after step 6, plus uncommitted WhatsApp work in progress. The hub's session service and view types (`src/hub/service.rs`) and the runner protocol (`src/proto/mod.rs`) were checked.
- **Herdr facts.** The lack of an output stream was checked against Herdr 0.9.1's exported API schema.
- **Tooling.** Xcode 27 is installed. Android Studio, the Android SDK, Gradle and Kotlin are not. JDK 17 is present.
- **From memory, unverified.** Several platform details: notification action options, push to start Live Activities, the APNs and FCM specifics, the Android key options and the client generator tools.
- **Review.** Both reviewing agents agreed on the draft, with the corrections folded in above. Neither saw this final wording.
