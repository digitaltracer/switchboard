# Switchboard native apps: revised plan

This replaces `.agents/tasks/debate-native-apps.md`. Two fresh reviewers, who hadn't seen the earlier debate, found real problems in that plan. This version fixes them.

## Outcome

The task: give Switchboard a native phone interface, with SwiftUI on iOS and Kotlin with Jetpack Compose on Android, in place of WhatsApp.

The architecture stays: the hub gets an app API next to the bots' MCP, both call the same session service, and each app is a thin client generated from an OpenAPI contract. The revision makes the first version smaller and more honest.

**No WebSocket in v1.**
- The app long-polls for changes. The hub already supports this for a single session, and it can't lose updates the way an event stream can.
- The live screen polls the hub about once a second while it's open. The hub forwards each poll to a read the runner already supports.

**Push from a durable queue.** Push notifications come from a queue in the database, not the hub's lossy in-memory event channel. Sending a push is an outbound connection from the hub, so it doesn't wait on the hub having public HTTPS.

**Approvals are honest about their limits.**
- Signed approvals protect the app's path only. Bots and WhatsApp can also approve, so every front door gets an "allowed to approve" setting, checked inside the hub.
- Bots stay allowed by default, because you required that bots can answer permission prompts.
- In v1, signed approvals exist on iOS only. Android gets them in its own milestone.

**The first release is one approval loop:** pair the iPhone, see sessions, start one, then deny or approve with Face ID from a push notification. The live screen and diffs come next, answering questions after that, and Android last.

## Rationale

The criteria: one developer, approvals that fail closed and are honest about their reach, nothing unmasked leaving the machine, and recovering cleanly from lost responses and crashes.

**The old plan's errors and gaps, all checked against the code:**
- **Lossy event channel.** The hub's event channel drops events for slow listeners. It's a broadcast with capacity 1024, documented "Lagging receivers miss events" (`src/hub/service.rs`). Sending screen frames every second through it, as the old plan proposed, could make the push sender miss approvals. So push uses database rows, and the live screen doesn't use the channel at all.
- **Live view.** The runner already serves a visible-screen read (the `ReadAgent` operation with `visible`, in `src/runner/sessions.rs`). The old plan's new screen-polling protocol in the runner is unnecessary: the hub polls only while an app is looking.
- **Answering questions.** The runner rejects it today ("screen dialog answers are not supported yet", `src/runner/sessions.rs`). The app can't promise it until that's built.
- **Raw tool input.** Stored approval input is already masked (`src/hub/service.rs`). The old plan wrongly said internal types expose raw tool input.
- **CI.** The repo has no CI (no `.github/`). The old plan's CI checks rested on something that doesn't exist.
- **Push and HTTPS.** Push to APNs and FCM is sent outbound by the hub. Only the phone's own calls need to reach the hub, through the VPS, a tunnel, or a private network such as Tailscale.

**Why long-polling instead of a WebSocket.** Session get already waits on a revision number driven by a single change signal (`src/hub/service.rs`). Because it carries state rather than events, a slow client misses nothing; it just gets the latest state. The sessions list needs the same revision-based wait added. iOS suspends network activity in the background either way, and push covers that case (unverified). A WebSocket can come later if polling proves too slow or too costly.

**Why per-front-door approval rights.** The hub's decide call currently accepts any authenticated caller, and the caller identity is only a string for the audit log (`src/hub/service.rs`). Signing app approvals is pointless if a leaked xAI bearer token, or a WhatsApp `approve 3`, can approve the same thing. So decide checks an "allowed to approve" right attached to the authenticated identity of each front door: app devices, the ChatGPT client, the xAI token, WhatsApp. Everything is allowed by default except where you turn it off. You decide whether the xAI static token keeps the right; see open questions.

**Duplicate starts.** The hub already returns the existing session when the same project, machine, agent and prompt are started again within 60 seconds, even with a new request id (`src/hub/service.rs`). The app shows that as "already running" with a link to the session. Changing one word starts a new one.

The reviewers split here: one wanted a force-new flag for intentional duplicates. It was left out because the case is rare, the workaround is trivial, and adding the flag later is easy.

**Notification-action deny is deferred.** An unanswered approval is denied at its deadline anyway, about 275 seconds (`src/agents/mod.rs`). Deny from the notification itself also depends on an unverified question: whether a locked phone can refresh its token. Deny inside the app is in v1.

