⏺ IMPLEMENT cloud-transc-impl implement

  Step 1: done. I ran the script with --dry-run on synthetic audio, and checked its scoring and its refusals (missing key, recordings inside
  a repo) offline. It was never run with a key. Files: phrase-test/ (phrase_test.py, README.md, phrases.example.json, results.md).

  Step 2: done. swift test passes 70 tests and ./gradlew :core:test passes 79. That Gradle command is correct as proposed. Files:
  Transcription.swift, TranscriptionTests.swift, Transcription.kt, TranscriptionTest.kt.

  Step 3: done, except the manual phone check. Both apps compile. A simulator UI test saves and removes a key on iOS. Files: iOS
  VoiceKey.swift, AppModel.swift, DevicesView.swift; Android VoiceKey.kt, AppModel.kt, SettingsScreen.kt.

  Step 4: done, but e2e.sh fails. It stops at "the push reached the simulator". The unchanged tests at 5abf48f, run in a throwaway worktree,
  fail the same assertion, so this work didn't cause it. I didn't investigate why. Everything before that assertion passes, including the
  new flow: success, failure with Retry, and typing kept during Retry. After my first direct run, I used a scratch copy of the script that
  keeps its log and waits less for xcodebuild's hang. Files: CloudListener.swift, Voice.swift, VoiceOverlay.swift, SendSheet.swift,
  SwitchboardApp.swift, HomeView.swift, SwitchboardUITests.swift.

  Step 5: done, except the physical-phone checks. :app:assembleDebug, :app:lintDebug (no errors) and :core:test pass. Files:
  CloudListener.kt, Voice.kt, VoiceOverlay.kt, SendSheet.kt, HomeScreen.kt, MainActivity.kt, SwitchboardApp.kt, AndroidManifest.xml.

  Step 6: done. Each remaining grep hit is still true. All three suites ran again, with the same result: only the push check fails. Files:
  project.pbxproj, PrivacyInfo.xcprivacy, apps/ios/README.md, the Voice comments and labels on both platforms, and debate-native-apps-2.md.

  Step 7: skipped as instructed. git diff --stat 5abf48f -- src api migrations is empty.

  Left
  - Step 1's results and the three decisions that depend on them.
  - The manual phone checks in steps 3 and 5.
  - Step 7.
  - The push check failure in e2e.sh.

  Deviations
  - The privacy manifest also declares other user content for the names, because criterion 10 asks for names as well as audio.
  - The release note is in apps/ios/README.md, since only plan.md and phrase-test/ may be committed in the task folder.
  - There are two error classes beyond the plan's: interrupted and unreadable.
  - The cloud UI flow relaunches the app before the push check, because a failure there ends the test. Run after it, the flow wouldn't run
    here at all.
  - I read OpenAI's docs for the wire format, which the plan doesn't give. This goes beyond "the plan is your only input", and I recorded
    it. The format is repeated keywords[] fields.

  Assumptions
  - A 429 without insufficient_quota is a rate limit and goes to Retry.
  - A 401, 403 or quota error deletes the recording.
  - The 10-minute expiry runs from the first failure, and Retry doesn't extend it.
  - Type instead discards a cloud recording without uploading it.
  - During an upload, a tap outside the panel does nothing.
  - Removing the key during a cloud listen or upload cancels it.
  - iOS treats only .background as leaving the foreground, and Android ignores rotation stops.
  - Gate thresholds are starting values, to tune in step 7. The plan's As built section has the numbers.
  - The phrase test's Claude pass defaults to claude-opus-5-5.

  Final commit: 99f20672aa0748d72cbf8066bbe7ac6128703194

  STATUS: done

  END IMPLEMENT implement
