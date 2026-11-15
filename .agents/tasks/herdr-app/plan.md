# Herdr TUI and Herdr app (herdr-core) in one runner

Draft, 2026-11-11. For review before implementation.

Evidence: herdr.dev (app, core and TUI docs); `hc --skill`, `herdr-app --skill`,
`~/.config/herdr-app/AGENTS.md`, `hc schema` (core 0.1.17 to 0.1.19, protocol generation 1); probes in
throwaway cores (status reports, a permission dialog, the event stream); herdr-mobile-relay v0.25.1
(`internal/backend`, `internal/hc`, `internal/coordinator`, `docs/herdr-core.md`); and a map of this
repo's runner.

## 1. Goal

One Switchboard runner per machine sees, reads, messages and answers agents in **both** Herdr TUI
sessions and the Herdr app's terminals, and can start new sessions in either. The phone shows them in
one list, labelled "space › agent". Nothing the owner runs moves, and no rule in `AGENTS.md` bends.

## 2. The two backends

|  | Herdr TUI (today) | herdr-core, behind the Herdr app |
| --- | --- | --- |
| Server | one per session, `~/.config/herdr/sessions/<s>/herdr.sock` | one per machine: `default`, or `HC_SESSION` |
| Unit | pane `w1:p2` in a workspace and tab | terminal `t7`, stable for the core's life |
| Agent kind | Herdr detects it | the program reports it (OSC 7501 `status.app`) |
| Status | Herdr's screen detection | the program's own word: idle, working, done, blocked with kind and message, error |
| Spaces and tabs | server API | the app's layout document `herdr-app` plus terminal labels `herdr.space` and `herdr.tab` |
| Agent names | `agent.rename` | terminal `name`, set at start; tab names in the layout |
| Session id | Herdr's integration hook | none; Switchboard's own lookups |
| Worktrees | `worktree.create` and `remove` | none; git |
| Screen | text and ANSI | text only |
| Events | lifecycle subscriptions plus snapshots | `events.subscribe`: a snapshot, then per-field deltas with `seq`, a heartbeat every 15 s, `missed` |
| Starting the server | Switchboard starts one per task | never; the app or the owner starts it |

Verified details the design relies on:

1. **Socket**: `HC_SOCKET_PATH`, then `HC_SOCKET`, then `$XDG_RUNTIME_DIR/herdr-core/<s>.sock`, then
   `/run/user/<uid>/herdr-core/<s>.sock` if that directory exists, else
   `<XDG_STATE_HOME or ~/.local/state>/herdr-core/<s>/<s>.sock`, where `<s>` is `HC_SESSION` or
   `default`. Every connection starts with `hello {generation: 1}`. The reply lists the methods and
   features; refuse a generation other than 1. **`hc server status` starts a core when none is
   running, so Switchboard only ever dials the socket.**
2. **Errors** carry `data.kind`: `not_found`, `blocked`, `conflict`, `needs_target`, `finished`,
   `status_mismatch`. Failing before the request is written means not sent. Failing after it is
   written, with no reply, means outcome unknown. A `kind` is a definite refusal.
3. **Terminal record**: `address`, `pid` (its first program), `created_ms`, `state` (live or
   finished), `argv`, `cwd`, `name`, `title`, `labels`, `reported`, `status {state, app, kind, msg}`,
   `status_ms`, `turn` (goes up each time status leaves `working`), and `foreground {pid, name, argv,
   cwd}`. An address containing `/` belongs to another machine.
4. **Identity**: addresses are never reused on a core, but a kept terminal whose program is lost gets
   a shell in the same address. An agent's identity is `core id : address : pid : created_ms`, and
   its live process is `foreground.pid`.
5. **Kind detection**: Claude reports `claude-code` and Codex reports `codex`. Claude's foreground
   process name is its version (`2.1.322`), so the fallback for a program that never reported is
   `argv[0]`'s base name (`claude`, `codex`), not the process name.
