# Plan: cloud transcription (draft for round two)

## 1. Outcome

The phone apps can send voice input to OpenAI for transcription in place of the phone's own recognizer. The owner turns this on by saving an OpenAI API key in the app's Settings on that phone. Each phone stores its own key and uploads a finished recording straight to OpenAI's `gpt-transcribe`, and the text lands in the same review sheet as today. The hub, relay, app API and database don't change. With no key, voice behaves exactly as it does now. A Claude cleanup pass is not in this build. A phrase test run before the build compares `gpt-transcribe` alone with `gpt-transcribe` followed by Claude, and a later change adds Claude only if the numbers justify it.

## 2. Current behaviour

Inspected at `5abf48f` on `main`.

- **iOS voice** (`apps/ios/Switchboard/Voice.swift`).
  - `Voice` has three phases: idle, listening and review (`Voice.swift:12-18`). `listen` picks a `Listener` (`Voice.swift:51-52`); the protocol is at `Voice.swift:131`.
  - `OnDeviceListener` uses `SpeechAnalyzer` on iOS 26 and newer, and otherwise `SFSpeechRecognizer` with `requiresOnDeviceRecognition`. `ScriptedListener` plays `SB_FAKE_VOICE` for UI tests.
  - Partial text streams into `heard`. The silence rule (`Voice.swift:73-81`) ends a listen 1.8 s after `heard` last changed, or after 8 s with nothing heard. It keys on transcript changes, not on sound.
  - `stop()` goes straight to review (`Voice.swift:85-97`).
  - Permissions ask for both speech recognition and the microphone (`Voice.swift:178-181`).
- **Android voice** (`apps/android/app/src/main/kotlin/com/digitaltracer/switchboard/Voice.kt`).
  - The same phases and silence rule (`Voice.kt:86-95`), but no listener abstraction. It calls `createOnDeviceSpeechRecognizer` directly (`Voice.kt:72`) and refuses when on-device recognition is unavailable (`Voice.kt:67-71`).
  - It biases toward the expected names on API 33 and up (`Voice.kt:82`).
- **Expected names.** iOS `AppModel.vocabulary` (`AppModel.swift:202`) and Android `AppModel.vocabulary` (`AppModel.kt:315`). Both cover projects and aliases, session labels and branches, and agent names. iOS caps the list at 100.
- **Overlays.** "Listening…" placeholder at `VoiceOverlay.swift:127` and `VoiceOverlay.kt:199`. The review sheet shows `voice.error` (`SendSheet.swift:29`) and has a mic that re-listens and keeps the text (`SendSheet.swift:80`).
- **Settings.** iOS `DevicesView` is titled Settings (`DevicesView.swift:69`) and has Paired devices, Appearance and Hub sections. Android has `SettingsScreen.kt`. Forget-hub promises "This phone's keys are deleted" (`DevicesView.swift:77`) and calls `AppModel.forget` (`AppModel.swift:148`, `AppModel.kt:235`).
- **Secret storage.** iOS keychain items use `AfterFirstUnlockThisDeviceOnly` (`Keys.swift:187`) and hold signing keys only. Android keeps signing keys in the Keystore (`KeystoreKeys.kt`). Its prefs are plain `SharedPreferences` (`AppModel.kt:66`), and backup is off (`AndroidManifest.xml:18`).
- **Disclosure today.** The iOS speech string says "Turns what you say into text on this iPhone. Only the text is sent to your hub." (`project.pbxproj:239`, `:321`). The design record says audio never leaves the phone (`.agents/tasks/debate-native-apps-2.md:276`).
- **Tests.** The iOS UI test sets `SB_FAKE_VOICE` (`SwitchboardUITests.swift:26`). Android core tests have MockWebServer (`apps/android/core/build.gradle.kts:51`). Android has no fake voice.
- **OpenAI** (docs fetched 2026-11-02):
  - `POST /v1/audio/transcriptions`, multipart, with `gpt-transcribe` as the recommended model.
  - Files up to 25 MB, in mp3, mp4, mpeg, mpga, m4a, wav or webm.
  - `gpt-transcribe` takes a prompt plus keywords and languages.
  - `whisper-1`, `gpt-4o-transcribe` and `gpt-4o-mini-transcribe` shut down on 2027-02-26.
- **Claude.** The Claude API takes no audio, so Claude can only rewrite text.

## 3. Decisions

