# Dialog fixtures from herdr-core terminals

`terminal.read` captures (text only; the core gives no colours), taken on 2026-11-11 with herdr-core
0.1.20 in a throwaway core, on a terminal started at 200 by 50. Home paths are rewritten to `/home/u`
and the repo to `/home/u/repo`. The core's own status for each blocked screen is listed: the runner
answers only when it agrees with the dialog it recognised.

## `claude-2.1.322/` (Claude Code 2.1.322, `--permission-mode default`, or `plan`)

| File | Screen | Core status |
|---|---|---|
| `question-single` | AskUserQuestion, one single-select question | `blocked`, kind `question` |
| `permission-bash` | Bash permission | `blocked`, kind `permission` |
| `plan` | ExitPlanMode plan approval | `blocked`, kind `permission`, msg `approve plan` |

## `codex-0.167.0/` (Codex 0.167.0, `--no-daemon -s workspace-write -a on-request`, folder untrusted)

| File | Screen | Core status |
|---|---|---|
| `update` | an "Update available" modal before anything else | not reported |
| `folder-access` | restricted-mode startup; row 2 is now `Back to Agent Command Center`, footer `esc back` | not reported |
| `ready` | composer, with an animated background; footer `← for agents · ? for shortcuts` | `idle`, turn 1 |
| `permission-command` | command permission, row 1 selected | `blocked`, kind `permission` |
| `after-deny-command` | after Esc | `idle` |

The Codex startup classifier in `src/agents/codex/screen.rs` doesn't recognise `update`,
`folder-access` or `ready` from this version, so a Codex start fails closed with nothing sent until
it is updated. The permission dialog is unchanged.