6. **Status quirks**: a new Codex reported `working` while idle at its prompt until its first turn.
   Claude cleared its status (`status: null`, `reported` still true) during a turn and when killed.
7. **Event data** (probed): `created` carries the whole record; then `foreground`, `title`, `status
   {status, reported, records}`, `labels`, `finished {exit_code, reason, signal}`, `removed`, `lost`,
   `restarted`, `size` and `doc` carry only what changed. The subscribe reply has the starting `seq`
   and the live terminals; heartbeats repeat the last `seq`.
8. **Input**: `terminal.send` pastes and presses Enter after about 3 s without typing at the
   desktop, and refuses a blocked program unless `force`. `terminal.keys` takes `enter`, `esc`, `up`,
   `down`, `tab`, `shift+a`; unknown names are refused before any byte is sent.
9. **Layout document**: `{spaces: [{id, name, folder, tabs: [{id, name, root: {content: {terminal},
   children}}]}]}` plus fields the app owns. Writes are compare-and-set on `version` (`conflict`),
   capped at 1 MiB. The format is undocumented.
10. **Launch without the window**: `terminal.run` with labels `herdr.space=sp<32 hex>`,
    `herdr.tab=tb<32 hex>`, `herdr.seen=0`, plus `keep: true` and `no_parent: true`. The app adopts
    the ids as a new space and tab; once the tab is in the document, a layout write names it.
11. **The core's environment is whoever started it**: launchd's short `PATH` when the app started it;
    stale `HERDR_*` and `CLAUDE_CODE*` when started from inside a TUI pane or a Claude session.
    `terminal.run` can add variables but not remove them.
12. **Codex** must run with `--no-daemon`, or its commands and hooks run in a shared background
    server outside the terminal.

## 3. What we have today

- `src/herdr/` is the TUI client. The runner holds the concrete `Client` everywhere, through `Herdrs`
  (`src/runner/herdrs.rs:85`), and uses Herdr's own `AgentInfo` (`src/herdr/types.rs:373`) as its
  agent model.
- The only existing seams are `CodexIo` and `DialogIo`, used by the key paths, and `link::Handler`, a
  whole-runner seam the scripted fake runner uses.
- **What's running** comes from:
  - Herdr session discovery: a ping every 2 s to each socket under `~/.config/herdr/sessions`
    (`herdrs.rs:94-130`);
  - the task watcher: one event mirror per task server, plus a 2 s snapshot tick
    (`src/runner/watch.rs:25-105`, `src/herdr/mirror.rs`);
  - outside agents: lifecycle events, per-pane status subscriptions for named panes, and a 5 s
    reconcile (`src/runner/agents.rs:112-209`), debounced to the hub (`src/runner/mod.rs:201-311`).
- **Exits**: a task's pane missing or exited, or its agent gone, means the session exited
  (`watch.rs:141-193`). A server refusing connections for 30 s exits all its tasks (`watch.rs:22`).
- **The hub sees**: `AgentSummary {name, pane_id, kind, agent_status, session_id, herdr_session,
  title, space}` (`src/proto/mod.rs:267`). Agents are named `<session>/<agent>`, or by pane id when
  unnamed (`herdrs.rs:267-279`). `OwnedSession.workspace_id` is required (`src/proto/mod.rs:44`), and
  the store keys seen agents by `(herdr_session, pane_id, kind)` (`src/hub/store/agents.rs:63`).
- **Already backend-neutral**:
  - hooks and decide: the helper uses a secret file and no `HERDR_*` (`src/hook/`);
  - git for changes, and stopped-worktree removal (`src/runner/sessions.rs:1417`);
  - the Claude session lookup from `sessions/<pid>.json` (`src/runner/transcript.rs:34`);
  - Codex's newest rollout for a folder;
  - the dialog recognisers in `src/agents/`.
