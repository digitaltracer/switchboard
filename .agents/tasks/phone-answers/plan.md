# Plan: answer an agent's questions from the phone, and get alerted when one is waiting

## 1. Outcome

When an agent stops to ask the owner something (a question with options, a plan to approve, or a permission prompt), the owner's phones get a generic push: on iOS through APNs, and on Android through Firebase Cloud Messaging, which is new. The agent's page shows the question and its options, and the owner picks one from the phone. This works for Claude and Codex, whether Switchboard started the agent or the owner started it directly in Herdr.

Two answer paths:
- **The hook path.** Claude agents Switchboard started are answered through Claude's own permission hook (`allow`, plus `updatedInput.answers` for questions). Those panes never receive keys.
- **The key path.** Every other agent is answered by keys sent through Herdr, and only on a dialog recognised by a version-pinned recogniser for that CLI. The runner moves the selection with arrow keys, re-reads the screen to confirm the same dialog has the chosen option selected, checks the agent process hasn't changed, and only then sends a single Enter.

Every answer is signed by the device (a deny needs no signature), requires the approve right, is idempotent by request id, and is audited.

**Residual risk, accepted by the owner on 2026-10-30.** Herdr can't make reading the screen and sending keys one atomic step. If someone answers the same dialog at the Mac, or presses an arrow there, in the milliseconds before Switchboard's final Enter, that Enter can land on the next dialog. The worst case is approving a permission the owner never saw. `AGENTS.md` documents this risk and the narrowed keystroke rule.

## 2. Current behaviour (main @ `6167591`)

**Hook path.**
- Claude's AskUserQuestion and ExitPlanMode reach the hub through the PermissionRequest hook, as approvals of kind `question` or `plan` (`src/hub/app/types.rs:170-218`).
- Answering a question with `allow` plus `updatedInput.answers` was verified (`.agents/tasks/spike-findings.md:116-124`). The helper passes `updatedInput` to Claude only (`src/hook/mod.rs:98-102`, `src/hook/wire.rs:59`).
- The runner always sends `updated_input: None` (`src/runner/hooks.rs:124`).
- The hub stores only the masked tool input (`src/hub/service.rs:2814-2815`).
- The app API refuses to allow non-permission kinds, with `answer_in_herdr` (`src/hub/app/mod.rs:955-967`). The iOS card tells the owner to answer in Herdr (`apps/ios/Switchboard/Components.swift:91-130`), and Android does the same.
- A plan allow through the hook has not been verified (`spike-findings.md:127`).

**Screen answers.**
- `SessionService::answer` and `Op::Answer` exist, but the runner refuses them (`src/runner/sessions.rs:369-372`).
- `answer` has no approve-right check and no request id (`src/hub/service.rs:1262-1290`), unlike `decide` (`service.rs:1675-1724`).
- Agents started in Herdr have no hooks, so the app says to answer in Herdr (`apps/ios/Switchboard/AgentDetailView.swift:4-5,40`).

**Keys.**
- `src/runner/codex.rs` sends the restricted-startup Enter (`:104-112`) and the permission deny Esc (`:204-232`). Both are built by the pinned Codex classifier (`src/agents/codex/screen.rs:53-143`).
- `SUPPORTED_VERSIONS` is `["0.163.0"]` (`src/agents/codex/mod.rs:11`). The installed Codex is 0.165.0.
- Herdr's `agent.send_keys` takes only `{target, keys}` (`src/herdr/mod.rs:328`, Herdr schema `AgentSendKeysParams`).
- Herdr's agent info carries `state_change_seq` and `agent_session` (`src/herdr/types.rs:367-372`).
- Visible reads report revision 0 on Herdr 0.9.5, as measured in the live-follow task, so a pane revision can't serve as a check.
- Spike finding 3: Claude draws its dialog while a hook is pending, so a key would answer it (`spike-findings.md:11`).

**Push.**
- There are three kinds, constrained by the CHECK in `migrations/0004_app_groundwork.sql:18`. The text is set in `src/push/mod.rs:62-75`.
- The relay maps kinds at `src/relay/server.rs:481-483` and accepts only hex tokens of at most 200 characters (`server.rs:491`).
- Pushes are queued per session (`src/store/mod.rs:598-607,1158-1190`).
- Android has no push at all (`apps/android/.../AppModel.kt:262`).

