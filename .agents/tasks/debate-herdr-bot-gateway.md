# Switchboard: let bots start and watch Herdr coding sessions

## Outcome

The task: build a service on your own VPS that a bot (an OpenAI dot, Grok) can call with a message. The service starts a coding agent in a new Herdr session for the right project, and the bot can watch what that session does.

Decision: build Switchboard as a single-user Rust service. It's one `switchboard` binary that runs as a hub on the VPS and as a runner on each machine that hosts agents. The hub exposes a remote MCP server over HTTPS, and Dots and Grok both connect to remote MCP servers as custom connectors. A bot lists your allowlisted projects and calls a start tool. Switchboard then creates a git worktree, opens a Herdr workspace and starts Claude Code or Codex in it on the machine you chose: the VPS by default, or your Mac. After that the bot polls status and the terminal tail, sends follow-up prompts, and approves or denies the agent's tool-permission requests.

The design makes four choices that are not obvious:

- **Permission requests go through each agent's own hook, not the terminal screen.** Herdr only reports "blocked" plus the raw screen. Claude Code and Codex both have a permission hook that runs before their on-screen dialog. Switchboard installs itself as that hook, turns each request into a structured approval the bot can answer, and denies anything left unanswered. If the hook helper itself dies, both CLIs fall back to their own dialog with "Yes" preselected. Claude Code turns that case into a deny with one setting. Codex has no such setting, so the runner presses the dialog's "No" key under narrow rules, described in Rationale.
- **Every machine runs the runner, which dials out to the hub.** That includes the VPS itself. Your Mac sits behind NAT and needs no open ports. Herdr's control socket has no authentication and is never exposed.
- **One static binary does every job.** The same file is the hub, the runner, the permission-hook helper and the admin tool. Installing on the Mac means downloading one signed file. No Rust toolchain is needed there.
- **The hub runs its own small OAuth server.** The Rust MCP SDK only does OAuth on the client side, and ChatGPT connectors expect OAuth. A deliberately narrow, single-user OAuth server inside the hub is cheaper to run than a separate identity server. This was the one point the reviewers did not agree on; see Rationale.

Practical consequence: a test spike comes first. Five things have to work before the dependent code is worth building: the permission hooks and how they fail, the folder-trust prompts that appear on every new worktree, ChatGPT's OAuth handshake against the hub, whether a dot may call tools that change things, and the static Linux build.

## Rationale

You're the only user. Your clarifications set the requirements: run on the VPS or on a chosen machine for each request, take repos only from an allowlist, let the bot read output, send prompts and answer permission prompts. You chose Rust. Under those constraints the design weighs a small codebase, a single artifact and a fail-closed approval path above flexibility.

**Remote MCP, nothing else, in v1.** xAI's API documents Remote MCP tools over Streamable HTTP or SSE. Dots get custom integrations through ChatGPT Developer Mode connectors, which are remote MCP servers. MCP Events, the webhook mechanism for waking a dot, exists, but recent reports say the event payload never reaches the dot's run (openai/codex issue 50714). So bots long-poll a status tool instead.

**The bot picks the project. The server doesn't classify.** The caller is already a language model. It reads the project list and passes an id. Switchboard matches that id exactly against the allowlist and rejects anything unknown.

**Approvals use native hooks because Herdr has no dialog data.** Herdr's socket API schema (v0.9.1, protocol 22) has no fields for dialogs, options or approvals. A blocked agent is a status plus screen text, and answering means sending keystrokes. Both CLIs pass the tool name and input to a permission hook and take allow or deny back from it. The spike ran this with Claude Code 2.1.302 and Codex 0.163. Both agents call the same `switchboard hook` helper, which prints a deny on any error, timeout or unreachable runner.

The helper can still be killed, and the spike showed that both CLIs then fall through to their on-screen dialog. Each agent gets its own handling for that case.