## Scope and acceptance criteria

**v1, the approval loop on iOS.**
- Pairing.
- A sessions list.
- Session detail: status, the output tail, prompt, stop and mute.
- A start sheet with project, machine and agent pickers.
- Deny, and signed approve.
- Push for "approval needed" and "finished".
- A devices screen with revoke.
- Questions from Claude (AskUserQuestion and plan approval, which arrive as approvals) are labelled as questions, with a note to answer them in Herdr for now. Approving one would otherwise leave the agent stuck at a dialog nobody can answer.

**v1.1.** The live screen and the diff view.

**Later.**
- Answering questions, once the runner supports it.
- Notification-action deny.
- A WebSocket, only if needed.
- The Android milestone.
- Live Activities and widgets.
- An audit-log view.

**Acceptance criteria:**
1. **Pairing** works only with a fresh, single-use code and a confirmed fingerprint. It's rate-limited and audited, and announced to devices already paired. Revoking a device rejects its next request.
2. **Approval rights.** Each front door's right to approve is enforced inside the hub. A caller without it gets a refusal for MCP, WhatsApp and the app alike. Bots keep it by default.
3. **App approvals** need a valid device signature over a canonical, versioned message: hub, device, approval, decision, nonce and expiry. Consuming the nonce and recording the decision happen in one database transaction. A stolen app token can deny but not approve.
4. **Push** for approvals and for finished sessions comes from durable rows. A hub restart, a slow client or a failed send doesn't lose a notification: it retries, and push tokens are rotated. Notification text is generic. Removing notifications is best-effort, and every action re-checks against the hub.
5. **Polling.** The sessions list and session detail update through long-polls without missing changes. The live screen polls only while open, with a cap on concurrent reads, and stops when it closes.
6. **Masking holds** for screens and diffs against test fixtures for wrapped, clipped, multi-line and partly visible secrets. When the hub can't mask a line safely, it withholds that line or frame rather than sending it.
7. **The diff** compares against the session's starting commit and covers commits plus the working copy. It includes untracked files without following symlinks and never touches the index. fsmonitor, textconv and external diff drivers are disabled. It has defined limits for binary files, submodules, size and time. The stale, dirty and merged flags reuse the runner's existing cleanup checks.
8. **Lost responses.** A start whose response is lost can be looked up by its request id. The app has clear states for "outcome unknown" and "reconciling". Pending approvals show a countdown to their deadline.
9. **Multi-device routing.** Push goes to every paired device that hasn't muted the session. While any app device is paired, WhatsApp stops sending notifications unless you configure it otherwise. An event never notifies the same device twice.
10. **Old clients.** The hub tells clients the oldest app build it supports. Old-client fixtures in the hub's tests guard against breaking changes.

## Implementation plan

New files are marked as proposed.

1. **Hub groundwork.**
   - Store each session's starting commit when it's created, with a migration. The diff needs it later, and sessions created before then won't have it.
   - Add a revision-based wait to the session list.
   - Add a visible-screen read to the hub's session read, forwarding to the runner's existing `ReadAgent`.
   - Attach the "allowed to approve" right to authenticated callers, and enforce it inside decide.
   - Add a notification outbox: rows written when an approval opens and when a session finishes, sent with retries.
2. **Device pairing and auth** (proposed `src/auth/device.rs`).
   - Pairing codes via an admin command, `switchboard admin pair-device`, with a fingerprint confirmation step, rate limits and audit.
   - Device keys, short-lived tokens refreshed by signing a challenge, revocation, and lost-device recovery (re-pair, revoke the old device).
   - Canonical signed approvals, with the atomic nonce-and-decision transaction.
   - App and bot identities stay separate.
3. **App API** (proposed `src/hub/app/`): REST routes under `/app/v1` over the session service, with public API types separate from internal ones.
   - The OpenAPI spec is generated from Rust (utoipa) and committed.
   - A cargo test fails if the generated spec drifts from the committed file.
   - The API reports the oldest client build it still supports.
   - No WebSocket.
4. **iOS app** (proposed `apps/ios/`).
   - A Swift client generated from the committed spec.
   - Device keys in the Secure Enclave, with the approval key requiring user presence (Face ID or passcode, not "current biometric set"). Exact key options are unverified.
   - Screens: pairing, sessions, detail, start, approvals with a countdown, questions shown as questions, and devices.
