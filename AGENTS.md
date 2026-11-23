# AGENTS.md

Instructions for coding agents working on Switchboard. Read `README.md` for the product overview.

## What this project is

1. **A product other people install.** Each person runs Switchboard on their own Mac, Linux box or VPS. It becomes the control board for the Herdr sessions and repositories on that machine. There are many installs, one owner each. It is not a shared multi-tenant service.
2. **One engine, several front doors.** The native apps (iOS built, Android planned), WhatsApp on the owner's own number (frozen), and AI bots over MCP (planned) all go through the same session service, with the same rules, idempotency and audit.
3. **Everything stays on the owner's machine.** The one shared service is the relay on the publisher's server. It forwards end-to-end encrypted phone requests to a hub by relay id and sends push wake-ups with the publisher's push key. It must never see plaintext, store user data, or run agents.
4. **Safety is a product feature.** Approvals fail closed, secrets are masked on the way out, and nothing types into an agent's terminal except Codex's narrow startup and deny keys and the owner's signed choice on a recognised dialog.
5. **Failure is expected.** Lost responses, crashes, restarts and dropped connections are designed for: request ids, outcome unknown with reconciliation, durable push delivery, and state rebuilt from Herdr plus Switchboard's own records.

## Rules you must not break

Each of these has tests. Keep them passing, and add tests when you touch the area.

- **Fail closed.** An approval that is unanswered, expired, or interrupted by a runner, hub or hook-helper loss ends in deny. Replaying a decision returns the recorded outcome and does nothing else.
- **Mask everything that leaves the machine** with `src/redact.rs`. That covers API responses, approvals, terminal output, push text and WhatsApp messages. Never store unmasked tool input.
- **Keystrokes.** Only `src/runner/codex.rs` and `src/runner/dialogs.rs` send keys to an agent, and only keys built from a dialog recognised by its layout (title, footer, option rows and selection marker) by a recogniser in `src/agents/`. Recognition doesn't depend on the CLI's version: Claude Code and Codex update themselves, and a screen that doesn't match is shown read-only. The captured screens under `tests/fixtures/` are regression tests; capture new ones when a CLI's layout changes.
  - **What may be sent:**
    - Codex's restricted-startup Enter and its permission-deny Esc.
    - The owner's signed choice of one listed option on a recognised question, plan or permission dialog. This is sent as arrow keys, then one Enter. The Enter goes only after a fresh read shows the same occurrence with that option selected, and the agent's process is unchanged. Choosing the deny option needs no signature.
    - A group of questions asked together is answered one question at a time. Each question is its own occurrence, with its own checks and single Enter. A pick that only moves to the group's next question submits nothing, so it needs no signature. The choice that submits the group is signed: "Submit answers" on Claude's review, whose "Cancel" is the deny, or any pick on Codex's last question.
    - One left arrow, to show a group's previous question again. It submits nothing, so it is unsigned. It goes only on a recognised group whose open question is known and isn't the first, while a listed option is selected; a fresh read must then show another question of the same group.
  - **What is never sent:**
    - free text, or an "Other" or free-text row;
    - answers to multi-select questions, alone or inside a group;
    - a right arrow or Tab, or a left arrow from a group's first question (Codex wraps to its last) or from a free-text row (it edits the text);
    - keys to a screen whose layout isn't recognised;
    - keys to a pane with a pending hook call;
    - keys to a Claude pane that Switchboard started, which the hook answers instead.
  - Nothing in `src/whatsapp/` sends keys.
  - On herdr-core the same keys go by `terminal.keys`, only from those two files. Nothing ever sends `terminal.send` with `force`, and only a listed agent is targeted, never a plain shell.
  - **Residual risk, accepted by the owner:** input at the agent's own terminal within milliseconds of the final Enter can make it answer the next dialog.
- **Approve rights and signatures.** Approvals go through the session service's decide, which checks the caller's approve right. App approvals need a valid device signature. Deny never needs one, and nor does a pick that only moves to the next question of a group; the choice that submits the group does.
- **The relay** forwards ciphertext and routing ids only. Only the app API is reachable through it, never the runner endpoint. Push text is generic, with ids only, and never names a project, tool, command or output.
- **Worktrees.** Never force-remove one. Only clean and merged worktrees may be removed.
- **Projects.** Only allowlisted projects. Never reroute a session to another machine. On herdr-core, only this machine's terminals: an address with `/` is never listed or targeted.
- **Secrets.** Hub, relay and runner secrets (runner tokens, APNs keys, the FCM service-account key, relay keys, the WhatsApp store) live outside any git working tree, with mode 0600. The code refuses paths inside a repo. Don't weaken that.
- **The app API contract.** Additive changes only within `/app/v1`. After changing app API types, regenerate `api/openapi.json` with `switchboard admin openapi`. A test fails on drift.
- **Migrations** in `migrations/` are numbered and append-only. Never edit one that has shipped.

If a change would bend one of these, stop and ask the user. Don't work around it.

## Working safely on this machine