- **Claude Code.** The hook is configured with `"onFailure": "block"`, which turns a killed, failed or timed-out helper into a deny. Claude draws its dialog while the hook is still running, so a stray keystroke could answer it. The runner therefore never sends keys to a Claude pane. The only code that sends keys takes a Codex session, so no Claude path exists.
- **Codex.** Codex has no fail-closed setting. Its launch profile marks the repo untrusted for that run and passes Switchboard's hook on the command line with `--dangerously-bypass-hook-trust`. The bypass alone would also run hooks that a repository ships in its own `.codex` folder, unreviewed. Untrusting the repo switches off the repo's `.codex` config, hooks and exec policies, so **Codex repos run without their own `.codex` settings**. When a Codex hook fails, the runner denies the dialog with keystrokes, under these rules.
  - It acts only on a Codex pane that Herdr reports blocked, with no pending approval and no helper still running, after a five-second grace period. If a helper delivered an answer around the time the pane blocked, the runner sends nothing and reports `hook_failed`.
  - A version-pinned adapter must recognise the screen as a Codex permission dialog first. The runner then sends the dialog's "No" key once, re-reads the screen and checks that the dialog is gone.
  - If the screen isn't recognised, or the dialog is still there afterwards, the runner sends nothing more. It leaves the session blocked, shows `hook_failed` in the session status and audits it.
  - The fallback never sends an approving key, and the adapter has none to give. Every fallback deny goes to the audit log with the screen excerpt.
  - The adapter supports only the Codex versions it was built from, 0.163.0 for now. The runner refuses to launch the Codex profile on any other version.

**Unanswered means deny.** That covers a timeout, a lost connection, an expired approval and nobody watching. The CLI hook timeout is 300 seconds. The helper answers by 290 seconds and the runner denies an undecided approval at 280, so the agent never sees a hook timeout.

**Outbound runners instead of Herdr's SSH forwarding.** `herdr --machine` would need inbound SSH from the VPS into your Mac, and nobody has verified that event streaming works through it. A runner that dials out over WebSocket works behind NAT. Running the same runner on the VPS keeps a single code path.

**One unprivileged execution account, no containers in v1.** The agent CLIs need a one-time interactive login, and containers would need that login state mounted in anyway. This is a guardrail against agent mistakes, not a security boundary: an agent can edit files its own account owns.

**Rust choices.**
- One cargo package with modules, not a workspace. Separate crates add manifests and dependency wiring and buy nothing until code is reused elsewhere.
- Hand-written serde types for the dozen Herdr methods Switchboard uses, rather than code generated from Herdr's 277 KB schema or a dependency on Herdr's own crates, which aren't known to be published as libraries. Contract tests check the requests against a vendored copy of the schema, and the runner refuses to start against a Herdr protocol version it doesn't know.
- JSON over WebSocket between hub and runner, not gRPC. gRPC would need HTTP/2 through Caddy and protobuf codegen, which buys nothing for one user.
- SQLite through tokio-rusqlite with the bundled SQLite build.
- TOML config, the Rust norm and what Herdr itself uses. YAML's main serde crate is reportedly unmaintained.
- A static x86_64 musl binary for Linux, because the VPS distribution is unknown and a static binary removes any glibc version question. rustls throughout, so there's no OpenSSL.

**OAuth: a narrow, single-user server built into the hub. Keycloak is the fallback.** The case for Keycloak is strong: don't write an authorization server yourself, and Keycloak documents dynamic client registration. Against it: it's a separate Java service with its own database, upgrades and memory on a VPS that also runs coding agents, and its compatibility with MCP clients is unverified too. Swapping later stays cheap because only the auth module and token validation change.

The hub version is deliberately narrow:
- Registration opens only for ten minutes after you run an admin command, and it keeps a single client.
- Redirect URIs must match exactly. Prefix matching is out, per the OAuth security best-practice RFC 9700.
- Only the authorization-code flow with S256 PKCE is supported.
- Consent is a rate-limited passphrase page, with the passphrase stored as an argon2 hash.
- Access tokens are short-lived. Refresh tokens rotate. Every token is opaque and stored only as a hash.
- Tokens are bound to the hub's URL when the client sends a resource parameter.

Every rejection path gets a test. If the spike shows ChatGPT accepts a manually entered client id and secret, registration is dropped entirely. Choose Keycloak if the hand-rolled server fails the spike, or if you'd rather not own this code.

## Scope and acceptance criteria

