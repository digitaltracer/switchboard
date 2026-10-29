# Plan: stop Switchboard scrolling agent panes, and follow agents live from the phone

## 1. Outcome

While an agent works, its pane on the Mac jumps by itself. The cause is Switchboard's WhatsApp "finished" notice:
- About 3 s after every turn ends, the notice reads the last 80 lines of the agent's terminal history.
- For a full-screen agent, such as Claude Code with `"tui": "fullscreen"` (the owner's setting), Herdr can only collect history beyond the visible screen by scrolling the agent's own interface up and back.

The notice will quote the agent's last reply from its conversation log instead, and it will never read the terminal. After that, nothing Switchboard does on its own reads past the visible screen, so nothing scrolls a pane.

The phone also stops polling every 3 s:
- **Conversation.** It long-polls the transcript. The runner holds the request open and is woken by the operating system the moment the agent's log file grows (FSEvents/kqueue on macOS, inotify on Linux), so it does no polling of its own while it waits. A new item reaches the phone within a fraction of a second, and a quiet agent costs one request every 20 s.
- **Screen tab.** It reads the visible screen once a second, and only while that tab is shown. Herdr 0.9.5 has no working push for screen changes (measured, section 2), so this one stays a poll.
- **Limits.** App screen reads all share the hub's four-at-once limit, and waits are capped per runner so they can't crowd out approvals.
- **Background.** On Android and iOS alike, following stops when the app goes to the background.

## 2. Current behaviour

Inspected at `main` 4fb988b.

**Measured on 2026-10-29: which Herdr calls move a pane.**

The setup was a throwaway Herdr 0.9.5 session (protocol 22) with an idle fullscreen Claude Code 2.1.312, 40 rows, and a headless Herdr client attached through a PTY. The table shows the bytes that client redrew in the 2 s after each call:

| Call | Bytes redrawn |
|---|---|
| `agent.read` visible, text | 0 |
| `agent.read` visible, ansi | 0 |
| `recent_unwrapped`, 20 lines (fits the screen) | 0 |
| `recent_unwrapped`, 80 lines | ~3.5–4 KB, on every run |
| `session.snapshot` | 0 |
| `pane.process_info` | 0 |
| Idle, no call | 0 |

- `pane.scroll_changed` did not fire for the 80-line read. It follows Herdr's own scrollback, not a full-screen app's.
- The pane's `revision` and the visible reads made right after it were also unchanged, because Herdr restores the screen before it answers.
- So only the attached-client redraw count detects the scroll.

**Measured on 2026-10-29: what Herdr pushes when a screen changes.** In a second throwaway session, with a shell pane printing every 2 s:
- `events.wait` on `pane_output_changed`, which the schema lists with `min_revision` (`tests/fixtures/herdr-api-schema.json`), is refused with `unsupported_event_wait_match`.
- Visible reads always report `revision: 0`.
- `pane.updated` fired once in 9 s, for a title change, not per output.
- `pane.output_matched` fires once, on the first match, not per change.

So nothing in Herdr 0.9.5 wakes a reader when a pane's screen changes.

**The WhatsApp finished notice.**
- `src/whatsapp/relay.rs:686-697` calls `read_agent(machine, agent, overfetch(20), false)`. `overfetch` multiplies by 4 (`src/whatsapp/text.rs:48`), so that is an 80-line history read.
- The runner turns it into `agent_read(RecentUnwrapped, 80)` (`src/runner/agents.rs:45-62`, `src/runner/sessions.rs:375-396`).
- It fires once per turn end, after a 3 s hold (`src/whatsapp/limits.rs:11,56-72`).
- Text: `"{label} finished:\n\n{last 20 lines, masked}"`. An empty read gives `"{label} finished:\n\n"`.
- The test is at `src/whatsapp/relay_tests.rs:632-662`, and the expected text is at `:662`. The scripted runner answers `ReadAgent` from its `screens` and `output` maps (`relay_tests.rs:75-87`).

**Other automatic reads are visible-only.**
- WhatsApp blocked notice and blocked-session reply: `relay.rs:516,672`.
- Apps' Screen tab: iOS `ConversationViews.swift:84-96`, Android `ConversationViews.kt:205-215`.
- Codex dialog reads: `src/runner/codex.rs:46-48`.
- Home tiles take their line from the transcript: `apps/ios/Switchboard/HomeView.swift:227-241`.

**History reads a person asks for (out of scope):**
- WhatsApp `read` (`relay.rs:528-540`).
- `GET …/output` without `visible` (`src/hub/app/mod.rs:424-440,840-855`).

**The screen-read limit.**
- `MAX_SCREEN_READS = 4` (`src/hub/service.rs:50`) is taken only by session screen reads (`service.rs:1235`).
- Agent reads skip it (`read_agent_with`, `service.rs:1332-1352`). `read_agent_with` is also what WhatsApp uses (`relay.rs:516,672,689`).

**Phone following.**
- `ConversationModel.follow` fetches the transcript after `end`, plus the visible screen when `wantsTerminal` or `unavailable`, then sleeps 3 s. iOS: `ConversationViews.swift:41-49`. Android: `ConversationViews.kt:157-165`.
- It is shared by:
  - the conversation screen;
  - the agent page (`AgentDetailView.swift:66`, Android `AgentScreen.kt:34`);
  - the session page (`SessionDetailView.swift:51`, Android `SessionScreen.kt:132`).
- Lifecycle:
  - iOS restarts tasks on wake through `model.wakes` (`SwitchboardApp.swift:19-20`) and otherwise relies on the OS suspending the app.
  - Android's `wentToBackground` cancels only the session-list poll (`AppModel.kt:256-258`), so its follow loops keep running in the background.

**The transcript path.**
- Endpoints: `GET /app/v1/sessions/{id}/transcript` and `/agents/transcript`, with `after` and `before` (`src/hub/app/types.rs:508-525`, `src/hub/app/mod.rs:457-470,819-835`).
- Route: they go to `service.rs:1144-1221` (masked by `redact_as`), then to `Op::Transcript` (`src/proto/mod.rs:125-131`), handled at `src/runner/sessions.rs:434-492`.
- Each call:
  - takes a Herdr snapshot;
  - for Claude, finds the session from the pane's pids and lists `~/.claude/projects`;
  - for Codex, walks `~/.codex/sessions` four levels deep and reads the first line of up to 200 logs (`src/runner/transcript.rs:30-91`).
- Item kinds:
  - Agent text is `assistant`, or `note` for API errors.
  - `ends_turn` marks a turn's final answer (`transcript.rs:333-344,429`).
  - A page that doesn't start at `after` makes the apps replace their list (`Conversation.swift:32-35`).

**Long-poll limits today.**
- Sessions long-poll with `after_revision` and `wait_ms`, capped at `MAX_WAIT` = 25 s (`service.rs:43`).
- A runner call times out after `CALL_TIMEOUT` = 30 s (`service.rs:38`).
- The relay answers within 45 s, and the apps' relay timeout is 50 s (`Relay.swift:111-112`, `Relay.kt:223-230`).
- Runner ops run on their own tasks and are never cancelled (`src/runner/link.rs:96-102`, `local.rs:43`).
- At most 64 requests can be in flight per runner (`src/proto/conn.rs:15,91`), shared with prompts and approval decisions.

## 3. Decisions

| # | Decision | Why | Strongest rejected alternative |
|---|---|---|---|
| 1 | The finished notice quotes the agent's last `assistant` reply from its transcript, never the terminal. | The user chose it. A transcript read touches no pane (measured), and the home tiles already use that source. | A visible-screen excerpt: it doesn't scroll, but it quotes terminal chrome. The user rejected it. |
| 2 | With no log, no reply or an error, the notice keeps its current empty form, `"{label} finished:\n\n"`. No terminal read of any kind happens. | It keeps the invariant "the finished path never reads the terminal", and leaves WhatsApp's wording unchanged. | Fall back to a visible read: safe, but adds a branch for a rare case. |
| 3 | The transcript endpoints get an additive `wait_ms`. The runner waits, woken by OS file-change notifications, until the log has a complete line past `after`. The notifications come from one shared watcher per runner (the `notify` crate, proposed new dependency) that watches folders, never single files. The whole op, lookup included, finishes within 20 s. If the watcher can't be set up, the wait falls back to a 1 s size check. | New items arrive sub-second. A quiet agent costs one request every 20 s instead of one every 3 s. The log is found once per wait instead of on every poll, which matters most for Codex's log-tree walk. It follows the existing session long-poll. | Poll every 750 ms with no API change: smaller, but repeats the snapshot, pid lookup and log search about 1.3 times a second per watched view, and makes a relay round-trip even when nothing changes. Withdrawn by its proposer. |
| 4 | During a wait, the runner looks for the log again only when something signals a switch: a file created or replaced in `~/.claude/sessions/` (for `/clear`, `/resume` or a new Claude in the pane); a new `rollout-*.jsonl` in today's Codex `sessions/YYYY/MM/DD` folder (for a new Codex session); or a real change in the agent's Herdr status, deduplicated so the runner's periodic reconcile doesn't re-trigger it. A changed path ends the wait at once. Every wait also ends at 20 s, which covers any switch no trigger catches. | `/clear` or a new session switches logs. A timer that re-ran the search every 2 s would repeat Codex's log-tree walk (`transcript.rs:62-91`) every 2 s per watched agent, and the reactive version costs nothing while nothing changes. | Re-find every 2 s: simple, but its cost grows with the number of Codex logs. Find once per wait: stalls up to 20 s after `/clear`. |
| 5 | A runner holds at most 8 transcript waits at once. Past that, it answers immediately without waiting. Waits are async: they await notifications, never block a thread or spin. The shared watcher adds one thread and one inotify instance per runner, not one per wait. Each watched folder is reference-counted and released when its last wait ends. | Ops aren't cancelled when a phone leaves, so abandoned waits linger until their deadline. The cap keeps them from using up the 64 shared slots that approvals and prompts need. | Add cancellation to the runner protocol: a wider protocol change than this needs. |
| 6 | The Screen tab reads the visible screen once a second, timed start to start, with no overlap, and only while shown. | Herdr 0.9.5 has no screen-change signal (measured, section 2). A visible read costs nothing measurable on the pane. | Wait on `pane_output_changed`: listed in the schema, but refused by Herdr 0.9.5. Revisit when Herdr supports it. |
| 7 | All visible screen reads from the app API share the four-at-once limit, and a full limit answers Busy. WhatsApp's reads stay outside it. | Faster screen polling must not bypass the bound that agent reads skip today. WhatsApp is frozen, and its blocked notice would turn a Busy into an empty excerpt (`relay.rs:672-676`). | Apply the limit inside `read_agent_with` for every caller: that changes WhatsApp's behaviour. |
| 8 | Following stops whenever the app is not in the foreground, on both platforms, and restarts on return. | Criterion 3: no load when nobody is watching. Android keeps polling in the background today. | Rely on OS suspension: Android doesn't suspend coroutines. |

## 4. Scope and acceptance criteria

**Delivered:**
- The WhatsApp finished notice's text source.
- The transcript long-poll across the runner, hub, API, iOS Kit and Android core.
- The runner wait cap.
- Screen-read cadence, and the screen-read limit on app reads.
- Both apps' follow loops and their lifecycle.
- Tests, including a live no-scroll test.

- CPU: following adds no periodic work beyond one transcript request per 20 s per watched view, and the Screen tab's 1 s read while it is shown.

**Excluded:**
- Reads a person asks for.
- Herdr.
- Visual design.
- A hub event stream.
- Runner-protocol cancellation.
- Any change to WhatsApp's wording or its other reads.

**Done when:**
1. No automatic Switchboard path sends an `agent.read` beyond the visible screen, and tests enforce it in Rust and in both app cores.
2. With WhatsApp configured, a turn ending sends `"{label} finished:\n\n{last reply}"`. In the live test, the attached client sees zero redraw bytes from the notice.
3. While a conversation is shown, a line appended to the agent's log reaches the phone within about 1 s. A quiet agent causes at most one transcript request per 20 s.
4. After `/clear`, `/resume` or a new Codex session, the follower picks up the new log within about 1 s once the watched record or folder changes, and within 20 s in any case.
5. While the Screen tab is shown, the screen refreshes about once a second. Otherwise, and whenever the app is in the background, no transcript or screen requests are made.
6. A hub that ignores `wait_ms` makes the apps poll no faster than once a second.
7. A ninth concurrent wait on one runner returns at once. Approvals still go through while 8 waits are held.
8. A fifth concurrent app screen read gets Busy, and WhatsApp's visible reads are unaffected.
9. `api/openapi.json` is regenerated, the change is additive, and the drift test passes.
10. With a phone following an idle agent on the Conversation tab, following adds nothing between waits except one log search per 20 s. In particular there are no file checks, no extra Herdr calls and no log searches on a timer. The runner's own background work, its 5 s reconcile (`src/runner/agents.rs:105`), and the hub's sweeps (`src/hub/service.rs:45-46`) are unchanged. Measure the extra lookups and wakeups (counters in tests), and the runner's CPU over several minutes against an unwatched baseline.

## 5. Implementation plan

### Step 1: the finished notice reads the transcript

**Change**
- In `relay.rs:686-697`, call `svc.agent_transcript(machine, agent, None, None)`.
- Pick the last item with `kind == "assistant"` and non-empty `text`. Prefer the last one with `ends_turn` if it comes after the last `user` item.
- Mask with `redact_str` before `tail(.., FINISHED_LINES)`, and keep the exact format.
- With no log, no reply or an error, the excerpt is empty.

**Implements:** Decisions 1 and 2.

**Invariant:** this path sends only `Op::Transcript`, never `ReadAgent` or `Read`. It fails toward a notice with no excerpt.

**Verified today:** it sends `ReadAgent{visible:false}` (section 2).

**Depends on:** nothing.

**Check**
- Teach the scripted runner in `relay_tests.rs` to answer `Op::Transcript` from a per-agent map.
- Update the test at `:632-662` to assert the new text, and to assert that the op log holds no `ReadAgent` or `Read` after the finished notice.
- Add cases for:
  - no log, giving an empty excerpt;
  - a muted agent, where nothing is read;
  - a secret in the reply, which comes out masked;
  - an earlier completed turn followed by a reply without `ends_turn`, where the later reply is chosen;
  - a `note` item, which is never chosen.
- Run `cargo test whatsapp`.

### Step 2: app screen reads share the limit

**Change**
- Have the app's visible agent read take a `screen_reads` permit with `try_acquire`, as `read_screen` does at `service.rs:1235`.
- The app endpoint is `get_agent_output` with `visible` (`src/hub/app/mod.rs:840-855`).
- Either give the app endpoint its own service method, or add a flag that only the app path sets.
- `read_agent` and `read_agent_with` as WhatsApp calls them stay unchanged.

**Implements:** Decision 7.

**Invariant:** a full limit refuses the request with Busy. It never queues the read or falls back to history, and WhatsApp's reads never see Busy.

**Verified today:** agent reads skip the limit (`service.rs:1332-1352`).

**Depends on:** nothing.

**Check**
- A hub test with five concurrent visible reads, mixing session and agent, against a slow scripted runner: exactly one gets Busy.
- A test that the WhatsApp blocked notice still reads its screen while four app reads are held.

### Step 3: the runner waits on the log, woken by the OS

**Change**
- Add `wait_ms: Option<u64>` to `Op::Transcript` (`src/proto/mod.rs:125-131`), with serde default and skipped when none.
- Add the `notify` crate (proposed new dependency).
- **A runner-wide watcher** (proposed module, for example `src/runner/logwatch.rs`): one `notify` watcher on one thread.
  - It watches folders, not single files. Atomic replacement by rename would silently end a watch on a single file.
  - Each watched folder is reference-counted. A guard returned to the caller unregisters the folder on drop.
  - It publishes changes per path on a tokio `watch` or broadcast channel. Events are treated as coalesced invalidations: the waiter re-checks the file, it never counts events.
- **A status tap.** The runner's Herdr status watch feeds one consumer today (`src/runner/mod.rs:222-237,252-260`). Add a broadcast of real status changes, deduplicated per agent so the periodic reconcile (`src/runner/agents.rs:163-199`) emits nothing when nothing changed.
- **The wait**, in `sessions.rs:434-492`, when `after` and `wait_ms` are both set:
  - Take a slot from a per-runner semaphore of 8. With no slot, read the page at once.
  - Note the start time, and resolve the log path with the existing lookup.
  - Register watches **first**:
    - the log's folder;
    - for Claude, `~/.claude/sessions/`;
    - for Codex, today's `sessions/YYYY/MM/DD`.
  - **Then** check the length, so an append made in between is not lost.
  - Wait, without timers, for whichever comes first:
    - a change to the log that leaves a newline past `after`;
    - a created or replaced file in the Claude sessions folder, or a new `rollout-*.jsonl` in the Codex day folder;
    - a status change for this agent from the tap;
    - the deadline, `start + min(wait_ms, 20 s)`.
  - On a trigger other than the log, resolve the path again. A different path ends the wait.
  - If the watcher can't be set up, check the length once a second instead.
  - Then call `read_page` as today, in `spawn_blocking`.
- `before` ignores `wait_ms`. A log that is missing or shorter than `after` ends the wait and is read as today; `Conversation.append` already replaces on a mismatch.

**Implements:** Decisions 3, 4 and 5.

**Invariant**
- Pages and masking are unchanged.
- A wait that times out returns the same page `after` returns today.
- The op always answers within 20 s of starting, well inside `CALL_TIMEOUT` (30 s).
- A held wait does no periodic work unless it is in the fallback.
- Watches are released when the wait ends, including an abandoned wait at its deadline.
- Failure goes toward the 20 s backstop, never toward a stuck or leaked wait. A watch that registers but never fires, as on a network filesystem, only delays the follower.

**Verified today**
- Each call re-resolves the log (`sessions.rs:441-490`).
- `read_page` with `after` at the end returns an empty page (`transcript.rs:106-153`).
- The runner subscribes to Herdr status changes (`agents.rs:150-170`), but it only forwards them to the hub (`mod.rs:222-237`); nothing on the transcript path sees them.

**Depends on:** nothing. Steps 4 and 5 build on it.

**Check:** runner tests, using temp folders and an injected lookup:
- A line appended at 300 ms returns before 1 s.
- A line appended between lookup and registration is still seen.
- No append returns empty at the deadline.
- A partial line without a newline doesn't end the wait.
- Replacing the Claude session record by rename, so it points at a new log, ends the wait promptly.
- A new rollout file in the Codex day folder ends the wait.
- A status change with a new path ends the wait.
- A periodic reconcile with unchanged statuses triggers no lookup (count lookups).
- With 8 waits held, a ninth returns at once.
- After the waits end, the watcher's registered-folder count is back to zero.
- A wait whose lookup is slow still answers by 20 s.
- A forced watcher failure uses the 1 s fallback.

### Step 4: hub and API

**Change**
- Add `wait_ms` to `TranscriptQuery` and `AgentTranscriptQuery` (`types.rs:508-525`), documented as capped at 20 s.
- Pass it through `get_transcript` and `get_agent_transcript` (`mod.rs:457-470,819-835`) and `service.rs:1144-1221` into `Op::Transcript`.
- Run `cargo build --release && ./target/release/switchboard admin openapi > api/openapi.json`.

**Implements:** Decision 3.

**Invariant:** the change is additive within `/app/v1`, and the runner call keeps `CALL_TIMEOUT`.

**Verified today:** the queries have no wait parameter.

**Depends on:** Step 3.

**Check**
- A hub test that `wait_ms` reaches `Op::Transcript`, capped.
- A hub test that approvals still decide while 8 transcript waits are pending on a scripted runner.
- The openapi drift test passes.

### Step 5: iOS and Android following

**Change**
- **Clients.** Regenerate the clients. Add `waitMs` to Kit `Hub.transcript` and `agentTranscript` (`Hub.swift:279,286`) and to Android core (`Hub.kt:297,307`).
- **Two loops.** In `ConversationViews.swift` and `ConversationViews.kt`, `follow` runs two child loops under one scope:
  - **Transcript.** The first load is as today. After that, request with `waitMs: 20000` and repeat as soon as the answer arrives. An empty answer that came back in under 1 s, for example from an old hub, is followed by a pause until 1 s has passed since the request started. Errors back off as today.
  - **Screen.** While `wantsTerminal || unavailable != nil`, read visible, then wait out the rest of 1 s, start to start, with no overlap. Busy skips that tick. Switching to the Screen tab still reads at once (`ConversationViews.swift:166-170`).
- **Lifecycle.** Both loops run only while the app is in the foreground.
  - **Android:** `AppModel` exposes a foreground flag, set in `becameActive` and `wentToBackground` (`AppModel.kt:249-258`), and `follow` suspends or cancels while it is false.
  - **iOS:** the follow task also ends when `scenePhase` leaves `.active`, and restarts through `wakes` as today.

**Implements:** Decisions 3, 6 and 8.

**Invariant**
- The `end` cursor and the loaded list survive tab switches.
- Terminal reads always send `visible=true`.
- Nothing is requested while the app is in the background or the screen has gone.

**Verified today:** a single 3 s loop (section 2), and Android follows in the background.

**Depends on:** Step 4.

**Check**
- Kit (`swift test`) and Android core (`./gradlew :core:test`) tests for:
  - old-hub pacing;
  - the terminal read sending `visible=true`;
  - transcript requests carrying `waitMs` after the first load.
- A test, where the model allows one, that no request is issued after the foreground flag goes false.
- `apps/ios/scripts/e2e.sh` still passes.
- Manually, with the phone following a live agent: a reply appears within about 1 s, and the Screen tab ticks each second.

### Step 6: the no-scroll guard

**Change (proposed tests)**
- **Rust.** Give the history read its own name in the service (for example `read_history`), separate from the visible read. The only callers are WhatsApp `read` (`relay.rs:536`) and the app `output` endpoints without `visible` (`mod.rs:434,847`). A test drives every automatic path through a recording scripted runner and fails on any `ReadAgent{visible:false}` or `Read`:
  - the finished notice;
  - the blocked notice;
  - the follow-style transcript calls;
  - app screen reads.
- **Live test, proposed.** Gate it on `SWITCHBOARD_LIVE_HERDR_SOCKET` and `SWITCHBOARD_LIVE_AGENTS=claude`. It runs against a throwaway Herdr session (`herdr --session <name> server` with `HERDR_*` unset). It attaches a headless Herdr client through a PTY, prompts the Claude agent, waits until it has been idle past the 3 s hold, then drives a finished notice and counts the client's redraw bytes for 2 s.
  - It must also run a positive control: an 80-line history read must redraw. If it doesn't, for example because the Claude on that machine isn't fullscreen, the test fails as uncalibrated.
  - The test does not edit the global Claude config. If fullscreen has to be turned on for it, back up and restore that config per `AGENTS.md`.

**Implements:** acceptance criteria 1 and 2.

**Check:** `cargo test`, then the live test with its variables set. Delete the throwaway session afterwards.

## 6. Testing strategy

| Criterion | Covered by |
|---|---|
| 1 | Step 6 Rust guard, Step 5 core tests (`visible=true`) |
| 2 | Step 1 tests, Step 6 live test |
| 3 | Step 3 runner tests, manual check in Step 5 |
| 4 | Step 3 path-switch test |
| 5 | Step 5 pacing and foreground tests, manual check |
| 6 | Step 5 old-hub pacing tests |
| 7 | Step 3 wait-cap test, Step 4 approvals-under-load test |
| 8 | Step 2 tests |
| 9 | The openapi drift test |
| 10 | Step 3 lookup and registration counters, plus a timed CPU comparison while a phone follows an idle agent |

**Commands, all from the repository:**
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt`
- `cd apps/ios/SwitchboardKit && swift test`
- `cd apps/android && ./gradlew :core:test`
- `apps/ios/scripts/e2e.sh`

The live test needs a throwaway Herdr session, a Claude CLI in fullscreen mode, and the variables above. The live test and the tests named in the steps are proposed, not written yet.

## 7. Constraints and open questions

**Settled constraints**
- Masking on every transcript page and on the notice.
- Additive `/app/v1` changes only.
- WhatsApp's wording and its other reads are unchanged.
- No Herdr changes.
- Live tests only in throwaway sessions.

**The implementer decides**
- How the app path takes its screen-read permit (Step 2).
- How the runner's wait cap is held.
- How each platform wires the foreground flag.

**Assumptions (non-blocking)**
- Claude writes or replaces `~/.claude/sessions/<pid>.json` on `/clear` and `/resume`, so the folder watch fires (Step 3). If it doesn't, those switches fall back to the 20 s backstop. Confirm this live during implementation.
- `notify` on macOS uses FSEvents with zero latency requested, and coalesced events are fine because the wait re-checks the file. Measure latency before considering the kqueue backend (Step 3).
- Logs live on a local filesystem. On a network filesystem, watches may never fire, and following falls back to the 20 s backstop.
- Herdr may later support `pane_output_changed` waits. The Screen tab could then become reactive, as a separate change.
- A cap of 8 waits per runner is enough for one owner's phones (Step 3).
- Agent logs are written per message. A long reply appears when Claude writes it, not word by word; the Screen tab is the live view while text streams.

**Blockers:** none. All steps are ready.

## 8. For the implementer

**Before starting:**
- Confirm `main` still matches 4fb988b in the files named in section 2, especially `src/whatsapp/relay.rs:686-697`, `src/hub/service.rs:1144-1352`, `src/runner/sessions.rs:434-492` and both `ConversationViews` files.
- Read `.agents/tasks/debate-native-apps-2.md:350-372` (the transcript and the earlier scroll fix).

**Order of work:**
- Do Steps 1 and 2 first, each on its own. Step 1 alone removes the reported scrolling.
- Then do Steps 3 → 4 → 5, then Step 6.

**For each step:** end it with its Check passing, then a commit with a conventional message, such as `fix(whatsapp): …`, `feat(runner): …`, `feat(app-api): …` or `feat(apps): …`. Never push.

**Stop and ask if:**
- the finished notice turns out to need a terminal read after all;
- the wait can't finish inside `CALL_TIMEOUT`;
- the API change can't be additive;
- WhatsApp's wording would change.

Resolve routine details yourself, such as naming, test helpers and how each platform expresses the foreground flag.

**As-built notes:** record them in a dated "As built" section at the end of this document, and add a short note to `.agents/tasks/debate-native-apps-2.md` beside the 2026-10-26 scroll fix.

## 9. Verification status

- **Inspected:** this repository at `main` 4fb988b. Every file reference above was checked against the code.
- **Measured live:** the scroll measurements in section 2, in a throwaway Herdr session that was deleted afterwards.
- **Not measured:** relay latency and battery.
- **Reviewed by the agents:** the draft. Their corrections are folded in here:
  - re-finding the log during a wait;
  - the wait cap and the 20 s bound;
  - keeping WhatsApp outside the screen limit;
  - Android background following;
  - reply selection;
  - guards in the app cores;
  - an isolated, calibrated live oracle.
- **CPU revision, checked by both agents:** at the user's request, a third round reviewed making waits reactive. Its corrections are folded in:
  - a shared folder watcher instead of one per wait;
  - registering watches before checking the file;
  - triggers for Claude and Codex log switches;
  - deduplicated status changes;
  - an honest CPU criterion.
- **Screen-change push tested live:** the agents proposed it, and the test found Herdr 0.9.5 refuses it.
- **Not re-checked by the agents:** this final text.

## As built (2026-10-29)

**Steps**
- **Step 1, done** (`08715d9`). The WhatsApp finished notice now quotes the reply from the conversation log and no longer reads the terminal. `cargo test whatsapp` passes. The end-to-end WhatsApp test (`tests/whatsapp_hub.rs`) now gives its fake Claude a log, and it checks that no `agent.read` beyond the visible screen reaches Herdr.
- **Step 2, done** (`896cf8f`). An app's visible agent read now goes through its own service method, `read_agent_screen`, which takes a screen-read permit. A hub test runs five mixed reads and exactly one gets Busy. A relay test confirms the blocked notice still reads the screen while the apps hold all four permits.
- **Step 3, blocked.** It is built and left uncommitted: `src/runner/logwatch.rs`, `wait_ms` on `Op::Transcript`, the status tap, and the cap of 8 waits. Its checks, `cargo clippy` and all 336 tests pass, but the finding below defeats criterion 3 for Codex on macOS.
- **Steps 4 to 6, not started.** iOS e2e was not run.

**Blocking finding: macOS doesn't report appends to a file that is still open**
- FSEvents reports a write to a file still held open only when the file is closed. Measured: a runner wait whose log was appended through a held-open handle got no event and ran to its 20 s deadline. Closing the handle woke it within a second.
- `lsof` shows that Codex keeps every live `rollout-*.jsonl` open. Claude holds none open.
- So on macOS, a follower of a Codex agent would update only every 20 s, or on a status change, which breaks criterion 3. Linux's inotify reports every write, and Claude logs work on both.
- A kqueue `NOTE_WRITE` watch on the log file itself fired 0.31 s after an append to a held-open file.
- Fixing this needs a change to Decision 3, "folders, never single files". Options:
  - (a) Also watch the log file itself with kqueue on macOS. The folder watch stays, to catch a replacement by rename. This costs one open file per followed log.
  - (b) Accept the 20 s backstop for Codex on macOS.
  - (c) Check Codex logs each second on macOS.
- Recommended: (a).

**Deviations (Step 3)**
- A folder stays watched for 30 s after its last wait ends, instead of being released straight away. Each FSEvents setup took 130–400 ms, and changing the watched set restarts the stream. With the linger, a follower's back-to-back waits never restart it. Tests use a zero linger and check that the folder count returns to zero.
- The watch is set up on a blocking thread, never on an async one.
- The switch folder (the Claude records or today's Codex folder) is listed by inode before the watch is set up and listed again after. A record created or replaced during setup still triggers a new lookup.
- Rewriting a record in place counts as a switch only when it is the pane's own Claude record and it now names another session. Claude rewrites its records in place on every status change (inode unchanged), so counting every rewrite would trigger lookups all the time.

**Assumptions**
- `notify` 8.2.1 is used, the latest stable release; FSEvents latency is 0 with NoDefer.
- Codex's day folder uses local time, going by the timestamps in its file names.
- The wait cap is a semaphore on `Sessions`.
- The status tap fires on a real change from `StatusForwarder`, not on its 60 s refresh.
- If the first lookup in a wait passes the deadline, it answers `deadline_exceeded`.
- Timing tests on OS events take turns through one test lock.
- It was not confirmed live that `/clear` or `/resume` replaces the Claude record (no throwaway session was run). If they rewrite it in place, the pane's-own-record check covers it.
