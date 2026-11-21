#!/usr/bin/env bash
# Takes a screenshot of each main screen, light and dark, against a throwaway local relay, hub
# and scripted runner, for comparing with the design. Nothing is pushed.
# Usage: apps/ios/scripts/shots.sh <output folder> [simulator name]
set -euo pipefail

OUT="${1:?usage: shots.sh <output folder> [simulator name]}"
SIM="${2:-iPhone 18 Pro}"
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
  rm -rf "$WORK"
}
trap cleanup EXIT

DERIVED="$ROOT/apps/ios/build"
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

TEST_RUNNER_SB_PAIR_LINK="$LINK" TEST_RUNNER_SB_SHOTS=1 \
  xcodebuild test-without-building "${XCB[@]}" -only-testing:SwitchboardUITests/ScreenshotsUITests \
  -resultBundlePath "$WORK/result.xcresult" > "$WORK/test.log" 2>&1 &
TEST_PID=$!

# Wait for the admin side, unless the tests end first.
while kill -0 "$PAIR_PID" 2>/dev/null; do
  if ! kill -0 "$TEST_PID" 2>/dev/null; then
    echo "UI tests ended before pairing finished:"; grep -E "Test Case|error:" "$WORK/test.log" || tail -40 "$WORK/test.log"; exit 1
  fi
  sleep 1
done
wait "$PAIR_PID" && echo "pair-device: the simulator is paired" || { echo "pair-device failed:"; tail -3 "$WORK/pair.log"; exit 1; }

# xcodebuild can hang after a failed UI test; give it ten minutes.
for _ in $(seq 600); do kill -0 "$TEST_PID" 2>/dev/null || break; sleep 1; done
kill "$TEST_PID" 2>/dev/null || true
STATUS=0
wait "$TEST_PID" || STATUS=$?
grep -E "Test Case|error:|\*\* TEST" "$WORK/test.log" || tail -40 "$WORK/test.log"
mkdir -p "$OUT"
xcrun xcresulttool export attachments --path "$WORK/result.xcresult" --output-path "$WORK/shots" > /dev/null
python3 - "$WORK/shots" "$OUT" <<'PY'
import json, shutil, sys
src, out = sys.argv[1:]
for test in json.load(open(f"{src}/manifest.json")):
    for a in test["attachments"]:
        shutil.copy(f"{src}/{a['exportedFileName']}", f"{out}/{a['suggestedHumanReadableName'].split('_0_')[0]}.png")
PY
ls "$OUT"
[[ $STATUS -eq 0 ]] || { echo "the screenshot tour failed ($STATUS)"; exit $STATUS; }
