# Spike findings: agent hooks and folder trust (local, macOS)

Run on 2026-10-18 on the user's Mac (arm64). Versions: Claude Code 2.1.302, Codex CLI 0.163.0 (model gpt-6-astra), Herdr 0.9.1 (protocol 22). Every agent ran interactively in a Herdr pane of a named test session `sb-test`, which the spike started and later deleted. The hook under test was the stub `switchboard hook --agent claude|codex`, which always prints a deny and logs its input. Shell scripts covered allow, sleep, kill -9, exit 1, garbage output and observe-only. Scratch files live in the session scratchpad under `spike/`.

## Contradictions with the plan

1. **A failed hook falls through to the on-screen dialog by default, in both CLIs.** A timeout, a killed helper (kill -9) or a non-zero exit leaves the tool waiting on the CLI's own approval dialog, with "Yes" preselected. Output that isn't JSON does the same. The plan's fail-closed claim holds only while the helper process itself survives and answers.
   - **Claude Code** can be made fail-closed: set `"onFailure": "block"` on the hook handler. That turns timeout, kill and non-zero exit into a deny. It does not cover invalid JSON on exit 0, which still falls through. The helper owns its stdout, so that case is under Switchboard's control.
   - **Codex has no fail-closed option.** It silently ignores `onFailure="block"`, and the binary has no equivalent. Under acceptance criterion 5, **the Codex launch profile has to be rejected** until some other fail-closed setup is verified.
2. **Codex can't prove its hook is live before the task prompt using SessionStart.** Its SessionStart hook runs when the first turn starts, not when the CLI starts. Acceptance 6 (`hooks_untrusted`) has no working mechanism for Codex yet. Claude's SessionStart does run at startup, before `agent start` reports ready.
3. **Claude draws its permission dialog while the PermissionRequest hook is still running.** For the whole time a hook is pending, Herdr reports `blocked` and the dialog is live on screen. A keystroke sent to the pane would answer it. The plan doesn't rely on keystrokes for approvals, so this is a constraint rather than a break: the runner must never send keys to a Claude pane that has a pending approval. Codex runs the hook first and shows its dialog only if the hook gives no decision.

## Summary

| Question | Claude Code 2.1.302 | Codex 0.163.0 | Confidence |
|---|---|---|---|
| Does the hook fire from config outside the worktree? | Yes, from `--settings <file>`. No review step | Yes, from `-c 'hooks.PermissionRequest=[...]'` on the command line. It also needs `--dangerously-bypass-hook-trust`, or the "Hooks need review" modal appears | Verified |
| Does a deny stop the tool? Does an allow let it run? | Yes and yes | Yes and yes | Verified |
| Feature flag | None | `hooks` is stable and on by default | Verified (`codex features list`) |
| Default hook timeout | 600 s, from the docs | 600 s, from the docs | Derived |
| Configured timeout honoured? | Yes. `timeout: 5` killed the hook at about 5 s | Yes. `timeout=5` reported "hook timed out after 5s" | Verified |
| Maximum timeout | Not found. The docs state none | Not found. The docs state none | Unverified |
| Hook hangs, killed, exits 1 | Falls through to the dialog. With `onFailure: "block"`: deny | Falls through to the dialog. No fix exists | Verified |
| Hook prints garbage, exit 0 | Falls through, even with `onFailure: "block"` | Falls through | Verified |
| Question dialogs through the hook? | AskUserQuestion arrives as PermissionRequest with the full questions and options. It can be answered with `allow` plus `updatedInput.answers` | Not tested. `request_user_input` is "under development" and off by default | Verified (Claude) |
| Plan approval through the hook? | ExitPlanMode arrives as PermissionRequest with `plan` and `planFilePath`. A deny keeps the agent in plan mode | Not tested | Verified (Claude deny only) |
| Other Codex approvals | n/a | Shell escalation (`tool_name: "Bash"`) and apply_patch edits (`tool_name: "apply_patch"`) both go through the hook | Verified |
| Liveness before the prompt | SessionStart (`source: startup`) runs at startup, about 3 s before `agent start` returns ready. It doesn't run until folder trust is accepted | SessionStart runs at the first turn, so it can't be used | Verified |
| Trust prompt on a fresh worktree | Yes | Yes | Verified |
| Can config pre-trust a path? | Yes: `~/.claude.json` → `projects["<main repo root>"].hasTrustDialogAccepted = true`. That's a persistent write to a global file | Yes, per invocation with no file write: `-c 'projects={"<main repo root>"={trust_level="trusted"}}'` | Verified |
| Does trust apply to the worktree or the repo root? | Recorded against the main repo root, so every worktree of a trusted repo is trusted | The prompt says "Trusting will apply to the repository root" | Verified |
| Does a trusted parent directory cover repos below it? | No | Not tested | Verified (Claude) |
| `herdr agent start` during the trust prompt | Returns `agent_not_ready` in about 3.7 s | Returns `agent_not_ready` | Verified |