In scope:
- The MCP tools below.
- Project and machine config files.
- One worktree per session.
- The hub-runner protocol.
- Permission hooks for Claude Code and Codex.
- Startup trust handling.
- The single-user OAuth server, plus a static bearer token for the xAI API.
- SQLite for state and the audit log.
- Release builds for Linux and macOS.
- Caddy and systemd on the VPS, and launchd for the Mac runner.

Out of scope for v1: pushing, merging or opening PRs automatically, multiple users, a browser live view, server-side message classification, MCP Events push, output history or read cursors, containers, Linux ARM builds (add them if the VPS is ARM).

### MCP tools

| Tool | What it does |
|---|---|
| `projects_list()` | Allowlisted projects with id, aliases, description and the machines each one exists on |
| `machines_list()` | Registered machines and whether each runner is connected |
| `session_start(project_id, prompt, machine_id="vps", agent?, notify=true, request_id?)` | Creates the worktree and workspace, handles startup trust, confirms the hook is live, then submits the prompt. With `notify=false` the session starts muted. Returns the session id and branch |
| `sessions_list()` | Sessions with status, plus flags for stale, dirty or unmerged worktrees |
| `session_get(id, after_revision?, wait_ms<=25000)` | Lifecycle, agent status, revision, time observed, any pending approval (tool name and input), any recognized question dialog, and the last N lines with ANSI stripped, marked as untrusted text |
| `session_prompt(id, text, request_id?)` | Sends a follow-up through Herdr's agent prompt call, which refuses an agent sitting at a dialog |
| `session_decide(id, approval_id, approve or deny, request_id?)` | Answers a pending permission request |
| `session_answer(id, dialog_id, option)` | Answers a non-permission question dialog the runner has recognized. Rejects permission dialogs, unknown screens and stale dialog ids |
| `session_stop(id, remove_worktree_if_clean=false)` | Expires pending approvals, ends the agent, closes the workspace. Keeps the worktree and branch unless the worktree is clean and merged and the caller asked for removal |
| `session_notify(id, muted)` | Mutes or unmutes notifications for one session and returns the resulting state |

**Notifications share one mute state.** Every session's agent is named `sb-<id>`, and the WhatsApp bridge watches those agents without being told to. The hub stores three things: a muted flag per session, a mute-all flag that also covers sessions created later, and the agents outside Switchboard that the user opted in. `session_start(notify=false)`, `session_notify` and the bridge's `mute`, `unmute` and `watch` commands all read and write that state, so a session muted from a bot is muted on WhatsApp too, and the reverse. Until the hub exists, the bridge keeps the same three things in its own file beside its WhatsApp store. When it moves onto the hub, it imports that file once and from then on calls the session service.

The MCP endpoint runs stateless, with JSON responses. rmcp's default session store lives in memory, so a hub restart would drop every bot's session. Statelessness avoids that. The spike confirms both clients accept it.

Idempotency: bots retry tool calls, and a retried start would spawn a second agent. Each mutating call takes an optional `request_id`. The hub stores the operation and its outcome before it contacts the runner, so the record survives crashes of either side. As a fallback, a start with the same project, machine, agent and prompt within 60 seconds returns the existing session.

If the runner disconnects while a change is in flight, the call returns `outcome_unknown` with its operation id instead of claiming failure. When the runner reconnects, the hub reconciles before it allows another start. The runner's hello message lists the workspaces it owns, each labelled with its session id.

### Acceptance criteria

