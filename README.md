# Switchboard

Switchboard is a control board for the AI coding agents running on your own machine. Install it on your Mac, a Linux box or a VPS. Then, from your phone, WhatsApp or an AI bot, you can see every Claude Code and Codex session working on your repositories, start new ones, send follow-ups, and approve or deny what the agents want to do.

Your code, sessions and secrets stay on your machine. The only shared piece is a small relay that lets your phone reach your machine and receive push notifications. Everything it carries is end-to-end encrypted or reduced to an opaque wake-up.

## What it does

- **Start work from anywhere.** Just write a message. One that names a project ("in kraft, fix the flaky test") starts a new task: Switchboard creates a git worktree and a Herdr session, and starts Claude Code or Codex in it. `use <session or agent>: …` sends to something already running, and a message that names nothing goes to whatever you last wrote to. If it can't tell, it asks. The app and WhatsApp route messages the same way.
- **See what's running.** Lists every agent in every Herdr session on the machine, including ones you started yourself, with status and recent output.
- **Approve from your phone.** Claude runs in its auto mode: routine work runs on its own, and Claude's safety check refuses risky actions and explains why. What still needs you, such as a question or a plan, reaches your phone. Approve with Face ID, or deny. Anything left unanswered is denied. A burst of requests from one session sends one notification at a time.
- **Several ways in, one engine:**
  - the native app (iOS now, Android later)
  - WhatsApp, linked to your own number
  - AI bots such as OpenAI Dots or Grok, over MCP (planned)
  All of them share the same rules, history and audit log.

## How it fits together

```
 phone app ──┐                                ┌── built-in runner ── Herdr ── Claude Code / Codex
 WhatsApp ───┼── hub (your machine) ──────────┤   (the hub's own machine, same process)
 AI bots ────┘    session service, SQLite     └── runner on another machine (optional; dials out to the hub)
      ▲
      └── relay (shared, public): forwards encrypted phone requests to your hub and sends push wake-ups
```

- **The hub** owns sessions, approvals, audit and every front door. It runs on the machine you choose: your Mac, a Linux box or a VPS.
- **A runner** drives Herdr on one machine: worktrees, agent launch, permission hooks and status. The hub has one built in for its own machine, so a single-machine setup is one process with no token. A machine without the hub runs `switchboard runner`, which dials out to the hub, so it works behind NAT.
- **Herdr runs separately.** Each task gets its own detached Herdr server, so restarting or losing Switchboard never stops your sessions or agents. Approvals that were pending at the time end in deny.
- **The relay** sits between phones and hubs, so your machine needs no public address, domain or tunnel:
  - Phones send end-to-end encrypted requests to the relay, which forwards them to your hub by its relay id. Only the app API is reachable this way.
  - Push notifications go out through the relay with the app publisher's push key. They carry generic text and ids only. The phone then fetches the details from your hub.
  - If you have your own public HTTPS address, the hub can also talk to phones directly, without the relay.

## Status

| Part | State |
|---|---|
| Hub, runner, session service, approvals, audit, masking | Built |
| One Herdr session per task, plus reuse of any running Herdr session | Built |
| Claude Code and Codex launch profiles with fail-closed permission hooks | Built (Codex 0.163.0 only) |
| WhatsApp front door: self-chat on your own number, start and reuse sessions, approve | Built, frozen |
| iOS app v1: pairing, sessions, start, approvals with Face ID, devices, push | Built; not yet tested on a physical iPhone |
| Voice-first app (design E): speak or type, see where it goes before sending; your own Herdr agents listed, readable and promptable | Built; voice not yet tested on a physical iPhone |
| Relay: end-to-end encrypted phone-to-hub forwarding and push | Built |
| MCP and OAuth for bots | Planned |
| Changed files per session in the app | Built |
| Live screen and full diff view in the app | Planned |
| Answering an agent's questions | Planned |
| Android app | Planned |
| Release builds and installers | Planned |

## Getting started