## Evidence

### Setup

- Headless named session. `herdr --session sb-test server` runs a headless server for that session:
  ```
  herdr server running; you can use any herdr CLI command in another terminal.
  api socket: /Users/adarsh/.config/herdr/sessions/sb-test/herdr.sock
  ```
  `herdr --session sb-test status` confirmed the socket. Teardown: `herdr --session sb-test server stop`, then `herdr session delete sb-test`. Two things to know:
  - Inside a Herdr pane, `HERDR_SOCKET_PATH`, `HERDR_SESSION` and the other `HERDR_*` variables must be unset first, or commands hit the current session.
  - **Panes inherit the server's environment.** The first server was started from inside Claude Code, so its panes inherited `CLAUDE_CODE_*` variables, including a messaging token. Claude then printed "Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker". The server was restarted with those variables unset. The runner must start agents with a clean environment.
- Workspaces created before a server restart were still listed after it. Workspace state persists per session.
- The user's global Claude settings set `permissions.defaultMode = "auto"`, and the pane showed "auto mode on". All runs passed `--permission-mode default`, and the pane then showed "manual mode on". The Switchboard launch profile must set the mode explicitly. Whether PermissionRequest fires in auto mode was not tested.

### Q1. The hook fires from config outside the worktree

**Claude.** The settings file at `spike/claude/c-deny.json` sits outside the worktree:
```json
{"hooks": {
  "SessionStart": [{"hooks": [{"type": "command", "command": ".../hooks/session-start.sh .../logs/c-deny-start.log"}]}],
  "PermissionRequest": [{"matcher": "*", "hooks": [{"type": "command", "command": ".../bin/switchboard hook --agent claude --log .../logs/c-deny.jsonl"}]}]
}}
```
Start command: `herdr agent start c2 --kind claude --pane w2:p1 -- --settings .../c-deny.json --permission-mode default`. Herdr passed the arguments through verbatim; the response included `"argv":["claude","--settings","…/c-deny.json","--permission-mode","default"]`.

Hook input (trimmed):
```json
{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"touch probe-1.txt","description":"Create empty probe file"},
 "permission_mode":"default","permission_suggestions":[{"type":"addDirectories",...},{"type":"setMode","mode":"acceptEdits",...}],
 "session_id":"…","transcript_path":"…","cwd":"…/wt/c2","prompt_id":"…","effort":{"level":"high"},"scratchpad_dir":"…"}
```
Pane after the stub's deny: `Ran 1 shell command ⎿ Denied by PermissionRequest hook`. probe-1.txt was not created. With an allow script: `⎿ Allowed by PermissionRequest hook`, and the file was created.

The first deny run showed the on-screen dialog before the hook's log line appeared. That was the first execution of a freshly copied binary, which macOS checks slowly. Q3 explains why the dialog was visible: Claude draws it while the hook runs.

