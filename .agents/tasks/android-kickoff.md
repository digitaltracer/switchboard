# Android app: kickoff brief

Written 2026-10-24 for the agent starting the Android milestone. The user asked for it to start now.

## The task

Build Switchboard's Android app in `apps/android/`, in Kotlin with Jetpack Compose. It is step 8 of `.agents/tasks/debate-native-apps-2.md`. The goal is parity with the iOS app as it ships today (TestFlight build 3).

## Read first

- `README.md` and `AGENTS.md`: the product, and the rules you must not break.
- `.agents/tasks/debate-native-apps-2.md`: the apps and relay plan. Read step 8, the open questions and every "As built" note. Design E, voice and one message box are described there.
- `apps/ios/README.md` and the iOS code: the reference implementation.
  - `apps/ios/SwitchboardKit` holds pairing, device keys, signed approvals, relay end-to-end encryption, the API client and routing options. Its tests include interop vectors against the Rust hub (`theHubsRustEnvelopesInteroperate`), which Android must pass too.
  - `apps/ios/Switchboard` holds the screens.
- `api/openapi.json`: the app API (`/app/v1`). Changes to it must be additive.
- `src/relay/`, `src/auth/`, `src/push/`: what the relay, pairing and push expect from a phone.

## Order of work (from the plan)

1. **Check the tools and ask the user.** Check what's installed: Android Studio or the command-line SDK, a JDK, Gradle, `adb`. Then ask the user:
   - Which Android phone they have, and its Android version.
   - Whether Android Studio is installed, or they're fine with you installing the command-line tools.
   - Whether they have a Firebase project yet. FCM is needed later, not at first.
   - Whether installing the app on their phone over USB is fine to begin with, before any Play Store track.
2. **Plan.** Propose the module layout and libraries before writing much code, and wait for the user's go-ahead. Suggested: an `app` module for the UI and a `core` library mirroring SwitchboardKit, with the API client generated from `api/openapi.json` or written by hand.
3. **Core first, with tests:**
   - pairing (QR or link);
   - device keys and request signing;
   - relay end-to-end encryption, byte-compatible with the Rust side and checked against the same vectors as iOS;
   - the API client.
4. **Keystore spike on the real phone.** Check whether a key bound to biometric or device-credential authentication is enforced at the key level, which approval signing needs. Viewing and denying work without it. Record the findings.
5. **Screens to parity:**
   - home ("Agents", "Started in Herdr", approvals, live tiles);
   - session and agent pages;
   - start and send with the hub's preview;
   - voice, using Android's on-device speech recognizer where available;
   - approvals, changed files, devices.
6. **Push via FCM, last.** The relay on the user's VPS sends only to APNs today, so it needs an FCM sender too. Design that with the user: credentials live on the VPS outside any repo, and the user runs production deploys themselves.

## Rules

- Follow `AGENTS.md`. Secrets never go in git or get printed. Approvals fail closed. Mask everything that leaves the machine.
- Don't commit, push or branch unless the user asks. When they do, make atomic conventional commits with no AI attribution.
- Ask the user on design or ambiguous decisions, and suggest better options when you see them.
- Don't disturb the user's running hub, Herdr sessions or other agents. Use throwaway Herdr sessions for any live test.
- Add a dated "As built" note to `debate-native-apps-2.md` when the build differs from the plan.