- **Herdr-specific**:
  - task servers (`herdrs.create/stop/delete`);
  - `worktree_create`, `agent_start` and its readiness fields (`interactive_ready`,
    `launch_pending`);
  - `workspace_close`;
  - the ANSI screen;
  - `agent_get` as the process identity for dialogs (`src/runner/dialogs.rs:413-426`, `terminal_id`,
    `agent_session`, `state_change_seq`);
  - `pane_foreground_pids`;
  - `check_agent_target`'s `w<n>:p<x>` grammar (`src/herdr/mod.rs:484`).

## 4. What herdr-mobile-relay does, and what we take from it

The relay runs **one backend per process**. A second relay serves the core, and the phone shows it
as another computer, "<host> (core)". Both implement one `Backend` interface. The core side refuses
what it can't do (workspace and tab create or close, worktrees, agent rename, colour) and advertises
static capabilities.

**Taken:**
- the socket lookup order;
- a per-call `hello` with a generation check;
- the not-sent versus unknown split around the write;
- `address:pid:created_ms` identity;
- `error` mapped to `done`;
- label-based launch with random space and tab ids and `herdr.seen=0`;
- tab naming by a compare-and-set layout write after adoption, keeping fields it doesn't know;
- waiting for a reported `idle` before the first prompt;
- `--no-daemon` for Codex;
- the absolute `argv[0]`;
- ignoring other machines' terminals;
- resubscribing after `missed` or 45 s of silence;
- real-core tests in a throwaway core.

**Done differently, on purpose:**
- **Status from deltas.** Their event path keeps the old status and waits for a 15 s poll on core
  (`state.go:431-453`). We apply `status` deltas at once.
- **No `force` on prompts.** Their prompts type into blocked agents (`operations.go:48-51`); our
  rules refuse.
- **Dialogs cross-checked.** They ignore core's `blocked` kind; we compare it with what the
  recogniser found.
- **No hooks for conversations.** Theirs need hooks installed into Claude and Codex, plus Codex
  trust. Our pid-record and rollout lookups need nothing.
- **One runner for both backends**, with one list and one pairing, rather than a second relay
  instance.
- **One-snapshot refreshes.** They re-read the whole topology on every event; we keep a mirror and
  re-read only on `missed`, a `seq` gap, or a `doc` event (layout only).

## 5. Design

### 5.1 A core client: `src/hc/`

- `mod.rs`:
  - socket discovery per fact 1;
  - `Client::call` (dial, hello, one request, a 15 s default timeout);
  - `HcError` with `kind`, and `outcome_unknown()` and `server_gone()` matching the Herdr client;
  - typed methods `list`, `get`, `read`, `send`, `keys`, `run`, `kill`, `rm`, `label`, `doc_get`,
    `doc_set`.
- `types.rs`: the terminal record, status and foreground, the read result, and the layout document
  as raw JSON with typed views of `spaces[].{id,name,folder,tabs[].{id,name,root}}`.
- `events.rs`: the subscription stream yielding `Snapshot(seq, terminals)`, `Delta(seq, address,
  change)`, `Heartbeat(seq)` and `Missed`, with a 45 s read timeout.
- `layout.rs`: `rename_tab` and `rename_space` by compare-and-set. They refuse duplicate ids, other
  machines' ids or a missing document, keep every unknown field, enforce 1 MiB, and return
  `conflict` for retry.

### 5.2 A runner-owned agent model: `src/runner/backend.rs`

- `Server` is an enum: `Herdr { session: Option<String>, client: herdr::Client }` or `Core { client:
  hc::Client, core_id: String }`. Its methods return runner types, so callers stop reading Herdr's
  `AgentInfo`:
  - `agents() -> Vec<Facts>`;
  - `read(target, Visible | Recent(lines))`;
  - `read_ansi(target)`, plain text on core;
  - `prompt(target, text)`;
  - `keys(target, keys)`;
  - `process(target) -> Process`;
  - `pids(target)`.