**Codex.** Hooks load from `~/.codex/hooks.json`, `~/.codex/config.toml`, `<repo>/.codex/…`, plugins, and command-line `-c` overrides. The `-c` form works:
```
herdr agent start x4 --kind codex --pane … -- -s read-only -a on-request \
  -c 'hooks.SessionStart=[{hooks=[{type="command",command=".../session-start.sh .../x4-start.log"}]}]' \
  -c 'hooks.PermissionRequest=[{matcher="*",hooks=[{type="command",command=".../switchboard hook --agent codex --log .../x4.jsonl",timeout=30}]}]' \
  -c 'projects={"<repo root>"={trust_level="trusted"}}' --dangerously-bypass-hook-trust
```
- Without `--dangerously-bypass-hook-trust`, a "Hooks need review" modal appears: "5 hooks are new or changed. Hooks can run outside the sandbox after you trust them. 1. Review hooks / 2. Trust all and continue / 3. Continue without trusting (hooks won't run)". Herdr reported `idle` and `interactive_ready: true` while this modal was up, so `agent start` succeeded with a modal on screen.
- With the flag there is no modal. The startup warnings include "`--dangerously-bypass-hook-trust` is enabled…" and "Running without the shared background server: command-line configuration overrides (-c…) requires embedded mode."
- The other way to avoid the modal is persisted trust: Codex stores `[hooks.state."<source>:<event>:<group>:<handler>"] trusted_hash = …` in `~/.codex/config.toml`. The spike did not write it.

Codex hook input:
```json
{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"touch probe-x4.txt","description":"Allow touch probe-x4.txt to create the file with write access?"},
 "permission_mode":"default","model":"gpt-6-astra","session_id":"…","turn_id":"…","cwd":"…/wt/x1","transcript_path":…}
```
- The output schema embedded in the binary (`permission-request.command.output`) is `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"|"deny","message":…}}}`. This is the same shape the stub prints for Codex, so the stub is correct.
- `interrupt: true`, `updatedInput` and `updatedPermissions` are reserved, and Codex fails closed if they are present.
- Exit code 2 with a reason on stderr also denies ("PermissionRequest hook exited with code 2 but did not write a denial reason to stderr" is the error when stderr is empty).
- Pane after the deny: `• Blocked by hook └ switchboard: no runner connected (hook stub)`. The file was not created. Allow run: `• Ran touch probe-xallow.txt`, and the file was created.

### Q2. Timeouts

- Claude: the docs give the `timeout` field in seconds, default 600 for command hooks, and no maximum. With `"timeout": 5` and a hook sleeping 40 s, the hook started at T, the dialog stayed up, and the hook never logged "woke": it was killed at the timeout. A grep of the 268 MB binary for a clamp ran out of time.
- Codex: the docs give `timeout` in seconds, default 600 for most events, and a 3 s maximum only for SessionEnd and Interrupt. The binary also contains `timeout_sec` and `timeoutSec` spellings. `timeout=5` produced "Hook failed └ hook timed out after 5s".
- The maximum for either CLI is unverified. Choose the helper's deadline well below 600 s and set `timeout` explicitly.

### Q3. Hook failure modes

Each run was a fresh agent asked to `touch probe-<variant>.txt`. The CLI flags were Claude `--permission-mode default`, Codex `-s read-only -a on-request`. In every run the file was not created, because no keystrokes were sent; the "Outcome" column says what the CLI did.

| Hook behaviour | Claude, default | Claude, `"onFailure": "block"` | Codex |
|---|---|---|---|
| sleep 40 with timeout 5 | Dialog stays up (still `blocked` 28 s later, "1. Yes" preselected) | `Denied by PermissionRequest hook` | "hook timed out after 5s", then the dialog |
| kill -9 itself | Dialog | Denied | "hook exited without a status code", then the dialog |
| exit 1, no output | Dialog | Denied | "hook exited with code 1", then the dialog |
| prints `this is not json {`, exit 0 | Dialog | **Dialog** | Dialog |
| sleep 8, then allow (timeout 30) | Dialog visible during the 8 s with status `blocked`; then "Allowed", and the file was created | n/a | n/a |

The Codex `onFailure="block"` run with exit 1 still showed "hook exited with code 1" followed by the dialog, so the field is ignored.

Claude handler used for the block variant: `{"type":"command","command":"…","timeout":5,"onFailure":"block"}`.

### Q4. Questions and plan approval

