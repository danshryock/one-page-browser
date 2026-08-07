#!/bin/bash
# The main entry point: cross-compiles browser-windows-reactor and its test
# harness on Linux (via `cargo xwin`, no MSVC/Windows toolchain needed here),
# deploys both into the running VM over the shared folder, runs the real
# test binaries inside real Windows (not cross-compiled *and* run on Linux —
# these are genuine x86_64-pc-windows-msvc binaries, executed by the actual
# VM), and does a screenshot-based visual smoke test of the real app window.
#
# Prerequisites: the VM is running and poll.bat is responsive inside it —
# run bootstrap.sh first (idempotent, safe to run every time before this).
#
# Usage: build-and-test.sh [--no-smoke]
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
source ./lib.sh

REPO_ROOT="$(cd ../.. && pwd)"
RUN_SMOKE=1
[ "${1:-}" = "--no-smoke" ] && RUN_SMOKE=0

echo "== checking the VM's poller is up =="
if ! vm_run <<< "echo ping" | grep -q "^ping$"; then
    echo "error: poll.bat isn't responding — run ./bootstrap.sh first." >&2
    exit 1
fi

echo "== building browser-windows-reactor.exe =="
(cd "$REPO_ROOT" && cargo build-windows-reactor)
APP_EXE=$(find "$REPO_ROOT/target/x86_64-pc-windows-msvc/debug" -maxdepth 1 -name 'browser-windows-reactor.exe')
if [ -z "$APP_EXE" ]; then
    echo "error: build succeeded but couldn't find the .exe under target/x86_64-pc-windows-msvc/debug/" >&2
    exit 1
fi
echo "built: $APP_EXE"

echo "== building the test harness (compile-only cross-compile, not run here) =="
TEST_EXES=$(cd "$REPO_ROOT" && cargo xwin test --no-run --target x86_64-pc-windows-msvc \
    -p browser-windows-reactor --message-format=json \
    | jq -r 'select(.reason == "compiler-artifact" and .executable != null and .profile.test == true) | .executable')
if [ -z "$TEST_EXES" ]; then
    echo "error: no test executables came out of the xwin build" >&2
    exit 1
fi
echo "built:"
echo "$TEST_EXES" | sed 's/^/  /'

echo "== deploying to the VM =="
vm_deploy_file "$APP_EXE"
# The app is framework-dependent, not self-contained (see build.rs): these
# three files have to sit next to the exe or it fails to launch at all
# ("...bootstrap.dll was not found" / a silently blank WebView2 control —
# see build.rs's own comments for the history on both).
app_dir="$(dirname "$APP_EXE")"
for sidecar in Microsoft.Web.WebView2.Core.dll microsoft.windowsappruntime.bootstrap.dll resources.pri; do
    vm_deploy_file "$app_dir/$sidecar"
done
overall_status=0
while IFS= read -r exe; do
    vm_deploy_file "$exe"
done <<< "$TEST_EXES"

echo "== running tests inside the VM =="
while IFS= read -r exe; do
    name=$(basename "$exe")
    echo "-- $name --"
    if vm_run <<EOF
C:\ClaudeBrowser\\$name --test-threads=1
EOF
    then
        echo "-- $name: PASS --"
    else
        echo "-- $name: FAIL --"
        overall_status=1
    fi
done <<< "$TEST_EXES"

echo "== building the web-standards driver =="
(cd "$REPO_ROOT" && cargo build-web-standards-driver-windows)
DRIVER_EXE=$(find "$REPO_ROOT/target/x86_64-pc-windows-msvc/debug" -maxdepth 1 -name 'web-standards-driver-windows.exe')
if [ -z "$DRIVER_EXE" ]; then
    echo "error: build succeeded but couldn't find web-standards-driver-windows.exe under target/x86_64-pc-windows-msvc/debug/" >&2
    exit 1
fi
echo "built: $DRIVER_EXE"
vm_deploy_file "$DRIVER_EXE"

echo "== deploying web-standards-tests fixtures =="
# Copied onto the shared folder's host-side directory first (cheap, plain
# `cp`), then onto the VM's own C: drive (a real Windows path, not a UNC
# one) via `xcopy` — the driver's fixture-URL-derived `expected.txt` reads
# and the app's own `file:///C:/...` navigation both need a local path;
# `\\host.lan\Data\...` UNC file:// URLs are a separate, murkier can of
# worms this sidesteps entirely (never fully root-caused — see
# `seed-fixture-session.ps1`'s own doc comment for the actual navigation
# mechanism this now relies on instead).
cp -r "$REPO_ROOT/web-standards-tests/fixtures" "$(vm_shared_dir)/"
printf 'xcopy \\\\host.lan\\Data\\fixtures C:\\ClaudeBrowser\\fixtures\\ /E /I /Y\n' | vm_run

echo "== seeding the default profile's session with every fixture case already open =="
vm_deploy_file "./seed-fixture-session.ps1"
printf 'powershell -ExecutionPolicy Bypass -File C:\\ClaudeBrowser\\seed-fixture-session.ps1\n' | vm_run

echo "== running the web-standards driver inside the VM =="
# `printf`, not a heredoc — matches `vm_deploy_file`'s own established fix
# for the same trap: an unquoted heredoc collapses `\\` to a single `\`,
# which silently breaks the `\\...\` paths below.
if printf 'C:\\ClaudeBrowser\\web-standards-driver-windows.exe C:\\ClaudeBrowser\\%s C:\\ClaudeBrowser\\fixtures\n' "$(basename "$APP_EXE")" | vm_run
then
    echo "-- web-standards-driver-windows: PASS --"
else
    echo "-- web-standards-driver-windows: FAIL --"
    overall_status=1
fi

if [ "$RUN_SMOKE" -eq 1 ]; then
    echo "== visual smoke test: launching the real app =="
    app_name=$(basename "$APP_EXE")
    # `start ""` with an explicit empty title — a bare `start "C:\...\x.exe"`
    # misparses the path as the title on a UNC/quoted target (bit us once
    # already getting poll.bat itself launched, see bootstrap.sh's history in
    # ROADMAP.md/session notes). `start` returns immediately, so this cmd.bat
    # (and vm_run) doesn't block waiting for the GUI app to exit.
    vm_run <<EOF
start "" "C:\ClaudeBrowser\\$app_name"
EOF
    echo "waiting for it to render..."
    sleep 4
    mkdir -p "$REPO_ROOT/target/windows-vm-screenshots"
    shot="$REPO_ROOT/target/windows-vm-screenshots/smoke-$(date +%Y%m%d-%H%M%S).png"
    ./screenshot.sh "$shot"
    echo "screenshot saved: $shot (eyeball it — this script can't tell a blank/crashed window from a real one on its own)"

    echo "== closing the app =="
    vm_run <<EOF
taskkill /IM "$app_name" /F
EOF
fi

if [ "$overall_status" -eq 0 ]; then
    echo "== all good =="
else
    echo "== one or more test binaries failed — see output above =="
fi
exit "$overall_status"
