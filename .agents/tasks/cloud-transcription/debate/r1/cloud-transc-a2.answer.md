• ROUND 1 cloud-transc-a2

  Position.

  Use each phone’s owner-entered OpenAI key and upload a completed recording directly to OpenAI. Store the key locally, disclose that audio
  and vocabulary leave the phone, and provide removal. Start with gpt-transcribe, batch uploads, and no Claude pass. Show recording levels,
  then “Transcribing…”, then the existing review sheet. With no usable key, use on-device recognition. After a cloud failure, offer an
  explicitly labelled on-device re-recording; do not retain audio for later retries.

  Reasoning.

  Direct upload avoids adding another network dependency and an audio API to the hub. Hub mediation would centralise credentials across
  phones, but must accommodate the relay’s 2 MiB request cap and 45-second timeout (src/relay/server.rs:44, src/relay/server.rs:31). Phone-
  held keys require setup on each phone; I accept that tradeoff for fewer components and no hub deployment dependency.

  Technical vocabulary already exists: iOS collects project aliases, session labels, branches and agent names, capped at 100 phrases (apps/
  ios/Switchboard/AppModel.swift:202). Android supplies vocabulary to its recognizer (apps/android/app/src/main/kotlin/com/digitaltracer/
  switchboard/Voice.kt:80). Send bounded, validated expected names as transcription hints. Defer Claude: another credential, disclosure and
  request add complexity, while text rewriting could change intended commands. Measure transcription accuracy before adding correction.

  Official OpenAI documentation currently recommends gpt-transcribe, documents multipart /v1/audio/transcriptions, a 25 MB file limit, M4A/
  WAV support and keyword hints. It distinguishes streamed results for completed files from live microphone transcription. Confidence is
  high in these documented contracts; account availability and observed latency remain untested. Official OpenAI documentation

  Batch cannot reuse the current silence logic unchanged: both implementations measure time since transcript changes, not actual speech
  activity (apps/ios/Switchboard/Voice.swift:60, apps/ios/Switchboard/Voice.swift:78; apps/android/app/src/main/kotlin/com/digitaltracer/
  switchboard/Voice.kt:90, apps/android/app/src/main/kotlin/com/digitaltracer/switchboard/Voice.kt:158). Cloud recording therefore needs
  independent activity detection; local recognition must not become a prerequisite.

  Existing credential classes implement signing keys, not provider-token storage (apps/ios/SwitchboardKit/Sources/SwitchboardKit/
  Keys.swift:13; apps/android/app/src/main/kotlin/com/digitaltracer/switchboard/KeystoreKeys.kt:51). Add separate storage rather than
  changing approval-key semantics.

  Outline.

  1. Credential and consent UI first. Add a voice section to apps/ios/Switchboard/DevicesView.swift:34 and apps/android/app/src/main/kotlin/
     com/digitaltracer/switchboard/SettingsScreen.kt:56. Use a separate device-only Keychain item on iOS; an Android Keystore encryption key
     protecting the saved token. Provide replace/remove, no plaintext preferences, backups or logs. Storage failure disables cloud use.
     Explain provider billing and audio/vocabulary disclosure before saving.

  2. Recording and upload next. Extend the lifecycle around apps/ios/Switchboard/Voice.swift:130 and apps/android/app/src/main/kotlin/com/
     digitaltracer/switchboard/Voice.kt:57, adding asynchronous finalisation and cancellation. Record bounded M4A, stop after 1.8 seconds of
     detected silence or eight seconds without speech, retain manual Done, and impose duration/byte caps below provider limits. Validate
     vocabulary constraints; request JSON transcription through a dedicated provider client.

  3. Integrate processing and fallback. Update apps/ios/Switchboard/VoiceOverlay.swift:86 and apps/android/app/src/main/kotlin/com/
     digitaltracer/switchboard/VoiceOverlay.kt:140 for recording, uploading and cancellation. Update both iOS usage-description
     configurations (apps/ios/Switchboard.xcodeproj/project.pbxproj:239, apps/ios/Switchboard.xcodeproj/project.pbxproj:321). Cloud
     permission requests need only the microphone; current iOS permission handling also requires speech authorization (apps/ios/Switchboard/
     Voice.swift:178). Offline-before-recording uses local recognition. Authentication errors, timeout, rate limits and malformed responses
     offer local re-recording or typing; unavailable local recognition offers typing.

  4. Keep invariants explicit. No automatic send or approval; retain review-before-send and voice-never-approves (.agents/tasks/debate-
     native-apps-2.md:278). Cancel, backgrounding or key removal stops work; late responses cannot overwrite edits or another recording.
     Delete temporary audio on every terminal path and clean crash leftovers. Never log credentials, audio, transcripts or raw provider
     errors. Preserve hub masking (src/redact.rs:71); no hub API, relay or migration change is proposed.

  5. Verify before release. Use fake recorder/provider responses for success, failure, removal, cancellation races, preserved draft text and
     no-key regression. Test silence, noise, accents and technical names on physical phones; compare exact-name accuracy and stop-to-review
     latency against local recognition.

  Assumptions.

  • Fallback may require repeating the utterance; seamless recovery of the same audio is not promised.
  • Opt-in authorises raw audio and expected names leaving the phone; audio cannot be protected by the text masker.
  • Multi-phone key synchronisation and Claude cleanup are deferred.

  Confidence.

  85/100.

  No files changed; builds, tests and live provider calls were skipped. Main risks are speech-activity calibration and fallback requiring
  repetition.
  END ROUND 1
