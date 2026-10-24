# AGENTS.md

Instructions for coding agents working on Switchboard. Read `README.md` for the product overview.

## What this project is

1. **A product other people install.** Each person runs Switchboard on their own Mac, Linux box or VPS. It becomes the control board for the Herdr sessions and repositories on that machine. There are many installs, one owner each. It is not a shared multi-tenant service.
2. **One engine, several front doors.** The native apps (iOS built, Android planned), WhatsApp on the owner's own number (frozen), and AI bots over MCP (planned) all go through the same session service, with the same rules, idempotency and audit.
3. **Everything stays on the owner's machine.** The one shared service is the relay on the publisher's server. It forwards end-to-end encrypted phone requests to a hub by relay id and sends push wake-ups with the publisher's push key. It must never see plaintext, store user data, or run agents.
4. **Safety is a product feature.** Approvals fail closed, secrets are masked on the way out, and nothing types into an agent's terminal except one narrow Codex fallback.
5. **Failure is expected.** Lost responses, crashes, restarts and dropped connections are designed for: request ids, outcome unknown with reconciliation, durable push delivery, and state rebuilt from Herdr plus Switchboard's own records.

## Rules you must not break

Each of these has tests. Keep them passing, and add tests when you touch the area.

- **Fail closed.** An approval that is unanswered, expired, or interrupted by a runner, hub or hook-helper loss ends in deny. Replaying a decision returns the recorded outcome and does nothing else.
- **Mask everything that leaves the machine** with `src/redact.rs`. That covers API responses, approvals, terminal output, push text and WhatsApp messages. Never store unmasked tool input.
- **Keystrokes.** Only `src/runner/codex.rs` may send keys to an agent, and only the deny choice of a recognised, version-pinned Codex dialog. Nothing in `src/whatsapp/` or the app paths sends keys. Claude panes never receive keys.
- **Approve rights and signatures.** Approvals go through the session service's decide, which checks the caller's approve right. App approvals need a valid device signature. Deny never needs one.
- **The relay** forwards ciphertext and routing ids only. Only the app API is reachable through it, never the runner endpoint. Push text is generic, with ids only, and never names a project, tool, command or output.
- **Worktrees.** Never force-remove one. Only clean and merged worktrees may be removed.
- **Projects.** Only allowlisted projects. Never reroute a session to another machine.
- **Secrets.** Hub, relay and runner secrets (runner tokens, APNs keys, relay keys, the WhatsApp store) live outside any git working tree, with mode 0600. The code refuses paths inside a repo. Don't weaken that.
- **The app API contract.** Additive changes only within `/app/v1`. After changing app API types, regenerate `api/openapi.json` with `switchboard admin openapi`. A test fails on drift.
- **Migrations** in `migrations/` are numbered and append-only. Never edit one that has shipped.

If a change would bend one of these, stop and ask the user. Don't work around it.

## Working safely on this machine

- **Never read or print env files, credentials, keys or tokens.** That includes `~/.config/switchboard/*.token`, `.p8` keys and the WhatsApp store.
- **Live tests run only in throwaway Herdr sessions,** started with `herdr --session <name> server` with the `HERDR_*` variables unset, or in a throwaway `XDG_CONFIG_HOME`. Delete them afterwards.
- **Don't disturb the owner's setup.** Never stop, restart or prompt the owner's own Herdr sessions or agents.
- **Back up any global CLI config before an experiment changes it,** for example `~/.claude.json` or `~/.codex/config.toml`, and restore it afterwards. Say that you did.
- **Don't run `herdr server stop`** on a session you didn't start.

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

Each test file's header says what it needs.

## Where things are

- **Hub** (`src/hub/`):
  - `service.rs` is the session service, the one place for policy.
  - `ws.rs` is the runner endpoint.
  - `route.rs` decides where a message goes (start, send, or ask). Every front door uses it through the session service.
  - `app/` is the app API.
  - `relay_link.rs` is the link to the relay.
- **Runner** (`src/runner/`):
  - `local.rs` is the runner built into the hub for its own machine; `link.rs` is a remote runner's WebSocket to the hub. Both carry the same masked frames.
  - `herdrs.rs` is every Herdr session on the machine.
  - `sessions.rs` covers worktrees and the agent lifecycle.
  - `hooks.rs` is the hook endpoint.
  - `codex.rs` is the only key path.
- **Agent profiles:** `src/agents/`. Claude Code runs in auto mode by default; what still asks goes through the hook, and deny patterns are checked before every tool. A machine can choose acceptEdits, where every shell command asks. Codex runs in restricted mode, version-pinned.
- **Hook helper:** `src/hook/`. It denies on any error.
- **Pairing and signatures:** `src/auth/`. **Push:** `src/push/`. **Relay:** `src/relay/`. **WhatsApp:** `src/whatsapp/`.
- **Config schema:** `src/config.rs`, with documented fields and validation.
- **Scripted runner** for the iOS UI tests: `examples/fake_runner.rs`. It speaks the runner protocol with canned answers and no Herdr.

**Decisions** live in `.agents/tasks/`:
- `debate-herdr-bot-gateway.md`: the main plan
- `debate-native-apps-2.md`: the apps and relay
- `spike-findings.md`: verified CLI and Herdr behaviour the code relies on
- `whatsapp-*.md`: the WhatsApp front door

Read the relevant plan before changing an area. When the build differs from the plan, add a short dated "As built" note to it.

## Conventions

- **Comments** are one line, and only where they add information.
- **Commits** are atomic, grouped by concern, with conventional messages (`feat(hub): …`, `fix(runner): …`, `docs: …`). No AI attribution, and no `Co-Authored-By` or "Generated with" lines. Commit, push or branch only when the user asks.
- **Herdr.** Prefer Herdr's socket API through `src/herdr/` over shelling out to the `herdr` CLI. Verify behaviour against the installed Herdr's `herdr api schema` rather than assuming it.
- **WhatsApp is frozen.** Keep it working and tested, but don't add features unless asked.
