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

## `claude-2.1.325/` (Claude Code 2.1.325, `--permission-mode default`), taken 2026-11-15

One AskUserQuestion call with three single-select questions, and one with a single-select and a
multi-select question. The core's status stayed `blocked`, kind `question`, with the same
`status_ms`, from the first tab to the last.

| File | Screen |
|---|---|
| `question-multi-1` | first question, row 1 selected; tab row `←  ☐ Size  ☐ Drink  ☐ Colour  ✔ Submit  →` |
| `question-multi-1-down` | the same after one `down` |
| `question-multi-2` | after Enter: the second question, the first tab now `☒` |
| `question-multi-3` | after Enter: the third question |
| `question-multi-3-revisit` | the third question shown again after it was answered: the earlier pick ends in ` ✔` |
| `question-multi-review` | after the last Enter: the Submit tab, "Review your answers", row 1 selected. No footer |
| `question-multi-review-down` | the same after one `down` (Cancel selected) |
| `question-multi-after` | after Enter on "Submit answers" |
| `question-mixed-multiselect` | a multi-select question inside a group (`[ ]` rows) |
| `question-single-long` | one question whose text wraps to three lines, each behind a `│` bar; descriptions wrap too |
| `question-single-long-down` | the same after one `down` |
| `question-mixed-review-unanswered` | the Submit tab reached with nothing answered: "You have not answered all questions" |

What the keys did:
- Enter on an option answers that question and shows the next tab; after the last one it shows the
  review. `left`/`right` and Tab move between tabs without answering.
- On the review, Enter on "Submit answers" sends every answer (`answers recorded: Large, Tea, Blue`).
  Enter on "Cancel" declines them all (`User declined to answer questions`).

## `codex-0.169.0/` (Codex 0.169.0, `--no-daemon -s workspace-write -a on-request`, Plan mode), taken 2026-11-15

In default mode Codex has no question tool and asks in plain text. In Plan mode (`/plan`) its
`request_user_input` tool draws a dialog. The core's status was `blocked`, kind `question`.

| File | Screen |
|---|---|
| `question-single` | one question, row 1 selected; footer `… enter to submit answer | esc to interrupt` |
| `question-single-down` | the same after one `down` |
| `question-single-after` | after Enter (`Questions 1/1 answered`) |
| `question-multi-1` | first of three, `Question 1/3 (3 unanswered)`; the footer gains `←/→ to navigate questions` |
| `question-multi-1-down` | the same after one `down` |
| `question-multi-2` | after Enter: `Question 2/3 (2 unanswered)` |
| `question-multi-2-revisit` | the second question shown again after it was answered |
| `question-multi-2-revisit-down` | the same after one `down`, from a later run: the count rises to `(2 unanswered)` until Enter |
| `question-multi-3` | the last question; the footer says `enter to submit all` |
| `question-multi-3-down` | the same after `down down` |
| `question-multi-after` | after the last Enter (`Questions 3/3 answered`) |

What the keys did:
- Enter on an option answers that question and shows the next; on the last it submits them all.
  There is no review step.
- `left` shows the previous question; on the first question it wraps to the last.
- Each row's description follows its label on the same line. Codex adds the last row, "None of the
  above", which takes notes typed after Tab.
- `-c 'projects={"<folder>"={trust_level="trusted"}}'` skips the folder prompt for one run and saves
  nothing; the dotted form of the same key does not.
