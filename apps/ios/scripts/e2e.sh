#!/usr/bin/env bash
# Runs the iOS UI tests against a throwaway local relay and hub. The app reaches the hub only
# through the relay, and pushes go hub -> relay -> simulator (the relay's simctl sender).
# `admin pair-device` (answering yes) hands its relay link to the tests; then a second, made-up
# device is paired and the "new device paired" push must reach the simulator.
# Usage: apps/ios/scripts/e2e.sh [simulator name]
set -euo pipefail

SIM="${1:-iPhone 18 Pro}"
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORK="$(mktemp -d)"
free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
PORT="$(free_port)"
RPORT="$(free_port)"
export PATH="$HOME/.cargo/bin:$PATH"

cleanup() {
  [[ -n "${HUB_PID:-}" ]] && kill "$HUB_PID" 2>/dev/null || true
  [[ -n "${RELAY_PID:-}" ]] && kill "$RELAY_PID" 2>/dev/null || true
  [[ -n "${RUNNER_PID:-}" ]] && kill "$RUNNER_PID" 2>/dev/null || true
  [[ -n "${PAIR_PID:-}" ]] && kill "$PAIR_PID" 2>/dev/null || true
  [[ -n "${KEEP_RESULT:-}" ]] && cp -R "$WORK/result.xcresult" "$KEEP_RESULT" 2>/dev/null; rm -rf "$WORK"
}
trap cleanup EXIT

DERIVED="$ROOT/apps/ios/build"
BUNDLE_ID="$(sed -n 's/^SB_BUNDLE_ID *= *//p' "$ROOT/apps/ios/Config.xcconfig")"
XCB=(-project "$ROOT/apps/ios/Switchboard.xcodeproj" -scheme Switchboard -destination "platform=iOS Simulator,name=$SIM"
     -derivedDataPath "$DERIVED" -skipPackagePluginValidation)

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" --bin switchboard --example fake_runner
SB="$ROOT/target/debug/switchboard"
# Build first: the pairing code is only valid for ten minutes.
xcodebuild build-for-testing "${XCB[@]}" -quiet
xcrun simctl boot "$SIM" 2>/dev/null || true

# A scripted runner stands in for the Mac: it accepts starts and answers with canned output.
cat > "$WORK/hub.toml" <<EOF
public_url = "http://127.0.0.1:$PORT"
listen = "127.0.0.1:$PORT"
database = "$WORK/hub.db"
default_machine = "mac"

[[machines]]
id = "mac"
token_hash = "$(printf 'e2e-unused' | shasum -a 256 | cut -d' ' -f1)"

[[projects]]
id = "demo"
description = "e2e project"
repos = { mac = "$WORK/demo" }

[relay]
url = "http://127.0.0.1:$RPORT"
key_file = "$WORK/relay-key.json"
EOF

cat > "$WORK/relay.toml" <<EOF
listen = "127.0.0.1:$RPORT"

[push]
sender = "simctl"
bundle_id = "$BUNDLE_ID"
EOF

"$SB" relay --config "$WORK/relay.toml" > "$WORK/relay.log" 2>&1 &
RELAY_PID=$!
for _ in $(seq 50); do curl -sf "http://127.0.0.1:$RPORT/health" > /dev/null && break; sleep 0.2; done

"$SB" hub --config "$WORK/hub.toml" > "$WORK/hub.log" 2>&1 &
HUB_PID=$!
for _ in $(seq 50); do grep -q "connected to the relay" "$WORK/hub.log" && break; sleep 0.2; done
grep -q "connected to the relay" "$WORK/hub.log" || { echo "the hub never reached the relay:"; tail -5 "$WORK/hub.log"; exit 1; }

"$ROOT/target/debug/examples/fake_runner" "ws://127.0.0.1:$PORT/runner" mac e2e-unused > "$WORK/runner.log" 2>&1 &
RUNNER_PID=$!
for _ in $(seq 50); do grep -q "runner connected" "$WORK/hub.log" && break; sleep 0.2; done
grep -q "runner connected" "$WORK/hub.log" || { echo "the fake runner never connected:"; tail -5 "$WORK/runner.log"; exit 1; }

# pair-device reads its yes only after a phone has claimed the code.
echo yes | "$SB" admin pair-device --config "$WORK/hub.toml" > "$WORK/pair.log" 2>&1 &
PAIR_PID=$!
for _ in $(seq 50); do grep -q "switchboard://pair" "$WORK/pair.log" && break; sleep 0.2; done
LINK="$(grep -o 'switchboard://pair[^[:space:]]*' "$WORK/pair.log" | head -1)"
[[ "$LINK" == *"relay="* ]] || { echo "expected a relay link: $LINK"; exit 1; }
[[ -n "$LINK" ]] || { cat "$WORK/pair.log"; exit 1; }

TEST_RUNNER_SB_PAIR_LINK="$LINK" TEST_RUNNER_SB_EXPECT_PROJECT=demo TEST_RUNNER_SB_EXPECT_PUSH=1 \
  xcodebuild test-without-building "${XCB[@]}" -resultBundlePath "$WORK/result.xcresult" > "$WORK/test.log" 2>&1 &
TEST_PID=$!

# Wait for the admin side, unless the tests end first.
while kill -0 "$PAIR_PID" 2>/dev/null; do
  if ! kill -0 "$TEST_PID" 2>/dev/null; then
    echo "UI tests ended before pairing finished:"; grep -E "Test Case|error:" "$WORK/test.log" || tail -40 "$WORK/test.log"; exit 1
  fi
  sleep 1
done
wait "$PAIR_PID" && echo "pair-device: the simulator is paired" || { echo "pair-device failed:"; tail -3 "$WORK/pair.log"; exit 1; }

# A second device: a claim needs only public keys, so openssl makes them.
pubkey() {
  openssl ecparam -genkey -name prime256v1 -noout 2>/dev/null \
    | openssl ec -pubout -conv_form uncompressed -outform DER 2>/dev/null \
    | tail -c 65 | base64 | tr '+/' '-_' | tr -d '='
}
echo yes | "$SB" admin pair-device --config "$WORK/hub.toml" > "$WORK/pair2.log" 2>&1 &
PAIR_PID=$!
for _ in $(seq 50); do grep -q "  code " "$WORK/pair2.log" && break; sleep 0.2; done
CODE2="$(grep -o '  code [0-9a-f]*' "$WORK/pair2.log" | awk '{print $2}')"
curl -sf "http://127.0.0.1:$PORT/app/v1/pair" -H 'content-type: application/json' \
  -d "{\"code\":\"$CODE2\",\"name\":\"Second phone\",\"auth_key\":\"$(pubkey)\",\"approval_key\":\"$(pubkey)\"}" > /dev/null
wait "$PAIR_PID" && echo "pair-device: second device paired"

# xcodebuild can hang after a failed UI test; give it ten minutes.
for _ in $(seq 600); do kill -0 "$TEST_PID" 2>/dev/null || break; sleep 1; done
kill "$TEST_PID" 2>/dev/null || true
STATUS=0
wait "$TEST_PID" || STATUS=$?
grep -E "Test Case|error:|\*\* TEST" "$WORK/test.log" || tail -40 "$WORK/test.log"
"$SB" admin list-devices --config "$WORK/hub.toml"
grep -E "relay|push" "$WORK/hub.log" | tail -3 || true
[[ $STATUS -eq 0 ]] || { echo "UI tests failed ($STATUS)"; exit $STATUS; }
