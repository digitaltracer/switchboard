# Plan: cloud transcription

## 1. Outcome

The phone apps can send voice input to OpenAI for transcription in place of the phone's own recognizer, which the owner finds poor on both iOS and Android. The owner turns it on by saving an OpenAI API key in Settings on each phone. Each phone stores its own key and uploads a finished recording straight to OpenAI's `gpt-transcribe`, along with the owner's project, branch, session and agent names so they come out spelled right. The text lands in the same review sheet as today, and nothing is ever sent without the owner's review.

The hub, relay, app API and database don't change. With no key, voice behaves exactly as it does now. If an upload fails, the owner taps Retry instead of saying it again.

Claude is not in this build, because the Claude API accepts no audio and can only rewrite text. A phrase test, run with the owner's own recordings alongside the build, measures whether a Claude cleanup pass after OpenAI improves the names enough to be worth a second key. The same test decides how the names are sent to OpenAI.

## 2. Current behaviour

Inspected at `5abf48f` on `main`, the only repo.

- **iOS voice** (`apps/ios/Switchboard/Voice.swift`).
  - `Voice` has three phases: idle, listening and review (`Voice.swift:12`). `listen` picks a `Listener` (`Voice.swift:51`); the protocol is at `Voice.swift:131`.
  - `OnDeviceListener` uses `SpeechAnalyzer` on iOS 26 and newer, and otherwise `SFSpeechRecognizer` with `requiresOnDeviceRecognition`. `ScriptedListener` plays `SB_FAKE_VOICE` for UI tests (`SwitchboardUITests.swift:26`).
  - Partial text streams into `heard`. The silence rule (`Voice.swift:74-81`) ends a listen 1.8 s after `heard` last changed, or 8 s after the listen started if nothing was heard. It keys on transcript changes, not on sound.
  - `stop()` goes to review, or back to idle when nothing was heard and there's no error (`Voice.swift:85-97`).
  - Permissions ask for both speech recognition and the microphone (`Voice.swift:178`).
- **Android voice** (`apps/android/app/src/main/kotlin/com/digitaltracer/switchboard/Voice.kt`).
  - The same phases and silence rule (`Voice.kt:87-95`), but no listener abstraction. It calls `createOnDeviceSpeechRecognizer` directly (`Voice.kt:72`) and refuses when on-device recognition is unavailable (`Voice.kt:67-71`).
  - It biases toward the expected names on API 33 and up (`Voice.kt:82`).
