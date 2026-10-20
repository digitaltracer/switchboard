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