- **Never read or print env files, credentials, keys or tokens.** That includes `~/.config/switchboard/*.token`, `.p8` keys and the WhatsApp store.
- **Live tests run only in throwaway Herdr sessions,** started with `herdr --session <name> server` with the `HERDR_*` variables unset, or in a throwaway `XDG_CONFIG_HOME`. Delete them afterwards.
- **Don't disturb the owner's setup.** Never stop, restart or prompt the owner's own Herdr sessions or agents.
- **Back up any global CLI config before an experiment changes it,** for example `~/.claude.json` or `~/.codex/config.toml`, and restore it afterwards. Say that you did.
- **Don't run `herdr server stop`** on a session you didn't start.
- **herdr-core:** Switchboard only dials the core; it never runs `hc server …`. Never touch the owner's core: don't send to, key, kill, remove or label its terminals, or write its layout document. Anything that needs a real core runs in a throwaway one under a unique short `HC_SESSION`, with every `HC_*` variable removed, stopped and its state folder deleted afterwards. Unit tests use the fake core in `src/hc/fake.rs`.

## Commands

```sh
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
./target/release/switchboard admin openapi > api/openapi.json
cd apps/ios/SwitchboardKit && swift test
apps/ios/scripts/e2e.sh
```

Live tests skip unless their variables are set:
- `SWITCHBOARD_LIVE_HERDR_SOCKET` and `SWITCHBOARD_LIVE_AGENTS` (`claude`, `codex` or both)
- `SWITCHBOARD_LIVE_HERDR_SESSIONS`
- `SWITCHBOARD_HERDR_TEST_SOCKET`
- `SWITCHBOARD_LIVE_HC=1` with `SWITCHBOARD_LIVE_AGENTS` (`tests/hc_live.rs` starts and removes its own core)

Each test file's header says what it needs.

## Where things are

- **Hub** (`src/hub/`):
  - `service.rs` is the session service, the one place for policy.
  - `ws.rs` is the runner endpoint.
  - `route.rs` decides where a message goes (start, send, or ask). Every front door uses it through the session service.
  - `app/` is the app API.
  - `relay_link.rs` is the link to the relay.
  - `live.rs` keeps each phone's lock-screen card current; `src/live.rs` builds the board and
    seals it to the device's live key, so pushes carry only ciphertext.
  - `away.rs` (under `service.rs`) is "while you were away": recorded agent events, and the
    opt-in summary.
- **herdr-core client:** `src/hc/`: socket discovery, a `hello` per connection, typed calls, the event stream and compare-and-set layout renames. It only dials.
- **Runner** (`src/runner/`):
  - `local.rs` is the runner built into the hub for its own machine; `link.rs` is a remote runner's WebSocket to the hub. Both carry the same masked frames.
  - `herdrs.rs` is every Herdr session on the machine.
  - `backend.rs` is the runner's own view of an agent server (a Herdr session or herdr-core), so callers don't read either one's types.
  - `core_watch.rs` mirrors herdr-core and lists its agents as `hc/<name>`; `core_sessions.rs` starts sessions there (`new_sessions` in the runner config), in a new space or one the owner names.
  - `manage.rs` renames an agent, a space or a session's tab when the owner asks from the phone: one API call, or one compare-and-set write of a `name` in the app's layout. It sends no keys.
  - `sessions.rs` covers worktrees and the agent lifecycle.
  - `hooks.rs` is the hook endpoint.
  - `codex.rs` and `dialogs.rs` are the only key paths: Codex's own startup and deny keys, and the owner's chosen option on a recognised dialog.
  - `diff.rs` is the read-only diff of a session's changed file; `summary.rs` runs the
    opt-in `claude -p` summary with every tool, setting, hook and MCP server off.
- **Agent profiles:** `src/agents/`. Claude Code runs in auto mode by default; what still asks goes through the hook, and deny patterns are checked before every tool. A machine can choose acceptEdits, where every shell command asks. Codex runs in restricted mode. The dialog recognisers are `dialog.rs` (the shared option-row parser) with a layer per CLI in `claude/screen.rs` and `codex/screen.rs` that knows its layouts.
- **Hook helper:** `src/hook/`. It denies on any error.
- **Pairing and signatures:** `src/auth/`. **Push:** `src/push/`. **Relay:** `src/relay/`. **WhatsApp:** `src/whatsapp/`.
- **Config schema:** `src/config.rs`, with documented fields and validation.
- **Scripted runner** for the iOS UI tests: `examples/fake_runner.rs`. It speaks the runner protocol with canned answers and no Herdr.

**Decisions** live in `.agents/tasks/`:
- `debate-herdr-bot-gateway.md`: the main plan
- `debate-native-apps-2.md`: the apps and relay
- `spike-findings.md`: verified CLI and Herdr behaviour the code relies on
- `herdr-app/plan.md`: the Herdr app (herdr-core) backend
- `whatsapp-*.md`: the WhatsApp front door

Read the relevant plan before changing an area. When the build differs from the plan, add a short dated "As built" note to it.

## Conventions

- **Comments** are one line, and only where they add information.
- **Commits** are atomic, grouped by concern, with conventional messages (`feat(hub): …`, `fix(runner): …`, `docs: …`). No AI attribution, and no `Co-Authored-By` or "Generated with" lines. Commit, push or branch only when the user asks.
- **Herdr.** Prefer Herdr's socket API through `src/herdr/` over shelling out to the `herdr` CLI. Verify behaviour against the installed Herdr's `herdr api schema` rather than assuming it.
- **WhatsApp is frozen.** Keep it working and tested, but don't add features unless asked.