- **Expected names.** iOS `AppModel.vocabulary` (`AppModel.swift:202`) and Android `AppModel.vocabulary` (`AppModel.kt:315`) hold project ids and aliases, session labels and branches, and agent names, plus a few fixed words. Both cap the list at 100 (`AppModel.kt:322`).
- **Review sheet.** It is shown only while the phase is review: iOS `HomeView.swift:94-97`, Android `HomeScreen.kt:87`. Its text field edits `voice.text` directly (`SendSheet.swift:32`, `SendSheet.kt:201`). It shows `voice.error` (`SendSheet.swift:29`). Its mic calls the same `listen` with `keepText` (`SendSheet.swift:80`, `SendSheet.kt:243`). Every exit calls `voice.finish()` (`SendSheet.swift:69`, `:229`, `:261`, and `HomeView.swift:96` on dismiss).
- **Overlay.** The iOS listen panel renders only while listening (`VoiceOverlay.swift:10`). Both platforms show "Listening…" as the placeholder (`VoiceOverlay.swift:127`, `VoiceOverlay.kt:199`).
- **App lifecycle.** iOS forwards scene changes to the model (`SwitchboardApp.swift:19-21`). Android does the same from `MainActivity.onStop` (`MainActivity.kt:57`). Neither touches voice.
- **Settings.** iOS `DevicesView` is titled Settings (`DevicesView.swift:69`) and has Paired devices, Appearance and Hub sections. Android has `SettingsScreen.kt`. Forget-hub promises "This phone's keys are deleted" (`DevicesView.swift:77`) and calls `forget` (`AppModel.swift:148`, `AppModel.kt:236`).
- **Secret storage.** iOS keychain items use `AfterFirstUnlockThisDeviceOnly` (`Keys.swift:187`) and hold signing keys only. Android keeps signing keys in the Keystore (`KeystoreKeys.kt`). Its prefs are plain `SharedPreferences` (`AppModel.kt:66`), and backup is off (`AndroidManifest.xml:18`).
- **Android permissions.** `INTERNET`, `USE_BIOMETRIC`, `RECORD_AUDIO` and `POST_NOTIFICATIONS` (`AndroidManifest.xml:4-7`). There's no `ACCESS_NETWORK_STATE`.
- **HTTP.** Android core uses OkHttp (`apps/android/core/build.gradle.kts:45`). Its hub client has a 40 s read timeout tuned for long-polls (`Hub.kt:217`). Core tests have MockWebServer (`core/build.gradle.kts:51`). The Android app module has `debug` and `main` source sets only, so no instrumented tests.
- **Disclosure today.** The iOS speech string says "Turns what you say into text on this iPhone. Only the text is sent to your hub." (`project.pbxproj:239`, `:321`). The mic string is at `:238` and `:320`. The design record says audio never leaves the phone (`.agents/tasks/debate-native-apps-2.md:276`).
- **OpenAI** (docs fetched 2026-11-02):
  - `POST /v1/audio/transcriptions`, multipart, with `gpt-transcribe` as the recommended model.
  - Files up to 25 MB, in mp3, mp4, mpeg, mpga, m4a, wav or webm.
  - `gpt-transcribe` takes a prompt plus keywords and languages.
  - `whisper-1`, `gpt-4o-transcribe` and `gpt-4o-mini-transcribe` shut down on 2027-02-26.
  - Live microphone transcription is a separate Realtime service (`gpt-live-transcribe`).
- **Claude.** The Claude API takes no audio. The Claude app's dictation runs on Anthropic's servers through a service the API doesn't offer.

## 3. Decisions