- `Facts`: `server` (the session name or `hc`), `target` (pane id or address), `name`, `kind`,
  `status`, `cwd` (live foreground cwd on core), `title`, `space`, `tab`, `identity` (Herdr
  `terminal_id`, or fact 4), `reported_session` (Herdr only), `seq` (`state_change_seq`, or
  `turn:status_ms` on core), and readiness (Herdr's `interactive_ready && !launch_pending`, or the
  core rule in 5.6).
- `Process`, for dialogs and codex: `identity`, `pid` (core `foreground.pid`; Herdr keeps
  `terminal_id`), `seq`, `blocked`, `blocked_kind` (core only).
- `DialogIo` and `CodexIo` get a `Server`-backed implementation. Pane locks are keyed
  `<server>/<target>` as today.
- Work order: `agents.rs`, `sessions.rs` (activity, find_log, skills, owned, read and prompt),
  `logcache.rs` (`Pane` built from `Facts`), then `dialogs.rs` and `codex.rs`. Herdr behaviour stays
  byte-identical; the existing tests are the check.

### 5.3 What's running on core: `src/runner/core_watch.rs`

1. **Presence.**
   - Every 5 s, the runner dials the socket and does `hello`. It never runs `hc server …`.
   - Up means the core's id from `hello` is recorded. Down with tasks on core: after 30 s, the same
     rule as a Herdr server, those tasks exit.
   - A changed core id means a different core: the mirror and caches are dropped, and the owned
     sessions are reconciled.
2. **Mirror.**
   - Subscribe, install the reply's terminals at its `seq`, then apply deltas in order.
   - Any `seq` gap, `missed`, a 45 s silence or a drop resubscribes and installs a fresh snapshot.
   - `doc` events re-read the layout document, so space and tab names stay current.
3. **Listing.**
   - A terminal counts when it is `live`, has a `pid`, and its address has no `/`.
   - It is an agent when `reported` with `status.app` in {`claude-code`, `codex`}, or when
     `argv[0]`'s base name is `claude` or `codex`.
   - Other reporting programs are listed with their `app` as the kind, read-only, as with unknown
     kinds today.
4. **Status.**
   - `idle`, `working`, `done` and `blocked` map one-to-one, `error` becomes `done`, and none or
     `null` becomes `unknown`.
   - Codex at `turn` 0 reporting `working` is checked against its screen with the existing ready
     classifier in `codex.rs`, and shown idle when ready.
5. **Gone or replaced.**
   - `finished`, `removed` and `lost` remove the agent.
   - A changed `pid` or `created_ms` makes it a new agent: occurrences reset and caches are dropped.
   - A foreground that becomes a shell means the agent left. This covers keep-recovery, and the
     session-exited rule is the same as `watch.rs` today.
6. **Hub updates.** Status deltas feed the debounced `NamedAgentStatus` path in `runner/mod.rs`, so
   core agents get push, blocked holds and attention like named TUI agents.
7. **No double listing.** A TUI pane whose foreground is `hc attach`, or the core binary attached
   to a terminal, is skipped. The agent is listed once, from the core.

### 5.4 Names, ids and the hub

- **Naming.**
  - A core agent is `hc/<display>`. The display is the terminal's `name`, else the tab's name, else
    the address.
  - It is resolved by address, which travels as `pane_id`, so a rename never breaks addressing.
  - `split_qualified` treats `hc` as the core before any TUI session. `free_name` never creates a
    TUI session named `hc`. An existing TUI session named `hc` is skipped with one warning in the
    log.
- **`check_agent_target`** also accepts core addresses (`t` followed by letters and digits, no `/`).
- **Space › agent**: the space name from the layout document, else the space folder's base name.
  Agents with no space label, such as sub-agents started with `hc run`, use their parent's space,
  else "core".
- **Store.** It keys by `(herdr_session, pane_id, kind)` = `("hc", address, kind)`. Attention's
  `terminal_id` gets the full identity, so a new core reusing `t1` is a new agent.
