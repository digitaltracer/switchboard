⏺ ROUND 1 cloud-transc-a1

  Position. The phone holds the key and uploads directly to OpenAI. Build batch only: record compressed audio, end on a level-based silence
  rule, upload, and put the text in the existing review sheet. The hub, relay and app API don't change. A Claude pass stays out of the first
  build unless a measured test shows names are still wrong. On failure, keep the recording until the sheet closes and offer Retry, and
  later listens can use on-device. Nothing substitutes text silently, and nothing is retained after the review ends.

  Reasoning.
  - Accuracy: key location doesn't affect it, and batch lets the model see the whole utterance. The app already has the names on the phone
    (apps/ios/Switchboard/AppModel.swift:202, apps/android/.../AppModel.kt:315), so they can go in OpenAI's prompt field. From memory,
    moderately sure: /v1/audio/transcriptions takes multipart m4a up to 25 MB with prompt. whisper-1 honours only about 224 prompt tokens. I
    don't know the limits for gpt-transcribe, so the spike checks them.
  - Disclosure: "this phone sends your recording and project names to OpenAI" is one true sentence. The names are a new disclosure the brief
    doesn't mention. The hub route would add the relay and hub to that sentence.
  - Key safety: this is the phone route's real cost, because revoking a device (SettingsScreen.kt:112) wouldn't revoke the key.
    - Mitigate with a restricted, per-phone key, and tell the owner to make one.
    - iOS: a separate keychain service, WhenUnlockedThisDeviceOnly, rather than the after-first-unlock class at Keys.swift:187.
    - Android: prefs is plain (AppModel.kt:66) and existing keys are EC signing only (KeystoreKeys.kt:80-81), so add a Keystore AES-GCM
      wrapper. Backup is already off (AndroidManifest.xml:18).
    - Don't validate the key on save. As I recall, a restricted key can fail /v1/models while audio works.
  - Latency: one hop beats phone, relay, hub, provider.
  - Hub route: the 2 MB limit isn't the blocker. The relay carries base64 (src/relay/server.rs:44,173, src/hub/relay_link.rs:14), leaving
    about 1.5 MB, which is minutes of AAC. It loses because it adds a raw-audio route, hand-built multipart over hyper (Cargo.toml:18-20),
    an OpenAPI change, a hub secret file and a way to enter the key there. It only wins if phones must never hold the key.
  - Streaming rejected: it needs a WebSocket client and PCM pipeline twice, for a benefit ranked fourth.
  - Running on-device alongside the upload rejected: it would keep live text, but Android's recognizer owns the mic (Voice.kt:72). It also
    can't serve phones that refuse on-device (Voice.kt:67).
  - Claude pass deferred: it means a second key, a second disclosure, more delay and a second failure path.

  Two corrections to the brief:
  - The Listener protocol is at Voice.swift:131.
  - iOS does have a settings screen (DevicesView.swift:69).

  Outline.

  No hub, relay or api/openapi.json change. iOS and Android are independent.

  1. Spike, before app code. Run about 20 owner-recorded technical phrases through gpt-transcribe and gpt-4o-transcribe with the names
     prompt. Record name accuracy and latency, then fix one model constant with no picker. Decide the Claude pass from the residual errors.
  2. Pure logic, tested in SwitchboardKit and apps/android/core (MockWebServer: core/build.gradle.kts:51):
     - request builder;
     - error classes;
     - silence gate with an adaptive noise floor and a hard 90-second cap.

     The current rule depends on heard changing (Voice.swift:60,78-79, Voice.kt:91), and batch produces no partials. Replace heard.isEmpty
     with a spoke flag, so on-device behaves as before.
  3. Key stores with save and remove, plus a settings section (DevicesView.swift:53, SettingsScreen.kt:76). Save carries the disclosure
     text, so saving is the consent. Show "Key saved", never the key. Forgetting the hub also deletes it (AppModel.swift:148,
     AppModel.kt:236), since that dialog promises keys are deleted (DevicesView.swift:77).
  4. Recorder and state.
     - Use AVAudioRecorder and MediaRecorder, because both meter loudness and write AAC to a temp file with little code.
     - Add a transcribing phase (Voice.swift:12, Voice.kt:26), because stop() currently goes straight to review (Voice.swift:96,
       Voice.kt:107).
     - Split iOS permissions so cloud needs only the mic (Voice.swift:179-180).
  5. Fallback.
     - Offline at listen start: use on-device, with a notice.
     - Upload failure or 20-second timeout: open the sheet (SendSheet.swift:29) with the provider's error class and Retry, and the sheet's
       mic (SendSheet.swift:80) listens on-device.
     - 401, 403 or no quota: on-device until the key is replaced or the app relaunches, with a notice.
  6. Overlay. Keep the panel up through transcribing (VoiceOverlay.swift:10, VoiceOverlay.kt:143). Replace "Listening…"
     (VoiceOverlay.swift:127, VoiceOverlay.kt:199) with "Recording for OpenAI…", then "Transcribing with OpenAI…".
  7. Text and records.
     - Mic string (project.pbxproj:238); the speech string stays true because that permission remains on-device (:239).
     - The on-device comments (Voice.swift:7-8, Voice.kt:22-23).
     - An as-built note at debate-native-apps-2.md:276.
     - Review PrivacyInfo.xcprivacy:9 and the store privacy forms.

  Invariants and how each fails:
  - Key absent or unreadable: on-device, no upload.
  - The key goes only to a hard-coded https://api.openai.com, never to the hub or logs. There's no base-URL setting.
  - Upload happens only after detected speech or an explicit Done. A silent auto-stop uploads nothing, which also avoids paying for silence
    and the prompt-echo failure I recall from Whisper.
  - Cancel and Type Instead (Voice.swift:100,111) delete the audio and abort the upload, and a late response is dropped.
  - The transcript only reaches the review sheet. It never sends or approves (debate-native-apps-2.md:278).
  - The audio file is deleted when the sheet closes, and swept at launch.
  - No key means byte-for-byte today's path.

  Verification:
  - Unit tests for the gate, request shape, each error class and key removal.
  - iOS UI test with a fake transcriber beside SB_FAKE_VOICE (SwitchboardUITests.swift:26).
  - On real iPhone and Android, with a throwaway key: normal, airplane mode, bad key, cancel mid-upload, backgrounding, remove key.
  - git diff --stat src/ api/ is empty.
  - Spike results kept in the task folder.

  Assumptions.
  - The owner accepts entering a key per phone.
  - Session labels and branches arriving from the hub are already masked; I didn't trace this. If not, mask before use, or the names prompt
    could carry a secret.
  - Audio can't be masked, and the opt-in disclosure accepts that.
  - The hub's src/redact.rs rule governs what leaves the owner's machine; the phone-to-OpenAI path is the owner's opt-in.
  - From memory, App Review now expects explicit consent for sharing data with third-party AI; check this before release.
  - Outside the brief: a local on-device name-correction step.

  Confidence. 74

  END ROUND 1