5. **APNs push** (proposed `src/push/`). Token-based APNs key from your Apple account, sending from the outbox. Test with the simulator first, then your iPhone through a tunnel or Tailscale.
6. **v1.1.** The live screen, which polls the hub's visible read about once a second while open, with the masking fixtures as a gate. Then the diff operation in the runner (proposed `src/runner/diff.rs`) and its view.
7. **Answers.** Runner support for answering question dialogs, then the app's question UI.
8. **Android milestone** (proposed `apps/android/`). Start with the spike on key-level enforcement of the authentication-bound key, then FCM after you create a Firebase project, then reach parity with iOS v1 and v1.1.

**Order:** 1 → 2 → 3 → 4 → 5 is v1. Step 6 is v1.1. Step 7 is next. Step 8 starts after the iOS app has been in daily use for a while.

Step 7 of the main plan (MCP and OAuth) can be built in parallel at any point, since it sits on the same session service and the same approval-rights check.

Adding CI is a separate, later task.

## Testing strategy

All planned; nothing has been built.

**Hub tests:**
- pairing: expired, reused, unconfirmed and rate-limited codes
- approval rights for each front door
- signatures: wrong device, wrong decision, a replayed nonce, an expired challenge, an old message version
- the nonce and the decision committed atomically
- two devices deciding the same approval at once
- revocation races
- outbox retries across a hub restart
- long-poll correctness under rapid changes
- masking fixtures for screens, diffs and push text
- diff limits, symlinks and the disabled git features
- lost-response lookup by request id
- old-client fixtures

**iOS:**
- XCUITest on the simulator
- simulator push
- on a real iPhone through TestFlight: pairing, Face ID approval, push while locked and after a reboot

**Android:** in its milestone, instrumented tests plus a real phone for Keystore enforcement and FCM.

`cargo test` is the existing hub command. App build and test commands get set up with each app.

## Constraints and open questions

**Settled:**
- Native SwiftUI and Kotlin, and the Rust hub.
- REST plus long-poll, with no WebSocket in v1.
- Code-first OpenAPI, with a drift test.
- Separate apps.
- QR pairing with fingerprint confirmation.
- Per-front-door approval rights, plus iOS signatures in v1.
- Durable push outbox, with generic text.
- FCM for Android.
- Plain-text masked screens.
- iOS first.
- No force-new flag.

**Things you need to do:**
- Create the app's bundle id and an APNs key in the Apple developer portal. It's stored on the hub only.
- Later, create a Firebase project and install Android Studio.

**Open questions:**

| Question | Kind | Affects |
|---|---|---|
| Should the xAI static bearer token keep the right to approve? It's the weakest credential that can approve | Your decision; non-blocking, defaults to allowed per your original requirement | Steps 1, 2 |
| Which iOS key options give "Face ID or passcode" at the key level | Non-blocking; settled while building step 4 | Step 4 |
| Whether Android enforces a passcode fallback at the key level | Blocks only Android approvals; viewing and denying work without it | Step 8 |
| Whether a locked phone can refresh its token from a notification action | Only matters for the deferred notification-action deny | Later |
| How the phone reaches the hub before the VPS exists: tunnel or Tailscale | Non-blocking for building; needed for testing on a real phone | Step 5 |

**Ready now:** steps 1 to 3 in the hub. **Ready once you create the APNs key:** step 5. Step 4 needs only Xcode, which is installed.

## Verification status

- **Code checked.** The `main` branch plus uncommitted WhatsApp-through-hub work. Every code claim above was checked against `src/hub/service.rs`, `src/runner/sessions.rs`, `src/agents/mod.rs` and the repo layout.
- **Herdr.** Checked against Herdr 0.9.1's exported API schema.
- **From memory, unverified.** Several platform details: iOS key options, background behaviour, FCM collapse semantics, and Tailscale's fit.
- **Review.** Two fresh reviewers, one Claude and one Codex, critiqued the old plan independently, then checked a revision draft. They agreed on everything here except the force-new flag, which was left out for the reasons above. Neither saw this final wording.

## As built (2026-10-19): steps 1 to 5

Steps 1 to 4 are built. Step 5 is built up to real APNs delivery, which needs your Apple account. v1.1 (the live screen and diff), answers and Android are not started.