**Waiting state.**
- Herdr status changes flow runner → hub (`src/runner/agents.rs:122-170`, `src/runner/mod.rs`).
- WhatsApp debounces them with a 3 s hold (`src/whatsapp/limits.rs:11,56-72`).

## 3. Decisions

| # | Decision | Why | Strongest rejected alternative |
|---|---|---|---|
| 1 | Claude agents Switchboard started are answered only through the hook. Questions get `allow` plus `updatedInput.answers`, built by the runner from the original hook request it holds in memory, using the option ids the phone chose. Plans and permissions get a plain `allow` or `deny`. These panes never receive keys. Multi-question and multi-select questions are allowed here, because the hook is structured. | The hook path is verified and structured, and has no keystroke race. Building the answers on the hub would use the masked input (`service.rs:2815`). | Keys for every agent: risk taken where a hook exists. |
| 2 | Every other agent is answered by keys, only on a dialog recognised by a **shared option-row parser plus a small version-pinned layer per CLI**. The per-CLI layer supplies the version list, the title and footer anchors, the selection marker glyph, the deny row, the rows to exclude (free text such as "No, and tell Codex what to do differently"), and the fixtures. A screen that only the shared parser recognises shows its options read-only, with "answer in Herdr", and gets no keys. | This adopts the owner's generic idea as the core. A numbered list alone could be Markdown in a reply, and can't tell which row denies or which rows open a text box (`src/agents/codex/screen.rs:37-40`). | A purely generic "blocked plus numbered list" recogniser. |
| 3 | The key sequence: take the per-pane lock; read and classify; send arrows to the chosen option; re-read and require the same occurrence with that option selected; `agent.get` must show `terminal_id`, `agent_session` and `state_change_seq` unchanged; send one Enter; read to confirm the dialog closed. On any mismatch nothing more is sent, Esc is never sent, and a send that may have happened is never retried. Free-text rows, multi-select and multi-question dialogs aren't answered by keys. | Only Enter commits, and it is sent only after a verified selection and an unchanged process. | Number shortcuts: one key commits, with no check in between. |
| 4 | **One action lock per pane** in the runner, shared by the answer path, `Op::Prompt` and the `codex.rs` paths. Each occurrence can be answered once. The attempt is persisted before any key is sent, and a lost acknowledgement is recorded as outcome unknown. | It closes Switchboard's own races: two phones, WhatsApp, a prompt, the Codex fallback. | — |
| 5 | **Residual risk (accepted by the owner).** Input at the Mac within milliseconds of the final Enter can make it land on the next dialog. The worst case approves a permission the owner didn't see. | The owner's explicit choice; documented in `AGENTS.md`. | Keys for questions only, or no keys. |
| 6 | **Occurrence identity** is the pending call's own id: Claude's tool_use id from the log (`src/runner/transcript.rs:349-351`), Codex's call_id (`:438,461`). With no id, it falls back to `terminal_id`, `state_change_seq` and a runner-private fingerprint of the raw dialog region. A dialog whose fingerprint was already answered gets no keys. The phone sees an opaque occurrence id and opaque option ids. | Two identical dialogs in a row stay distinct, even if Herdr never leaves `blocked`. A fingerprint of the masked text can't see changes that only touch a secret. | A count of blocked episodes. |
| 7 | **Hub `answer` is hardened** to reuse `decide`'s machinery: the approve-right check for any non-deny choice (`service.rs:1681`), a request id with recorded replay, and the audit. | Fail closed; a replay returns the recorded outcome. | — |
| 8 | **Signature v3** (`switchboard-answer-v1`) binds the hub, device, target, occurrence id, option id, label hash, request id, nonce and expiry. Version 1 stays valid for permission approvals. Deny is unsigned. | A stale or replayed tap can't match anything else. | Signing an option index only. |
| 9 | **A `waiting` push** is queued when `blocked` holds for 3 s, once per occurrence across both kinds: a hook approval already sends its `approval` push and is not pushed again. The text is "Agent waiting" / "An agent is waiting for your answer." The payload holds an opaque attention id. The hub stores the attention's target (machine, Herdr instance, pane incarnation, optional session, occurrence), its expiry and its resolution. The app resolves it through an authenticated lookup, and an expired attention never resolves to a replacement agent. It follows mute and mute-all. | The push stays generic, with ids only. | Putting the agent name in the push: names can reveal projects. |
| 10 | **Migration 0008** rebuilds `outbox` with the new kind, copying every row: SQLite can't alter a CHECK. It also adds the attention table. | Migrations are append-only. | — |
| 11 | **Android push via FCM HTTP v1.** Device registration gains a provider (`apns` or `fcm`), and the relay validates each token by its provider (`server.rs:491`). The relay holds the FCM service-account key outside git, with mode 0600. The relay is deployed before any hub sends `waiting`; an old relay drops it, which is safe. | The owner chose FCM. | ntfy or UnifiedPush. |
| 12 | **One option card on iOS and Android,** shared by hook questions and screen dialogs. It deep-links from a push. | Criterion 3. | — |
| 13 | **Codex 0.165.0** is added to `SUPPORTED_VERSIONS` only with fixtures captured from that version. If a plan allow through the hook fails in capture, plans for agents Switchboard started go through the key path after their hook times out and denies; Step 1 settles which. | Version pinning. | Loosening the pin. |

