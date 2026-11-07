⏺ ROUND 2 cloud-transc-a1

  Position. I keep Retry and the phrase test, with changes to both. Retry needs an expiry and must not change phase. The phrase test must
  not block app code.

  Reasoning.
  - Retry cost: the file already has to exist during upload, and the launch sweep is already needed for crashes. What's new is one kept URL,
    one button and one delete in finish(), which every sheet exit reaches (SendSheet.swift:69,229,261, HomeView.swift:96). I estimate 30
    lines per platform.
  - Retry risk: "bounded by the sheet's lifetime" is weak, because a sheet can sit open for hours. Add a 10-minute expiry.
  - Retry gain: re-recording hands a repeat of the utterance to the recognizer the owner rejected. Where on-device refuses (Voice.kt:67-71),
    Retry is the only remaining voice path.
  - Phrase test: docs can supply a default format, but not whether names help or cause false matches. If they don't help, names stop leaving
    the phone, shrinking the disclosure. It also settles Claude. Default to the documented keywords field and let the test replace it.
    Require results before disclosure text and release.

  Outline. Plan defects:
  1. Decision 9 says "before app code" while section 8 runs them in parallel. Use parallel; step 6 and step 3's save text depend on step 1.
  2. Retry through a transcribing phase closes the sheet, which is shown only during review (HomeView.swift:95, HomeScreen.kt:87). Retry
     needs a flag inside review, Send disabled meanwhile, and the result appended, never overwriting edits.
  3. Step 4 picks cloud whenever a key exists, but decision 7 says the sheet's mic is on-device. That mic calls the same listen
     (SendSheet.swift:80, SendSheet.kt:243). Add a parameter. A new listen deletes the kept recording.
  4. Criterion 6's backgrounding is in no step, and it conflicts with Retry. Hook SwitchboardApp.swift:19 and MainActivity.kt:57, and keep
     the file for Retry on return.
  5. Criterion 7 can't be checked on a phone, and step 4's UI test covers only success and Retry. Put the recording lifecycle in the kit and
     core with a fake transport, testing every terminal path.
  6. Step 3's Android wrapper test has nowhere to run: app/src has only debug and main, and Keystore isn't on the JVM. Use an instrumented
     test or an explicit manual check.
  7. Step 2's constant base URL conflicts with MockWebServer. Inject it for tests only.
  8. Criterion 3 misses redirects. Disable them and test it. Android needs its own client, not Hub.kt:217.
  9. Offline detection needs ACCESS_NETWORK_STATE, absent from AndroidManifest.xml:4-7, and section 8's stop rule would halt on it. Exempt
     it.
  10. Recorder sample rate and bitrate are unspecified. Pin them in step 2 and use them in step 1, or the 20 s limit and the test results
      mean little.
  11. Criterion 8 versus Done: state that Done uploads regardless. An empty transcript follows Voice.swift:96.
  12. Removing the key must also delete a kept recording.
  13. iOS keychain items survive uninstall, so cloud mode could resume after reinstall. Clear the key on first launch using a UserDefaults
      flag.
  14. Recordings under phrase-test/ aren't gitignored. Keep them outside the repo.
  15. No criterion checks names in the app. Add name phrases to step 7.
  16. Reference errors: Android also caps at 100 (AppModel.kt:322); forget is at AppModel.kt:236.

  Assumptions.
  - From memory, not tested: iOS keychain survival across uninstall.
  - Thirty lines is an estimate.

  Confidence. 80

  END ROUND 2
