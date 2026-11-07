# Record: cloud transcription

Invocation: `/council plan — support transcription services using OpenAI or Claude; the owner enters an API key to enable it. Primary reason: the built-in speech transcription on Android and iOS is very bad. Debaters: Claude and Codex.`

## Roster
| Name | Kind | Model | Tab | Pane |
|---|---|---|---|---|
| cloud-transc-a1 | claude | configured (Opus 5.5, high) | w1:tN | w1:pN |
| cloud-transc-a2 | codex | configured (GPT-6-Astra, high) | w1:tP | w1:pP |
| cloud-transc-impl (implementer) | claude | configured | w1:tQ | w1:pQ |

Tabs were created without the OpenCode permission variable: a policy hook blocks any command naming env files, and no OpenCode agent is in the roster.

## Paths
- Task folder: `/Users/adarsh/personal/switchboard/.agents/tasks/cloud-transcription`
- Scratch: `/private/var/folders/k9/5lplp70s46q8x0whbcf77bx40000gn/T/council.h3QRteG3Gg`
- Repo: `/Users/adarsh/personal/switchboard` (main, `5abf48f`)
- Plan: `/Users/adarsh/personal/switchboard/.agents/tasks/cloud-transcription/plan.md`

Started: 2026-11-02 00:14 IST

## Stages
- Brief: started 2026-11-02 00:14 IST. Approved by the user 2026-11-02 00:18 IST ("looks good"); question 1 taken as the recommended answer: OpenAI transcribes, optional Claude cleanup pass.
- Debate round 1: started 2026-11-02 00:20 IST.
- Debate round 1: ended 2026-11-02 00:23 IST (18:50:20Z to 18:53:21Z). Both answered; tree unchanged against the baseline.

### Verified externally before synthesis (2026-11-02)
- OpenAI speech-to-text guide: `gpt-transcribe` is the recommended file model; `whisper-1`, `gpt-4o-transcribe`, `gpt-4o-mini-transcribe` and `-diarize` are deprecated and shut down 2027-02-26. Files up to 25 MB; mp3, mp4, mpeg, mpga, m4a, wav, webm. `gpt-transcribe` takes a prompt plus keywords and languages; `stream=true` streams text for a completed file; live microphone audio uses Realtime with `gpt-live-transcribe`.
- Claude API: no audio input. Checked the features overview, models overview, Messages reference, vision and files pages (no audio input anywhere), plus a web search: the Python SDK feature request for audio input is open; Claude's own voice features use an internal service, reportedly blocked for plain API keys. The user questioned the OpenAI focus at 00:2x; told them this and offered a Claude-only option (on-device recognizer plus Claude name correction). Awaiting their answer.

### Round 1 synthesis
- **a1 (Claude, 74).** Phone holds the key, uploads m4a straight to OpenAI, batch only, no hub/relay/API change. Spike first: ~20 owner-recorded technical phrases through `gpt-transcribe` with the names prompt, fix one model constant, and decide a Claude pass from residual errors. Keeps the recording until the sheet closes and offers Retry; 401/403/quota switches to on-device until the key changes. New level-based silence gate with a 90 s cap. iOS key in a separate keychain item, `WhenUnlockedThisDeviceOnly`; Android AES-GCM Keystore wrapper. Forgetting the hub deletes the key. Key goes only to hard-coded `api.openai.com`. Uploads only after detected speech.
- **a2 (Codex, 85).** Same core: phone key, direct batch upload, `gpt-transcribe`, defer Claude. Differs: no audio retained; after a cloud failure offers an explicitly labelled on-device re-recording or typing. No spike step; measure on physical phones at the end.
- **Agreements:** key on the phone, upload direct to OpenAI; hub, relay, app API and migrations untouched; batch, not streaming; `gpt-transcribe`; the silence rule must change because it keys on transcript changes, not speech; cloud mode needs only the mic permission on iOS; separate key storage from the signing keys; disclosure covers audio and the names list; Claude cleanup deferred until measured.
- **Disputes:** (1) retry the same audio vs re-record; (2) spike before app code vs measure at the end.
- **Verified:** `protocol Listener` at `Voice.swift:131` (brief said 149; corrected). iOS has a Settings screen: `DevicesView.swift:69` sets `.navigationTitle("Settings")` (brief said none; corrected). Keychain items use `AfterFirstUnlockThisDeviceOnly` (`Keys.swift:187`). Android prefs are plain `SharedPreferences` (`AppModel.kt:66`); `allowBackup="false"` (`AndroidManifest.xml:18`). Relay timeout 45 s (`server.rs:31`), body cap checked on ciphertext length (`server.rs:173`). Android vocabulary at `AppModel.kt:315`. iOS forget at `AppModel.swift:148`; the forget dialog promises "This phone's keys are deleted" (`DevicesView.swift:77`).
- **Decisions so far:** phone-held key, direct batch upload to OpenAI with `gpt-transcribe`, no hub change. Pending: the user's answer on a Claude-only option, which changes scope.
- Draft plan written to debate/draft.md. Debate round 2: started 2026-11-02 01:18 IST.
- Debate round 2: ended (19:48:54Z to 19:50:35Z). Both answered; tree unchanged against the baseline.

