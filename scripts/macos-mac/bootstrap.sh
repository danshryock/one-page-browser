#!/bin/bash
# Idempotent connectivity check for the macos-mac pipeline — makes sure the
# Mac is reachable and looks correctly configured before build-and-test.sh
# bothers cross-compiling anything. See README.md's Phase 0 checklist for
# the one-time physical setup this assumes is already done (Remote Login,
# automatic login, sleep disabled, this pipeline's SSH key authorized, and
# — separately, can't be checked remotely — the one manual Accessibility/
# Input Monitoring permission click for the driver binary).
#
# Usage: ./bootstrap.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
source ./lib.sh

echo "== checking SSH reachability =="
if ! mac_run "echo ping" | grep -q "^ping$"; then
    echo "error: couldn't reach $MAC_USER@$MAC_HOST over SSH." >&2
    echo "Check: Remote Login is on, the SSH key is in ~/.ssh/authorized_keys," >&2
    echo "the Mac is awake, and MAC_HOST/MAC_USER are set correctly." >&2
    exit 1
fi
echo "ok"

echo "== checking there's a real logged-in GUI session =="
# `stat -f%Su /dev/console` prints the console (loginwindow) session's owner
# — the same check `launchd`-adjacent tooling uses to tell "someone's really
# logged into the desktop" apart from "just an SSH session exists". This
# matters because browser-macos-appkit is a real windowed AppKit app: if
# nobody's logged into Aqua (e.g. automatic login isn't actually on, or the
# Mac is sitting at the login screen), it can still *launch* over SSH but
# has no WindowServer session to render into or receive synthetic input
# through.
console_user=$(mac_run "stat -f%Su /dev/console" 2>/dev/null || echo "")
if [ -z "$console_user" ] || [ "$console_user" = "root" ]; then
    echo "warning: no real user appears logged into the console (/dev/console owner: '${console_user:-unknown}')." >&2
    echo "Check System Settings > Users & Groups > Login Options > Automatic login." >&2
else
    echo "ok (console user: $console_user)"
fi

echo "== checking macOS/arch =="
mac_run "sw_vers -productVersion; uname -m"

echo "== ensuring remote deploy dir exists =="
mac_run "mkdir -p '$MAC_REMOTE_DIR'"
echo "ok ($MAC_REMOTE_DIR)"

echo "== all good — run ./build-and-test.sh next =="
