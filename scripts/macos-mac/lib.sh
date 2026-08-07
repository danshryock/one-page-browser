#!/bin/bash
# Shared helpers for the macos-mac scripts. Not meant to be run directly —
# sourced by the others.
#
# Unlike scripts/windows-vm/ (which has to fake a remote-exec channel out of
# a Samba share and a polling batch file — see its own lib.sh header for
# why), a real Mac has OpenSSH built in. So this is just thin wrappers
# around `ssh`/`scp` against a physical machine on the same LAN — no shared
# folder, no polling loop, no synthetic keystrokes.
#
# Configuration is env vars, not hardcoded, since (unlike the Windows VM,
# which is a fixed local Docker container name) this is a specific piece of
# hardware whose hostname/username only you know:
#   MAC_HOST     required — e.g. "dans-macbook.local" (mDNS) or a LAN IP.
#   MAC_USER     required — the macOS account name Remote Login is enabled
#                for (see README.md's Phase 0 checklist).
#   MAC_SSH_KEY  optional — defaults to the dedicated key generated for this
#                purpose, ~/.ssh/id_ed25519_claude_browser_mac_test.
#   MAC_REMOTE_DIR
#                optional — where files get deployed on the Mac, defaults to
#                ~/ClaudeBrowserTests.
set -euo pipefail

MAC_SSH_KEY="${MAC_SSH_KEY:-$HOME/.ssh/id_ed25519_claude_browser_mac_test}"
MAC_REMOTE_DIR="${MAC_REMOTE_DIR:-ClaudeBrowserTests}"

mac_require_config() {
    if [ -z "${MAC_HOST:-}" ]; then
        echo "error: MAC_HOST isn't set — export it (e.g. export MAC_HOST=dans-macbook.local)" >&2
        exit 1
    fi
    if [ -z "${MAC_USER:-}" ]; then
        echo "error: MAC_USER isn't set — export it (the macOS account name)" >&2
        exit 1
    fi
    if [ ! -f "$MAC_SSH_KEY" ]; then
        echo "error: SSH key not found at $MAC_SSH_KEY — see README.md's Phase 0 checklist" >&2
        exit 1
    fi
}

# `accept-new`, not the default `ask`: this only ever targets one specific,
# known piece of hardware you have physical access to (unlike a public host),
# so trust-on-first-use is a reasonable default that also keeps these
# scripts non-interactive. It still refuses silently on a *changed* key
# (e.g. the Mac got reimaged), which is the case actually worth stopping for.
mac_ssh_opts() {
    echo -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -i "$MAC_SSH_KEY"
}

mac_target() {
    mac_require_config
    echo "$MAC_USER@$MAC_HOST"
}

# Runs a shell command on the Mac and streams stdout/stderr straight
# through — real SSH, so no timeout-and-poll dance like vm_run needs.
# Usage: mac_run "command"  or  mac_run <<'EOF' ... EOF (piped to `sh -s`).
mac_run() {
    local opts
    read -r -a opts <<< "$(mac_ssh_opts)"
    if [ "$#" -eq 0 ]; then
        ssh "${opts[@]}" "$(mac_target)" sh -s
    else
        ssh "${opts[@]}" "$(mac_target)" "$@"
    fi
}

# Copies a local file into $MAC_REMOTE_DIR on the Mac, creating it first if
# needed, then strips any quarantine xattr and ad-hoc codesigns it. `scp`
# doesn't set com.apple.quarantine itself (that's a downloaded-via-browser/
# curl thing, from LSQuarantine), so Gatekeeper shouldn't have an opinion
# here — but ad-hoc signing costs nothing and removes a whole class of
# "worked yesterday, silently refused to launch today" flakiness if it ever
# does. `xattr -d` on a file with no such attribute exits non-zero, hence
# the `|| true`.
mac_deploy_file() {
    local src="$1"
    local name
    name="$(basename "$src")"
    if [ ! -f "$src" ]; then
        echo "error: '$src' doesn't exist" >&2
        return 1
    fi
    mac_run "mkdir -p '$MAC_REMOTE_DIR'"
    local opts
    read -r -a opts <<< "$(mac_ssh_opts)"
    scp -q "${opts[@]}" "$src" "$(mac_target):$MAC_REMOTE_DIR/$name"
    mac_run "xattr -d com.apple.quarantine '$MAC_REMOTE_DIR/$name' 2>/dev/null; codesign --force -s - '$MAC_REMOTE_DIR/$name' 2>/dev/null; chmod +x '$MAC_REMOTE_DIR/$name'; true"
}

# Recursively copies a local directory into $MAC_REMOTE_DIR/<name> on the
# Mac (used for the fixtures/ tree — many small files, not one binary).
mac_deploy_dir() {
    local src="$1"
    local name
    name="$(basename "$src")"
    if [ ! -d "$src" ]; then
        echo "error: '$src' doesn't exist" >&2
        return 1
    fi
    mac_run "mkdir -p '$MAC_REMOTE_DIR'"
    local opts
    read -r -a opts <<< "$(mac_ssh_opts)"
    scp -q -r "${opts[@]}" "$src" "$(mac_target):$MAC_REMOTE_DIR/$name"
}

# Pulls a file back from the Mac to a local path (screenshots, mainly).
mac_fetch_file() {
    local remote_name="$1" dest="$2"
    local opts
    read -r -a opts <<< "$(mac_ssh_opts)"
    scp -q "${opts[@]}" "$(mac_target):$MAC_REMOTE_DIR/$remote_name" "$dest"
}