### Round 2 synthesis
- **a1 (Claude, 80).** Held Retry and the phrase test, with changes: a 10-minute expiry on the kept recording; Retry inside review, not a phase change; the phrase test runs in parallel and gates only the disclosure text and release. Plan check, 16 points: the decision-9 and section-8 ordering contradiction; Retry closing the sheet (the sheet shows only in review, `HomeView.swift:95`, `HomeScreen.kt:87`); the sheet's mic calls the same listen (`SendSheet.swift:80`, `SendSheet.kt:243`); backgrounding is in no step (`SwitchboardApp.swift:19`, `MainActivity.kt:57`); lifecycle tests belong in kit and core; no place for an Android Keystore test; constant URL against MockWebServer; redirects; `ACCESS_NETWORK_STATE` missing; pin the recording format; Done against silent auto-stop; key removal deletes the kept file; keychain survives uninstall; recordings outside the repo; a name check on devices; reference fixes (`AppModel.kt:322`, `:236`).
- **a2 (Codex, 91).** CONSENSUS; conceded both disputes. Added: never auto-retry, since a timeout may already be billed; the end-of-build test also catches capture, encoding and gate problems, so both tests stay; a stronger phrase test (prompt, keywords or both, list sizes, held-out phrases, scoring meaning changes); background cancellation and launch sweep named; Retry UI and edit ownership (`SendSheet.swift:32`, `SendSheet.kt:201`); the overlay renders only while listening (`VoiceOverlay.swift:10`); redirect test; disclosure must name branches, sessions and agents.
- **Verified:** all cited lines checked.
- **Resolutions:**
  - Sheet mic: same mode selection as the home mic (decision 10), not a forced on-device mic. Simpler, and the offline and auth routing already covers the cases a1 raised.
  - Backgrounding mid-upload counts as a retryable failure that keeps the file.
  - Android offline check adds the install-time `ACCESS_NETWORK_STATE`; the stop rule names runtime permissions only.
  - Android key storage gets a manual check.
- No third round: both agree on the decisions, and every correction fits inside them.

## Implement stage
Invocation: `/council implement --plan .agents/tasks/cloud-transcription/plan.md using claude`. `implement` takes no roster, so `using claude` was read as implementer `claude` with its configured model, which is the default.

- Scratch for this stage: `/private/var/folders/k9/5lplp70s46q8x0whbcf77bx40000gn/T/council.GR8kuGjNTl`
- Started 2026-11-03 03:18 IST on main at `5abf48f`. Index empty.
- Blocker asked once: the disclosure text waits on the phrase test. The owner chose to write it now, assuming names are sent. Recorded in the plan as "Decisions before implementation (2026-11-03)". The real-phone check and the phrase test results stay with the owner.
- A policy hook blocks Bash commands that name env-file patterns, so the implementer's deny-rule file was written with the Write tool, and its tab has no OpenCode permission variable, as in the plan stage.
- Implementer ran 21:49:02Z to about 22:54:35Z (66 min). The script wrote `missing` at 22:10Z because the implementer was parked on a background monitor; the watcher waited for the report and `wait` collected it without prompting.
- Ended 2026-11-03 04:48 IST, `outcome: done`, at `99f2067` with 12 commits, not pushed. See `implement/verification.md`. `e2e.sh` fails on the push check at both `99f2067` and `5abf48f`, both run by the orchestrator.

## Final resolution
- **Agreed from round one:** phone-held key, direct batch upload to `gpt-transcribe`, no hub change, a new speech-activity gate, mic-only permission on iOS, separate key storage, Claude deferred.
- **Retry versus re-record:** Codex conceded. Retry is kept, with the expiry, in-review retry and no automatic retry.
- **Phrase test ordering:** resolved as parallel with the build, gating disclosure and release, plus an end-of-build device check.
- **Plan-check points:** all applied in plan.md; none deferred.
- **Limit:** the final plan applies round-two corrections that the agents did not re-check.
- Plan stage ended 2026-11-02 01:22 IST.

## Timing
| Stage | Start | End | Duration |
|---|---|---|---|
| Brief | 00:14 IST | 00:18 IST (approved) | 4 min |
| Round 1 | 18:50:20Z | 18:53:21Z | 3 min |
| Round 2 | 19:48:54Z | 19:50:35Z | 2 min |
| Implementer | 21:49:02Z | about 22:54:35Z | 66 min |
| Implement stage, with verification | 03:18 IST (11-03) | 04:48 IST (11-03) | 90 min |
