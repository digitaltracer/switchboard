# Switchboard for Android

Kotlin and Jetpack Compose app for the Switchboard hub. It's being built to match the iOS app;
so far it pairs with a hub, directly or through its relay, and signs in.

## Layout

- `core/`: plain Kotlin with everything that isn't Android. It's the counterpart of `SwitchboardKit`.
  - The API client is generated at build time by openapi-generator from `api/openapi.json`.
  - It holds the signed-message formats, relay encryption, the hub client (tokens, error
    codes), the start/reconcile flow and the routing options.
  - Its tests run on the Mac against the fixtures in `tests/fixtures/app-v1/`, which the hub's
    Rust tests and the iOS package share.
- `app/`: the Android app, holding the Keystore keys and the Compose screens.
  - `src/debug/` holds the Keystore spike screen ("SB Keystore spike") and plain HTTP to
    `127.0.0.1` for testing against a local hub. Release builds have neither.

Android 12 (API 31) or newer. The application id is `com.digitaltracer.switchboard`, as on iOS.

## Commands

Needs JDK 17 and the Android SDK in `~/Library/Android/sdk` (`local.properties` points to it).

```sh
cd apps/android
./gradlew :core:test            # unit tests on the Mac, including the relay interop vector
./gradlew :app:assembleDebug    # app/build/outputs/apk/debug/app-debug.apk
~/Library/Android/sdk/platform-tools/adb install -r app/build/outputs/apk/debug/app-debug.apk
```

When `api/openapi.json` changes, the next build regenerates the client.

## Keys

Two P-256 keys in the Android Keystore, in StrongBox where the phone has one and the TEE
otherwise:
- The auth key signs token challenges without asking.
- The approval key needs a strong biometric or the screen lock for every signature. The key
  itself enforces this; a new fingerprint doesn't invalidate it. Creating it needs a screen lock.

## Push

Push goes through Firebase Cloud Messaging. Both halves come from your own Firebase project and
stay outside the repo:
- **The app** reads its Firebase settings at build time from
  `~/.config/switchboard/android-firebase.properties` (or `-Pswitchboard.firebase=<file>`), with
  `appId`, `apiKey`, `projectId` and `senderId` from the project's Android app. Without the file
  the app builds with push off. There is no `google-services.json`.
- **The relay** (or a hub that sends its own pushes) sends with the project's service-account
  key, set in its config as `[push.fcm] key_file = "<absolute path>"`. The key must be mode 0600
  and outside any git working tree. Deploy that relay before any hub sends `waiting` pushes; an
  older relay drops them.

The phone registers its FCM token with the hub, and notifications carry only generic text and ids.

## Pairing against a throwaway hub

Run a relay and a hub on the Mac with a scratch config, as `apps/ios/scripts/e2e.sh` does, then
forward the relay's port to the phone and open the link `pair-device` prints:

```sh
adb reverse tcp:$RELAY_PORT tcp:$RELAY_PORT
adb shell "am start -a android.intent.action.VIEW -d '$LINK' com.digitaltracer.switchboard"
```

Afterwards, `adb reverse --remove-all` removes the port forward, and
`adb shell pm clear com.digitaltracer.switchboard` makes the app forget the throwaway hub.