- **App API**: an additive optional `backend` (`herdr` or `hc`) on the agent view and session, so
  the app can mark app-hosted agents. Then regenerate `api/openapi.json`. There is no new op;
  `Delete` and `Stop` route by `herdr_session == "hc"`.

### 5.5 Reading, messaging, logs and dialogs on core

- **Read**: `terminal.read` with `lines` 0 for visible and N for recent, and `complete` passed on
  as truncated. `drawsTerminal` gets plain text.
- **Prompt**: `terminal.send` without `force`, so `blocked` refuses as today. A pre-write failure is
  not sent; a post-write failure is outcome unknown, with the hub's reconciliation unchanged.
  Multi-line text is pasted.
- **Logs**:
  - Claude: `live_claude_record` with `[foreground.pid, pid]`.
  - Codex: the newest rollout for `foreground.cwd`.
  - `logcache` keys `(core socket, address)`, with `terminal_id` set to the identity and no
    reported session. No hooks are installed.
  - Inside core terminals, Herdr's own Claude hook may report to a stale TUI pane (fact 11). The
    pid record already wins over Herdr's reported session; keep it that way and test it.
- **Dialogs**:
  - Same recognisers and keys (`up` or `down`, then one `enter`).
  - The Enter goes only after a fresh read shows the same occurrence with that option selected, and
    the same identity and `foreground.pid`.
  - **New fail-closed check:** core's `blocked_kind` must agree with the dialog kind recognised
    (permission or question). A disagreement is shown read-only.
  - Occurrence = hash of identity, `turn`, `status_ms`, the dialog fingerprint and the call id.
- **Codex keys**: startup Enter and deny Esc through `terminal.keys`, under the same gates.

### 5.6 New sessions on core

Config: a runner `new_sessions = "herdr" | "hc"` (`src/config.rs`, validated, documented). A start
on `hc` with no core answering fails with a clear error; it never falls back to the other backend
silently.

`start` on core, in `sessions.rs`:
1. **Same as today**: the secret file, hook command, Claude settings or Codex args, and
   `hooks.register`.
2. **Worktree with git**: `git worktree add -b <branch> ~/.herdr/worktrees/<repo>/<branch>`. That is
   the app's own location, so its sidebar groups it under the repo. Record `start_commit`.
3. **Launch**: `terminal.run` with:
   - `argv`: `/usr/bin/env -u <every inherited HERDR_* and CLAUDE_CODE*> <absolute agent> <args>`;
     Codex also gets `--no-daemon`;
   - `cwd`: the worktree, and `name`: `agent_name` (`sb-…`, valid on core);
   - `labels`: `herdr.space`, `herdr.tab` (new random ids), `herdr.seen=0`, and
     `switchboard.session=<id>`;
   - `keep: true` and `no_parent: true`.

   The absolute agent path is resolved by the runner (`find_bin`, as for `herdr`).
4. **Readiness**: Claude waits for its SessionStart hook to mark the session live, as today, then
   for a reported `idle`. Codex runs `clear_startup`, then waits for its ready screen. Both use the
   existing 60 s cap.
5. **First prompt** with `terminal.send`, as today. If the agent never became ready, the start fails
   and the session is aborted, as today, rather than the relay's "started with a warning".
6. **Naming**: in the background, wait up to a minute for the app to adopt the tab, then name the
   tab after the session's name and the space after the project, retrying on `conflict`. If the app
   isn't running, the tab stays unnamed; that is harmless.
7. **Persisted fields**: `herdr_session = "hc"`, `workspace_id` = the space id, `pane_id` = the
   address, plus new optional `core_id` and `terminal_identity` (serde default, so old `meta.json`
   still loads).

Then:
- **Abort and stop**: `terminal.kill` then `terminal.rm`; the app closes the pane. The worktree
  follows the existing clean-and-merged-only rule, with git removal (`remove_stopped_worktree`).
  There is no server to stop.
