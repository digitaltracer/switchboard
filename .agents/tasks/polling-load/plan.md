# Plan: polling load

## 1. Outcome

Three load problems share one root: working out an agent's conversation log from scratch is expensive, and both apps ask for it far more than they need to.
- **Home tiles:** they download whole conversations every 6 s to show one line each.
- **Every request:** conversation pages, dialog checks and "waiting" checks each search the filesystem for the log.
- **Android:** it keeps polling with nothing on screen, and over the relay it can't cancel requests, so it runs out of connections.

The fix is three steps built on one change:
1. **A log cache in the runner.** It remembers, per pane, which log the agent is writing, and checks that cheaply on each use. It also caches the dialog check's pending-call result, so the 1 MiB parse runs only when the log grows.
2. **An opt-in activity list.** The agents API gets an opt-in list with each agent's masked last line and its time. Home tiles poll that once every 5 s instead of fetching up to 8 conversations.
3. **Phone fixes.** Android relay calls become cancellable, the per-host connection limit rises, and both apps poll only while in the foreground. The session and agent pages stop reading a screen they never draw.
4. **Fewer database writes.** The hub stops writing on every request and every agent-list call, and the approval sweep stops taking a write lock every second when nothing is due.

The hub, relay and database schema keep their shape: one additive API field, one new runner operation, and an optional capability list in the runner handshake. Old phones, old hubs and old remote runners keep working with new ones.

## 2. Current behaviour

Inspected at `5fdd89c` on `main`.

- **Home tiles.**
  - iOS: `HomeView.swift:83-89` refreshes agents and then calls `loadLastLines` every 6 s. That fetches a full conversation page for up to 8 busy tiles and keeps the first line of the last non-user item (`HomeView.swift:269-282`).
  - Android does the same (`HomeScreen.kt:117-134`).
- **Conversation pages.** `read_page` reads up to the last 1 MiB of the log and returns up to 300 items (`src/runner/transcript.rs:20-27`, `:127`).
- **Finding the log.** `find_log` (`src/runner/sessions.rs:540-604`) runs on every conversation page, dialog check and waiting check:
  1. It takes a Herdr snapshot to find the agent's pane and kind.
  2. For Claude, it asks Herdr for the pane's foreground pids, then works out the live session from `~/.claude/sessions/<pid>.json`. If the record names a `parkedJobId`, the session belongs to the background job (`transcript.rs:33-60`).
  3. It then scans every folder under `~/.claude/projects` for `<session>.jsonl` (`transcript.rs:63-75`).
  4. For Codex, it walks `~/.codex/sessions`, sorts the logs newest-modified first and parses the first line of up to 200 to match the cwd (`transcript.rs:78-124`). A resumed Codex session can write to an older day's log, which then becomes the newest-modified match.
  5. The scan runs in `spawn_blocking` (`sessions.rs:600-602`), so it holds a blocking-pool thread, not an async worker.
- **Dialog checks.**
  - `look` reads the process, the screen and the pending call. The pending call is the last tool call without a result, found by parsing the last 1 MiB of the log (`src/runner/dialogs.rs:116-127`, `:433-439`, `transcript.rs:177-181`).
  - The occurrence id hashes in the pending call's id (`dialogs.rs:116-127`).
  - The options card polls every 3–5 s on the conversation, session and agent screens (`apps/ios/Switchboard/OptionViews.swift`, `apps/android/.../OptionViews.kt`).
- **The agent list.**
  - `GET /app/v1/agents` (`src/hub/app/mod.rs:807-826`) returns Herdr agents Switchboard didn't start, from `herdr_agents` and `outside_agents` (`src/hub/service.rs:1659-1700`). That calls `Op::Agents` on every connected runner, a unit variant (`src/proto/mod.rs:112`), answered by `all_agents` (`sessions.rs:414`).
  - Session agents are filtered out.
  - The background agent watch calls the same list every 15 s.
  - `outside_agents` asks the machines one after another (`service.rs:1664-1665`).