- **AskUserQuestion (Claude).** An observe-only hook (no decision) logged:
  ```json
  {"hook_event_name":"PermissionRequest","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Red or blue?","header":"Color","options":[{"label":"Red","description":"Choose red."},{"label":"Blue","description":"Choose blue."}],"multiSelect":false}]}}
  ```
  PreToolUse fired too. With no decision, the question UI stayed on screen.

  A hook that returned the following answered it, and the pane showed `User answered Claude's questions: · Red or blue? → Blue ⎿ Allowed by PermissionRequest hook ⏺ Blue`:
  ```json
  {"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow","updatedInput":{"questions":[…],"answers":{"Red or blue?":"Blue"}}}}}
  ```
  So Claude's question dialogs can be handled as structured data instead of screen adapters. It also means a blanket deny denies questions.
- **ExitPlanMode (Claude).** Started with `--permission-mode plan`. The hook got `tool_name: "ExitPlanMode"`, `permission_mode: "plan"` and `tool_input` keys `plan` and `planFilePath`. A deny with a message gave `Error: plan rejected by switchboard spike ⎿ Denied by PermissionRequest hook`, and the agent stayed in plan mode. Allowing a plan was not tested.
- **Codex.** Shell escalation and apply_patch edits both reach the hook. The apply_patch input was `{"tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: …/README.md\n@@\n-hi\n+hello\n*** End Patch"}}`. Codex's question tool (`default_mode_request_user_input`), MCP tool approvals, elicitations and network approvals were not tested.

### Q5. Liveness

- Claude: a SessionStart hook in the same `--settings` file logged `{"hook_event_name":"SessionStart","source":"startup",…}` about 3 s before `herdr agent start` returned ready. In four runs it fired at T and ready arrived at T+3 to 4 s. It did not fire while the folder-trust dialog was up. That proves the settings file's hooks are loaded. It doesn't exercise PermissionRequest itself.
- Codex: SessionStart (`source: "startup"`) logged at T+24 s after `agent start` returned. That was right after the first prompt was submitted, so it can't gate the task prompt.

### Q6. Folder trust

- Claude, on a fresh worktree `wt/c1` of `repo`: `agent start` returned `{"error":{"code":"agent_not_ready","message":"agent c1 is blocked during startup and is not ready for prompts"}}` in 3.7 s. `agent get` showed `agent_status: "blocked", launch_pending: true, interactive_ready: null`. The screen read "Accessing workspace: … Quick safety check: Is this a project you created or one you trust? ❯ No, exit / Yes, I trust this folder".
  - Accepting wrote `projects["…/spike/repo"].hasTrustDialogAccepted = true`, keyed on the main repo, not on `wt/c1`.
  - A second worktree `wt/c2` then started straight to idle.
  - Setting the key directly with jq for a fresh `repo2` also started straight to idle.
  - Setting the key on a parent directory `tparent/` did not cover `tparent/repo3`: its worktree still prompted.
- Codex, on a fresh worktree: `agent_not_ready`, with the screen "Note: You're in a subdirectory of a Git project. Trusting will apply to the repository root: …/spike/repo … 1. Trust and continue 2. Quit".
  - `-c 'projects."<repo>".trust_level="trusted"'` (dotted key) did **not** suppress the prompt.
  - `-c 'projects={"<repo>"={trust_level="trusted"}}'` (inline table) did, and nothing was written to `~/.codex/config.toml`.
  - The inline table replaces the whole `projects` table for that run. The user's other project trust settings don't apply inside that agent, which is harmless for a single-repo session.

### Q7. Herdr behaviour

- `agent start <name> --kind K --pane P -- <args>` passes the arguments verbatim and echoes them back in `argv`.
- `agent start` immediately after `workspace create` can fail with `agent_pane_busy` ("is not an available shell") while the shell is still starting. Retrying for about 0.5 to 2 s works.
- While Claude was blocked on a dialog: `agent_status: "blocked"`. `agent read --lines 60 --source recent-unwrapped` was refused with `agent_not_idle` ("its alternate-screen history can only be captured by scrolling while idle … use --source visible"). `session_get` should read `--source visible` when the agent is blocked.
- `agent prompt --wait` returns at the first `blocked`. For Claude that happens while the hook is still deciding, so `blocked` doesn't mean the hook failed.
- Herdr classifies Codex's "Hooks need review" modal as `idle`. Startup in the Codex profile must never rely on that modal being absent: always pass the trust bypass, or persist the hook trust.