- **Delete**: the same as for a stopped session.
- **After a runner restart**:
  - Owned core sessions are matched by the `switchboard.session` label and identity.
  - A missing terminal means exited.
  - A different identity under the same label means a replaced agent, also exited.
  - `undo_interrupted_starts` kills an unlaunched terminal found by label.

### 5.7 Protocol and compatibility

- **Runner ↔ hub**: no new op. The `hello` capability `hc` tells the hub the runner can host core
  agents. An old hub ignores it. A new hub sends core targets only to runners that advertise it.
- **App API**: additive `backend` only. Old apps ignore it.
- **No migration**: the store already holds `herdr_session` and `pane_id` strings.

## 6. Safety rules carried over

- Switchboard never starts, stops or restarts any core, and never calls the `hc server` commands.
  The owner's core is never used for experiments; tests use a throwaway `HC_SESSION` and delete its
  state folder.
- Keys go only through the core-backed `DialogIo` and `CodexIo`, under the rules above. No
  `force`, and no raw attach writes.
- Only local terminals are used; an address with `/` is never listed or targeted. Sessions are never
  routed to `hc machine` remotes.
- Layout writes touch only the `name` of spaces and tabs that Switchboard created, and only by
  compare-and-set.
- Everything leaving the machine is masked as today, including core titles and layout names.
- Secrets: none added. The core socket is the owner's, with mode set by the core.

`AGENTS.md` gets the backend in "Where things are", the core safety lines above, and the new live-test
variables.

## 7. Tests

- **Fake core** (`src/hc/tests.rs`, `tests/` rigs): a scripted JSON-RPC Unix socket with hello,
  list, get, read, send, keys, run, kill, label, doc and a scriptable event stream. It covers:
  - seq gaps, missed and silence;
  - identity change, a shell takeover and Codex at turn 0;
  - blocked kind disagreement and send refusals;
  - layout conflicts and unknown fields kept;
  - discovery order;
  - "never auto-start": no process is spawned.
- **Contract test**: a vendored `tests/fixtures/hc-api-schema.json` from `hc schema`. A test checks
  that every method, param and result field we use exists, like `tests/herdr_contract.rs`.
- **Runner tests**:
  - mixed listing (TUI and core together, no double listing, names and spaces);
  - start, abort, stop and delete on core;
  - restart reconciliation by label;
  - core down for 30 s;
  - the core id changing.
- **Dialog fixtures**: Claude and Codex question, plan and permission screens captured from core
  terminals at the app window's width, added under `tests/fixtures/`.
- **Live**: `SWITCHBOARD_LIVE_HC=1` with `SWITCHBOARD_LIVE_AGENTS` runs Claude and Codex in a
  throwaway core:
  - listing and status, a prompt and a reply;
  - a permission dialog answered and denied;
  - start and stop from the hub;
  - a kill detected.

  Teardown stops that core and removes its state folder.
- The existing Herdr suites must pass unchanged at every step.

## 8. Order of work

Each step builds, passes `cargo test`, clippy and fmt, and is its own commit.

1. **Core client and contract test.** `src/hc/` with a fake core. Nothing uses it yet.
2. **Runner seam, Herdr only.** `Server`, `Facts`, `Process`, and the call sites moved off
   `AgentInfo`. No behaviour change.
3. **Core listing and watch.** Discovery, mirror, status and names; `check_agent_target`; the
   `hello` capability; the app API `backend` and regenerated OpenAPI.
4. **Core read, prompt and logs.** Read, `terminal.send`, logcache and transcript.
5. **Core dialogs and Codex keys.** Plus the new dialog fixtures.
6. **New sessions on core.** Config, git worktree, launch, readiness, naming, stop and delete,
   restart reconciliation.
7. **Live tests, docs, and an As built note.**
8. **App builds** (iOS, Android), only if the `backend` marker is shown.

Steps 1 to 5 give full visibility and control of the owner's app agents. Step 6 adds starting there.