| # | Decision | Why | Strongest rejected alternative |
|---|---|---|---|
| 1 | Each phone holds its own OpenAI key and calls OpenAI directly. The hub, relay, app API and migrations don't change. | It is the fewest components and one network hop, and the phone already holds the names. No hub secret or key-entry flow is needed. | Hub-held key with audio through the relay. It centralises one key, but adds a raw-audio app API route, a hub secret file, a key-entry path and the relay's 45 s timeout (`src/relay/server.rs:31`). It is deferred, and wins only if phones must never hold the key. |
| 2 | Batch: record a compressed m4a, stop, upload, show the result. No streaming. | The model hears the whole utterance, and there is one request shape per platform. | Realtime streaming with `gpt-live-transcribe` keeps live text, but needs a WebSocket and PCM pipeline on both platforms, for latency, which ranks fourth. |
| 3 | `gpt-transcribe` as one constant with no picker. Recording format pinned in one place: AAC in m4a, mono, 16 kHz, 32 kbit/s (proposed values; step 1 confirms them). The request carries the expected names in the form the phrase test picks. Until then, the documented keywords field. | It is the current recommended model, and the others shut down 2027-02-26. Pinning the format makes the phrase test and the size and duration caps meaningful. | `gpt-4o-transcribe` is deprecated. A model picker is unnecessary surface. Unpinned recorder defaults would make the test results unrepresentative. |
| 4 | In cloud mode, a speech-activity gate based on loudness ends the listen. It has an adaptive noise floor: stop 1.8 s after speech goes quiet, 8 s with no speech, and a hard cap of 90 s. An auto-stop with no detected speech uploads nothing. An explicit Done always uploads. On-device mode keeps today's rule unchanged. | Batch produces no partial text, so today's rule would stop at 8 s every time. | Running the on-device recognizer alongside, just for its partials. Android's recognizer owns the mic (`Voice.kt:72`), and phones without on-device recognition would get nothing (`Voice.kt:67`). |
| 5 | Key storage separate from the signing keys. On iOS, its own keychain item, `WhenUnlockedThisDeviceOnly`, cleared on the first launch after an install (a UserDefaults flag), because keychain items can outlive the app. On Android, a Keystore AES-GCM key wraps the token, and the ciphertext goes in prefs. Removing the key and forget-hub both delete it, and any kept recording. | The existing stores are for signing keys (`Keys.swift`, `KeystoreKeys.kt`), Android prefs are plain (`AppModel.kt:66`), and forget-hub promises the keys are deleted (`DevicesView.swift:77`). | Reusing the signing-key classes mixes semantics. Plain prefs leave the token readable from a rooted device or a backup. |
| 6 | Saving the key is the consent. The save screen says that recordings, plus the project, branch, session and agent names, go to OpenAI and are billed to the owner's key. The overlay says "Recording for OpenAI…" and then "Transcribing with OpenAI…". The usage strings, privacy manifest and design record are updated. If the phrase test shows names don't help, names aren't sent, and the text says so. | Honest disclosure is the second criterion, and the names list is a new disclosure that matches what `vocabulary` actually holds (`AppModel.swift:202-206`). | A separate consent toggle would be one more control meaning the same as having a key. |
| 7 | The key goes only to `https://api.openai.com`. Redirects are disabled on the provider client. Each platform uses its own client for this, not the hub's. The base URL is injectable only in tests. | A constant URL alone doesn't stop a redirect from carrying the key elsewhere. The hub client's long-poll timeout (`Hub.kt:217`) is wrong for uploads. | Reusing the hub client would mean wrong timeouts and shared configuration. |
| 8 | **Failures and Retry.** Offline at listen start (iOS path monitor; Android `ConnectivityManager`, which needs the install-time `ACCESS_NETWORK_STATE` permission): on-device, with a notice. Upload error, no response within 20 s, or the app going to the background mid-upload: the review sheet opens with the error and **Retry**. Retry resends the same recording only when tapped, never automatically, because a timed-out request may already have been billed. A 401, 403 or quota error: on-device, with a notice, until the key is replaced or the app relaunches. | Making the owner repeat a long technical instruction after a network blip is the pain this task fixes. Where on-device recognition is refused (`Voice.kt:67-71`), Retry is the only voice path left. | Keep nothing and re-record on-device. It is simpler, but forces repetition into the recognizer the owner rejected. |
| 9 | **Kept recording.** The failed recording is kept while the sheet is open, for at most 10 minutes. It is deleted by any sheet exit (`finish()`), a new listen, removing the key, forget-hub, or the expiry, and swept at every launch. It lives in the app's temporary storage, excluded from backup. Retry happens inside the review phase: a retrying flag disables Send, and the result is appended to the text, never replacing what the owner typed. | Every sheet exit already reaches `finish()`. A phase change would close the sheet, because it only shows in review (`HomeView.swift:95`, `HomeScreen.kt:87`). The expiry bounds a sheet left open for hours. | Bounding retention by the sheet's lifetime alone, which can be indefinite. |
| 10 | The sheet's mic uses the same mode selection as the home mic: cloud with a usable key while online, otherwise on-device. | One rule, and no new `listen` parameter. Offline and rejected-key cases already route to on-device. | A forced on-device mic in the sheet. It would add a parameter and contradict the reason for the feature. |
| 11 | No Claude pass in this build. The phrase test runs alongside the build and decides how names are sent and whether Claude is worth adding later. Its results are needed before the disclosure text (step 6) and release, not before the other app code. | Docs give a default format but can't say whether names help or cause false matches on the owner's speech. If they don't help, names stop leaving the phone. The test also settles the Claude question with data. | Testing only at the end catches capture and gate problems, but not the context choice. The end-of-build device check (step 7) covers those, so both tests stay. |

## 4. Scope and acceptance criteria

**Delivered:**
- an OpenAI key setting on iOS and Android;
- cloud recording, the speech-activity gate and upload;
- Retry with a kept recording;
- the offline, auth and timeout paths;
- disclosure text and records;
- phrase test results in the task folder.

**Excluded:**
- hub, relay, app API and migrations;
- streaming;
- Claude in the app;
- syncing the key across phones;
- WhatsApp;
- text-to-speech;
- voice approvals.

