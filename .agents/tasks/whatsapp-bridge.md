# WhatsApp bridge, version zero

Since 2026-10-19 the bridge runs inside the hub; "Front door to the hub" below says what changed.

## Outcome

Add a `switchboard whatsapp` subcommand so the user can follow and steer Herdr agents from WhatsApp while away from the terminal. It links to WhatsApp as a device (the same WhatsApp Web emulation approach Hermes Agent uses), runs on the Mac and talks to the local Herdr socket directly. It notifies the user when a watched agent needs attention or finishes, shows recent output on request and forwards plain messages to the agent as follow-up prompts. It cannot approve or deny anything in version zero.

Once Switchboard's hub exists, this becomes a second front door next to MCP and calls the same session service, so approvals can arrive later through the structured permission hooks instead of keystrokes. Keep the relay logic separate from the Herdr calls so that swap stays small.

## Decisions (made by the user)

- **Connection:** WhatsApp Web emulation in Rust: `whatsapp-rust` or its stable-Rust fork `wa-rs`, picked after checking which builds on stable and is maintained. Record the crate and version. No Node or Go sidecar.
- **Number:** a separate WhatsApp number, linked as a device by QR code or pair code. Never the user's personal account, except in self-chat mode (added 2026-10-19), an explicit opt-in for testing with your own number as the only tester.
- **Powers:** notify, read and prompt only. The bridge never sends keys to an agent and never answers a dialog.
- **Order:** built after the current scaffold and hook-spike task, on top of its config, Herdr client and test layout.

## Scope

- Subcommand `switchboard whatsapp`, configured from TOML. Either a section in the runner config or its own file, whichever fits the scaffold. Settings:
  - the allowed sender number or numbers
  - the Herdr session to use
  - where WhatsApp session data is stored
- WhatsApp session data is a credential. Store it outside the repo with `0600` permissions, and make sure git ignores any local path.
- Allowlist. Accept messages only from the configured sender numbers. Drop everything else silently, and log only the sender id, never the content.
- Which agents notify. There is no watch list in the config (changed 2026-10-18).
  - Every agent Switchboard creates is watched automatically. The runner names them `sb-<id>`, and the bridge picks up new ones as they appear, with no restart.
  - Other agents are opted in from the chat with `watch <agent>`.
  - Mutes are made from the chat. `mute all` also covers `sb-` agents created after it, until `unmute all`.
  - These choices are kept in the bridge's own store and survive restarts.
- Commands:
  - `status`: each watched agent's state, and whether it's muted.
  - `list`: live Herdr agents, each with its watch state.
  - `use <agent>`: pick the target for later messages.
  - `read [n]`: the last n lines with ANSI codes stripped. Cap n, and split long output to fit WhatsApp message limits.
  - `mute <agent>` and `mute all`: stop notifications.
  - `unmute <agent>` and `unmute all`: resume them.
  - `watch <agent>`: opt in an agent Switchboard didn't create, for example `switchboard-impl`.
  - `help`.
- `use`, `read` and prompts work on any live agent, watched or not.
- Any other text goes to the selected agent through Herdr's agent prompt call. If the agent is blocked, Herdr refuses the prompt. The bridge then replies with the dialog's screen excerpt and says to approve it in the tab or through Claude's remote control.
- Notifications. Subscribe to Herdr's agent status events for every named agent, and notify for the watched, unmuted ones. On blocked, send the screen excerpt. On done or idle, send the last lines. Debounce repeated changes and rate-limit outgoing messages, since a low-volume chat lowers the ban risk. On `events_lost`, resubscribe and reconcile from Herdr's session snapshot.
- Show the QR or pair code in the terminal on first run. Reconnect with backoff after drops. Give a clear message when the link is revoked.

Out of scope: approvals, starting new sessions from WhatsApp, the official Cloud API, group chats, media, any language-model summarizing.

## Acceptance checks

1. The user links the separate number. `status` from the user's own number gets an answer. A message from any other number gets no reply, and its content isn't logged.
2. A Switchboard agent (`sb-…`) started while the bridge runs is watched with no configuration. Going blocked produces a WhatsApp message with the dialog excerpt within a few seconds, and going done produces the last lines. `mute`, `mute all` and `watch` behave as described above and survive a bridge restart.
3. A plain message reaches the agent as a prompt. A prompt sent to a blocked agent is refused with the explanation above, and no keys are sent.
4. Restarting the bridge reuses the stored link without a new QR code. Killing the network and restoring it reconnects.
5. Nothing in this task can send keystrokes to an agent. Check this in code review: there is no send-keys call in the WhatsApp module.

## Testing