## 4. Scope and acceptance criteria

**Delivered:** recognisers, the hook answers, the key path, the hub, API and signature, the waiting push with migration and relay, FCM, both apps, and the `AGENTS.md` rule.

**Excluded:** free-text answers; answering multi-select or multi-question dialogs by keys; Herdr's own trust screens; new WhatsApp features.

1. An agent blocked for 3 s produces exactly one push per occurrence on each paired iOS and Android device: `waiting` for screen dialogs, `approval` for hook ones. Mute and mute-all are respected.
2. The agent's page shows the question and its options for every recognised dialog. Anything else shows the screen with "answer in Herdr".
3. A pick answers through the hook for Claude agents Switchboard started, and by keys otherwise, on Claude 2.1.312 and Codex 0.165.0.
4. A stale occurrence, a changed screen, a changed process, a bad signature or a missing approve right answers nothing.
5. A replay returns the recorded outcome, and an ambiguous send is never retried.
6. No keys reach a hooked Claude pane or a pane with a pending hook call, even when both paths run at once.
7. `AGENTS.md` states the new rule and the residual risk. A test fails if any module other than `src/runner/codex.rs` and `src/runner/dialogs.rs` (proposed) calls `agent_send_keys`.
8. `api/openapi.json` is regenerated, and the change is additive. The migration keeps existing outbox rows.

## 5. Implementation plan

### Step 1: Capture fixtures

**Change.**
- In a throwaway Herdr session (`herdr --session <name> server`, with `HERDR_*` unset), capture visible screens with `--format ansi` and as text.
- Claude 2.1.312: a single question; a multi-select question; a multi-question dialog; a Bash permission; an Edit permission; a plan approval.
- Codex 0.165.0: a command permission, an edit permission, and the question UI if it is enabled.
- For each screen, also record what the arrow keys, Enter and the digit keys do, and which tool_use id or call_id the log shows.
- Verify whether a plan allow through the hook works.
- Fixtures go under `tests/fixtures/dialogs/` (proposed).

**Implements:** Decisions 2, 6 and 13.

**Invariant:** the owner's sessions are untouched, and any global CLI config is backed up and restored.

**Check:** the fixtures exist, with a README noting each capture's version.

### Step 2: Recognisers

**Change.**
- A shared option-row parser, `src/agents/dialog.rs` (proposed), handling footer-anchored rows, the selection marker and option labels.
- Per-CLI layers: `src/agents/claude/screen.rs` (proposed) and an extended `src/agents/codex/screen.rs`.
- The output is `Dialog{kind, prompt, options[{id,label,is_deny,excluded}], selected, region}` or `Unknown`.

**Implements:** Decision 2.

**Invariant:** an unknown or unpinned screen gives no actionable options.

**Check:** fixture tests, plus mutated fixtures (a one-character change, the marker moved, a wrong version, the same list in a reply) that must yield `Unknown` or read-only.

### Step 3: Harden the hub's answer path

**Change.**
- Move `answer` onto `decide`'s machinery: the approve-right check, the request id and replay, and the audit.
- Add the attention table and migration 0008, which also rebuilds `outbox`.