**Acceptance criteria:**
1. With no key saved, voice on both platforms behaves exactly as at `5abf48f`, and the existing tests pass unchanged.
2. With a key saved and the phone online, a listen records in the pinned format, ends on the gate or Done, uploads to `https://api.openai.com/v1/audio/transcriptions` with `gpt-transcribe` and the names context chosen in step 1, and appends the text in the review sheet. Nothing is ever sent without the owner's review.
3. The key is never shown after saving, never logged, never sent to the hub, and never sent to any host but `api.openai.com`. A redirect response from the provider is not followed.
4. Removing the key, or forget-hub, deletes the key and any kept recording, and the next listen uses on-device. After a reinstall on iOS, no key is present.
5. Offline at listen start: on-device, with a notice. An upload error, no response within 20 s, or backgrounding mid-upload: the sheet shows the error and Retry, which resends the same recording only when tapped. A 401, 403 or quota error: on-device, with a notice, until the key is replaced or the app relaunches.
6. While Retry runs, Send is disabled and the owner's typed text is kept, with the result appended. Double Retry, dismissing the sheet, or starting a new listen during Retry discards the in-flight response.
7. The recording is deleted on every exit: success, sheet exit, new listen, key removal, forget-hub, the 10-minute expiry. Anything left by a crash is swept at the next launch.
8. An auto-stop with no detected speech uploads nothing and returns to idle. An explicit Done uploads what was recorded. An empty transcript behaves like today's empty listen (`Voice.swift:96`).
9. On iOS, cloud mode needs only the microphone permission.
10. The save screen, overlay labels, usage strings and privacy manifest say that audio, and names if sent, go to OpenAI.
11. On physical phones, technical phrases containing the owner's project and branch names come out with the names right at least as often as in the phrase test.

## 5. Implementation plan

### Step 1: phrase test (owner-run, alongside steps 2 to 5)
- **Change.**
  - Proposed: a script under `.agents/tasks/cloud-transcription/phrase-test/` and a `results.md` there.
  - The owner records about 25 technical phrases. They must include project, branch and agent names, and five are held out for checking the winner.
  - The recordings live outside the repo, in a folder the owner passes to the script.
  - The script converts each recording to the pinned format (decision 3), then calls `gpt-transcribe` with:
    - no context;
    - the names as prompt, as keywords, and as both;
    - the full list against the 30 most relevant names;
    - the best of those followed by a Claude correction pass.
  - It scores three things: exact names right, words dropped, and changes in command meaning. It also records latency.
- **Implements.** Decisions 3, 6 and 11.
- **Invariant.** Keys are read from the environment only, and never written into the repo or results. Recordings never enter the repo.
- **Verified today.** Nothing like this exists.
- **Depends on.** The owner's recordings and keys. The implementer writes the script but never runs it with a key.
- **Check.** `results.md` contains the scores per variant and on the held-out phrases. The chosen context format, and the decisions on names and on Claude, are written in this plan's As built section.

### Step 2: shared transcription logic, both platforms
- **Change.** Proposed `apps/ios/SwitchboardKit/Sources/SwitchboardKit/Transcription.swift` and `apps/android/core/src/main/kotlin/com/digitaltracer/switchboard/core/Transcription.kt`. Each contains:
  - the pinned recording format;
  - the multipart request builder, with the model constant and the names context bounded;
  - response parsing;
  - error classification: offline, timeout, auth, quota or rate, server, malformed;
  - a provider client with redirects disabled and a base URL injectable only in tests;
  - the speech-activity gate, as a pure function of loudness samples over time;
  - the recording lifecycle as a small state machine with a fake transport and a fake clock: kept-file rules, the 10-minute expiry, generation counter, delete on every exit.
- **Implements.** Decisions 2, 3, 4, 7 and 9.
- **Invariant.** Production code can't change the base URL. Nothing here touches the hub or key storage.
- **Verified today.** No third-party HTTP exists on either platform. Android core already has OkHttp and MockWebServer (`core/build.gradle.kts:45`, `:51`).
- **Depends on.** None. Step 1 later fills in the context format.
- **Check.** Proposed `TranscriptionTests.swift` in SwitchboardKit and `TranscriptionTest.kt` in core. They cover the request shape, each error class, a redirect not followed, the gate's decisions (speech then quiet, no speech, the cap), and every lifecycle exit deleting the file, including expiry and double Retry. Commands: `cd apps/ios/SwitchboardKit && swift test`, and proposed `cd apps/android && ./gradlew :core:test`.