| # | Decision | Why | Strongest rejected alternative |
|---|---|---|---|
| 1 | Each phone holds its own OpenAI key and calls OpenAI directly. The hub, relay, app API and migrations don't change. | It is the fewest components and one network hop. The phone already holds the expected names. No hub secret or key-entry flow is needed. | Hub-held key with audio sent through the relay. It centralises one key, but adds a raw-audio app API route, a hub secret file, a key-entry path, hand-built multipart, and a 45 s relay timeout (`src/relay/server.rs:31`) in the latency path. It is deferred, and wins only if phones must never hold the key. |
| 2 | Batch: record a compressed m4a, stop, upload, show the result. No streaming. | The model sees the whole utterance, which helps accuracy, and there is one request shape on each platform. | Realtime streaming with `gpt-live-transcribe` keeps live partial text. It would need a WebSocket and PCM pipeline twice, for latency, which ranks fourth. Rejected for now. |
| 3 | Model is `gpt-transcribe`, as one constant with no picker. The request carries the expected names as transcription context. | It is the current recommended model, and the older ones shut down on 2027-02-26. Names are the main accuracy problem. | `gpt-4o-transcribe` is deprecated. A model picker is unnecessary surface. |
| 4 | In cloud mode, a speech-activity gate based on loudness ends the listen. It has an adaptive noise floor and a hard cap. On-device mode keeps today's rule unchanged. | Batch produces no partial text, so today's rule (`Voice.swift:73-81`, `Voice.kt:86-95`) would stop at 8 s every time. | Running the on-device recognizer alongside, just for its partials. Android's recognizer owns the mic (`Voice.kt:72`), and phones without on-device recognition would get nothing (`Voice.kt:67`). |
| 5 | Key storage separate from the signing keys. On iOS, its own keychain item, `WhenUnlockedThisDeviceOnly`. On Android, a Keystore AES-GCM key wraps the token, and the ciphertext goes in prefs. Removing the key and forget-hub both delete it. | The existing stores are for signing keys only (`Keys.swift`, `KeystoreKeys.kt`), and Android prefs are plain (`AppModel.kt:66`). Forget-hub already promises the keys are deleted (`DevicesView.swift:77`). | Reusing the signing-key classes would mix semantics. Plain prefs would leave the token readable on rooted or backed-up devices. |
| 6 | Saving the key is the consent. The save screen says that recordings and the list of project names go to OpenAI and are billed to the owner's key. The overlay says "Recording for OpenAI…" and then "Transcribing with OpenAI…". The usage strings and design record are updated. | Honest disclosure is the second criterion. The names list is a new disclosure. | A separate consent toggle would be one more control with the same meaning as having a key. |
| 7 | **Failures.** Offline at listen start uses on-device, with a notice. A failed or timed-out upload opens the review sheet with the error and **Retry**, which resends the same recording. The sheet's mic still listens on-device, and typing still works. A rejected key (401 or 403) or exhausted quota uses on-device until the key is replaced or the app relaunches. The recording is deleted when the sheet closes, on Cancel or Type Instead, and swept at launch. | Making the owner repeat a long utterance after a network blip is exactly the bad experience this task fixes. Retention is bounded by the sheet's lifetime. | No retention, with re-recording on-device. It is simpler and keeps nothing, but forces repetition. *Disputed; see round two.* |
| 8 | No Claude pass in this build. The phrase test measures `gpt-transcribe` alone against `gpt-transcribe` followed by Claude name correction. | Claude needs a second key, a second disclosure and a second failure path, and a rewrite can change the command. Measure first. | (b) either key works, or (c) the on-device recognizer plus Claude. These are deferred until the test shows residual name errors, or until the owner asks. |
| 9 | Run a phrase test before app code: about 20 owner-recorded technical phrases, with and without the names context, with and without Claude cleanup, recording name accuracy and latency. | It fixes the request's context format and settles decision 8 with data. | Measuring only at the end would risk building around a context format that doesn't help. *Disputed; see round two.* |

## 4. Scope and acceptance criteria

**Delivered:**
- an OpenAI key setting on iOS and Android;
- cloud recording and upload;
- the transcribing state;
- the failure paths in decision 7;
- the disclosure text;
- the phrase test results in the task folder.

**Excluded:**
- hub, relay, app API and migrations;
- streaming;
- Claude;
- syncing the key across phones;
- WhatsApp;
- text-to-speech;
- voice approvals.