1. A dot and an xAI API call can each list projects, start a session on the VPS, watch it go from working to blocked, idle or done, read the output and send a follow-up.
2. The same flow works on the Mac runner, including a disconnect and reconnect. The session keeps its machine and is never rerouted.
3. A permission request from Claude Code and from Codex shows up as a pending approval with tool name and input. Approve and deny both take effect.
4. An unanswered approval, a lost runner, a lost hub, a failed or killed hook helper, a hook-framework timeout and an expired approval id all end in deny. None of them lets the tool run. Replaying an approval that was already used returns its recorded outcome and doesn't run the tool again.
5. A killed or failed hook helper never lets the tool run. For Claude Code, `onFailure: block` turns it into a deny. For Codex, the runner's keystroke fallback denies the dialog, or leaves the session blocked as `hook_failed` when the screen isn't a recognised Codex permission dialog. The runner never sends keys to a Claude pane and never sends an approving key to any pane. A Codex version the adapter doesn't support is refused at launch.
6. No Claude Code agent receives its task prompt until its SessionStart hook has reached the runner, which proves the hook settings are loaded. If it can't, the session fails with `hooks_untrusted`. Codex runs its SessionStart hook only at the first turn, so a Codex agent can't prove its hook is live in advance, and it relies on the keystroke fallback instead. Its task prompt waits until the runner has recognised and cleared the restricted-mode startup screen, which also shows that the repo was opened untrusted.
7. A new worktree's agent reaches ready with no keystrokes from the bot, because the runner handles trust prompts.
8. Unknown project or machine ids are rejected. A start on an offline machine fails fast.
9. A crash between creating the worktree and recording it reconciles without a second agent. Restarting the hub or a runner, or losing events, reconciles from Herdr's session snapshot plus Switchboard's own records.
10. A retried `session_start` with the same `request_id` returns the same session, including after a hub restart.
11. Dirty or unmerged worktrees are never removed. Switchboard never passes `force` to Herdr's worktree removal.
12. The audit log records every prompt, decision and dialog answer, with obvious secrets redacted from tool input.
13. ChatGPT completes discovery, registration or manual client entry, PKCE, token issue, refresh after expiry and re-authorization after a hub restart. A wrong redirect URI, a missing or wrong PKCE verifier, a reused code, an expired token and a closed registration window are each rejected.
14. The Linux binary has no dynamic library dependencies (check with `ldd` or `file`). On a minimal Linux install it opens SQLite, resolves DNS, verifies TLS certificates and connects over WSS.
15. The Mac runner installs and upgrades without cargo. The upgrade checks the release signature and keeps the previous binary for rollback.
16. A session started with `notify=false` sends no notifications. Muting or unmuting through `session_notify` shows up in the bridge's `status`, a WhatsApp `mute` shows up for bots, and both survive a hub restart.

## Implementation plan

The repository is empty, so every file below is new. All code lives in one cargo package: `src/main.rs` is a thin clap entry point over a library in `src/lib.rs`.

0. **Spike before anything else.** The answers gate steps 4, 5 and 7.
   - **Claude Code hook.** Run `switchboard hook` (a stub at first) as the permission hook, from a settings file passed with `--settings` and stored outside the worktree. Measure the CLI's maximum hook timeout. Find out what happens when the helper is killed, and whether a session-start hook can prove the hook is live.
   - **Codex hook.** The same questions. Also find where hooks are configured, whether a feature flag is needed, and how to avoid the "hooks need review" prompt. Codex 0.163 showed that prompt on first start in a fresh repo and won't run untrusted hooks.
   - **Folder trust.** On a fresh worktree path both CLIs may ask whether to trust the folder (Codex 0.163 did). Find out whether config can pre-trust a single path. Run this on macOS as well as the VPS, because pre-trusting on the Mac writes to your personal CLI config. Either disclose that or give the Mac runner its own config home.
   - **Connectors.** From a real dot and the xAI Responses API, call a tool that changes something on a throwaway hub build exposing one tool. Check the stateless MCP mode. Run the OAuth flow from acceptance 13 and record the exact redirect URI ChatGPT uses.
   - **Linux build.** Produce the musl build and run the checks from acceptance 14.
1. **Scaffold.**
   - Create `Cargo.toml` and the `src/main.rs` and `src/lib.rs` entry points.
   - Add clap subcommands: `hub`, `runner`, `hook --agent claude|codex`, and `admin`. The `admin` subcommand covers check-config, hash-passphrase, mint-runner-token, open-registration, install-systemd, install-launchd and upgrade.
   - `src/config.rs` loads two strictly validated TOML files. `hub.toml` holds projects (id, aliases, description, per-machine repo path, default agent, deny patterns), machines (id, token hash), the auth settings and the public URL. `runner.toml` holds the machine id, hub URL, token file path and Herdr session.