### Step 3: key storage and the Settings section
- **Change.**
  - iOS: a proposed key store with a separate keychain service, `WhenUnlockedThisDeviceOnly`, and the first-launch clear.
  - Android: a proposed Keystore AES-GCM wrapper, with the ciphertext in prefs.
  - A "Voice" section in `DevicesView.swift` and `SettingsScreen.kt`: paste the key, save under the disclosure text, show "Key saved", Remove.
  - `forget` on both platforms (`AppModel.swift:148`, `AppModel.kt:236`) also deletes the key.
  - Removing the key also deletes a kept recording.
- **Implements.** Decisions 5 and 6.
- **Invariant.** A key that can't be read means on-device. A storage failure disables cloud mode, never falling back to plaintext.
- **Verified today.** `Keys.swift:187`, `AppModel.kt:66`, `DevicesView.swift:77`.
- **Depends on.** None. The disclosure wording is finalised in step 6.
- **Check.** The Android Keystore can't run on the JVM, and there's no instrumented test set, so a manual check on a phone: save, relaunch, remove, save, forget-hub; confirm cloud mode is off after each removal. iOS: the same, plus delete and reinstall the app and confirm no key is present.

### Step 4: iOS cloud listener and Retry
- **Change.**
  - A proposed `CloudListener` records in the pinned format with `AVAudioRecorder`, metering loudness into the gate.
  - `Voice` gains a transcribing state between listen and review. The overlay (`VoiceOverlay.swift:10`) also renders while transcribing, with Cancel.
  - Mode selection: cloud with a usable key and a network path, otherwise on-device. It applies to the home mic and the sheet's mic alike.
  - Permissions are split so cloud mode asks only for the microphone.
  - `SendSheet.swift` shows Retry with the error and disables Send while retrying. The result is appended.
  - `SwitchboardApp.swift:19-21` cancels an in-flight upload on leaving the foreground, keeping the file for Retry. The launch sweep runs at start.
  - All of this goes through the step 2 lifecycle.
- **Implements.** Decisions 2, 4, 8, 9 and 10.
- **Invariant.** No key means the `OnDeviceListener` path is untouched. Every exit deletes the file through the lifecycle.
- **Verified today.** `Voice.swift:51`, `:74-81`, `:85-97`, `:178`; `HomeView.swift:94-97`; `SendSheet.swift:29-32`, `:80`.
- **Depends on.** Steps 2 and 3.
- **Check.** A proposed `SB_FAKE_TRANSCRIBE` hook beside `SB_FAKE_VOICE` returns canned text or a canned error. A UI test covers success, then an error with Retry, then typing during Retry with the text kept. Run `apps/ios/scripts/e2e.sh`.

### Step 5: Android cloud listener and Retry
- **Change.**
  - Introduce a listener seam in `Voice.kt` so the on-device code moves behind it unchanged.
  - Add a cloud listener on `MediaRecorder`, in the pinned format, with loudness from `maxAmplitude`.
  - The transcribing state, mode selection, Retry in `SendSheet.kt`, and the backgrounding hook in `MainActivity.kt:57` match step 4.
  - Add `ACCESS_NETWORK_STATE` to the manifest (install-time, no prompt).
- **Implements.** Decisions 2, 4, 8, 9 and 10.
- **Invariant.** As step 4. The no-key path keeps today's logic.
- **Verified today.** `Voice.kt:57-96`, `:67-72`; `HomeScreen.kt:87`; `SendSheet.kt:201`, `:243`; `AndroidManifest.xml:4-7`.
- **Depends on.** Steps 2 and 3.
- **Check.** The step 2 core tests, plus the step 7 checklist on a physical Android phone. Android has no UI test harness.

