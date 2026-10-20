# WhatsApp as a front door to the hub

## Outcome

A WhatsApp message should decide where work goes. "In switchboard, fix the flaky hub test" starts a new Switchboard session on the switchboard project. "Use the auth session: also add a test" sends a follow-up to an existing session. Today the bridge only follows agents that already exist and needs a hard-coded Herdr session in its config. Both change.

The bridge becomes a second front door to the hub, next to the MCP tools that step 7 will add. It calls the same session service, so WhatsApp sessions get what bot sessions get:
- a worktree per session
- the project allowlist
- permission hooks with structured approvals
- masking, audit and the shared mute state

Locally, the hub and one runner run on the Mac, and the bridge runs inside the hub process. Step 7's MCP and OAuth aren't needed for this.

## Decisions (made by the user)

- **Routing by alias.** The bridge matches project names and aliases from the hub's allowlist against the message text. With no match or several, it replies with a numbered list of candidates, and the user picks one by number. There's no language model in the loop.
- **New session by default.** A message that names a project starts a new session in a fresh worktree. It reuses an existing session only when the message says so: "use this session" means the session the conversation is currently on, and "use X session" or "use X" means a match against live sessions. Match X against the hub's sessions (id, project, branch or a short label) and the live Herdr agent names from `herdr agent list`. If X matches several, reply with a numbered list. Note that `herdr session list` lists Herdr servers, not agents, so it isn't the source here.
- **Structured approvals from WhatsApp.** An approval notification shows the masked tool name and input with a short approval number. The user replies `approve <n>` or `deny <n>`, which goes to the session service's decide method. Never send keystrokes for this. The Codex keystroke fallback stays deny-only and unchanged. Expired or already-decided numbers return the recorded outcome.
- **The allowlist starts as every git repo under `~/personal` and `~/work`.** Write it into the local `hub.toml` with folder names as aliases. Disambiguate name collisions between the two trees, for example `work/foo` and `personal/foo`. The user edits `hub.toml` afterwards.
- **No hard-coded Herdr session.** Remove `herdr_session` from `whatsapp.toml`. The runner finds its Herdr server the way the `herdr` CLI does: the socket from its environment, otherwise the default. A config value may still override that, but it's optional.

## Scope

- The bridge moves into the hub process. It's enabled by a `[whatsapp]` section in `hub.toml` that replaces `whatsapp.toml`. Keep self-chat mode, the allowlist of senders, masking, rate limits, debounce and the echo guards. The standalone `switchboard whatsapp` subcommand either goes away or becomes a thin wrapper; pick whichever keeps the code smaller, and say which.
- The mute state moves onto the hub's shared store, as the plan already describes, importing the bridge's old file once.
- **Agents Switchboard didn't create**, like `switchboard-impl`, can still be read, prompted and watched, through the runner rather than the bridge talking to Herdr directly. Add the runner operations this needs. They have no hooks, so they never get approvals from WhatsApp. If one is blocked at a dialog, the bridge says to answer it in the tab.
- **Machine selection.** The local Mac runner is the only machine for now. If a message names a machine, match it against configured machine ids; otherwise use the default machine from `hub.toml`.
- **Commands:**
  - Free text that names a project starts a session.
  - `use this` and `use <x>` select a session to reuse.
  - `new <project> <prompt>` explicitly starts a session.
  - `sessions` lists live sessions with short numbers.
  - `status`, `read [n]`, `stop <x>`, `approve <n>`, `deny <n>`, `mute` and `unmute` work as before, and `help`.
  - When a message names no project and no session, it goes to the current session if there is one. Otherwise the bridge asks which project.
- **Local setup:** write `~/.config/switchboard/hub.toml` and `runner.toml` for this Mac, mint a runner token with the admin command, and document the exact commands to start the hub and runner. The user fills in their own number in the `[whatsapp]` section; don't guess it. Use the existing self-chat placeholder from `~/.config/switchboard/whatsapp.toml`, which the user hasn't filled in yet.

## Acceptance checks

1. "In <project>, <task>" starts a new session on that project: a worktree, an `sb-` agent and the prompt delivered. The bridge replies with the session's short label and branch.
2. "Use this session: <text>" goes to the current session. "Use <x>: <text>" goes to the matched session, or to a matched Herdr agent Switchboard didn't create. An ambiguous match gets a numbered list back.
3. A message matching no project asks with a numbered list. Answering with a number starts the session.
4. A permission request in a Switchboard session arrives as a WhatsApp message with a number and masked input. `approve <n>` lets the tool run, and `deny <n>` blocks it. Replaying a number returns the recorded outcome. No key is ever sent.
5. Notifications, mutes, masking, the echo guards and self-chat behave as before. A mute set from WhatsApp is visible through the session service.
6. Nothing in any config names a Herdr session unless the user chooses to override it.

## Testing

- **Unit tests:** alias matching and collisions, the reuse parser, numbered disambiguation, and the approval-number lifecycle.
- **In-process tests:** the hub, the bridge with a fake WhatsApp transport, and the real runner over local WebSocket, covering acceptance checks 1 to 5.
- **A live run** against a named Herdr test session with real Claude Code. Never use the user's `switchboard` session.
- **Keep passing:** the no-send-keys guard tests for the WhatsApp code and the Codex-only key path.

## Rules

- Same as before:
  - Back up and restore any global CLI config you touch.
  - Never read or print env files, credentials or keys.
  - One-line comments at most.
  - Don't commit until the user asks.
- Stop and report if this conflicts with the main plan. Update the plan and `whatsapp-bridge.md` where the design changes.
