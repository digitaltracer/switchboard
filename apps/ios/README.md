# Switchboard for iOS

SwiftUI app for the Switchboard hub: pair the phone, see and start sessions, approve with Face ID
or deny, and get pushes for approvals and finished turns.

## Layout

- `SwitchboardKit/`: a Swift package with everything that isn't UI.
  - `SwitchboardAPI` is the client generated at build time by Apple's swift-openapi-generator
    from `api/openapi.json` (a symlink to the hub's committed spec).
  - `SwitchboardKit` holds the signed-message formats, the Secure Enclave keys, the hub client
    (token refresh, error codes) and the start/reconcile flow.
- `Switchboard/`: the app target, SwiftUI only.
- `SwitchboardUITests/`: XCUITests that run against a real local hub.
- `Switchboard.xcodeproj`: written by hand and kept small. Its folders are "synchronized", so
  adding a Swift file needs no project change.

There is no project generator to install, so the project builds with plain `xcodebuild`.

## Your identifiers

`Config.xcconfig` holds the bundle id (`SB_BUNDLE_ID`, `com.digitaltracer.switchboard`), the
team (`DEVELOPMENT_TEAM`) and the push entitlement. Use the same bundle id in `hub.toml`'s
`[push] bundle_id`.

## Commands

```sh
# Unit tests of the package (macOS host)
cd apps/ios/SwitchboardKit && swift test

# Build the app for the simulator
cd apps/ios && xcodebuild -project Switchboard.xcodeproj -scheme Switchboard \
  -destination 'platform=iOS Simulator,name=iPhone 18 Pro' -derivedDataPath build \
  -skipPackagePluginValidation build

# UI tests against a throwaway local hub and a scripted runner, with a simulator push at the end
apps/ios/scripts/e2e.sh
```

`-skipPackagePluginValidation` lets the OpenAPI generator plugin run without Xcode's
trust prompt. When `api/openapi.json` changes, the next build regenerates the client.

## Keys

- **On a phone:** two P-256 keys in the Secure Enclave. The auth key signs token challenges
  without asking. The approval key needs `.userPresence` (Face ID, Touch ID or the passcode)
  for every signature, and the keychain item is `WhenPasscodeSetThisDeviceOnly`.
- **On the simulator,** which has no Secure Enclave: software keys in the keychain, with a
  LocalAuthentication check before each approval. To simulate Face ID, use Features > Face ID
  > Enrolled, then Matching Face.

## Push

- On the simulator, the hub's `sender = "simctl"` delivers through `xcrun simctl push`. No Apple
  account is needed.
- On a phone you need:
  - an App ID with the Push Notifications capability;
  - an APNs auth key (`.p8`) configured in `hub.toml` under `[push.apns]`;
  - the `aps-environment` entitlement, which needs your team for signing.
- See the main report for the exact steps.