**Acceptance criteria:**
1. With no key saved, voice on both platforms behaves exactly as at `5abf48f`, and the existing tests pass unchanged.
2. With a key saved, a listen records audio, ends on the speech-activity gate or Done, uploads to `https://api.openai.com/v1/audio/transcriptions` with `gpt-transcribe` and the names context, and puts the text in the review sheet. It never sends anything by itself.
3. The key is never shown after saving, never logged, never sent to the hub, and never sent to any host but `api.openai.com`.
4. Removing the key, or forgetting the hub, deletes it from the keychain or Keystore. The next listen uses on-device.
5. Offline at start: on-device with a notice. A failed upload, or one with no response within 20 s: the sheet shows the error with Retry, which resends the same audio. A 401, 403 or quota error: on-device with a notice until the key changes.
6. Cancel, Type Instead, or backgrounding during the upload aborts it. A late response never overwrites the sheet's text or a newer listen.
7. The audio file is deleted on every terminal path, and swept at launch.
8. A silent listen (no speech detected) uploads nothing.
9. On iOS, cloud mode needs only the microphone permission.
10. The save screen, overlay labels and usage strings say audio and project names go to OpenAI.

## 5. Implementation plan

### Step 1: phrase test (owner-run)
- **Change.** Proposed `.agents/tasks/cloud-transcription/phrase-test/`:
  - a small script that takes a folder of m4a files and an expected-text list;
  - it calls `gpt-transcribe` with no context, with the names context, and with names then a Claude correction pass;
  - it writes name accuracy and latency to `results.md`.
  - The owner records the phrases and runs it with their own keys. The implementer never sees or reads a key.
- **Implements.** Decisions 3, 8 and 9.
- **Invariant.** No key is ever written into the repo. The script reads keys from the environment only.
- **Verified today.** Nothing like this exists.
- **Depends on.** The owner's recordings and keys.
- **Check.** `results.md` exists with the three columns. The context format that won is noted in the plan's As built section.

### Step 2: shared transcription logic, both platforms
- **Change.** Proposed `apps/ios/SwitchboardKit/Sources/SwitchboardKit/Transcription.swift` and `apps/android/core/src/main/kotlin/com/digitaltracer/switchboard/core/Transcription.kt`. Each contains:
  - the request builder (multipart body, model constant, names context bounded to what step 1 settles);
  - response parsing;
  - error classification: offline, timeout, auth, quota or rate, server, malformed;
  - the speech-activity gate as a pure function of loudness samples over time (noise floor, 1.8 s quiet after speech, 8 s with no speech, 90 s cap).
- **Implements.** Decisions 2, 3 and 4.
- **Invariant.** The base URL is a constant. Nothing here touches storage or the hub.
- **Verified today.** No HTTP client to third parties exists on either platform. Android core already depends on OkHttp (`core/build.gradle.kts:45`).
- **Depends on.** Step 1 for the context format. The rest can start without it.
- **Check.** New `TranscriptionTests.swift` in SwitchboardKit and `TranscriptionTest.kt` in core (MockWebServer), covering request shape, each error class and the gate's decisions. Run `cd apps/ios/SwitchboardKit && swift test` and `cd apps/android && ./gradlew :core:test`.

### Step 3: key storage and the Settings section
- **Change.**
  - iOS: a proposed key store with a separate keychain service, `WhenUnlockedThisDeviceOnly`.
  - Android: a proposed Keystore AES-GCM wrapper, with the ciphertext in prefs.
  - A "Voice" section in `DevicesView.swift` and `SettingsScreen.kt`: paste the key, save with the disclosure text, show "Key saved", Remove.
  - `AppModel.forget` on both platforms also deletes the key.
- **Implements.** Decisions 5 and 6.
- **Invariant.** A key that can't be read means on-device. A storage failure disables cloud mode rather than falling back to plaintext.
- **Verified today.** `Keys.swift:187`, `AppModel.kt:66`, `DevicesView.swift:77`, `AppModel.swift:148`, `AppModel.kt:235`.
- **Depends on.** None.
- **Check.** A unit test for the Android wrapper's round trip and removal. Manual: save, relaunch, remove, forget-hub, then confirm the key is gone.

### Step 4: iOS cloud listener and the transcribing phase
- **Change.**
  - A proposed `CloudListener` records m4a with `AVAudioRecorder` and meters loudness into the gate.
  - `Voice` gains a `transcribing` phase between listening and review.
  - Permissions are split so cloud mode asks only for the microphone.
  - Cancel and Type Instead abort the upload and delete the file. A generation counter drops late responses.
  - The failure paths follow decision 7.
  - `listen` picks the cloud listener when a key exists and the phone is online.