### Step 6: disclosure text and records
- **Change.**
  - The save-screen text, final per step 1's decision on names.
  - Overlay labels (`VoiceOverlay.swift:127`, `VoiceOverlay.kt:199`).
  - The iOS mic usage string (`project.pbxproj:238`, `:320`). The speech string stays, since that permission remains on-device only.
  - The on-device comments (`Voice.swift:7-8`, `Voice.kt:22-23`).
  - `PrivacyInfo.xcprivacy` collected data types: audio, sent to a third party at the user's choice.
  - A release note for the owner about the App Store and Play privacy forms, and whether App Review wants extra consent for third-party AI.
  - An As built note under `debate-native-apps-2.md:276`.
- **Implements.** Decision 6.
- **Invariant.** No text claims that audio stays on the phone while cloud mode is possible.
- **Verified today.** The lines cited.
- **Depends on.** Steps 1, 4 and 5.
- **Check.** Run `grep -rn "on this iPhone\|never leaves\|only the text" apps/` and confirm each remaining hit is still true.

### Step 7: physical-device verification
- **Change.** None. A checklist on a real iPhone and a real Android phone, with a throwaway key:
  - normal use, with name-heavy phrases (criterion 11);
  - airplane mode at listen start;
  - airplane mode mid-upload, then Retry;
  - a bad key;
  - Cancel mid-upload;
  - backgrounding mid-upload, then return and Retry;
  - Retry after the 10-minute expiry;
  - typing during Retry;
  - removing the key;
  - a silent listen;
  - a noisy room;
  - force-quit with a kept recording, then relaunch.
- **Implements.** All acceptance criteria. It catches capture, encoding and gate-calibration problems that step 1 can't.
- **Depends on.** Steps 1 to 6.
- **Check.** Results go in the As built section. `git diff --stat 5abf48f -- src/ api/ migrations/` is empty.

## 6. Testing strategy

| Criterion | Checks |
|---|---|
| 1 | Existing suites unchanged (`swift test`, `e2e.sh`, `:core:test`). Step 7 with no key. |
| 2 | Step 2 request tests, step 4 UI test, step 7. |
| 3 | Step 2 redirect test. Review of every log call on the new paths. Step 3 manual check. |
| 4 | Step 3 manual checks, including the iOS reinstall. Step 2 lifecycle test for the kept recording. |
| 5, 6 | Step 2 error-class and lifecycle tests. Step 4 UI test (Retry, typing during Retry). Step 7. |
| 7 | Step 2 lifecycle tests for every exit and the expiry. Step 7 force-quit case. |
| 8 | Step 2 gate tests. Step 7 silent listen. |
| 9 | Step 7 on a fresh iOS install: only the mic prompt appears in cloud mode. |
| 10 | Step 6 grep and a read-through. |
| 11 | Step 1 results against step 7's name-heavy phrases. |

Verified commands: `cd apps/ios/SwitchboardKit && swift test` and `apps/ios/scripts/e2e.sh`. Proposed and unverified: `cd apps/android && ./gradlew :core:test`. Step 1 and step 7 need the owner, real phones and a key.

## 7. Constraints and open questions

- **Settled.**
  - No hub, relay, API or migration change.
  - OpenAI only in this build; this was the owner's choice of the recommended option, after learning the Claude API has no audio input.
  - Voice never approves.
- **Implementation discretion.** Type and file names. Exact loudness thresholds, tuned in step 7. Layout of the Settings section and the Retry control.
- **Non-blocking assumptions.**
  - iOS keychain items can survive an uninstall. This is from memory; the first-launch clear is harmless if they don't. Affects step 3.
  - A 20 s upload timeout suits a 90 s recording at 32 kbit/s, about 360 KB. Revisit if step 7 shows otherwise. Affects steps 4 and 5.
  - App Review may want an explicit third-party AI consent screen. Checked at release. Affects step 6.
- **Blockers.** None for steps 2 to 5. Step 6 and release wait on step 1's results, which need the owner.

## 8. For the implementer

- **Before starting.** Confirm `main` still matches `5abf48f` in the files cited in section 2. If a cited line has moved, find the same code. If the behaviour differs, stop and report.
- **Order.** Steps 2 and 3, then 4 and 5. Write step 1's script early so the owner can run it while you build. Then step 6 once step 1's results are in, then step 7 with the owner.
- **Per step.** End each step with its Check and a commit.
- **As built.** Record deviations and step 1's outcome in a dated "As built" section at the end of this document.
- **Stop and ask if:**
  - anything would touch `src/`, `api/` or `migrations/`;
  - a platform API needs a runtime permission beyond the microphone;
  - a key would have to be read, printed or committed;
  - the no-key path would have to change.