There are no release builds yet, so build from source. You need Rust (via rustup), [Herdr](https://herdr.dev), and Claude Code and/or Codex logged in on the machine.

```sh
git clone <this repo> && cd switchboard
cargo build --release
SB=./target/release/switchboard
```

1. **Configure the hub.** Create `~/.config/switchboard/hub.toml`.
   - Declare this machine as the hub's own. The state folder holds per-task hook secrets and must be outside every repo:
     ```toml
     [[machines]]
     id = "mac"
     [machines.local]
     state_dir = "/Users/you/.local/state/switchboard"
     ```
   - `$SB admin scan-projects --machine mac ~/personal ~/work` prints a `[[projects]]` entry for every git repo under those folders.
   - Add `[relay]` to reach your phone through the relay, or set `public_url` to your own HTTPS address.

   The full schema, with comments, is in `src/config.rs`.
2. **Check the config:**
   ```sh
   $SB admin check-config --hub ~/.config/switchboard/hub.toml
   ```
3. **Run it:**
   ```sh
   $SB hub --config ~/.config/switchboard/hub.toml
   ```
4. **Another machine (optional).** To drive Herdr on a second machine, run `$SB admin mint-runner-token --machine <id> --out <path>` and add its `[[machines]]` entry with the printed `token_hash`. On that machine, write a `runner.toml` (machine id, hub URL, token file, state folder) and run `$SB runner --config runner.toml`.
5. **Pair your phone.** Run `$SB admin pair-device --config ~/.config/switchboard/hub.toml`, scan the QR code in the app, check that the fingerprints match, and type `yes`. `admin list-devices` and `admin revoke-device` manage paired phones.
6. **WhatsApp (optional).** Add a `[whatsapp]` section with `self_chat = true` and your number, start the hub, and link it under WhatsApp → Linked devices. You then talk to Switchboard in your "Message yourself" chat. This uses an unofficial WhatsApp client: your number carries a ban risk, and the linked device can decrypt all your chats.

The iOS app builds with plain `xcodebuild`. See `apps/ios/README.md`.

## Security model

- **Approvals fail closed.** Agent permission requests come through each CLI's own hook, not the terminal screen. Unanswered, expired or disconnected requests are denied.
- **Claude's mode.** Claude Code starts in auto mode. Set `claude_permission_mode = "acceptEdits"` under a machine's `[machines.local]` (or in `runner.toml`) to have every shell command and tool call ask you instead. A project's `deny_patterns` are checked before every tool call in either mode.
- **App approvals are signed.** They need a signature from a device key protected by Face ID or a passcode. Each front door's right to approve can be turned off in `hub.toml`.
- **Secrets are masked.** That covers everything that leaves the machine: approvals, terminal output, notifications and WhatsApp messages. Raw tool input is never stored.
- **No keystrokes into agents,** except one narrow, deny-only fallback for Codex.
- **Worktrees with uncommitted or unmerged work are never deleted.**
- **Only allowlisted repositories** can be used.
- **The relay** sees ciphertext and routing ids only, and runs no agents.

## Repository layout

| Path | What's there |
|---|---|
| `src/hub/` | Session service, runner endpoint, app API, relay link |
| `src/runner/` | Herdr sessions, worktrees, agent launch, hook endpoint |
| `src/agents/` | Claude Code and Codex launch profiles |
| `src/hook/` | Permission-hook helper |
| `src/herdr/` | Herdr socket client |
| `src/proto/` | Hub-runner protocol |
| `src/auth/` | Device pairing and signed approvals |
| `src/push/` | Push delivery |
| `src/relay/` | The relay and its encryption |
| `src/whatsapp/` | WhatsApp front door |
| `src/store/`, `migrations/` | SQLite storage |
| `src/redact.rs` | Secret masking |
| `api/openapi.json` | The app API contract, generated from the Rust types |
| `apps/ios/` | The iOS app (SwiftUI) and its Swift package |
| `tests/` | Integration and live tests |
| `.agents/tasks/` | Design plans, decisions and spike findings |

## Development

```sh
cargo test                                   # unit, contract and in-process hub-runner tests
cargo clippy --all-targets -- -D warnings
$SB admin openapi > api/openapi.json         # after changing the app API; a test fails on drift
cd apps/ios/SwitchboardKit && swift test     # iOS package tests
apps/ios/scripts/e2e.sh                      # iOS UI tests against a throwaway local hub
```

Live tests drive real Herdr, Claude Code and Codex. They're skipped unless pointed at a throwaway Herdr session through the `SWITCHBOARD_LIVE_*` variables; see the header comment of each file in `tests/`. Never point them at a Herdr session you use.

Design decisions and their reasoning are in `.agents/tasks/`. Start with `debate-herdr-bot-gateway.md` and `debate-native-apps-2.md`.