- **Runner protocol.**
  - A runner's handshake (`Hello`, `src/proto/mod.rs:27-33`) carries its machine, protocol and runner versions, but no capabilities.
  - A remote runner that receives an op it can't decode logs "undecodable frame from hub" and sends no reply (`src/runner/link.rs:93-105`). The hub then waits out its 30 s call timeout (`src/hub/service.rs:39`).
  - Herdr's snapshot gives each pane its `terminal_id`, which changes with every new process in the pane, plus the agent's reported session and cwd (`src/herdr/types.rs:380-399`).
- **Android transport.**
  - `RelayInterceptor` is an application interceptor (`core/Hub.kt:181`). It answers each request with a second, blocking `http.newCall(post).execute()` (`core/Relay.kt:137-180`), checking outer cancellation only after a failure (`Relay.kt:179`).
  - The relay client has a 50 s call and read timeout (`Relay.kt:227-233`).
  - All clients share OkHttp's default dispatcher, with 5 requests per host (`core/Hub.kt:178-182`).
- **Android lifecycle.**
  - `foreground` starts true (`AppModel.kt:115`), and loading the model calls `startPolling()` (`AppModel.kt:184`). That includes loads from `Application.onCreate` in a process FCM started to deliver a push (`SwitchboardApp.kt:13`).
  - The home, session and Herdr-session loops key on `wakes`, not on being in the foreground (`HomeScreen.kt:117`, `SessionScreen.kt:113-132`, `SessionGroups.kt:94-98`).
- **iOS lifecycle.**
  - `startPolling()` runs when the pairing loads (`AppModel.swift:102`), and `wentInactive` only clears `active` (`AppModel.swift:194-196`).
  - The home loop and the session page's long-poll key on `wakes` (`HomeView.swift:83`, `SessionDetailView.swift:50`, `:144-150`).
  - Android also refreshes agents every 1.5 s for up to 15 s after a send (`AppModel.kt:345-352`).
  - iOS suspends the app soon after it leaves the foreground, and no content-available pushes exist (`src/push/`), so iOS isn't woken for pushes.
- **Database writes driven by polling.**
  - Every authenticated app request runs `UPDATE devices SET last_seen_at`, including long-polls and screen reads (`src/store/devices.rs:379-402`, called from `src/hub/app/mod.rs:204`). The value is only shown on the Paired devices list (`src/hub/app/types.rs:1002`, `:1018`).
  - Every agent listing records the agents (`src/hub/service.rs:1681`), updating every live agent's name, title and `last_seen` (`src/store/agents.rs:52-104`). That happens on each agent-list call and each 15 s watch tick. `last_seen` is only a fallback end time for ended agents (`types.rs:837`).
  - The approval sweeper runs `UPDATE approvals … WHERE outcome IS NULL AND deadline <= ?1 RETURNING *` every second (`service.rs:48`, `:2139-2162`; `src/store/mod.rs:843-848`). With nothing due, it changes no rows, but it still takes SQLite's write lock on the one database thread.
- **Invisible screen reads.** Conversation models follow the screen when the terminal is wanted or the conversation is unavailable. The session and agent pages never draw it, and an unavailable conversation is retried every 3 s (`apps/ios/Switchboard/ConversationViews.swift:49-53`, `:78-86`; `apps/android/.../ConversationViews.kt:165-207`).

## 3. Decisions

