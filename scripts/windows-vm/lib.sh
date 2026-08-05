#!/bin/bash
# Shared helpers for the windows-vm scripts. Not meant to be run directly —
# sourced by the others.
#
# The whole mechanism: dockur/windows (see README.md in this directory)
# already gives the VM a guest-only Samba share, `\\host.lan\Data`, backed
# by a bind-mounted host directory. A tiny polling loop batch script
# (poll.bat) runs inside the VM's real interactive desktop session (started
# once — see bootstrap.sh — persists across VM reboots via the Startup
# folder) and watches that share for a `request.flag`: when it appears, the
# loop runs whatever `cmd.bat` says, writes stdout+stderr to `output.txt`
# and the exit code to `exitcode.txt`, deletes `request.flag`, and touches
# `done.flag`. `run-in-vm.sh` is the other half: write `cmd.bat`, touch
# `request.flag`, poll for `done.flag`, print the result. No SSH, no RDP
# automation, no Rust/MSVC toolchain inside the VM — just a Samba share and
# a batch file, which is already all `cmd.exe`/PowerShell need.
set -euo pipefail

VM_CONTAINER="${VM_CONTAINER:-dockur-windows}"

# The host-side path backing \\host.lan\Data inside the VM — resolved from
# the running container's actual mounts, not hardcoded, so this keeps
# working regardless of which session/host directory dockur/windows was
# started with.
vm_shared_dir() {
    local dir
    dir=$(docker inspect "$VM_CONTAINER" --format '{{range .Mounts}}{{if eq .Destination "/shared"}}{{.Source}}{{end}}{{end}}' 2>/dev/null)
    if [ -z "$dir" ]; then
        echo "error: couldn't find a /shared mount on container '$VM_CONTAINER' — is it running? (docker ps)" >&2
        exit 1
    fi
    echo "$dir"
}

# Runs a batch command inside the VM via the shared-folder poll loop.
# Usage: vm_run <<'EOF'
#   echo hello
#   whoami
# EOF
# Prints stdout+stderr on success; on failure (including "the poller never
# picked this up") prints a diagnostic to stderr and returns non-zero.
#
# Every request uses a unique filename (cmd-<id>.bat, output-<id>.txt, ...),
# never the same path twice. The VM's SMB client caches file *content* per
# path and doesn't reliably notice when the host overwrites a file directly
# on disk (bypassing the SMB layer) — reusing one fixed cmd.bat name got
# stale, previously-run command output back instead of the fresh command's
# (confirmed directly: poll.bat kept answering a brand-new request with a
# previous request's output, even though the *existence* check on the flag
# file, a separate mechanism, was noticing new flags fine every time).
vm_run() {
    local shared timeout_secs=60
    shared=$(vm_shared_dir)
    local id
    id="$(date +%s%N)-$$"
    cat > "$shared/cmd-$id.bat"
    touch "$shared/request-$id.flag"

    local waited=0
    while [ ! -f "$shared/done-$id.flag" ]; do
        sleep 1
        waited=$((waited + 1))
        if [ "$waited" -ge "$timeout_secs" ]; then
            echo "error: no response from the VM's poll.bat after ${timeout_secs}s — is it still running inside the VM? See bootstrap.sh." >&2
            rm -f "$shared/cmd-$id.bat" "$shared/request-$id.flag"
            return 1
        fi
    done

    # dos2unix by hand: strip CR, and exitcode-*.txt has come back with a
    # trailing space after the digits (`echo %ERRORLEVEL%`'s own formatting,
    # not a bug on this side) — awk's field split handles both at once.
    tr -d '\r' < "$shared/output-$id.txt"
    local code
    code=$(tr -d '\r' < "$shared/exitcode-$id.txt" 2>/dev/null | awk '{print $1}')
    [ -z "$code" ] && code=1
    rm -f "$shared/cmd-$id.bat" "$shared/output-$id.txt" "$shared/exitcode-$id.txt" "$shared/done-$id.flag"
    return "$code"
}

# Copies a local file into the VM's C:\ClaudeBrowser\ via the shared folder
# (a real two-step copy: host -> shared folder -> VM's own C: drive, so the
# file survives even if the shared folder later becomes unavailable).
vm_deploy_file() {
    local src="$1"
    local shared name
    shared=$(vm_shared_dir)
    if [ ! -f "$src" ]; then
        echo "error: '$src' doesn't exist" >&2
        return 1
    fi
    # Built with printf, not a heredoc: an unquoted heredoc (`<<EOF`) treats
    # `\\` as an escaped backslash and silently collapses it to one,
    # anywhere it appears — including the literal `\\host.lan` UNC prefix
    # below, which needs to survive as a real double backslash. A
    # single-quoted printf format string doesn't get that treatment.
    name="$(basename "$src")"
    cp "$src" "$shared/$name"
    printf 'if not exist C:\\ClaudeBrowser mkdir C:\\ClaudeBrowser\ncopy /y "\\\\host.lan\\Data\\%s" "C:\\ClaudeBrowser\\%s"\n' "$name" "$name" | vm_run
}
