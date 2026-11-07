• ROUND 2 cloud-transc-a2

  ## Position

  CONSENSUS: I concede both disputes. Keep failed recordings for explicit Retry until review ends, and run the phrase test before fixing the
  context format. The draft needs the implementation and verification corrections below.

  ## Reasoning

  Retention leaves a concrete risk: crash leftovers survive until another launch, potentially indefinitely. Protected, backup-excluded
  temporary storage and launch cleanup make that acceptable; it does not outweigh avoiding repetition. A timeout also leaves processing
  uncertain, so Retry may incur another charge. Never retry automatically.

  An upfront test can choose transcription context. End-of-build testing additionally catches microphone processing, encoding, loudness
  calibration and premature silence stops. Those require the actual capture pipeline; both tests belong.

  ## Outline

  • Strengthen the phrase experiment. Step 1 specifies one “names context,” then claims to select its format. Compare prompt, keywords and
    both, including vocabulary sizes. Score exact technical names, omitted words and changed command meaning. Merely checking three result
    columns cannot establish accuracy. Use some held-out phrases.

  • Carry lifecycle requirements into steps. Add explicit background cancellation and launch sweeping. Current iOS lifecycle wiring only
    updates the model (apps/ios/Switchboard/SwitchboardApp.swift:19); Android similarly calls its model (apps/android/app/src/main/kotlin/
    com/digitaltracer/switchboard/MainActivity.kt:57). Name the files implementing these hooks and test retained-file deletion after forced
    termination and relaunch.

  • Specify Retry UI and edit ownership. Steps 4–6 omit explicit SendSheet changes. Existing fields directly edit voice text (apps/ios/
    Switchboard/SendSheet.swift:32; apps/android/app/src/main/kotlin/com/digitaltracer/switchboard/SendSheet.kt:201). A generation counter
    alone does not protect edits unless edits invalidate the request. Test edit-during-Retry, double Retry, dismissal and replacement
    recordings. Include transcribing controls: the iOS overlay currently renders its panel only while listening (apps/ios/Switchboard/
    VoiceOverlay.swift:10).

  • Test the host invariant. A constant URL does not establish acceptance criterion 3. Disable redirects for provider requests and test
    redirect responses, credential-free errors and logs. Fake successful transcription bypasses these checks.

  • Resolve contradictions and precision. Decision 7 permits recovery on relaunch; criterion 5 says only key changes. Define “terminal” so
    retryable failure retains audio but dismissal deletes it. Current-behaviour claim “stop goes straight to review” misses empty-input idle
    (apps/ios/Switchboard/Voice.swift:96). Disclosure should name branches, session labels and agents too, matching vocabulary construction
    (apps/ios/Switchboard/AppModel.swift:203).

  ## Assumptions

  The owner supplies recordings and runs credential-dependent comparisons.

  ## Confidence

  91/100.

  No modifications, builds or tests. Remaining risks are retained crash audio and uncalibrated speech detection.
  END ROUND 2