2. **Store.** `src/store/` uses tokio-rusqlite with bundled SQLite and WAL mode. Numbered SQL files live in `migrations/`, tracked with `PRAGMA user_version`, and the database file is backed up before each migration. Tables: sessions, operations keyed by request id, approvals (deadline and outcome), audit, OAuth clients, codes and tokens. The hub's second migration adds notification state (the per-session muted flag, mute-all, and other named agents with `watched` and `muted` flags), a session's agent name, `hook_failed` flag and error, and each approval's delivery state. Approvals store only the masked tool input.
3. **Herdr client.** `src/herdr/` speaks the newline-delimited JSON socket protocol with hand-written serde types for these methods: `worktree.create`, `worktree.list`, `worktree.remove`, `agent.start`, `agent.prompt`, `agent.wait`, `agent.read`, `agent.send_keys`, `workspace.close`, `events.subscribe` and `session.snapshot`. Unknown response fields are ignored, and unknown statuses map to Unknown. On `events_lost` it resubscribes and reconciles from the snapshot. A vendored `tests/fixtures/herdr-api-schema.json` drives the contract tests. This module imports nothing else from the crate.
4. **Hub-runner protocol.** `src/proto/` defines tagged serde enums: Hello (machine id, protocol version, owned sessions), Req (id, operation, deadline in relative milliseconds, so clock drift between Mac and VPS doesn't matter), Resp and Event. Protocol rules:
   - The runner presents its token in the Authorization header at the WebSocket upgrade. The hub compares token hashes in constant time.
   - Each connection gets u64 correlation ids, a 20-second ping and bounded queues.
   - The runner reconnects with jittered backoff. A generation counter stops stale replies from landing on a new connection.
   - Changes still in flight when the connection drops come back as `outcome_unknown` and are never replayed.
5. **Runner and hook helper.** `src/runner/` holds the WebSocket client (tokio-tungstenite with rustls), typed operations on top of the Herdr client, and a 127.0.0.1 endpoint for the helper. `src/agents/` holds a launch profile per agent. Each session gets a directory outside the worktree holding its hook settings and secret, and the hook command line names the secret file.
   - Claude Code gets a `--settings` file with the fail-closed permission hook and a SessionStart hook for liveness. It runs with `--permission-mode acceptEdits` (decided 2026-10-18), set explicitly because a user's global default may be auto mode. File edits are accepted without asking. That mode on its own also runs `touch`, `mkdir`, `mv`, `cp`, `rm`, redirects and `git status` in the worktree without calling the hook, which would skip approvals and deny patterns. So the settings file also carries `permissions.ask = ["Bash"]`, which sends every shell command back through the hook (spike-findings.md, Q10). The runner pre-trusts the main repo root in `~/.claude.json`, which covers every worktree of that repo. That is a lasting write to the user's Claude config, and the Mac README has to say so.
   - Codex gets the restricted-mode arguments from Rationale and the pinned screen adapter. Every restricted start shows a "Folder access" screen, which Herdr reports as idle and which swallows any prompt sent while it is up. The runner presses Enter only when the adapter recognises "Open restricted" as the selected option, never blindly, because Esc on that screen quits Codex. The prompt waits until the normal input box shows on two reads in a row.
   - For both agents the prompt also waits until Herdr marks the agent ready. After a startup screen Herdr can keep the launch pending for about 15 seconds, and it refuses prompts meanwhile.
   - An unknown startup screen fails the start with `startup_failed`. The runner closes the workspace and keeps the worktree.

   `src/hook/` reads the agent's hook input on stdin and calls the runner. It enforces its own deadline, shorter than the CLI's hook timeout, and prints the agent-specific deny on any error. Stdout carries only hook protocol output. The runner checks deny patterns before it creates an approval. Decisions arrive only from the hub.
6. **Hub.** `src/hub/` is an axum app holding:
   - The runner WebSocket endpoint.
   - The session service: lifecycle, routing to the requested machine, idempotency, reconciliation, the approval state machine with deadlines, and the notification mute state shared with the WhatsApp bridge.
   - The audit log.

   As built (2026-10-18):
   - **Endpoint.** `GET /runner` checks the bearer token against every machine's hash before the upgrade and answers 401 otherwise. The first frame must be a hello for that machine within 10 seconds. A newer connection for the same machine replaces the older one, and requests in flight on the older one fail, changes as `outcome_unknown`. The hub pings every 20 seconds and drops a runner that has been silent for 40.
   - **Routing.** A session stays on the machine it was started on. A start, prompt, decision or stop for an offline machine fails at once with `machine_offline` and leaves no operation record, so a retry with the same `request_id` runs normally once the machine is back. Project ids match the id or an alias exactly. Session ids are 12 hex characters, the branch is `sb/<id>` and the agent `sb-<id>`.
   - **Idempotency.** Every change records an operation before the runner is asked, under the caller's `request_id` or a generated id. A start writes its session row in the same transaction. A retried id returns the recorded result or failure, or `outcome_unknown` while the outcome is still open. Reusing an id with other arguments is refused. The 60-second start dedupe skips failed sessions.
   - **Reconciliation.** A machine takes no start until the hello of its current connection is reconciled, and none while one of its starts has an unknown outcome (`reconciling`). Sessions the hello lists become running. A running session it doesn't list becomes `exited`, a new lifecycle state, and a stopping one becomes `stopped`. The hello can't list a start that is still creating its worktree, so a missing start is failed only 240 seconds after it began, after a fresh session list. Before then, the runner's own audit of the first prompt, or of a failed start, settles it. Events from a runner only touch sessions recorded on that runner's machine.
   - **Approvals.** The hub's deadline is the runner's minus 5 seconds, so a decision never races the runner's own expiry. A decision is recorded once, before it is sent, together with what became of it:
     - `delivered`.
     - `not_delivered`: the runner had already closed it or was unreachable, so an allow is recorded as a deny.
     - `unknown`: the connection dropped mid-send, and an allow may have run.

     A lost runner denies its machine's open approvals. A hub start denies all of them and marks unconfirmed allows `unknown`. Denials the runner hasn't confirmed are sent again on its next hello, so a helper still waiting gets its deny at once instead of at 280 seconds.
   - **Hub restart.** On start the hub marks open operations `outcome_unknown`. A graceful stop closes the runner connections first, so open approvals end as `runner_lost` rather than `hub_restart`.
   - **Audit.** Every start, prompt, decision, dialog answer, stop, mute and runner event is logged with its actor: the caller, `hub`, or `runner:<machine>`.
   - **Masking.** Nothing that leaves a machine carries an unmasked secret (decided 2026-10-18). One redactor in `src/redact.rs` masks by key name (password, token, secret, API key, authorization and similar) and by pattern: bearer and basic credentials, passwords in URLs, `--token`-style flags, `*_TOKEN=` assignments, private-key blocks and common token prefixes. It runs at each exit:
     - The runner masks every frame it sends to the hub: tool input, terminal tails, screen excerpts, error messages and its own audit entries.
     - The hub masks every reply and event it takes from a runner, again, and stores only the masked tool input. A decision doesn't need the raw input, so it isn't kept anywhere. Deny patterns still match the raw input, on the runner, before it leaves.
     - The WhatsApp bridge masks agent output before splitting it into messages, so a secret can't be cut in two and missed, and masks every message again just before sending.

     One exception: a prompt a bot sends reaches the agent as written, because it is the user's instruction. Only its audit copy is masked.
   - **Mutes.** The service reads and writes the mute state by session id or by agent name, and returns it as the bridge's own `WatchState`. The bridge's file can be imported once. The bridge itself still uses its file; moving it onto the service is separate work.
   - **API for step 7.** `SessionService` has a typed method per tool: `projects`, `machines`, `start`, `list`, `get` (with `after_revision` and a wait capped at 25 seconds), `read`, `prompt`, `decide`, `answer`, `stop` and `notify`. For the bridge it adds `set_mute_all`, `set_agent_muted`, `set_agent_watched`, `notify_state` and `import_watch_state`, and there is `audit_log`. Each change takes a `Caller` naming the actor, and every error has a stable code.
   - **Not in step 6.** `sessions_list`'s stale, dirty and unmerged flags, because the runner protocol has no call that inspects a worktree. `session_answer` reaches the runner, which still refuses screen dialogs. `session_get`'s recognized question dialog has no source yet.
   - **Crates.** axum 0.8.9 depends on tungstenite 0.29, so the binary carries it next to the runner's 0.30. Redaction uses the `regex` crate.
7. **MCP and auth.** `src/hub/mcp.rs` mounts rmcp's Streamable HTTP service in stateless JSON mode, with `allowed_hosts` set to your public hostname, and defines the tools with rmcp's tool macros.

   `src/auth/` serves protected-resource metadata, authorization-server metadata, the registration endpoint (open only for a window started by an admin command), `/authorize` with the passphrase page, and `/token`. Its middleware accepts either a hub-issued OAuth token or the separately scoped xAI bearer token, answers missing or bad tokens with 401 and a pointer to the metadata, and hands the caller's identity to tools through the request parts.
8. **Release and deploy.**
   - A GitHub Actions workflow builds `x86_64-unknown-linux-musl` on an Ubuntu runner with musl-tools, and `aarch64-apple-darwin` on a macOS runner.
   - It signs the archives with minisign. The `admin upgrade` command checks signatures against a public key compiled into the binary, then replaces the binary atomically and keeps the previous one.
   - `deploy/` holds a Caddyfile, systemd units for the hub, the VPS runner and the Herdr server (hub and agents run under separate accounts), and a launchd plist template.
   - A README covers the one-time CLI logins, the runner tokens, opening the registration window, and the pre-trust disclosure.

Order: the spike first. Steps 1 to 4 can be built alongside it, because they don't depend on its answers. Step 5 waits for the hook and trust answers, and step 7's auth waits for the OAuth answer. Step 6 needs steps 2 to 5, and step 8's workflow can come early because the spike needs the musl build.

## Testing strategy

Steps 1 to 6 and the WhatsApp bridge are built, on stable Rust from rustup. `cargo test` runs the unit tests, the Herdr contract tests and an in-process hub test. That test connects the real runner code to the hub over WebSocket on 127.0.0.1, through a proxy that cuts the connection mid-start and mid-approval, and restarts the hub on its database. The real-Herdr tests in `tests/` run only when `SWITCHBOARD_LIVE_HERDR_SOCKET` points at a named test session. They have passed against Claude Code 2.1.302 and Codex 0.163.0 on macOS, and a Claude run through the hub passed too. The OAuth, MCP end-to-end, killed-process, release and live-client checks below are still planned.

- **Unit tests** sit next to the code, run with `cargo test` (proposed). They cover config validation, routing, idempotency, the approval state machine with every deny path in acceptance 4, the proto codecs, and OAuth rejection cases from acceptance 13. Deadline tests use a manual clock instead of tokio's paused clock, because tokio-rusqlite runs SQLite on its own thread and paused time jumps ahead while a query runs.
- **Herdr contract tests** in `tests/` check serialized requests against the vendored schema with the jsonschema crate.
- **Real-Herdr integration tests** are gated behind an environment variable and run against a named Herdr test session, never your default one, deleted afterwards. A fake agent drives idle, working and blocked states. Real Claude Code and Codex runs cover hooks and startup (acceptance 3, 5, 6, 7). A test session is started with `herdr --session <name> server`, with the caller's `HERDR_*` variables unset.
- **End-to-end tests** use rmcp's client against an in-process hub on a random local port, with an in-process runner connected over real WebSocket.
- **Fault injection.** A transport wrapper that drops frames, a killed hook helper, killed hub and runner processes during an approval and during a start, and a failed migration.
- **Release checks.** `ldd` or `file` on the Linux binary, a smoke run on a minimal Linux image, and an upgrade with signature checking on the Mac (acceptance 14 and 15).
- **Live clients.** MCP Inspector first, then a real dot and an xAI Responses call (acceptance 1, 2, 13).

## Constraints and open questions

Settled:
- Rust, one binary, one package.
- Single user, allowlisted projects only.
- No automatic push or merge.
- The Herdr socket is never exposed.
- Unanswered approvals become deny.
- Nothing that leaves a machine carries an unmasked secret.
- Dirty or unmerged worktrees are never removed.
- JSON over WebSocket between hub and runner.
- SQLite with TOML config.
- Static musl build for Linux.

Left to the implementer: exact crate versions, the default number of tail lines, the approval deadline within the hook's limit, the dedupe window, and the token lifetimes.

Open, owned by the spike:

| Question | Kind | Steps affected | Spike result (local Mac, 2026-10-18; see spike-findings.md) |
|---|---|---|---|
| Hook timeout limits, and what each CLI does when the helper is killed | Blocker | 5 | Answered for the killed helper. Claude Code 2.1.302 and Codex 0.163 both fall through to the on-screen dialog, with "Yes" preselected, when the helper is killed, exits non-zero or times out. Claude becomes fail-closed with `"onFailure": "block"` on the hook. Codex ignores that field and has no equivalent. Timeouts: the default is 600 s for both and a configured `timeout` is honoured; the maximum is still unverified |
| Codex hook config, feature flag and hook trust | Blocker for Codex. Claude Code can go ahead alone | 5 | Answered. Configure with `-c 'hooks.PermissionRequest=[...]'` on the command line. The `hooks` feature is stable and on by default. Pass `--dangerously-bypass-hook-trust` to avoid the "Hooks need review" modal, which Herdr reports as `idle`. Output shape is the same as Claude's. The bypass also runs hooks a repo ships in `.codex`, so the profile marks the repo untrusted for each run (spike-findings.md, Q8 and Q9) |
| Pre-trusting a worktree path through config, or falling back to adapters | Blocker for unattended start | 5 | Answered. Both CLIs record trust against the main repo root, which covers every worktree of that repo. Codex: per run with `-c 'projects={"<repo root>"={trust_level="trusted"}}'` (the inline-table form; the dotted form doesn't work), with no file write. Claude: `projects["<repo root>"].hasTrustDialogAccepted = true` in `~/.claude.json`, a persistent global write. Trusting a parent directory doesn't cover repos below it |
| ChatGPT's OAuth flow against the hub: registration or manual client entry, redirect URI, resource parameter | Blocker for the dot path. Keycloak is the fallback | 7 | Open |
| Whether a dot may call tools that change things on your plan | Blocker for the dot path only. Grok through the xAI API isn't affected | 7, live tests | Open |
| Whether both clients accept stateless MCP and which protocol version they need (rmcp's docs list 2025-11-25 as the newest) | Blocker if rejected. Fallback is a database-backed session store | 7 | Open |
| MCP client tool-call timeouts | Non-blocking. The 25-second cap is configurable | 7 | Open |

Built: steps 1 to 6. Step 5 ran end to end against real Claude Code 2.1.302 and Codex 0.163.0 in a Herdr test session on macOS, and step 6 ran a Claude session through the hub the same way. Ready now: step 7's MCP tools on the session service, and the release workflow. Not ready: the OAuth server and the live connector path, until the connector spike answers. Don't treat the plan as ready to build in full yet.

## Verification status

- **Repository state.** The repo was empty, with no commits on `main` and a clean tree.
- **Herdr.** Checked against the installed Herdr 0.9.1: its CLI help, its exported socket API schema (protocol 22) and its socket docs. The schema confirmed the methods in step 3, including worktree list and remove (removal takes a workspace id, and force defaults to off). It also confirmed that creating a worktree takes no environment variables, that the read call has no cursor, and that there are no dialog or approval fields.
- **Rust MCP SDK.** Checked against the rmcp docs. It has a Streamable HTTP server that mounts in axum, with a pluggable session store (in memory by default, or stateless), allowed-host settings and access to the request from tool handlers. Its OAuth module is client-side only.
- **Agent hooks.** Run on macOS with Claude Code 2.1.302 and Codex 0.163.0, first by the spike and then by the runner's live tests. spike-findings.md has the evidence. Behaviour on Linux and on other CLI versions is unverified.
- **Hub.** Run on macOS: the in-process hub and runner test, and one Claude Code session started, approved, denied, prompted and stopped through the hub in a Herdr test session, in `acceptEdits` mode with a file edit accepted without an approval. The hub binary served the runner endpoint and refused a missing token. Nothing has run against a Linux hub or over WSS through Caddy yet.
- **Integrations.** The dot and Grok integration facts come from web sources dated September and October 2026, some of them third-party.
- **Unverified.** Crate maintenance status and versions, release-signing details and Keycloak's fit were not checked. Nothing was built or run.
- **Review.** Both reviewing agents checked the architecture across three rounds and the Rust draft across two. They agreed on everything here except OAuth, where one held that Keycloak is the safer choice. Their corrections are folded in: exact redirect matching, relative deadlines, `outcome_unknown` with reconciliation, stateless MCP, signed releases, the missing worktree calls and hook secret files kept outside the worktree. They haven't seen this final wording.