| # | Decision | Why | Strongest rejected alternative |
|---|---|---|---|
| 1 | One root cause, fixed in order: the log cache first, then activity, then phones. | Conversation pages and dialog checks call `find_log` (`sessions.rs:501`, `:641`), and the waiting check reaches it through `dialog_pane` (`sessions.rs:707-708`). A last-line feed without a cache would multiply the search. | Activity first. Without the cache, every activity request costs a search per agent. |
| 2 | **Log cache.** Per runner, keyed by Herdr server and pane, and validated from the one snapshot each request already takes, with no Herdr call per agent. An entry holds the pane's `terminal_id`, the agent kind, the reported session and cwd, the pid it resolved from, the resolved Claude session or Codex cwd, and the log path and inode. A hit needs the same `terminal_id`, kind, reported session and cwd. For Claude, a re-read of the cached pid's record must give the same session, including a parked job; this catches `/clear` and `/resume`, which rewrite the record in place. For Codex, the cached log must still be the newest-modified of the known logs for that cwd, and today's date folder must be unchanged. The log must exist with the same inode. Anything else is a miss and a full search. A "no log yet" result is never cached. Each entry is revalidated by a full search at most every 60 s, and entries for panes no longer in the snapshot are dropped. A Codex header memo, keyed by path, inode and mtime, makes rescans read only new or changed files. | Every invalidating event changes something the hit check reads: a new process, a session hand-off, `/clear`, a new or resumed Codex session, a replaced log. The pid record, a few stats and the inode are cheap reads. | Filesystem watchers that invalidate on events (more moving parts, missed-event handling). Kept as a later option. |
| 3 | **Pending-call cache.** The dialog check's pending-call result is cached by log path, inode, length and mtime, still parsed from the same 1 MiB window. Its entries live and die with the Step 1 entry for the pane. | While a dialog waits the log doesn't grow, so the parse runs once per change. The occurrence semantics are unchanged (`dialogs.rs:116-127`). | A 128 KB tail, which changes which call is found when a tool input is large. An incremental parser, which costs more code. |
| 4 | **Activity.** A new runner op returns, for every agent including session agents, a masked last line (the first non-empty line of the last non-user item, masked on the full text before cutting to about 200 characters) and its time. It's read from a bounded tail of the log (about 256 KB) and cached per log by path, inode, length and mtime. Runners advertise it in an optional capability list in `Hello` (old runners send none), and the hub sends the op only to runners that advertise it, querying machines in parallel with a short timeout. A new opt-in query on `GET /app/v1/agents` (`activity=true`) adds an `activity` list keyed by machine and agent name, with an entry for every agent on a capable runner even when it has no line. The list is absent when the hub doesn't support it. | It's one request for home instead of nine. A cached answer costs a stat per agent when nothing changed, and a growing log re-reads only a bounded tail. A new op avoids changing the unit `Op::Agents`. Gating on capability matters because an old remote runner never answers an op it can't decode (`link.rs:93-105`). | A long-polled activity snapshot with its own revision and producer. It saves idle requests but needs server-side change detection; deferred. |
| 5 | **Home tiles.** Both apps request activity on the existing agents refresh at 5 s and stop fetching conversations. Only when activity is unsupported (the list is absent, or the tile's machine is a runner without the capability) does a tile fall back to a conversation fetch, at most every 30 s. An entry with no line shows no line, with no fallback. | It meets the 5 s freshness and keeps working against old hubs. | Keeping today's per-tile fetch as the fallback at 6 s, which keeps the old cost against old hubs. |
| 6 | **Android relay.** `RelayInterceptor` sends through `chain.proceed(post)` in the original call, rather than a nested blocking call. The fresh-envelope retry on a dead connection stays, as a second `proceed`. The shared dispatcher's `maxRequestsPerHost` goes to 16. The relay timeouts move to the client that carries relay traffic. | Cancelling the outer call then cancels the network call and frees its slot. Application interceptors may call `proceed` more than once. | Enqueuing the inner call and cancelling it on outer cancellation. It works, but it's more code that duplicates OkHttp. |
| 7 | **Foreground only.** Android: `foreground` starts false, the model doesn't start polling at load, and `becameActive` starts it. The home, session and Herdr-session loops, and the after-send refresh, key on `wakes` and `foreground`, and stop when not in the foreground. iOS: the home loop, the session list long-poll and the session page's long-poll also stop while not active. | A process FCM started for a push polls today with nothing on screen. iOS gets the same rule for consistency, though it's suspended in the background. | Lifecycle-aware `repeatOnLifecycle` everywhere. It's more churn for the same effect. |
| 8 | **Invisible screen reads.** Conversation models get a "draws terminal" flag. Session and agent pages pass false and never follow the screen. An unavailable conversation backs off from 3 s to about 60 s. | About 60 screen reads a minute per open page today, for nothing. | Leaving it; it's a cheap fix in the same files. |
| 9 | **Fewer database writes.** A device's `last_seen_at` is written only when the stored value is more than 60 s old; the authentication read already loads it. Recording agents updates a row only when its name or title changed or its `last_seen` is more than 60 s old; inserts and the ended marking are unchanged. The sweeper first reads the earliest undecided deadline (a read, no write lock) and runs the expiring update only when one is due. Approvals still expire within about a second of their deadline. | A phone on home makes about 12 authenticated requests a minute, each a write today, and each agent-list call rewrites every live agent. Both values are display-only, so a minute of staleness costs nothing. The sweeper keeps its 1 s granularity, so failing closed on time is unchanged. | Sleeping the sweeper until the next deadline. It saves more, but must be woken when a new approval arrives, and a missed wake would delay a deny. |

## 4. Scope and acceptance criteria

**Delivered:**
- the runner log cache and pending-call cache;
- the activity op and the opt-in activity list on `/app/v1/agents`;
- home tiles on activity, in both apps;
- the Android relay, connection-limit and lifecycle fixes, and the iOS active gating;
- no invisible screen reads;
- fewer database writes from polling and the approval sweep.

**Excluded:**
- a long-poll for activity;
- watcher-based invalidation;
- push, masking-pass or session-list query changes; database pruning and indexes;
- WhatsApp.

**Acceptance criteria:**
1. With home open and 8 busy agents, the hub serves one agents request with activity every 5 s per phone, and no conversation pages. Log searches stay near zero after warm-up, counted with a debug counter. Bytes read from logs per minute fall by at least 10× against today's 8 page reads every 6 s, measured with a parse-byte counter.
2. A new reply's first line shows on its tile within about 6 s.
3. The cache never returns a log for a different process or session:
   - after the pane's process changes;
   - after a Claude session hands off to a background job;
   - after Claude `/clear` or `/resume` in the same process;
   - after Codex starts a new session, or resumes an older one, in the same pane;
   - after the log is replaced (new inode), or rewritten to the same length.

   A stale entry fails toward a full search.
4. Dialog checks parse the log only when its length, inode or mtime changed. Occurrence ids for an unchanged dialog are the same as before the change.
5. Every last line is masked with `src/redact.rs` on the full text before cutting. The hub masks it again at the edge.
6. An old phone with a new hub, and a new phone with an old hub or old remote runner, both work. The new phone falls back to per-tile fetches at most every 30 s. A new hub never waits on an old remote runner for activity.
7. On Android, cancelling a screen's request over the relay frees its dispatcher slot within a second, including during the fresh-envelope retry. Ten quick screen changes never stall new requests.
8. An Android process started by FCM does no polling until an activity is shown; it may still send the push token. Leaving the foreground stops all polling on both platforms within a few seconds.
9. Session and agent pages make no screen reads, and an unavailable conversation is retried at most about once a minute.
10. `api/openapi.json` is regenerated and the drift test passes.
11. With one phone on home for a minute and no changes, the hub makes at most a few database writes: device and agent rows at most once a minute. An idle hub's sweep takes no write lock. An approval still expires and is denied within about a second of its deadline.

## 5. Implementation plan

### Step 1: runner log cache
- **Change.**
  - In `src/runner/sessions.rs`, add a cache owned by `Sessions`, keyed by Herdr server and pane id, as Decision 2 describes.
  - `find_log` checks the cache right after the snapshot, before any pid read, and fills it on a miss. Pids are read only on a miss or a revalidation.
  - In `src/runner/transcript.rs`, add a Codex header memo (path, inode and mtime to cwd) used by `find_codex_log`, plus the per-cwd list of known matching logs used by the hit check.
  - Drop entries for panes missing from the latest snapshot.
  - Add a search counter (debug log or test hook).
- **Implements.** Decisions 1 and 2.
- **Invariant.** A hit only returns a log whose terminal, reported session and cwd, resolved session (Claude) or newest matching candidate (Codex), and inode all match now. Anything uncertain is a miss, so it fails toward a fresh search, never toward another agent's log. "No log yet" is never cached.
- **Verified today.** `sessions.rs:540-604`; `transcript.rs:33-75`, `:78-124`.
- **Depends on.** None.
- **Check.** Proposed unit tests in `src/runner/` with temporary Claude and Codex folders:
  - a terminal change invalidates;
  - a parked-job hand-off invalidates (reuse the `5abf48f` fixture pattern);
  - a `/clear` that rewrites the same pid record with a new session invalidates;
  - an inode change invalidates;
  - a new Codex log in today's folder invalidates;
  - new writes to an older-day Codex log for the same cwd switch to it;
  - a "no log yet" result is not cached, and the log is found once it appears;
  - the 60 s revalidation runs;
  - repeated hits make no directory scan (counter).

  Run with `cargo test --lib runner::`.

### Step 2: pending-call cache
- **Change.** In `src/runner/dialogs.rs` and `transcript.rs`, cache `pending_call` results by path, inode, length and mtime, stored with the pane's Step 1 entry so they're dropped with it, and reuse the agent fetch in the dialog path (`sessions.rs:636-641`).
- **Implements.** Decision 3.
- **Invariant.** The same log content gives the same pending call as today, so occurrence ids don't change. The cache is per runner and bounded to live panes.
- **Verified today.** `dialogs.rs:116-127`, `:433-439`; `transcript.rs:177-181`.
- **Depends on.** Step 1.
- **Check.** Proposed tests: the occurrence for an unchanged fixture equals today's; an appended line re-parses; a same-length rewrite with a different pending call re-parses; existing `dialogs_tests.rs` pass.

### Step 3: activity op and API
- **Change.**
  - In `src/proto/mod.rs`, add a new runner op, `Op::Activity` (proposed name), returning per agent: machine-local name, session id if any, last line and time.
  - Add an optional capability list to `Hello` (`#[serde(default)]`). New runners advertise activity; the hub records each runner's capabilities when it connects.
  - The runner answers the op with the Step 1 cache plus a last-line cache keyed by path, inode, length and mtime, reading a bounded tail of about 256 KB. The line is masked on the full text, then cut to about 200 characters.
  - The hub adds `activity=true` handling to `list_agents` (`src/hub/app/mod.rs:807-826`). It sends the op only to connected runners that advertise it, in parallel with a short timeout. It masks again and returns an additive `activity` list with an entry for every agent on those runners. A machine whose runner lacks the capability gets no entries, and the response says which machines support activity, so the phone can fall back for those alone.
  - Add a parse-byte counter alongside the search counter.
  - Regenerate `api/openapi.json`.
- **Implements.** Decision 4.
- **Invariant.** No unmasked text leaves the runner or the hub. An old runner gives no activity, not an error and not a wait.
- **Verified today.** `app/mod.rs:807-833`; `service.rs:1659-1700`; `proto/mod.rs:112`, `:255`.
- **Depends on.** Step 1.
- **Check.** Proposed tests:
  - the hub returns activity for a fake runner that advertises it;
  - a runner without the capability is never sent the op (a fake runner that, like the real old one, never replies to an unknown op), and the request returns promptly;
  - masking happens before the cut (a secret straddling the cut is still masked).

  Then `cargo test` and the openapi drift test.

### Step 4: home tiles on activity (iOS and Android)
- **Change.**
  - iOS: `HomeView.swift:83-89` and `:269-282`, plus SwitchboardKit `Hub.swift`.
  - Android: `HomeScreen.kt:117-134`, plus core `Hub.kt`.
  - Request activity on the 5 s agents refresh. Tiles read their line from it. Only tiles whose machine doesn't support activity (or a hub with no activity list) fall back to a conversation fetch, at most every 30 s. A supported entry without a line shows none.
- **Implements.** Decision 5.
- **Invariant.** Against an old hub, the app still shows lines, just less often.
- **Verified today.** The lines cited.
- **Depends on.** Step 3.
- **Check.** Proposed kit and core unit tests for mapping activity to tiles and for the fallback timing. `swift test`, `./gradlew :core:test`, and an app build.

### Step 5: Android relay and connection limit
- **Change.** `core/Relay.kt:137-180`: replace the nested `execute()` with `chain.proceed(post)`, keeping the fresh-envelope retry. `core/Hub.kt:178-182`: one dispatcher with `maxRequestsPerHost = 16`, and the relay timeouts on the client that carries relay traffic.
- **Implements.** Decision 6.
- **Invariant.** A sealed envelope is never resent; a retry seals afresh. Cancelling the outer call cancels the network call.
- **Verified today.** `Relay.kt:137-180`, `:227-233`; `Hub.kt:178-182`.
- **Depends on.** None.
- **Check.** Proposed core tests with MockWebServer: start a relay call the server holds, cancel it, and assert the dispatcher's running-call count drains within a second and the server sees the connection close. Do the same for a cancel during the fresh-envelope retry. Run `./gradlew :core:test`.

### Step 6: foreground-only polling (Android and iOS)
- **Change.**
  - Android: `AppModel.kt:115` and `:184` (start false, no polling at load); `becameActive` and `wentToBackground` start and stop polling. `HomeScreen.kt:117`, `SessionScreen.kt:113-132`, `SessionGroups.kt:94-98` and the after-send refresh (`AppModel.kt:345-352`) key on `foreground` too, and stop when it's false.
  - iOS: `wentInactive` cancels the session list long-poll, and `becameActive` restarts it. The home loop and the session page's long-poll (`HomeView.swift:83`, `SessionDetailView.swift:50`) key on `model.followKey`.
- **Implements.** Decision 7.
- **Invariant.** On returning to the foreground, every poll restarts (`wakes`). Nothing polls while not in the foreground.
- **Verified today.** The lines cited.
- **Depends on.** None.
- **Check.** Android, manual: background the app, and send a push with the app killed. With `adb logcat` or the hub's request log, no polling requests should follow until it opens. Count requests for 30 s after backgrounding on both platforms: there should be none after the first few seconds. iOS: the UI tests still pass (`apps/ios/scripts/e2e.sh`, which fails today only at its push check).

### Step 7: no invisible screen reads
- **Change.** The conversation models on both platforms (`ConversationViews.swift:49-53`, `:78-86`; `ConversationViews.kt:165-207`) get a "draws terminal" flag. Session and agent pages pass false. An unavailable conversation backs off to about 60 s.
- **Implements.** Decision 8.
- **Invariant.** The conversation screen's Screen tab still follows the screen.
- **Verified today.** The lines cited.
- **Depends on.** None.
- **Check.** Proposed unit test of the follow decision in the kit and core. A manual check that the session page makes no screen reads, from the hub's request log.

### Step 8: fewer database writes
- **Change.**
  - `src/store/devices.rs:379-402`: in `device_for_token`, run the `last_seen_at` update only when the loaded value is missing or more than 60 s older than `now`.
  - `src/store/agents.rs:52-104`: in `record_agents`, update an existing row only when its name or title differs, or its `last_seen` is more than 60 s old. Keep inserts and the ended marking as they are.
  - `src/hub/service.rs:2139-2150` with `src/store/mod.rs:843-848`: the sweep first reads the earliest undecided deadline (proposed store method). It calls `expire_approvals` only when that deadline is at or before now.
- **Implements.** Decision 9.
- **Invariant.**
  - Approvals expire and are denied on the same 1 s schedule as today, so failing closed is untouched. Any error in the new read falls back to running the expiring update.
  - An agent missing from a listing is still marked ended at once.
- **Verified today.** The lines cited.
- **Depends on.** None.
- **Check.** Proposed store tests:
  - a second authentication within 60 s doesn't change `last_seen_at`, and one after 60 s does;
  - a repeated agent listing within 60 s with the same names and titles changes no rows (use `changes()` or compare the rows), while a renamed agent or a missing one still updates;
  - the sweep with nothing due issues no update, and an approval past its deadline is still denied by the next sweep.

  The existing approval expiry tests in `src/hub/service_tests.rs` must pass. Run with `cargo test --lib store::` and `cargo test --lib hub::service`.

## 6. Testing strategy

| Criterion | Checks |
|---|---|
| 1, 2 | Step 3 hub tests, Step 4 unit tests, the search and parse-byte counters during a manual 60 s home session against today's baseline |
| 3 | Step 1 tests |
| 4 | Step 2 tests, existing `dialogs_tests.rs` |
| 5 | Step 3 masking test |
| 6 | Step 3 no-reply old-runner test; Step 4 fallback test; a new app against today's hub binary |
| 7 | Step 5 MockWebServer test |
| 8 | Step 6 manual checks |
| 9 | Step 7 test and manual check |
| 10 | `cargo test` (openapi drift test) |
| 11 | Step 8 store tests; existing approval expiry tests; a manual minute on home while watching the database's WAL size |

Verified commands: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cd apps/ios/SwitchboardKit && swift test`, `cd apps/android && ./gradlew :core:test :app:assembleDebug :app:lintDebug`.

## 7. Constraints and open questions

- **Settled.** Additive `/app/v1` only; mask everything that leaves the machine; append-only migrations (none needed); the dialog safety checks are untouched.
- **Implementation discretion.** Names; cache size bounds; the exact last-line length; test structure.
- **Non-blocking assumptions.**
  - A directory's modification time changes when a file is created in it (APFS and ext4). Affects Step 1.
  - Claude's `/clear` and `/resume` rewrite `sessions/<pid>.json` with the new session. The runner's log follower already relies on this (`src/runner/logwatch.rs:392-405`), but the spike findings don't record it. Affects Step 1.
  - Herdr gives a new `terminal_id` to every new process in a pane (`src/herdr/types.rs:397-399`), so a reused pane id can't pass the hit check. Affects Step 1.
  - Application interceptors may call `proceed` more than once. Affects Step 5.
- **Blockers.** None.

## 8. For the implementer

- Confirm `main` still matches `5fdd89c` in the cited files.
- Order: Steps 1, 2, 3, then 4. Steps 5, 6, 7 and 8 are independent and can go in any order.
- Each step ends with its Check and a commit.
- Record as-built notes in a dated "As built" section at the end.
- Stop if a change would weaken masking, change dialog occurrence semantics, need a non-additive API or protocol change, or make the hub wait on a runner that doesn't advertise activity.

## 9. Verification status

- `main` at `5fdd89c` inspected; every reference above was checked against it.
- Both debaters checked the draft. This final version applies their corrections, each verified against the code: capability gating for old runners, snapshot-based validation, the Codex resume case, mtime keys, never caching "no log", lifecycle coverage, and stronger checks. It wasn't sent back for a further check.
- Step 8 and Decision 9 (database writes) were added afterwards at the owner's request. They come from the audit's database-write finding, and every reference was checked against the code; the debaters haven't reviewed them.

## As built (2026-11-07)

**Steps**
- **Step 1, log cache.** Built as a new module, `src/runner/logcache.rs`, owned by `Sessions`. `find_log` checks it right after the snapshot and drops panes the snapshot no longer has. Claude rechecks the cached pid's record (parked jobs included); Codex checks today's folder and its mtime, and that no other known log for the folder is newer. Both then check the log's inode and revalidate after 60 s. Codex rescans read only new or changed first lines. 12 unit tests.
- **Step 2, pending-call cache.** Kept with the pane's entry and keyed by the log's inode, length and mtime. It parses the same 1 MiB window, so occurrence ids are unchanged. A write during the parse leaves nothing cached. 4 unit tests.
- **Step 3, activity.**
  - Runner and protocol: new `Op::Activity` with `AgentActivity` replies. `Hello.capabilities` is `#[serde(default)]` and both runners advertise `activity`. Each line comes from the last 256 KB, is masked on the whole text, then cut to 200 characters.
  - Hub: `GET /app/v1/agents?activity=true` adds `activity` and `activity_machines`. The hub asks only runners that advertised the capability, in parallel, with a 3 s timeout. `api/openapi.json` is regenerated.
  - Tests: an advertising runner, an old runner that is never asked and returns at once, mask-before-cut, and a hello without capabilities.
- **Step 4, home tiles.** Kit `TileLines` and `FallbackPacer` in both apps, with unit tests. Home polls every 5 s with activity. A tile whose machine isn't covered, or every tile against an older hub, reads its conversation at most every 30 s, at most 8 reads per round.
- **Step 5, Android relay.** `RelayInterceptor` sends through `chain.proceed(post)` and keeps the one retry, sealed afresh. All hub clients share one dispatcher allowing 16 calls per host. A raw-socket relay test covers cancelling a held call and cancelling during the retry: the slot is free and the connection is closed within 1 s. Both tests fail on the old code.
- **Step 6, foreground only.** Android: `foreground` starts false, loading a pairing polls only when in the foreground, and home, the session page, the Herdr session page and the after-send refresh stop when it's false. iOS: `wentInactive` cancels the session-list long-poll; home and the session page key on `followKey`. `apps/ios/scripts/e2e.sh` ran twice. After Step 6 it passed in full, including its push check. After Step 7 every check passed except "the push reached the simulator", the failure already known before this work.
- **Step 7, no invisible screen reads.** Both conversation models take `drawsTerminal`, and the session and agent pages pass false. An unavailable conversation waits 3, 6, 12, 24, 48, then 60 s. Kit and core tests cover the decision and the backoff.
- **Step 8, fewer writes.**
  - `last_seen_at` is written only when it's more than 60 s old, and an agent row only on a name or title change or a `last_seen` more than 60 s old.
  - The sweep reads `MIN(deadline)` first, and a failed read sweeps anyway.
  - Tests: device, agent-row and next-deadline store tests, plus a sweep test that holds the write lock on another connection. That test fails on the old sweep.

**Assumptions**
- Cache keys are the Herdr socket path plus the pane id.
- Some entries are never cached and are searched each time: a Claude session known only from Herdr's report (no pid record), and a pane without a `terminal_id`.
- Codex keeps up to 32 known logs per folder, out of the 200 newest candidates.
- "Reuse the agent fetch" means the dialog path checks the cache against the `agent.get` it already makes, with no second snapshot. Pruning therefore happens only on snapshot paths: conversation pages and activity.
- `at` is the log's own RFC 3339 timestamp string. The last-line cache keeps only logs seen in the latest activity op.
- A capable machine whose activity call fails or times out is left out of `activity_machines`, so phones fall back for it. A covered agent with no entry shows no line.
- The hub's second masking is the existing `redact` in `Runners::call`, which every runner reply goes through. It's covered by a test.
- The parse-byte counter counts last-line tails and pending-call parses (up to 1 MiB each), not conversation pages. It's logged at debug level after each activity op, together with the search counter.
- To meet criterion 8 on both platforms, Step 6 also gates iOS's Herdr session page and after-send refresh. The bounded reconcile loops after a send or start, and the approval countdown, are unchanged.
- I ran the app checks only after steps that touched app code, plus once more at the end. Rust checks ran after every step.

**Deviations.** None from the decisions.

**Left for the owner**
- Step 6, manual:
  - Background the Android app, and kill it and send a push. There should be no polling requests until it opens.
  - Count requests for 30 s after backgrounding, on both platforms.
- Step 7, manual: confirm from the hub's request log that the session page makes no screen reads.
- Criteria 1, 2 and 11: a 60 s home session read against the debug counters, and the WAL size over one idle minute on home.
- Criterion 6: run a new app against today's hub binary.

**Not built (rare)**
- Another pid in the same pane writing a newer Claude record is caught only by the 60 s revalidation.
- A resumed Codex log older than the 200 newest candidates is still not found, as before.