### Q8. Does `--dangerously-bypass-hook-trust` also run hooks shipped in the repo?

**Yes.** Run on 2026-10-18 in a separate test session `sb-test2`, against Codex 0.163.0. The throwaway repo `spike2/repohooks` committed a `.codex/hooks.json` with two hooks, SessionStart and UserPromptSubmit, each of which only touched a marker file. Every run used `-s read-only -a on-request` and passed a per-run trust override for the repo.

| Run | Trust override | Bypass flag | Startup screen | Repo hooks ran? |
|---|---|---|---|---|
| Control | `trust_level="trusted"` | no | "Hooks need review: 5 hooks are new or changed." Herdr reported `idle` | No (the run stopped at the modal) |
| A | `trust_level="trusted"` | yes | Straight to the prompt | **Yes.** Both markers appeared after one harmless prompt |
| C | `trust_level="untrusted"` | yes, plus Switchboard-style `-c` hooks | "Folder access … Config, hooks, and exec policies from untrusted folders stay disabled … 1. Open restricted 2. Quit". Herdr reported `idle` | No. The `-c` hooks ran, and the repo's hooks did not |

- In the control run, the review screen listed the repo hook as `Source: Project config - …/repohooks/.codex/hooks.json`, `Trust: New hook - review required`. Nothing was toggled; the screen toggles trust on space or enter, so only cursor keys were sent.
- So the planned Codex launch (repo pre-trusted plus the bypass flag) would run any hook a repository ships, unreviewed. The bypass also covers the user's own `~/.codex/hooks.json` hooks, including one that shows as "modified" and is therefore untrusted.
- Explicit untrust (run C) disables project-local config, hooks and exec policies, and keeps the `-c` hooks. It needs one startup keystroke ("Open restricted", the default choice) on a modal that Herdr reports as `idle`. In run C the first `agent prompt` was swallowed by that modal.
- Not checked in restricted mode: PermissionRequest, the permission dialog the keystroke fallback would answer, and whether losing the repo's own `.codex` settings matters.
- `~/.codex/config.toml` was not written: its SHA-256 matches the backup. `sb-test2` is stopped and deleted.

### Q9. Restricted-mode check for the Codex launch profile

**Passed.** Run on 2026-10-18 in a separate test session `sb-test3`, against Codex 0.163.0. Each run used the planned profile on a fresh `herdr worktree create` checkout:

```
-s workspace-write -a on-request -c 'projects={"<repo root>"={trust_level="untrusted"}}' --dangerously-bypass-hook-trust -c 'hooks.PermissionRequest=[{matcher="*",hooks=[{type="command",command="<hook>",timeout=30}]}]'
```

The repo shipped its own `.codex/hooks.json` with a SessionStart hook and a PermissionRequest hook that always allows. The probe touched a file under `~/.cache`. A first attempt that wrote under `/tmp` needed no approval at all, because the workspace-write sandbox allows writes under `/tmp` (and Codex likely includes `$TMPDIR` too).

| Run | Hook | Result |
|---|---|---|
| r1b | `switchboard hook` stub (deny) | Hook logged `PermissionRequest`, tool `Bash`. Pane: `Blocked by hook └ switchboard: no runner connected (hook stub)`, then `DENIED`. No file |
| r2 | allow script | `Ran touch …`, and the file was created with escalation |
| r3 | exit 1 | `Hook failed └ hook exited with code 1`, then the command dialog. `esc` gave `✗ You canceled the request to run touch …`. The dialog was gone and there was no file |
| r4 | exit 1, apply_patch edit | Hook input `tool_name: "apply_patch"`. The edits dialog appeared; `esc` removed it and no file was written |

