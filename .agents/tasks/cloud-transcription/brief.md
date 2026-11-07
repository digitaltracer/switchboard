# Brief: cloud transcription

**Name.** `cloud-transcription`

**Task.** The phone apps turn speech into a prompt with the phone's own recognizer, and the owner finds it poor on both iOS and Android. Decide how the owner can switch voice input to a cloud transcription service by entering their own API key, and plan the build for iOS and Android. Done means: with a key entered, voice mode records, sends the audio to the chosen provider, and puts the text in the same review sheet as today. With no key, or when the provider fails, voice works the way it does now. The owner can see that audio leaves the phone, and can remove the key.

**Repos.** One: `/Users/adarsh/personal/switchboard`, branch `main`, HEAD `5abf48f`, primary.
- **iOS voice:** `apps/ios/Switchboard/Voice.swift`. `Voice` owns the listen, review and silence state (it stops 1.8 s after speech pauses, or after 8 s with nothing heard). It feeds a `Listener` protocol (`Voice.swift:149`), which has three implementations: `OnDeviceListener` (iOS 26 `SpeechAnalyzer`, else `SFSpeechRecognizer` with `requiresOnDeviceRecognition`), and `ScriptedListener` for UI tests (`SB_FAKE_VOICE`). Partial text streams into `heard`. The app sends up to 100 expected names (`AppModel.vocabulary`, `AppModel.swift:202`) as contextual strings. The usage string says "Turns what you say into text on this iPhone. Only the text is sent to your hub." (`project.pbxproj:239`).
- **Android voice:** `apps/android/app/src/main/kotlin/com/digitaltracer/switchboard/Voice.kt`. It has the same phases and silence rule. It uses `SpeechRecognizer.createOnDeviceSpeechRecognizer` only, has no listener abstraction, and refuses when on-device recognition is unavailable. It biases toward the vocabulary on API 33 and up. The manifest has `RECORD_AUDIO` and `INTERNET`.
- **Overlays and settings:** `VoiceOverlay.swift` / `VoiceOverlay.kt` draw the listen UI. Android has `SettingsScreen.kt`; iOS has `DevicesView.swift` and no general settings screen.
- **Phone secret storage today:** iOS keeps Secure Enclave keys in the keychain (`SwitchboardKit/Sources/SwitchboardKit/Keys.swift`). Android keeps its keys in the Keystore (`KeystoreKeys.kt`).
- **Path to the hub:** the phone reaches the hub directly or through the relay, end-to-end encrypted. The relay caps a request body at 2 MB (`src/relay/server.rs:44`) and the hub's response at 4 MB (`src/hub/relay_link.rs:39`). The app API is `src/hub/app/`.

**Binding rules** (`AGENTS.md`):
- Everything stays on the owner's machine. The relay never sees plaintext and stores no user data.
- Hub-side secrets live outside any git tree with mode 0600, and the code refuses paths inside a repo.
- Mask everything that leaves the machine with `src/redact.rs`.
- App API changes are additive within `/app/v1`. Regenerate `api/openapi.json`.
- Migrations are append-only.
- Voice never approves (`debate-native-apps-2.md:278`).
- WhatsApp is frozen.

**Criteria** (the owner gave one: the built-in recognizers are bad). Optimise for, in order:
1. transcription accuracy on technical speech (project, branch and tool names);
2. honest disclosure of where audio goes;
3. keeping the key safe;
4. latency to text in the review sheet;
5. the least new code across two platforms.

**Non-goals.**
- Text-to-speech.
- Voice approvals.
- WhatsApp voice notes.
- Choosing a paid provider on the owner's behalf beyond what the key implies.
- Changing how a reviewed prompt is routed or sent.

**Known facts.**
- Voice was designed so audio never leaves the phone (`debate-native-apps-2.md:276`). This task deliberately relaxes that as an opt-in. It is not a bug fix.
- OpenAI's speech-to-text guide lists `gpt-transcribe`, `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1`, and a streaming `gpt-live-transcribe` (fetched 2026-11-02).
- The Claude API documents no audio input or speech-to-text on any model. The features overview and the Messages reference were checked on 2026-11-02. Claude can only take text, for example to clean up a transcript against the vocabulary.

**Questions.**
1. *(user, answered 2026-11-02)* OpenAI transcribes. An optional Claude pass, off by default, may tidy the transcript against the expected names before it reaches the review sheet. Whether that pass belongs in the first build, and where it runs, is for the agents.
2. *(agents)* Who holds the key and calls the provider? Either the phone does, with the key in the keychain or Keystore and audio sent straight to the provider, or the hub does, with the key on the owner's machine and audio sent through the hub and relay, within the 2 MB body limit. Each option has consequences for pairing several phones, for the relay rule and for latency.
3. *(agents)* Batch upload after the silence stop, or streaming with live partial text? What does each do to the listen UI and the silence rule?
4. *(agents)* When the provider fails or the phone is offline, should voice fall back to on-device, show an error, or keep the audio for a retry?