**Implements:** Decisions 7 and 10.

**Check:** hub tests for a missing right, a replay, a deny without a signature, and that the migration keeps existing rows.

### Step 4: Hook answers

**Change.**
- The decision protocol carries the chosen option ids.
- The runner keeps the raw hook request in memory and builds `updatedInput.answers` (`src/runner/hooks.rs`).
- The app API accepts questions and plans with signature v3, and deny stays unsigned.
- A timeout or the loss of the helper still denies (`hooks.rs:267`).

**Implements:** Decisions 1 and 8.

**Check:** runner tests where masked labels collide; a hook end-to-end test with the scripted runner.

### Step 5: The key path

**Change.** `src/runner/dialogs.rs` (proposed), with:
- the per-pane lock, shared with `Op::Prompt` and `codex.rs`;
- `Op::Answer{agent, occurrence, option, label_sha}`;
- the sequence from Decision 3;
- the attempt persisted before any key is sent;
- `agent.get` checking the process is unchanged;
- each occurrence answered once.

**Implements:** Decisions 3, 4 and 6.

**Check:** runner tests with a fake reader covering a changed screen, a selection mismatch, a changed `state_change_seq`, two answers at once, and a pending hook racing an answer. Also a deterministic test of a dialog being replaced between the read and Enter, which documents the accepted risk.

### Step 6: The app API for dialogs

**Change.**
- New endpoints: `GET /app/v1/attention/{id}`, `GET` dialog by agent, `POST` challenge, and `POST` answer.
- `Agent` and `Approval` gain the dialog and its options. All of it is masked.
- Regenerate `api/openapi.json`.

**Implements:** Decisions 8 and 9.

**Check:** hub app tests; the openapi drift test.

### Step 7: The waiting push

**Change.**
- A 3 s hold on the runner's status forwarding queues a `waiting` push per occurrence, deduplicated against `approval`.
- Add the push text.
- The relay maps `waiting` and passes the attention id.

**Implements:** Decision 9.

**Check:**
- store tests for the dedupe key, mute and mute-all;
- a relay test for the new kind;
- a test that the push text is generic.

### Step 8: FCM

**Change.**
- An FCM HTTP v1 sender on the relay, with the key path set in config outside any repo, mode 0600, so the code refuses a path inside a repo.
- Device registration gains a provider, validated per provider.
- Android registers its FCM token and shows the notification.
- The relay is deployed before any hub sends `waiting`.

**Implements:** Decision 11.

**Check:** relay tests for token validation and the FCM payload shape, using a mock HTTP server; Android core tests.

### Step 9: The apps

**Change.**
- The shared option card replaces the "Answer in Herdr" copy on iOS and Android.
- Deep links open from a push.
- A stale answer refreshes the dialog.

**Implements:** Decision 12.

**Check:** Kit and Android core tests; both apps build; e2e against `fake_runner`, extended with a question dialog.

### Step 10: The rule, the guard and a live run

**Change.**
- Rewrite `AGENTS.md`'s keystroke rule, wording below.
- A guard test on callers of `agent_send_keys`.
- A live test in a throwaway session, gated by the `SWITCHBOARD_LIVE_*` variables.

**Implements:** acceptance criterion 7.

**Check:** `cargo test`; the gated live test.

**New `AGENTS.md` keystroke rule:**

> **Keystrokes.** Only `src/runner/codex.rs` and `src/runner/dialogs.rs` send keys to an agent, and only keys built from a dialog recognised by a version-pinned recogniser in `src/agents/`.
>
> **What may be sent:**
> - Codex's restricted-startup Enter and its permission-deny Esc.
> - The owner's signed choice of one listed option on a recognised question, plan or permission dialog. This is sent as arrow keys, then one Enter. The Enter goes only after a fresh read shows the same occurrence with that option selected, and the agent's process is unchanged. Choosing the deny option needs no signature.
>
> **What is never sent:**
> - free text, or an "Other" or free-text row;
> - answers to multi-select or multi-question dialogs;
> - keys to an unknown or unpinned screen;
> - keys to a pane with a pending hook call;
> - keys to a Claude pane that Switchboard started, which the hook answers instead.
>
> Nothing in `src/whatsapp/` sends keys.
>
> **Residual risk, accepted by the owner:** input at the agent's own terminal within milliseconds of the final Enter can make it answer the next dialog.

