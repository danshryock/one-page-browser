#!/bin/bash
# Idempotent: makes sure poll.bat (see that file's own comment) is running
# inside the VM's real interactive desktop session. Safe to run any time —
# if the poller already responds, this is a no-op; only touches the VM via
# synthetic keystrokes (QEMU HMP `sendkey`) if it doesn't.
#
# Needs the VM already installed and logged in to a real desktop session —
# this doesn't do the Windows install itself (see README.md's "Prerequisites"
# section for that one-time setup, which does need a GUI/VNC session at
# 127.0.0.1:8006).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
source ./lib.sh

echo "checking whether poll.bat is already running..."
if vm_run <<< "echo ping" > /tmp/windows-vm-bootstrap-check.$$ 2>/dev/null; then
    if grep -q "^ping$" /tmp/windows-vm-bootstrap-check.$$; then
        rm -f /tmp/windows-vm-bootstrap-check.$$
        echo "already running — nothing to do."
        exit 0
    fi
fi
rm -f /tmp/windows-vm-bootstrap-check.$$
echo "not responding — bootstrapping via synthetic keystrokes (one-time; needs the VM's desktop session to be idle/unlocked)."

shared=$(vm_shared_dir)
cp poll.bat "$shared/poll.bat"

docker cp type-in-vm.py "$VM_CONTAINER:/tmp/type-in-vm.py"
type_text() {
    docker exec -i "$VM_CONTAINER" python3 /tmp/type-in-vm.py
}

echo "opening Run dialog..."
docker exec "$VM_CONTAINER" python3 -c "
import socket, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('/run/shm/monitor.sock')
time.sleep(0.3); s.recv(65536)
s.sendall(b'sendkey meta_l-r\n')
time.sleep(0.5); s.recv(65536)
s.close()
"
sleep 1

echo "opening a command prompt..."
printf 'cmd\n' | type_text
sleep 2

echo "launching poll.bat..."
printf '\\\\host.lan\\Data\\poll.bat\n' | type_text
sleep 2

echo "verifying..."
if vm_run <<< "echo ping" | grep -q "^ping$"; then
    echo "poll.bat is now running."
else
    echo "still not responding — check the VM's screen (127.0.0.1:8006, or ./screenshot.sh) and see this script's comments." >&2
    exit 1
fi

echo "installing a Startup-folder entry so this survives VM reboots (best-effort — not verified against a real reboot; if the VM ever comes back up unresponsive, just re-run this script)..."
vm_deploy_file poll.bat
vm_run <<'EOF'
(
  echo @echo off
  echo start /min "" "C:\ClaudeBrowser\poll.bat"
) > "%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\claudebrowser-poll.bat"
EOF

echo "done."