## 9. Open questions for the owner

1. **Default for new phone sessions:** the TUI or the app. Recommended: the app when its core
   answers, otherwise the TUI.
2. **Show core agents in the same machine list** (recommended), or as a separate "(core)" machine
   like the relay.
3. **Colour**: plain text on core for now. Later, the hub could render the raw attach stream with a
   terminal emulator crate.

## 10. Not covered

- Space and tab names on a machine where the app has never run (no layout document): folder names
  are shown.
- The Herdr TUI tab-title plugin; the app's equivalent would be a JavaScript mod.
- Agents that report no OSC 7501 status and aren't Claude or Codex: listed read-only by `app` or
  argv, with no status.
- Agents on other machines reached through `hc machine`: each machine keeps its own runner.

## Decisions before implementation (2026-11-11)

The owner settled section 9 and the scope before implementation:

- **Build as written.** Section 8 is the implementation plan and section 7 is the testing strategy.
  Each step in section 8 builds, passes `cargo test`, `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check`, and is committed on its own.
- **Default for new phone sessions: the app when its core answers, otherwise the TUI.** The runner
  setting `new_sessions` takes `"auto"`, `"hc"` or `"herdr"`, and the default is `"auto"`. Under
  `"auto"`, each start dials the core and does `hello`. Up means the session starts on core; down
  means it starts on Herdr, and the runner logs the choice. `"hc"` with no core answering fails with
  a clear error and never falls back. `"herdr"` is exactly today's behaviour. The session's recorded
  backend (`herdr_session = "hc"` or a TUI session) is what later stop, delete and reconciliation
  follow, never the setting.
- **One machine list.** Core agents appear in the same machine's list, labelled "space › agent",
  with the additive `backend` field. There is no separate "(core)" machine.
- **Colour**: plain text on core for now (question 3). Nothing is built for it.
- **Step 8 (app builds) is skipped.** The app API gains the optional `backend` field and the apps
  ignore it. iOS and Android are not changed in this run.
- The agent store cited in section 3 as `src/hub/store/agents.rs:63` is `src/store/agents.rs`.

## As built (2026-11-11)

Steps:
1. Done (`11ea599`): `src/hc/` client, fake core, contract test on a vendored `hc schema` from herdr-core 0.1.20.
2. Done (`5c98b89`): `Server`, `Facts` and the call sites off `AgentInfo`; existing tests unchanged.
3. Done (`7927b93`): core mirror, listing as `hc/<name>` with space names, status forwarding, no double listing, `hc` capability with a hub gate, additive `backend` in the app API and OpenAPI.
4. Done (`2a53695`): core read, plain-text screen, `terminal.send` prompts, Claude and Codex logs.
5. Done (`fde004a`): core dialogs and Codex keys, blocked-kind cross-check, fixtures from core terminals.
6. Done (`e7886be`): `new_sessions` (`auto` default, `hc`, `herdr`), core starts, naming, stop, abort, delete and exit tracking.
7. Done (`e99d4d0`, `e6ffff0`, this note): live tests, `AGENTS.md` and README.
8. Skipped by decision.

Assumptions:
- herdr-core is 0.1.20 here; the plan probed 0.1.17 to 0.1.19.
- Events: `missed` resubscribes and its payload is ignored. `restarted` resubscribes too. A `status` delta without `status_ms` keeps the old one in the mirror; dialogs always read `terminal.get` fresh.
- A core dialog occurrence uses `terminal_id` = identity plus `:<foreground pid>` and `state_change_seq` = `turn << 42 | status_ms`.
- Claude reports its plan approval as kind `permission` (seen live), so a plan only agrees with `permission`.
- Listing sees the program through `env` and through Switchboard's own launcher. A shell in front ends an agent (fixed list of shell names).
- Each core session gets its own new space, named after the project, and a tab named with the hub's default label `<project>-<id4>`. The app gets 60 s to adopt them, then they stay unnamed.
- Worktrees go to `~/.herdr/worktrees/<repo folder>/<branch with / as ->`, from the repo's HEAD.
- `terminal.send` waits up to 65 s, past the 60 s draft hold.
- Fixtures were captured at 200 by 50, since the app window's width isn't known.
- The TUI double-listing check makes one `pane.process_info` call per Herdr agent, only while a core is up.
- The hub answers `not_found` for an `hc/` op to a runner without the capability.