## 6. Testing strategy

| Criterion | Tests |
|---|---|
| 1 | Steps 7 and 8 |
| 2 | Steps 2 and 6 |
| 3 | Steps 4 and 5, plus the live test |
| 4 | Steps 3 and 5 |
| 5 | Steps 3 and 5 |
| 6 | Step 5's race test |
| 7 | Step 10's guard |
| 8 | Step 6, plus Step 3's migration test |

**Commands:**
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt`
- `cd apps/ios/SwitchboardKit && swift test`
- `cd apps/android && ./gradlew :core:test`
- `apps/ios/scripts/e2e.sh`
- The iOS app build passes `-skipPackagePluginValidation`.

**Setup:**
- The live test needs a throwaway Herdr session with Claude and Codex.
- The FCM end-to-end test needs the owner's Firebase project.

## 7. Constraints and open questions

**Settled by the owner:**
- Phone answers for Claude and Codex, including agents started in Herdr.
- Android push via FCM.
- The residual race is accepted.

**Non-blocking assumptions:**
- Step 1 settles whether a plan allow through the hook works (Decision 13).
- The pending tool_use id or call_id is in the log before the dialog is drawn. If not, Decision 6's fallback applies.
- `state_change_seq` changes when the process restarts.

**Blocking the FCM end-to-end check only:** the owner creates the Firebase project and provides the app config and the service-account key. Steps 1–7 and 9 are not blocked; the FCM code and its mock tests are not blocked either.

## 8. For the implementer

- Re-check that `main` matches `6167591` in the files named in section 2.
- Work in step order. Step 1's captures may change details of Steps 2, 5 and 13: record any such change under As built, but stop if it contradicts a decision.
- End each step with its Check, then a commit with a conventional message and no attribution. Never push.
- Stop and ask if:
  - a key sequence can't be verified before Enter;
  - a recogniser can't anchor on a footer or title;
  - the plan allow fails and the fallback in Decision 13 doesn't work;
  - an API change can't be additive.
- Record as-built notes in a dated "As built" section at the end of this document.

## 9. Verification status

- Inspected at `main` `6167591`.
- Checked against the code: the masked storage, `state_change_seq`, the relay's token check, the Codex startup Enter, the missing approve-right check in `answer`, the supported Codex versions, the parameters of Herdr's `send_keys`, and Android having no push.
- Not yet captured: Claude's dialog screens, Codex 0.165.0's screens, and what each key does in them.
- The decisions were debated in two rounds. This final text folds in the round-two plan checks, and the agents have not re-read it.

## As built (2026-10-30)

**Steps**
1. Done. Captures in `tests/fixtures/dialogs/` with a README. Claude 2.1.312: single, multi-select and multi-question questions, Bash and Edit permissions, and a plan. Codex 0.165.0: folder access, composer, command and edit permissions, a queued question. Done in throwaway Herdr session `sbcap-phone`, deleted afterwards.
2. Done. `src/agents/dialog.rs` is the shared row parser plus a read-only generic list; `src/agents/claude/screen.rs`; `recognise` in `src/agents/codex/screen.rs`. 0.165.0 is pinned.
3. Done. `SessionService::answer` works by machine and agent, with the approve right (otherwise deny only), request-id replay, audit, and outcome unknown. Migration 0008 rebuilds `outbox` (and `outbox_deliveries`, see below) and `challenges`, and adds `attention`.
4. Done. `Op::Decide` carries `choices`. The runner builds `updatedInput` from the raw hook request (`agents/claude/answers.rs`). App allows for questions and plans sign version 3.
5. Done. `src/runner/dialogs.rs`: one action lock per pane (shared with `Op::Prompt`, prompts to outside agents, and both `codex.rs` paths); arrows; a re-read that must show the same occurrence and selection; unchanged `agent.get`; one Enter; a check that the dialog closed. Attempts are on disk before any key.
6. Done. `GET /agents/dialog`, `POST /agents/dialog/challenge`, `POST /agents/dialog/answer`, `GET /attention/{id}`. The spec is regenerated, additive only.
7. Done. 3 s hold in the runner's named-agent status forwarding, `Event::Waiting`, `Store::create_waiting` (dedupe per occurrence, agent and session mute, mute-all), `waiting` text, relay kind and attention id. A non-blocked status resolves open attentions.
8. Done except the end-to-end check. `src/push/fcm.rs` (FCM HTTP v1, RS256 service-account token); `[push.fcm] key_file`; migration 0009 adds the device push provider; the relay checks tokens per provider; Android registers its FCM token and shows the notification.
9. Done. One option card on iOS (`OptionViews.swift`) and Android (`OptionViews.kt`) for hook questions and screen dialogs, plus plan approval. A `waiting` tap opens only an open attention. `dialog_changed` re-reads the dialog. `fake_runner` and the UI test are extended.
10. Done. `AGENTS.md` keystroke rule, as worded above. Key-path guards: `agent_send_keys` callers, and nothing in `src/whatsapp/` reaches the answer path. `tests/dialogs_live.rs` passed against Codex 0.165.0 in throwaway session `sbdlg-live3`, deleted afterwards.

**Deviations, and why**
- Plans through the hook need `allow` plus `updatedInput` set to the original input. Captured: Claude ignores a plain `allow` on ExitPlanMode and leaves the dialog up. Decision 1's "plain allow" holds for permissions only. Decision 13's fallback was not needed.
- Codex's "No, and tell Codex what to do differently" is the deny row, not free text. Captured: Enter on it denies and interrupts the turn, with no text box.
- Standing-permission rows are shown but not offered, as on the hook path: Claude's "always allow" and "switch to … mode", and Codex's "don't ask again".
- Codex 0.165.0's question tool draws no modal in default mode. It queues "? 1 question" above the composer, so Codex questions are not recognised.
- Claude's tool_use id may reach the log only after the dialog is answered. So the occurrence hashes the pane's terminal id, `state_change_seq`, the dialog's fingerprint (marker and footer left out) and the call id when present. A call id that appears between reads changes the occurrence; that answer fails closed and the app re-reads.
- Migration 0008 rebuilds `outbox_deliveries` too. With foreign keys on, renaming `outbox` repoints its children, so copying it alone fails at commit.
- The dialog is fetched per agent rather than embedded in `Agent`, so the agent list doesn't read every blocked agent's screen on each poll.
- `Op::Answer` gains `deny_only`. The hub decides the right; the runner checks the option against a fresh read.
- The guard test was widened in Step 5, so that step's tests pass, and strengthened in Step 10.
- Retrying a signed answer with a used nonce gives `invalid_challenge`, as signed approvals already do. The recorded outcome is at `/operations/{request_id}`.
- Optional object fields are marked `nullable = false`, as the repo does elsewhere. The Swift generator drops `oneOf`-with-null fields.

**Assumptions**
- For agents Switchboard didn't start, the CLI version comes from `claude --version` or `codex --version`, cached 60 s.
- Waiting pushes cover named agents and Switchboard sessions, the agents the runner already forwards status for. Unnamed panes get none.
- An attention expires after 30 minutes, the push stale limit.
- FCM: `INVALID_ARGUMENT` keeps the token; only 404 or `UNREGISTERED` forgets it. FCM devices are stored as production.
- Android reads its Firebase settings from `~/.config/switchboard/android-firebase.properties` (no google-services plugin) and builds with push off without it.
- Keys are refused to a session latched `hook_failed`.

**Dependencies:** `ring` (already in the lockfile) becomes a direct dependency. The `http1` features of hyper, hyper-util and hyper-rustls are enabled, with no new crates. Android adds `com.google.firebase:firebase-messaging:25.0.1`, fetched from Google's Maven at build time.

**Not done**
- The FCM end-to-end check: it needs the owner's Firebase project, app config and service-account key.
- The gated live test covers Codex only. The Claude key path was verified by hand in Step 1: arrows plus Enter answered a question and a plan and denied an edit. A Claude live run would need a trusted folder, which writes `~/.claude.json`.
- `apps/ios/scripts/e2e.sh` passes every new assertion. Its push assertion still fails, as before this task; it moved from line 146 to 161.

**Notes**
- No global CLI config was edited. Running Claude in `target/phonecap` recorded per-folder state in `~/.claude.json` and accepted that folder's "No, disable external imports" prompt, as any Claude run does. The plan files, transcripts and Codex logs these runs created were deleted.