- **Unit tests.** Command parsing, the allowlist, message splitting, debouncing, rate limiting, and the watch rules: a new `sb-` agent is watched automatically, mutes persist across a restart, `mute all` covers later agents, and `watch` opts in another agent.
- **Fake transport.** Put the WhatsApp connection behind a small trait. A fake implementation lets the relay be tested end to end without a network.
- **Herdr integration tests** run against a named Herdr test session, never the user's current session.
- **Manual test** with the real linked number, covering acceptance checks 1 to 4. The user has to scan the code with the phone that holds the separate number.

## Built (2026-10-18)

- **Crate.** `whatsapp-rust` 0.7.0, released August 2026, with frequent releases before it. `wa-rs` has a single release from February 2026. `whatsapp-rust` builds on stable Rust 1.99.1 once its default `simd` feature is off, because that feature uses nightly `#![feature]`.
- **SQLite.** The crate's SQLite store bundles `libsqlite3-sys` 0.37, while Switchboard's `rusqlite` 0.40 uses 0.38, and Cargo allows only one. Switchboard therefore turns off the store feature and depends on `whatsapp-rust-sqlite-storage` 0.7.0 directly with default features off, and diesel then shares the 0.38 build. There's no OpenSSL or aws-lc in the tree; TLS uses ring.
- **Config.** The bridge has its own `whatsapp.toml`, because `runner.toml` requires hub settings the bridge doesn't use. A leftover `watch = [...]` line is rejected as an unknown field. The config check refuses a `store_path` inside any git working tree. The bridge sets a `077` umask, so the store and its WAL files are `0600` in a `0700` directory. `.gitignore` covers `whatsapp.toml` and `*.db*`.
- **Senders.** WhatsApp may identify a sender by a privacy id (`…@lid`) and carry the phone number separately. The allowlist checks both, and drops a message it can't match. Notifications go to every allowed sender and to nobody else.
- **Linking.** If the linked account turns out to be one of the allowed senders, which means a personal account, the bridge disconnects and says so. `--pair-phone` refuses an allowed sender's number for the same reason.
- **Reconnects.** The library reconnects on its own. The bridge rebuilds the client with backoff if it stops, and exits with a clear message when the link is revoked or the number is temporarily banned.
- **Lost events.** Herdr 0.9.1 has no `events_lost` message. A subscription stream that ends means events may be lost, and the bridge resubscribes and reconciles from the session snapshot. It also reconciles every 5 seconds.
- **Notifications.** A blocked status has to hold for 3 seconds before it notifies, which hides Claude's brief blocked state while a hook runs. Any working or blocked sighting arms a "finished" notification, so a turn shorter than 3 seconds still reports. Outgoing messages share a bucket of 5 that refills one every 12 seconds. Replies go ahead of notifications, and a newer notification for an agent replaces a queued one.
- **Watch state.** The opted-in agents, the per-agent mutes and the mute-all flag live in `whatsapp-watch.json`, next to `whatsapp.db`. The file is written atomically with `0600` permissions, and a change is confirmed in chat only after it is saved. An unreadable file stops the bridge at startup instead of silently dropping your mutes. The watcher follows every named agent, so watching or unmuting takes effect at once. `unmute <agent>` is refused while `mute all` is on. Only named agents can be watched; an agent started by hand can be named with `herdr agent rename <pane> <name>`. With no watch list there's no agent selected at startup, so the first `use` picks one. When the hub exists, this file is imported into the hub's shared mute state (see the main plan).
- **No keys.** Nothing under `src/whatsapp/` can send keys. A test scans that folder for any send-keys call.
- **Self-chat mode (2026-10-19).** `self_chat = true` links your own account instead of a separate number.
  - Config. `allowed_senders` must hold exactly one number, your own, or the config is refused. When the link connects, the account must be that number, or the bridge disconnects and stops. `--pair-phone` must be that number too. With `self_chat` off, the refusal to link an allowed sender's account is unchanged.
  - The bridge reads only your "Message yourself" chat: messages from you whose chat is your own phone JID or your own privacy id (`…@lid`), learned from the link when it connects. Every other chat, group and DM is dropped before any parsing. Nothing about it is logged or stored, not even its id; only a count of dropped messages is kept.
  - Replies and notifications go to that same chat.
  - The bridge's own messages come back as messages from you. Each one it sends has its id recorded, the last 1024 of them, and is ignored when it returns. As a second guard, every message the bridge sends starts with `🤖 `, and any message starting with it is ignored. The marker also covers an echo that arrives before the send has returned its id.
  - Masking, rate limits and the no-keys rule are unchanged; masking runs before the marker is added.
- **Masking.** Every message the bridge sends has secrets masked by Switchboard's shared redactor (decided 2026-10-18). Agent output and dialog excerpts are masked before they're split into messages, and each message again just before it's sent.

## Front door to the hub (2026-10-19)

whatsapp-sessions.md moved the bridge into the hub. These points replace the version-zero design above wherever they differ.