Deviations:
- The launch is `/bin/sh -c <prelude> sb-launch <agent> <args>`, not `/usr/bin/env -u <list>`. The runner can't see which variables the core inherited, so the prelude unsets every `HERDR_*`, `CLAUDE_CODE*` and `CLAUDECODE` inside the terminal, then execs the agent.
- The core's blocked kind reaches the dialog check through a new `DialogIo::blocked_kind` (default none), not a new `Process` field, so the dialog tests compile unchanged.
- One existing test's `RunnerSession` literal gained the two new fields.
- The hc client's key method is `send_keys`, so a guard test finds every call. Only `codex.rs` and `dialogs.rs` call it, and nothing sends `force`.

Left and found:
- Step 8 (app builds).
- Codex 0.167.0, installed here, changed its own screens. It shows an "Update available" modal first. Its restricted startup now has `Back to Agent Command Center` and `esc back`. Its composer is animated, with footer `← for agents · ? for shortcuts`. `src/agents/codex/screen.rs` doesn't recognise these, so every Codex start fails closed with nothing sent, on Herdr and core alike. The live Codex test fails for this reason. Screens are under `tests/fixtures/dialogs/core/codex-0.167.0/`; the classifier update is for the owner. The turn-0 Codex check uses the same classifier.
- An interrupted core start found while the core is down is forgotten with its terminal left.
- The Herdr TUI live suites were not rerun.
- Live runs: the Claude core start and the outside-Claude prompt, answer, deny and kill passed. Codex failed as above. `~/.claude.json` was backed up and only the scratch trust keys were removed. The plan file and scratch transcripts were deleted. `~/.codex/config.toml` is unchanged. Every throwaway core was stopped and its state deleted.

## Review (2026-11-12)

Reviewed from `4187482` to `2bc2531`, fixed through `c9ceb37`. The report is in `review.md` beside this plan.

Plan amendments:
- Fact 7: herdr-core 0.1.20 sends `foreground` events, though `hc schema`'s `events.subscribe` description omits them. The mirror relies on them (verified by a probe).
- 5.4: the app leaves terminal names null, so the tab name is the usual display, and tab names repeat across spaces. A shared name is listed as `<name>-<address>`, an ambiguous name is refused, and sessions are addressed by their stored address.
- 5.5: a prompt reads the terminal fresh before `terminal.send` and is refused unless the same agent is in front.
- 5.6: the existing start cap is 20 s (`startup_timeout`), not 60 s. The launch is a `/bin/sh` prelude. A stop whose terminal won't end keeps the worktree.
- Section 3: the agent store is `src/store/agents.rs`.
- Section 7: the live Claude tests that edit `~/.claude.json` run one at a time. The Codex fixtures for question and plan dialogs, and the live Codex run, wait on Codex 0.167 support.

Known issues accepted (likelihood × severity):
- The launch prelude's unquoted env loop (1×3).
- An interrupted core start left while the core is down (1×3).
- `Core::down` clearing the core id but keeping the mirror's terminals (2×2).
- The owner lookup's name fallback without an identity check (1×3).
- An occasional "the dialog changed" refusal in the live outside-Claude test, which fails safe.

Closing check:
- Steps 1 to 6: delivered.
- Step 7: delivered.
- Step 8: skipped by decision.
- Section 6 safety rules: delivered.
- Section 7 tests: delivered, except the Codex items above.
- Next: Codex 0.167 recogniser support, as its own task.