- **Step 1.**
  - Migration 4 adds `sessions.start_commit`, which the runner reads with `git rev-parse` in the new worktree.
  - It also adds a durable `revisions` counter. Session revisions now come from this counter, which the session list long-polls on, and it moves on mute-all and runner connect or disconnect too.
  - It adds the `outbox`, written in the same transaction as the approval or the finished turn. A finished turn is working or blocked to idle or done, or an exit.
  - `read_screen` forwards to the runner's `ReadAgent { visible: true }`, at most 4 at once.
  - `hub.toml [approve]` has `whatsapp`, `xai`, `chatgpt` and `app`, all defaulting to true, so the xAI token keeps its right as decided. `Caller` carries its front door. Deny is always allowed.
- **Step 2.** `src/auth/device.rs` and migration 5.
  - Pairing: 10-minute single-use codes stored as hashes. The fingerprint is SHA-256 over the hub id and both keys, in six groups of four hex digits. Failed claims are capped at 5 per 10 minutes.
  - Tokens last 15 minutes and are refreshed by signing a 60-second challenge with the auth key.
  - Approval challenges are issued by the hub; the decision uses the nonce in the same transaction.
  - Admin commands: `pair-device`, `list-devices`, `revoke-device`. The canonical messages are pinned by tests in Rust and Swift, and by a CryptoKit signature fixture.
- **Step 3.** `src/hub/app/`, REST under `/app/v1`.
  - The spec is committed at `api/openapi.json` and regenerated with `switchboard admin openapi`. A cargo test checks drift, and another checks compatibility with `tests/fixtures/app-v1/`.
  - `min_client_build` is 1; older builds get 426.
  - Status-like fields are strings, so old clients never fail to decode a new value.
  - Approving a question or plan from the app is refused with `answer_in_herdr`.
- **Step 4.** `apps/ios/`:
  - A Swift package with the generated client, plus a hand-written Xcode project with synchronized folders. It builds with plain `xcodebuild`; no project generator is needed.
  - Secure Enclave keys: the approval key uses `.userPresence`. The simulator falls back to keychain software keys with a LocalAuthentication check.
- **Step 5.** `src/push/`:
  - A dispatcher over per-device delivery rows: backoff, 8 attempts, stale after 30 minutes, decided approvals dropped, invalid tokens forgotten.
  - Senders are `apns` (ES256 JWT, HTTP/2, rustls with ring), `simctl` and `log`, chosen in `hub.toml [push]`.
- **Not done.** WhatsApp does not go quiet while an app device is paired (acceptance 9), because WhatsApp is frozen. Per-device mutes are not built either; push follows the shared mute state.

## Relay (2026-10-20)

Decided: the VPS is only a relay. Every user runs the hub and runner on their own machine; nobody else needs a VPS, domain or Tailscale.

