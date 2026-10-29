# Dialog fixtures

Herdr `agent read --source visible` captures, as text (`.txt`) and with colours (`.ansi`), taken on
2026-10-30 with Herdr 0.9.5 in a throwaway session on a 119-column pane. Home paths are rewritten to
`/home/u`. The recognisers in `src/agents/` are pinned to these versions.

## `claude-2.1.312/` (Claude Code 2.1.312, `--permission-mode default`)

| File | Screen | Selected |
|---|---|---|
| `question-single` | AskUserQuestion, one single-select question | 1 |
| `question-single-down` | the same after one `down` | 2 |
| `question-multiselect` | one multi-select question (`[ ]` rows, Submit tab) | 1 |
| `question-multi` | two questions in one call (tab row `← ☐ Size ☐ Drink ✔ Submit →`) | 1 |
| `permission-bash` | Bash permission | 1 |
| `permission-edit` | Edit permission | 1 |
| `plan` | ExitPlanMode plan approval | 1 |
| `plan-down` | the same after one `down` | 2 |

What the keys did:
- `up`/`down` move the `❯` marker one row and commit nothing. Enter commits the selected row.
- A digit commits that row at once, with no confirmation (a `3` answered the question with row 3).
- Questions: Enter on an option answered it (`· Which colour do you prefer? → Green`). Rows "Type
  something." and "Chat about this" open free text or a chat, so they're never offered.
- Permissions: the last row (`No`) rejects (`User rejected update to notes.txt`). The "always allow"
  and "switch to … mode" rows change standing permissions.
- Plans: Enter on row 2 approved the plan (`User approved Claude's plan`). There is no deny row; row 3
  ("Tell Claude what to change") is free text, and Esc rejects.
- The footer varies with the selection: `Esc to cancel · Tab to amend` on some rows, `Esc to cancel` on
  others.

Log identity: the pending tool_use id was in the session log while a Bash permission was showing,
but not while the first question was showing (it appeared after the answer). So the id is used when
present, and the fallback identity otherwise.

Hook answers (a `--settings` file with a PermissionRequest hook, no global config changed):
- ExitPlanMode with a plain `{"behavior":"allow"}` left the plan dialog on screen.
- ExitPlanMode with `{"behavior":"allow","updatedInput":<the original tool_input>}` approved it
  (`User approved Claude's plan … Allowed by PermissionRequest hook`).

## `codex-0.165.0/` (Codex 0.165.0, `-s read-only -a on-request`, folder untrusted, no hooks)

| File | Screen |
|---|---|
| `folder-access` | restricted-mode startup, `Open restricted` selected |
| `ready` | composer |
| `permission-command` | command permission, row 1 selected |
| `permission-command-no-selected` | the same after `down down` |
| `after-deny-command` | after Enter on row 3 |
| `permission-edit` | edit permission, row 1 selected |
| `after-allow-edit` | after Enter on row 1 (the edit was applied) |
| `question-queued` | the request_user_input tool in default mode: no modal, `? 1 question ⇧← to answer` above the composer |

What the keys did:
- `up`/`down` move the `›` marker; Enter commits it. Enter on row 3 ("No, and tell Codex what to do
  differently") denied and interrupted the turn with no text box, so it is the deny row.
- Row 2 ("don't ask again") grants a standing approval.
- The log had the pending `custom_tool_call` (`exec`) with its `call_id` while the dialog showed.
- The dialog layout is the same as 0.163.0's.