- **Where it runs.** Inside `switchboard hub`, enabled by a `[whatsapp]` section in `hub.toml` with `allowed_senders`, `store_path` and `self_chat`. `whatsapp.toml` and the `switchboard whatsapp` subcommand are gone; a wrapper would have had to start the hub anyway. Pair-code linking is `switchboard hub --whatsapp-pair-phone +<number>`. A bridge that can't continue (unlinked, banned, wrong account) stops the hub with its reason.
- **No Herdr access.** The bridge calls the session service only. Agents Switchboard didn't create are listed, read and prompted through three runner operations (`agents`, `read_agent`, `prompt_agent`), and the runner reports every named agent's status as an event. The status watcher that used to live in the bridge now runs in the runner, unchanged. The runner refuses `prompt_agent` for its own sessions' agents, so session prompts stay audited as session prompts.
- **Starting sessions.** Free text that names a project starts a session in a fresh worktree on the default machine (`default_machine` in `hub.toml`), or on a machine whose id the text names. Words are matched against project ids and aliases, case-insensitively, with `_`, `/` and `.` read as `-`, so `work/foo` matches `work-foo`. A leading "in/on/for <project>" settles it on its own. No match, or several, gets a numbered list, and the reply number starts the session. WhatsApp's message id is the start's `request_id`, so a redelivered message can't start a second session. The start runs in the background; the bridge answers "Starting…" at once and then "Started <label> in <project>, branch sb/<id>". A session's short label is its project plus the first four characters of its id.
- **Reusing sessions.** `use this[: text]` and `use [the] <x> [session][: text]`. `<x>` matches live sessions by id or id prefix, label, branch, agent name, project id or alias, and named Herdr agents Switchboard didn't create; exact matches win over prefixes, and several get a numbered list. A bare number also picks from the last `sessions` listing. Text that names no project goes to the current session; with none, the bridge asks which project.
- **Approvals.** Each approval gets a short number from the hub database, assigned once and never reused, so it survives restarts. The notification shows the masked tool name and input. `approve <n>` and `deny <n>` call the session service's decide; a used or expired number returns the recorded outcome and sends nothing. No key is ever sent. A blocked notification for a session that has a pending approval is skipped, because the approval message says it. Approval messages follow mutes like other notifications; `status` still lists pending approvals.
- **Mute state.** On the hub's store. The old `whatsapp-watch.json` is imported once at startup and not written again. `unmute all` also clears per-agent mutes, as before.
- **Unchanged.** Self-chat mode, the sender allowlist, masking, rate limits, debounce and the echo guards. The no-keys test now also scans the runner's agent module.

## Setup

The bridge runs inside the hub. See whatsapp-sessions.md's report, or the steps here.

1. Configs live in `~/.config/switchboard/`: `hub.toml` (machines, projects, `[whatsapp]`) and `runner.toml`. Put your own number in `[whatsapp].allowed_senders`; the hub refuses the placeholder.
2. Start the hub in a normal terminal so the QR code shows: `./target/release/switchboard hub --config ~/.config/switchboard/hub.toml`.
3. Start the runner from a pane of the Herdr session it should drive, so it inherits `HERDR_SOCKET_PATH`: `./target/release/switchboard runner --config ~/.config/switchboard/runner.toml`. Or set `herdr_session` in `runner.toml`.
4. Link the device: WhatsApp > Settings > Linked devices > Link a device, scanning with your own phone in self-chat mode. Then send `help` in your "Message yourself" chat.

The risks under self-chat mode below still apply: the linked device decrypts all your chats, and your main number carries the ban risk. To move to a separate number later, unlink the device on your phone, delete the store (`whatsapp.db` and its `-wal` and `-shm` files), set `self_chat = false` and put your personal number in `allowed_senders`; link with the phone that holds the separate number.

### Self-chat mode risks

- **Ban risk on your main number.** The bridge is an unofficial WhatsApp Web client. Meta can restrict or ban the number it runs on, and here that is your personal account, not a spare. Keep traffic low, and switch to a separate number once testing is over.
- **The linked device decrypts all your chats.** As a linked device, the bridge receives and decrypts every message in every chat, not only your own. It discards everything outside your own chat without logging it, but the device keys in `whatsapp.db` can read all of it. Treat that file like your phone: keep it `0600`, never copy it off the Mac, and unlink the device (WhatsApp > Settings > Linked devices) when you stop testing. History sync is skipped, so older messages aren't downloaded.
- Your phone may not notify you of the bridge's messages, because they show as sent by you. Open the chat to see them.
- A message you type that starts with `🤖 ` is ignored, because that is the bridge's marker.

## Rules

- Same as the current task:
  - Never read or print env files, credentials or keys.
  - Don't commit, push or create branches.
  - Comments: one line maximum.
  - Stop and report if the design here conflicts with the main plan in `.agents/tasks/debate-herdr-bot-gateway.md`.
- Using an unofficial WhatsApp client may break Meta's terms and can get the number restricted. Keep traffic low and never send unsolicited messages to anyone but the configured sender.