- `switchboard relay` (`src/relay/`), deployed at `https://adarshnb.com/relay` behind nginx (`location /relay/` to 127.0.0.1:9090), as the `switchboard-relay` systemd unit with `/etc/switchboard/relay.toml`. It holds the APNs key and runs no agents.
- A hub with `[relay]` dials `wss://…/relay/hub` and proves its relay id: `hex(sha256(kx_pub ‖ sign_pub))[..32]`, signed challenge. Phones post to `/relay/h/<relay_id>`.
- End-to-end encryption phone to hub (`src/relay/seal.rs`, the app's `Relay.swift`): one-off P-256 ECDH, HKDF-SHA256, ChaCha20-Poly1305. A ±5-minute window and a replay cache stop resent requests. Only the app API is reachable through it, never the runner endpoint. A shared fixture pins the format between Rust and CryptoKit.
- Pushes: a hub with `[relay]` and no `[push]` asks the relay, which writes the generic text from the event kind and sends with its own key, at most 120 per hub per minute.
- The pairing QR code for a relay hub carries `relay`, `k` and `s` instead of `url`. Direct mode stays for self-hosters.

### As built (2026-10-21): relay audit fixes

- **Replies never share a keystream.** Format version 2: each reply carries a random 12-byte nonce (`n`), and the reply key is consumed when it seals. A replayed or stale request gets an unsealed `Reject`. Before this, the hub sealed a predictable 409 under the same key and zero nonce as the first reply, which let a relay that replays a request recover that reply, including a device token. Hub, relay and app must run version 2 together.
- **Relay limits:** pushes per hub (120 per minute, kept across reconnects), per device token (30), and in total (3000). At most 4096 hub connections. Phone envelopes are shape-checked before they're forwarded. A second connection for the same hub closes the first. A phone that hangs up frees its slot. Ready goes out before any request. SIGTERM shuts down cleanly. Push ids are checked.
- **Hub link:** timeouts on the connect and every send. The backoff resets only after a minute connected.
- **App:** a relay or nginx 5xx without the relay's own error body is "can't reach", so a start goes to reconciling.
- **Config:** `public_url` is optional when `[relay]` is set.
- **Deployed** to the VPS on 2026-10-21: built from a synced copy in `/opt/switchboard-src`, with the previous binary kept as `/usr/local/bin/switchboard.bak-20261020`. The live round trip passed through nginx and TLS.
- **Deploy notes, not in code:** nginx `client_max_body_size 3m` (envelopes grow about 1.8 times), `proxy_read_timeout` of at least 60 s, WebSocket upgrade headers, and `limit_req` / `limit_conn` per IP on `/relay/`.

## As built (2026-10-21): one message box

- **Routing lives in the hub** (`src/hub/route.rs`, `SessionService::route`):
  - A message that names one project starts a session there.
  - `new <project> <prompt>` starts one explicitly.
  - `use <x>[: text]` picks a running session, or an agent Switchboard didn't start.
  - Anything else goes to the current target.
  - When the hub can't tell, it asks.
  - WhatsApp's free text and `new` go through the same step. Only its numbered `use <n>` stays WhatsApp-only.
- **`POST /app/v1/messages` is stateless.** The app sends `current`, and the reply says where plain text goes next. Its outcomes are `started`, `sent`, `selected`, `choose_project`, `choose_target` and `refused`. A choice is answered with `project_id` or `target` and the same `request_id`. Starts and session prompts are idempotent by that id; prompts to outside agents are not.
- **Agents Switchboard didn't start:**
  - `GET /app/v1/agents` lists them.
  - `GET /app/v1/agents/output` reads them, masked like everything else.
  - Prompting them goes through `/messages` with a `target`.
  - They have no hooks, so the app says to answer their dialogs in Herdr.
- **The app:**
  - A compose bar sits under the sessions list, with a chip for the current target, and a "Which one?" sheet when the hub asks.
  - A "Your Herdr agents" section refreshes every 5 s, and each agent has a detail screen whose output refreshes every 3 s.
  - Sending in a session's detail makes it current.
- **Schema note:** optional object fields carry `#[schema(nullable = false)]`. swift-openapi-generator silently drops `oneOf [ref, null]` properties.

## As built (2026-10-21): app audit fixes

- **Push environment:** `aps-environment` is read from the build's `embedded.mobileprovision`. A development profile means sandbox; no profile (TestFlight, App Store) means production. It no longer follows `#if DEBUG`, which gave Release builds installed from Xcode tokens APNs rejects. The push token is resent after each successful list poll until the hub has it. Forget asks the hub to drop it first.
- **Freshness:** a session's output is read every 3 s while it runs and is on screen, and a failed read says so. Coming back to the foreground restarts every poll (`AppModel.wakes`). Direct requests time out after 40 s of silence, relayed ones after 50 s.
- **Lost answers:**
  - Session prompts use one request id per draft.
  - The message box checks a lost send for up to a minute.
  - Start reconciliation doesn't call a request "never reached the hub" until the third look, since it may still be queued at the relay.
  - Cancelling the start sheet stops watching a start that's still reconciling.
- **The rest:**
  - A cold-launch notification tap opens its session.
  - Action errors aren't cleared by the next poll.
  - Pairing has Cancel and gives up after the code's 10 minutes.
  - Camera access being off is explained.
  - The unused badge permission is gone.
- **Not done (low):** one shared token refresh for parallel requests, real Secure Enclave error messages beyond "set a passcode", and approval countdowns that ignore the phone's clock skew.

## As built (2026-10-22): design E, voice first

- **Layout:** the home screen is "Agents", and the old sessions list and compose bar are gone. From the top it has:
  - Approval cards.
  - Live tiles for running sessions, each showing its last output line.
  - Finished today, then your Herdr sessions.
  - Browse: all sessions, machines and projects.

  A session reads like a message: status, what this phone last said, the approval, the latest output, changed files and details.
- **Voice:** a mic sits on the home screen and in each session's toolbar.
  - Speech is transcribed on the device only (`requiresOnDeviceRecognition`). Where that isn't available it refuses and offers the keyboard, so audio never leaves the phone.
  - A pause of 1.8 s ends the listen.
  - Voice never approves: approvals stay a tap plus Face ID.
- **The listening glow** is a Metal shader compiled from source at runtime (`EdgeGlowView`), so building the app needs no Metal toolchain.
- **Review before sending:** `POST /app/v1/messages/preview` runs the hub's routing step without acting. The sheet shows the hub's choice first, then alternatives:
  - A start offers the project's running sessions.
  - A send offers a new task in that session's project.

  An alternative is sent with `project_id` or `target`, as before.
- **Changed files:** `GET /app/v1/sessions/{id}/changes` asks the runner for `git diff --numstat <start commit>` plus untracked files, at most 200. Paths are masked. A runner that predates the op fails the call, and the section stays hidden.
- **"You said"** is kept on the phone (`AppModel.said`): the hub stores only a prompt's hash.
- **e2e:** `examples/fake_runner.rs` stands in for a Mac. The UI test now:
  - starts a session from the keyboard;
  - opens it from the "Sent" card and checks its output, changes and approval;
  - denies the approval;
  - runs a scripted voice listen (`SB_FAKE_VOICE`).

  `KEEP_RESULT=<path>` keeps the result bundle and its screenshots.

## As built (2026-10-23): Apple's newer speech model

- On iOS 26 and newer the mic uses `SpeechAnalyzer` with `SpeechTranscriber`, which handles long speech better. It still runs on the device.
- The first listen on a phone whose model isn't installed starts the download and uses the older recognizer meanwhile. Phones without the new model, or on older iOS, keep the older recognizer.
- Both are given the names to expect: project ids and aliases, session labels and branches, and agent names, as written and as spoken (`AppModel.vocabulary`, at most 100).

## As built (2026-10-24): Android core and the Keystore spike

Step 8 has started in `apps/android/`: a Gradle project with a plain-Kotlin `core` module (the counterpart of SwitchboardKit) and an `app` module. Android 12 (API 31) is the minimum; the application id is `com.digitaltracer.switchboard`, as on iOS.

- **Client:** generated at build time from `api/openapi.json` by openapi-generator (`kotlin`, `jvm-okhttp4`, kotlinx.serialization, coroutines), as iOS generates its own.
  - Its validator rejects the spec's empty `license`, so validation is off (`skipValidateSpec`). The hub's drift test still guards the spec.
  - Absent optional fields are left out rather than sent as `null` (`explicitNulls = false`), matching iOS.
- **Relay:** an OkHttp interceptor seals each request and opens the reply. Crypto is the platform's own (ECDH, HMAC-based HKDF, ChaCha20-Poly1305), with no Tink or BouncyCastle.
- **Tests:** `./gradlew :core:test` on the Mac.
  - They pass the shared relay vector (same request bytes as the Swift half, opens the Rust reply) and verify the CryptoKit signature fixture.
  - They send the request bodies in `tests/fixtures/app-v1/requests/`.
  - They also cover the Swift unit tests, ported one for one.
- **Approval prompt:** Android's own `BiometricPrompt` rather than AndroidX Biometric, since API 31 has "strong biometric or screen lock" bound to a key.
- **QR:** Google's code scanner, the closest match to iOS's system scanner. Pasting the link always works. Not built yet.

**Keystore spike** on a moto g57 power, Android 17 (API 37), patch 2026-08-01. It's a debug-only screen, `KeystoreSpikeActivity`.

The approval key is P-256 with `setUserAuthenticationParameters(0, AUTH_BIOMETRIC_STRONG | AUTH_DEVICE_CREDENTIAL)`, not invalidated by a new enrolment, and requiring an unlocked device. Results:
- It lives in the TEE; this phone has no StrongBox.
- `KeyInfo` reports the auth requirement as enforced by secure hardware, with validity 0 (every use).
- Signing without a prompt is refused by keystore2 (`KEY_USER_NOT_AUTHENTICATED`, "No operation auth token received").
- After a fingerprint, and separately after the PIN, one signature succeeds and verifies.
- A second signature on the same unlock is refused.

This answers the open question: **Android enforces the passcode fallback at the key level**, so signed approvals can work as on iOS. The key accepts only strong biometrics or the screen lock. Whether Motorola's face unlock counts as strong wasn't tested; fingerprint and PIN both work.

**Still open, before Android ships:**
- The push-token call and the relay are APNs-only. FCM needs an additive API value and a relay sender.
- `min_client_build` is one number for every app. Raising it for an iOS build would lock out lower-numbered Android builds. The additive fix is a platform header or a minimum per platform.

## As built (2026-10-25): deleting a session

Stop keeps a task's Herdr session listed and its worktree. Delete is new and goes further, decided with the owner:

- **What it does.** If the session is still running, Delete stops it first. It then removes the worktree only if it's clean and merged, never forced, and keeps the branch. Last, it deletes the task's own Herdr session (`herdr session delete`).
- **Which sessions.** Only sessions Switchboard started. Each task's Herdr session is one the runner created for it, so the owner's own sessions are never touched.
- **Where it shows.** The hub marks the session `deleted` and leaves it out of the list. It's still readable by id, and still in the audit log.
- **The runner.**
  - `Op::Delete` carries the hub's record (Herdr session name, repo path, worktree path, branch), since a stopped session is no longer the runner's.
  - **If the runner still has the session**, it runs the normal stop with `remove_worktree_if_clean`.
  - **If it doesn't**, it checks that the path is a registered linked worktree of that repo and isn't the main checkout, runs the clean-and-merged check, then `git worktree remove` without `--force`.
  - **Retries are safe.** Each step accepts having been done already.
- **The API.** `POST /app/v1/sessions/{id}/delete` returns `worktree_removed`, `kept_because`, `branch` and `already_deleted`. The change is additive.
- **The apps.** The session toolbar's stop button opens a choice: Stop or Delete while running, Delete once stopped. The result stays on the page.
- **Herdr 0.9.1, checked in a throwaway session.**
  - `herdr session delete` refuses a running session.
  - On a stopped one it removes only the session folder (`session.json` and the server log). It doesn't touch worktrees, which live in `~/.herdr/worktrees/<repo>/<branch>`, or branches.
  - `herdr worktree remove` without `--force` removes the checkout and keeps the branch.
  - `tests/herdr_sessions_live.rs` now deletes a running and a stopped session through the runner's code.
- **Upgrading.** A remote runner that predates `Op::Delete` can't decode it and never answers, so the delete ends after 90 s as `outcome_unknown`. Update remote runners along with the hub. WhatsApp is frozen and has no delete.

## As built (2026-10-26): the conversation view

The user found the terminal tail unreadable on a phone. Claude draws for a terminal about 150 columns wide, so every line wrapped, and the tail stopped at the visible screen while the agent worked.
- **Source.** `Op::Transcript` reads the agent's own log.
  - **Claude:** `~/.claude/projects/*/<session>.jsonl` (or under `CLAUDE_CONFIG_DIR`), found by the session id Herdr reports in `agent_session`.
  - **Codex:** `~/.codex/sessions/<date>/rollout-*.jsonl` (or under `CODEX_HOME`). Herdr reports no session id for Codex (checked with Codex 0.163 and Herdr 0.9.1), so the runner takes the newest of the last 200 logs whose first line names the agent's folder. That's the session's own worktree, so it's exact for Switchboard's sessions; for a Codex started by hand it's the newest in that folder. Messages, commands (`exec_command` scripts and shell calls) and their output are shown; reasoning and the context Codex adds for itself are left out.
  - With no log yet, or an older hub or runner, the apps show only the agent's screen, with no tabs to switch.
- **Paging.** `src/runner/transcript.rs` reads a 1 MB window and returns at most 300 items, with byte offsets `start` and `end`.
  - `after` continues from `end`; a page that doesn't start there skipped some, and the app replaces its list.
  - `before` goes back from `start`. A line longer than the window, such as a request copy, is skipped unread.
  - This conversation's own 33 MB log pages back to its start in 34 pages.
- **Items.** Prompts, replies, and tool calls as one-line summaries, with input and result shortened. Thinking, side-chain, meta and command-output lines are left out. A result arriving on a later page than its call comes as a bare item with the call's id, and the apps join them.
- **API.** `GET /app/v1/sessions/{id}/transcript` and `GET /app/v1/agents/transcript`, additive. Masked like every runner reply, and untrusted.
- **Apps, iOS and Android alike.**
  - A session or agent page shows its last six items, then "Open the conversation".
  - That screen has Conversation and Terminal tabs.
  - Polling continues from `end` every 3 s while on screen.
- **Reads that scrolled the Mac (fixed 2026-10-26).** Reading an idle agent's history makes Herdr scroll the agent's own interface to collect it, then scroll back. The home screen's tiles read every running session's last lines every 6 s, and the Terminal tab read 500 lines every 3 s, so terminals on the Mac kept jumping.
  - The output endpoints take `visible=true` for the screen as it is, which Herdr reads without scrolling. The apps' terminal reads only that.
  - The tiles take their line from the conversation.
  - WhatsApp's `read`, which the user asks for, still reads history.
- **Loading and empty states.** While the conversation loads, pulsing placeholder shapes; a status line only once it's loaded. The terminal says it's loading until the first read, and "The screen is blank" only after one. Errors show a line and keep retrying.
- **The terminal, zoomable.** The screen is drawn like a photo of it: fitted to the phone's width at first, pinched to zoom up to 1.5×, double-tapped between fitted and full size. Text is laid out at full size, so it stays sharp. It uses a `UIScrollView` on iOS and a Compose canvas on Android.

## As built (2026-10-26): sessions, then their agents

The apps listed every agent as its own "session", and kept the agents Switchboard started apart from the ones started in Herdr. Now there is one tree, on iOS and Android alike: **Sessions → a Herdr session → its agents → an agent.**

- **A session is a Herdr session.** Its agents are listed together whoever started them. A task started from the phone is an agent in the Herdr session made for it. Its row opens the task page, which has its approvals, branch and worktree; any other agent opens the agent page. The list always shows, even with one agent.
- **Ended agents at the bottom.** Each session page lists running agents first, then those that ended today, on a slightly different background. A session with no agent left sinks to the bottom of the sessions list the same way.
- **The hub remembers agents it saw.** Herdr forgets an agent once it exits, so the hub lists every machine's agents every 15 s, and whenever the app asks, into `seen_agents` (migration 7). An agent missing from a full listing has ended; a new agent in the same pane is a new record. Ended records are kept for a week. `GET /app/v1/agents` gains `ended`, the last day's, additive and optional for older hubs. Titles are stored masked, like every runner reply.
- **Home.** "Running" tiles became "Busy now": any agent working or waiting for you, in any session. "Finished today" and "Started in Herdr" became one "Sessions" list. Machines → a machine shows that machine's sessions.
- **Known gaps.** An agent that ended outside Switchboard has no page yet; its conversation is still in its log, so one could follow. If one Herdr session can't be listed, its agents are marked ended and come back as new records when they're seen again.

## As built (2026-10-28): the right conversation, agent names, a coloured terminal

Found on the first real Android phone, and the same on iOS.

- **The wrong conversation.** Herdr's `agent_session` can name the wrong Claude log. A background Claude that Claude's own daemon started from a pane inherited the pane's `HERDR_*` variables, so its startup hook told Herdr the pane was on the conversation it had resumed. The runner now asks Herdr for the pane's foreground processes (`pane.process_info`) and reads the running Claude's own record, `<claude dir>/sessions/<pid>.json`. A record counts only when its `pid` matches its file name. Herdr's id is the fallback. Only the `.json` records are read, never the keys beside them.
  - **Codex** still goes by its folder. Two Codex agents in one folder can show the same log. The open log file isn't a reliable link either: one Codex held two logs open, and listing open files took over a minute.
- **Names.** An agent without a Herdr name went by its terminal title, which is Claude's own title for the conversation ("Session approval"), not a name. It now goes by its kind and pane, `claude · w2:p1`. The title shows under it and on its page, as what it's working on.
- **A coloured terminal.** `/output` and `/agents/output` take `ansi=true` with `visible=true`, and return `ansi`: the screen with Herdr's colour codes, so the apps draw it as the Mac does, with your messages on Claude's shaded band. It's additive; older hubs and runners leave it out.
  - **Masking.** The hub keeps `ansi` only when it holds nothing but colour codes and, without them, matches the masked text line for line. A secret split by colour codes is masked in the text but not in the coloured copy, so the colours are dropped for that screen.
