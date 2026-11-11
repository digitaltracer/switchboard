# Cloud transcription: owner checklist

What's left that only you can do: it needs your keys, your voice, or your phones. Use a throwaway OpenAI key with a low spending limit, and revoke it when you're done. Put results in the As built section of `plan.md`.

## 1. Phrase test

It decides how the names reach OpenAI, whether names are sent at all, and whether a Claude pass is worth a second key.

1. Make a folder outside any git working tree, for example `mkdir ~/phrases`. The script refuses a folder inside a repo.
2. Record about 25 short instructions you'd really give an agent, saying your project, branch and agent names. Save them in `~/phrases` as `01.m4a`, `02.m4a` and so on. Voice Memos files work.
3. Write `~/phrases/phrases.json`, shaped like `phrase-test/phrases.example.json`: for each file, the text you meant, the `names` that must come out exactly, the `meaning` words, and `held_out`. Mark at least five `held_out: true`.
4. Write `~/phrases/names.txt` with one name per line, most relevant first: project ids and aliases, session labels, branches, agent names.
5. Set up Python outside the repo. It has no `.venv`, and the Claude variant needs `anthropic`:
   ```sh
   python3 -m venv ~/phrases/.venv && ~/phrases/.venv/bin/pip install anthropic
   ```
6. Dry run. It converts the recordings, checks the manifest and calls nothing:
   ```sh
   cd ~/personal/switchboard/.agents/tasks/cloud-transcription/phrase-test
   ~/phrases/.venv/bin/python phrase_test.py --recordings ~/phrases --names ~/phrases/names.txt --dry-run
   ```
   Look for: no errors, and every recording listed.
7. Set `OPENAI_API_KEY`, and the Anthropic key for the Claude variant, in your own shell. Then run the test, adding `--skip-claude` if you have no Anthropic key:
   ```sh
   ~/phrases/.venv/bin/python phrase_test.py --recordings ~/phrases --names ~/phrases/names.txt
   ```
   Look for: `phrase-test/results.md` filled in. Compare "Names right" across the variants, and read the transcripts for changes in meaning.
8. Write the three decisions into `plan.md`: which context format to use, whether names are sent, and whether Claude is worth it. If names don't help, the names field and the disclosure wording change in a follow-up.

## 2. Key storage checks (plan steps 3 and 5)

In each app, Settings has a Voice section. On iOS, tap the gear at the top right of the Agents screen.

**iPhone**
1. Paste the key and Save. Look for "Key saved", with the key itself never shown.
2. Force-quit and relaunch. Look for "Key saved" still there, and the overlay showing cloud mode on the next listen.
3. Remove. Look for the key field back, and the next listen running on-device.
4. Save the key again, then forget the hub. Look for the key gone after re-pairing, and voice on-device.
5. Save the key, then delete the app and reinstall it. Look for no key after the reinstall.

**Android** (needs a phone; none was connected on 2026-11-11)
- Repeat steps 1 to 4. Look for the same results. If the Keystore fails, cloud mode should stay off and never fall back to plain text.

## 3. Real-phone checks (plan step 7)

The iPhone 11 has a development build of `8bdf00c`, installed on 2026-11-11. To reinstall after changes, with the phone unlocked:

```sh
xcodebuild build -project apps/ios/Switchboard.xcodeproj -scheme Switchboard -configuration Debug -destination id=00008030-001D24542178802E -derivedDataPath /tmp/sb-device
xcrun devicectl device install app --device 00008030-001D24542178802E /tmp/sb-device/Build/Products/Debug-iphoneos/Switchboard.app
```

Do these on the iPhone first, and on Android once a phone is connected. Save the throwaway key before starting. Each listen should end in the review sheet, and nothing should send without your tap.

1. **Normal use.** Say five name-heavy phrases from the phrase test. Look for names right at least as often as the phrase test's winning variant.
2. **Airplane mode at listen start.** Look for the listen running on-device, with a notice.
3. **Airplane mode mid-upload.** Turn it on while the overlay shows transcribing. Look for an error and Retry. Turn airplane mode off and tap Retry. Look for the same recording transcribed.
4. **Bad key.** Save a revoked or wrong key. Look for a notice and on-device voice until you replace the key or relaunch.
5. **Cancel mid-upload.** Look for the sheet closing with the text cleared.
6. **Background mid-upload.** Go home while uploading, come back and tap Retry. Look for an error with Retry, then the text.
7. **Expiry.** Make an upload fail, wait over 10 minutes, then tap Retry. Look for the recording gone, with a clear message.
8. **Typing during Retry.** Type while Retry runs. Look for Send disabled, your text kept, and the result appended.
9. **Remove the key** with a kept recording. Look for the recording gone and the next listen on-device.
10. **Silent listen.** Start a listen and say nothing. Look for an auto-stop, nothing uploaded and a return to idle.
11. **Noisy room.** Look for speech still detected, and the gate not stopping mid-sentence.
12. **Force-quit with a kept recording**, then relaunch. Look for no stale recording and no Retry offered.
13. **Fresh iOS install, cloud mode.** Look for the microphone prompt only, with no speech-recognition prompt.

## 4. Store privacy forms (release note in `apps/ios/README.md`)

1. App Store Connect, App Privacy: add audio data and other user content (the project, branch, session and agent names), shared with a third party, only at the owner's choice, not used for tracking.
2. Play Console, Data safety: declare the same, audio and names shared with a third party at the user's choice.
3. Check whether App Review wants an explicit consent screen for sharing with a third-party AI. Today, saving the key is the consent, and the Settings text says what is sent.
