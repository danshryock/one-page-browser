#!/bin/bash
# Cross-compiles browser-macos-appkit and the web-standards test driver on
# Linux (via `cargo zigbuild`, see `.cargo/build-macos-appkit.sh`), deploys
# both plus the fixtures/ tree to the real Mac over SSH/SCP, and runs the
# real driver there — genuine x86_64 Apple hardware execution, not a
# cross-compile-and-hope. This is the first time either of those binaries
# ever actually *runs* rather than just links.
#
# Prerequisites: ./bootstrap.sh has passed (Mac reachable, console session
# logged in) — see README.md's Phase 0 checklist for the one-time physical
# setup, including the one step that can't be scripted at all: granting the
# driver binary Accessibility/Input Monitoring permission by hand, once.
#
# Usage: build-and-test.sh [--no-smoke]
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
source ./lib.sh

REPO_ROOT="$(cd ../.. && pwd)"
TARGET="x86_64-apple-darwin"
RUN_SMOKE=1
[ "${1:-}" = "--no-smoke" ] && RUN_SMOKE=0

mac_require_config

echo "== checking the Mac is reachable =="
if ! mac_run "echo ping" | grep -q "^ping$"; then
    echo "error: can't reach $MAC_USER@$MAC_HOST — run ./bootstrap.sh first." >&2
    exit 1
fi

echo "== cross-compiling browser-macos-appkit ($TARGET) =="
(cd "$REPO_ROOT" && ./.cargo/build-macos-appkit.sh "$TARGET")
APP_BIN="$REPO_ROOT/target/$TARGET/debug/browser-macos-appkit"
[ -x "$APP_BIN" ] || { echo "error: build succeeded but $APP_BIN is missing" >&2; exit 1; }

echo "== cross-compiling the web-standards driver ($TARGET) =="
(cd "$REPO_ROOT" && ./.cargo/build-macos-appkit.sh "$TARGET" -p web-standards-tests --bin web-standards-driver-macos)
DRIVER_BIN="$REPO_ROOT/target/$TARGET/debug/web-standards-driver-macos"
[ -x "$DRIVER_BIN" ] || { echo "error: build succeeded but $DRIVER_BIN is missing" >&2; exit 1; }

echo "== deploying to the Mac =="
mac_deploy_file "$APP_BIN"
mac_deploy_file "$DRIVER_BIN"
mac_run "rm -rf '$MAC_REMOTE_DIR/fixtures'"
mac_deploy_dir "$REPO_ROOT/web-standards-tests/fixtures"

echo "== running the web-standards driver on the Mac =="
if mac_run "cd '$MAC_REMOTE_DIR' && ./web-standards-driver-macos ./browser-macos-appkit ./fixtures"; then
    echo "-- web-standards-driver-macos: PASS --"
    overall_status=0
else
    echo "-- web-standards-driver-macos: FAIL --"
    overall_status=1
fi

if [ "$RUN_SMOKE" -eq 1 ]; then
    echo "== visual smoke test: launching the real app and screenshotting it =="
    mac_run "cd '$MAC_REMOTE_DIR' && (./browser-macos-appkit & echo \$! > app.pid); sleep 4; screencapture -x smoke.png; kill \$(cat app.pid) 2>/dev/null; rm -f app.pid"
    mkdir -p "$REPO_ROOT/target/macos-mac-screenshots"
    shot="$REPO_ROOT/target/macos-mac-screenshots/smoke-$(date +%Y%m%d-%H%M%S).png"
    mac_fetch_file "smoke.png" "$shot"
    echo "screenshot saved: $shot (eyeball it — this script can't tell a blank/crashed window from a real one on its own)"
fi

if [ "$overall_status" -eq 0 ]; then
    echo "== all good =="
else
    echo "== the driver reported a failure — see output above =="
fi
exit "$overall_status"