- **Implements.** Decisions 2, 4 and 7.
- **Invariant.** No key means the `OnDeviceListener` path is untouched. Every terminal path deletes the audio.
- **Verified today.** `Voice.swift:51-52`, `:73-81`, `:85-97`, `:178-181`.
- **Depends on.** Steps 2 and 3.
- **Check.** A proposed `SB_FAKE_TRANSCRIBE` UI-test hook, beside `SB_FAKE_VOICE`, returns canned text or a canned error. A UI test covers success and Retry.

### Step 5: Android cloud listener and the transcribing phase
- **Change.**
  - Introduce a listener seam in `Voice.kt` so the on-device code moves behind it unchanged.
  - Add a cloud listener using `MediaRecorder` (AAC/m4a, `maxAmplitude` for loudness).
  - The phase, abort, late-response and failure handling match step 4.
- **Implements.** Decisions 2, 4 and 7.
- **Invariant.** As step 4. The no-key path stays byte-for-byte today's logic.
- **Verified today.** `Voice.kt:57-96`, `:67-72`.
- **Depends on.** Steps 2 and 3.
- **Check.** Core tests from step 2, plus a manual run on a physical Android phone.

### Step 6: disclosure text and records
- **Change.**
  - Overlay labels (`VoiceOverlay.swift:127`, `VoiceOverlay.kt:199`).
  - The iOS mic usage string (`project.pbxproj:238`, `:320`). The speech string stays, since that permission is still on-device only.
  - The on-device comments (`Voice.swift:7-8`, `Voice.kt:22-23`).
  - `PrivacyInfo.xcprivacy` collected data types (audio, sent to a third party at the user's choice).
  - Store privacy forms, as a release note for the owner.
  - An As built note under `debate-native-apps-2.md:276`.
- **Implements.** Decision 6.
- **Invariant.** No text claims audio stays on the phone while cloud mode is possible.
- **Verified today.** The lines cited.
- **Depends on.** Steps 4 and 5.
- **Check.** Grep for "on this iPhone" and "never leaves" and confirm each remaining hit is still true.

### Step 7: physical-device verification
- **Change.** None. A checklist on a real iPhone and Android phone with a throwaway key: normal use, airplane mode, bad key, cancel mid-upload, backgrounding, remove key, silent listen.
- **Implements.** All the acceptance criteria.
- **Depends on.** Steps 1 to 6.
- **Check.** Results recorded in the As built section. `git diff --stat 5abf48f -- src/ api/ migrations/` is empty.

## 6. Testing strategy
- Criteria 1 and 9: the existing iOS and Android test suites, plus a UI test with no key.
- Criteria 2 and 8: step 2 unit tests, the step 4 UI test, and step 7.
- Criteria 3, 4 and 10: the step 3 tests, the step 6 grep, and code review for any logging of the key.
- Criteria 5, 6 and 7: step 2 error classes, the step 4 UI test (Retry), and step 7.
- Commands, verified: `cd apps/ios/SwitchboardKit && swift test`, `apps/ios/scripts/e2e.sh`. Proposed and unverified: `cd apps/android && ./gradlew :core:test`.

## 7. Constraints and open questions
- **Settled.** No hub change (decision 1). OpenAI only (decision 8, the owner's choice of the recommended option).
- **Open (non-blocking).**
  - Whether the gate's thresholds hold in noisy rooms. Tuned in step 7.
  - Whether App Review needs an extra consent screen for third-party AI data sharing. Checked before release.
- **Disputed, to settle in round two.** Decision 7's Retry versus re-recording, and decision 9's test-first ordering.

## 8. For the implementer
Re-verify that `main` still matches `5abf48f` in the files above. Order: step 2 and step 3 in parallel with the owner's step 1, then steps 4 and 5, then 6, then 7. End each step with its Check and a commit. Stop if anything would touch `src/`, `api/` or `migrations/`, or if a platform API needs more than the microphone permission.

## 9. Verification status
`main` at `5abf48f` inspected. Every file reference above was checked against that revision. The OpenAI facts are from the docs fetched on 2026-11-02, and the Claude facts are from Anthropic's docs and help pages. This draft has not yet been checked by the agents.