- **The repo's hooks never ran** in any run: no marker files appeared, including from its allow-everything PermissionRequest hook.
- **Startup screen.**
  - Every run opened on: `Folder access / <worktree path> / Config, hooks, and exec policies from untrusted folders stay disabled. … / › 1. Open restricted / 2. Quit / enter continue · esc quit`.
  - Herdr reported `idle`, and `agent start` succeeded.
  - Enter on `1. Open restricted` reached the normal prompt (`>_ OpenAI Codex (v0.163.0)` … `› Ask Codex to do anything`).
  - On this screen, **esc quits Codex**, so the adapter may send Enter only while option 1 is selected.
- **The dialogs match the trusted-mode capture from Q3.** Both have the title `Would you like to run the following command?` or `Would you like to make the following edits?`, then the options `› 1. Yes, proceed (y)`, `2. Yes, and don't ask again … (p|a)`, `3. No, and tell Codex what to do differently (esc)`, then the footer `Press enter to confirm or esc to cancel`. The deny choice is `esc`.
- **After the deny,** Herdr still reported `working` for several seconds before `done`. The fallback has to confirm from the screen, not from the status.
- The screens are vendored, with paths anonymised, under `tests/fixtures/codex-0.163.0/`.
- Cleanup: `sb-test3` stopped and deleted, the scratch worktrees and `~/.cache/switchboard-spike3` removed, no global config written.

### Q10. What `acceptEdits` sends through the hook (Claude Code)

Run on 2026-10-18 in a separate test session `sb-hub-test2`, against Claude Code 2.1.302, through the hub with Switchboard's own settings file. Each command was a one-command prompt in the session's worktree.

| Command | `acceptEdits` alone | `acceptEdits` plus `permissions.ask = ["Bash"]` |
|---|---|---|
| `touch`, `mkdir`, `mv`, `cp`, `rm` | Ran with no hook call | Hook asked |
| `echo hi > r.txt` | Ran with no hook call | Hook asked |
| `git status --short` | Ran with no hook call | Hook asked |
| `python3 -c …`, `curl -sI …` | Hook asked | Not rerun |
| Write tool creating `notes.md` | Not tested | No hook call; the file was written |

- With the mode alone, file-changing shell commands inside the worktree skip both the approval and the runner's deny patterns, because the runner checks deny patterns inside the hook.
- With the `ask` rule, the status bar still read "accept edits on", and each command's approval arrived with its exact command line. Allow and deny both took effect.
- Not checked: whether a repo's own `.claude/settings.json` `allow` rules can override the `ask` rule, and edits outside the worktree.
- `~/.claude.json` was backed up first, and its project keys matched the backup afterwards. `sb-hub-test2` is stopped and deleted.

## Changes to global config

- `~/.claude.json`:
  - Backed up first.
  - Accepting the trust dialog in run c1 wrote `projects["…/scratchpad/spike/repo"]`.
  - The spike set `hasTrustDialogAccepted` on `…/spike/repo2` and `…/spike/tparent` with jq.
  - All keys under the scratch path were removed afterwards with a targeted jq filter. The project count went back to 29, matching the backup.
- `~/.codex/config.toml`: backed up. Never written: the SHA-256 after the spike matches the backup.
- `~/.claude/settings.json`: not touched.
- Side effects that remain: the agents' transcripts and session logs under `~/.claude/projects/-private-tmp-claude-501-…-spike-*` and `~/.codex/sessions/`. These are ordinary per-run history.

## Still unverified

- The maximum hook timeout for either CLI.
- Whether PermissionRequest fires when Claude runs in auto mode (the user's global default), or under Codex `--approve-for-me` or guardian review.
- Allowing ExitPlanMode through the hook, and whether the agent then leaves plan mode.
- The Codex question tool, MCP tool approvals, elicitations and network-access approvals.
- Any fail-closed setup for Codex. One untested idea that would be a design change: the runner treats a Codex pane that is `blocked` with no pending approval as a failed hook and sends the dialog's "No" key.
- A Codex liveness check before the first prompt.
- Whether Claude's `--settings` hooks or `onFailure` behave the same on Linux on the VPS. The folder-trust behaviour on the VPS.
- Whether the Claude dialog-before-hook race can let a human or another client answer first. Herdr makes the dialog visible to any attached TUI client.
