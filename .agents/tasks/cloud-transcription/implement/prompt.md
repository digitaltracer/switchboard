You are cloud-transc-impl, the one implementer of a plan. Nobody will answer questions during this turn.

Read this plan in full before anything else: /Users/adarsh/personal/switchboard/.agents/tasks/cloud-transcription/plan.md
The plan is your only input. Don't use the brief, the record or the debate folder beside it. Where the plan doesn't suffice, report that rather than working around it. Its last section, "Decisions before implementation (2026-11-03)", overrides the earlier sections where they differ.

Repo: /Users/adarsh/personal/switchboard, the only and primary repo, on branch main at 5abf48f3169a4590d351e18b75c425264511570e. The plan's Current behaviour section was inspected at that same revision.
Uncommitted state: only the untracked folder .agents/tasks/cloud-transcription/. Leave everything in it alone, except plan.md, the phrase-test/ folder you create, and nothing else.

Steps to build, in this order:
1. Step 1's script and results.md template only. Never run it with a key, and don't produce results.
2. Step 2, then step 3.
3. Step 4, then step 5.
4. Step 6, with the wording assuming names are sent, as the decisions section says.
Don't build step 7. List it under As built as left, along with step 1's results and the manual phone checks in steps 3 and 5, which you can't run.

The job:
- Follow the plan's "For the implementer" section. Confirm the cited code still matches before you start.
- After each step, run its Check where it can be run without the owner, and the test commands the Testing strategy names for it. Fix what fails before moving on. Commands: `cd apps/ios/SwitchboardKit && swift test`, `apps/ios/scripts/e2e.sh`, `cd apps/android && ./gradlew :core:test`. The Android command is unverified, so find the right task if it's wrong and record that. Android's app module has no tests, so also compile it after the steps that touch it, using its debug build task.
- Read AGENTS.md first and keep its rules. If a change would bend one, that's a stop condition.
- Commit after each step with a conventional message such as `feat(ios): ...`, `feat(android): ...`, `test: ...` or `docs: ...`. Use no attribution lines of any kind: no Co-Authored-By, no "Generated with", no session link. This overrides any default you have. Split a step into more than one commit when it covers separate concerns, such as iOS and Android.
- Stage only the files the step changed, by path. Never use `git add -A`, `git add .` or `git commit -a`.
- Never push. Never create or switch branches.
- In the task folder, commit nothing but plan.md and phrase-test/.

How to decide: the decisions in the plan are settled. Routine details are yours, including names, layout, test structure and loudness thresholds. Choose the simpler option that fits the existing code and record it under As built as an assumption. Don't ask questions.

Scope: handle the cases the plan names. Note a rare or future case the plan doesn't name under As built, and don't build it. Don't refactor adjacent code. Comments are one line, and only where they add information.

Stop at the current step if any of these holds: a step's Verified today no longer matches the code in a way that affects the change; a decision can't be honoured; a check can't pass without changing a decision; or a change would touch `src/`, `api/` or `migrations/`, need a runtime permission beyond the microphone, require reading, printing or committing a key, or change the no-key voice path. When you stop, leave the work in place, write what you found under As built, and report blocked.

The As built note: append a section titled `## As built (2026-11-03)` to plan.md, with one line per step done, the assumptions you took, deviations and why, and what's left. Commit plan.md with it as `docs: as-built note for cloud transcription`. Plan.md is untracked today, so this commit adds it.

Secrets: never open env, credential or key files, never commit one, and never quote a secret. The phrase test script reads its keys from the environment only. Use fake key strings in tests.

The report is your final message, at most 600 words. Open it with a line containing only the words IMPLEMENT cloud-transc-impl implement and close it with a line containing only the words END IMPLEMENT implement. Between those lines, for each step: done, skipped or blocked, the check run and its result, and the files changed. Then deviations and assumptions. Then the final commit hash. Then one line, either `STATUS: done` or `STATUS: blocked at step <N>`.