- **Resolve alone.** Naming, layout and test structure.

## 9. Verification status

- Inspected `main` at `5abf48f`. Every file reference in this plan was checked against that revision.
- The OpenAI facts come from OpenAI's speech-to-text guide, fetched on 2026-11-02. The Claude facts come from Anthropic's API docs and help-centre pages, fetched the same day.
- Both debaters checked the draft. This final version applies their corrections, all of which were verified against the code, and was not sent back for a further check.

## Decisions before implementation (2026-11-03)

The phrase test needs the owner's recordings and keys, so its results aren't available yet. The owner decided:

- **Disclosure text is written now, not after the phrase test.** This changes step 6's dependency: it depends on steps 4 and 5 only. The text assumes the expected names are sent to OpenAI, which is what the build does by default through the documented keywords field (decision 3). This keeps the tree from ever claiming that audio stays on the phone while cloud mode exists. If the phrase test later shows names don't help, the names field and that wording change in a follow-up.
- **Step 1:** the implementer writes the script and the `results.md` template, and commits them under `.agents/tasks/cloud-transcription/phrase-test/`. The owner runs it later. Its results and the decisions that depend on them stay open.
- **Step 7 is not built in this stage.** It needs the owner, real phones and a key, and so do the manual checks in steps 3 and 5. These are listed as left in the As built section. Release still waits on the phrase test and step 7.

## As built (2026-11-03)

**Steps**
- Step 1: script and `results.md` template are in `phrase-test/`, with a README and a sample manifest. It was checked offline with synthetic audio and no keys, and never run with a key.
- Step 2: `Transcription.swift` and `Transcription.kt` are in, each with tests. `swift test` passes 70 tests and `:core:test` passes 79. `./gradlew :core:test` is the right Android command.
- Step 3: key stores and the Voice settings section are in, and forget-hub deletes the key. Both apps compile. The manual phone check wasn't run. A simulator UI test covers saving and removing a key on iOS.
- Step 4: iOS recording, the transcribing state, mode selection, Retry and the background hook are in. The new UI flow passes: success, a failure with Retry, typing kept during Retry.
- Step 5: Android listener seam, recording, Retry and the background hook are in, and `ACCESS_NETWORK_STATE` is added. `:app:assembleDebug` and `:app:lintDebug` report no errors. The on-device code moved behind the seam unchanged.
- Step 6: labels, usage string, comments, privacy manifest and records are in, with wording that assumes names are sent. The save-screen text from step 3 was already final. In the grep check, each remaining hit is true: the speech-permission string and the comments on the on-device listeners and the key store.

**`e2e.sh` does not pass, at this revision or at `5abf48f`.** It stops at "the push reached the simulator". I ran the unchanged tests at `5abf48f` in a throwaway worktree and they fail on the same assertion, so this work didn't cause it. I didn't investigate why. Everything before that assertion passes, including the new cloud flow. Later runs used a scratch copy of the script that keeps its log and waits less for xcodebuild's hang after a failure.

**Assumptions**
- Wire format, read from OpenAI's docs on 2026-11-03 because the plan doesn't give it: repeated `keywords[]` fields. The API refuses a whole request for a keyword with `<`, `>` or a line break, so those names are left out. At most 100 names of at most 100 characters are sent.
- A 429 without `insufficient_quota` counts as a rate limit, which goes to Retry. Only 401, 403 and `insufficient_quota` move voice to on-device.
- A 401, 403 or quota error deletes the recording, since Retry can't help. The sheet says so.
- The 10-minute expiry runs from the first failure, and a Retry doesn't extend it.
- Type instead during a cloud listen discards the recording without uploading it. Only Done and the gate upload.
- While uploading, a tap outside the panel does nothing, so only Cancel, or Back on Android, loses the recording. Cancel clears the text, as it does today.
- An empty transcript from Retry with no text in the field closes the sheet, like today's empty listen.
- Removing the key during a cloud listen or upload cancels it, so nothing uploads with the removed key.
- Gate values, to tune in step 7: speech is 12 dB over the floor for 0.2 s, still talking is 6 dB over the floor, the floor starts at or below -45 dB and rises 1 dB/s, and nothing under -55 dB counts as speech.
- iOS reuses the existing audio session setup. Android records from `VOICE_RECOGNITION`.
- iOS treats only `.background` as leaving the foreground, so Face ID or Control Center don't stop an upload. Android ignores the stop caused by a rotation.
- The phrase test's Claude pass defaults to `claude-opus-5-5`. `--claude-model` changes it.

**Deviations**
- The privacy manifest also declares other user content for the names, because criterion 10 asks for names as well as audio. The manifest format has no field for a third party, so that goes in the store forms.
- The release note is in `apps/ios/README.md`, since only this plan and `phrase-test/` may be committed in this folder.
- Two error classes beyond the plan's: interrupted, for backgrounding, and unreadable, for a recording that's gone.
- The cloud UI flow relaunches the app before the push check, because a failure at that check ends the test. Run after it, the flow wouldn't run here at all.

**Rare cases not built**
- Backgrounding during cloud recording, as opposed to upload, has no hook. The gate ends the listen on return, and an upload then goes through the normal paths.
- A kept recording can't be retried with a replaced key, but Settings can't be reached while the sheet is open.
- After a crash, a kept recording stays on disk until the next launch sweeps it, which can exceed 10 minutes.

**Left**
- Step 1's results, and the three decisions that depend on them: context format, whether names are sent, and whether Claude is worth a second key. The owner runs the script. If names don't help, the names field and the disclosure wording change in a follow-up.
- The manual phone checks in steps 3 and 5.
- Step 7, not built. `git diff --stat 5abf48f -- src/ api/ migrations/` is empty today.
- The push check failure in `e2e.sh`.
- Release waits on all of these, and on the store privacy forms in the release note.

## As built (2026-11-11)

**Verified on `a00618b`**, after about 28 later commits. Nothing in them broke cloud voice.
- `swift test`: 76 tests pass.
- `./gradlew :core:test :app:assembleDebug :app:lintDebug`: 87 core tests pass, on a forced rerun, and the build succeeds with no lint errors. No Java is on `PATH`, so Gradle needs `JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home`. Android Studio's bundled JDK is 25, and the toolchain wants 17.
- `cargo test`: everything passes.

**Fixed: the push check in `e2e.sh`** (`8bdf00c`, test-only).
- The cause was in the UI test, not in Apple's tooling. "New device paired" did reach the simulator, about two minutes before the check.
- The scripted runner's two "Approval needed" pushes then arrived, and iOS stacked all three into one group. Only the newest is in the accessibility tree until the stack is expanded, so the label lookup never matched. This is also why it failed at `5abf48f`.
- The check now taps the app's grouped stack when the note isn't visible. The unchanged `apps/ios/scripts/e2e.sh` passes end to end, both UI tests included, with the cloud voice flow and the push check.

**iPhone.** A Debug build of `8bdf00c` signed with the existing settings (Apple Development, the team provisioning profile, team `N3B962X7RX`) and was installed on the iPhone 11 with `devicectl`. It's a development build: `aps-environment` is development, and the app reports a sandbox token. No phone checks have run on it yet.

**Owner checklist.** `owner-checklist.md` lists the steps only the owner can do: the phrase test, the key checks from steps 3 and 5, the step 7 checks, and the store privacy forms.

**Left**
- Everything in `owner-checklist.md`: the phrase test and its three decisions, the manual key checks, step 7 on the iPhone, and on Android once a phone is connected (none was on 2026-11-11), and the store forms.
- The cloud transcription commits (`cb4a235` to `99f2067`) and this pass leave `src/`, `api/` and `migrations/` untouched: `git diff --stat cb4a235^ 99f2067 -- src/ api/ migrations/` is empty. Diffing against `5abf48f` no longer shows that, because later work changed `src/`.
- Release still waits on these.
