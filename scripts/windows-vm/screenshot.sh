#!/bin/bash
# Grabs a real screenshot of the VM's current display via QEMU's HMP
# monitor (the same mechanism dockur/windows' own web viewer at
# 127.0.0.1:8006 uses, just scripted) and saves it as a PNG at the given
# path. Useful after launching the real app inside the VM (see
# build-and-test.sh) for a genuine visual smoke test — "it launched and
# didn't crash" from exitcode.txt alone doesn't prove a window actually
# rendered.
#
# Usage: screenshot.sh <output.png>
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

VM_CONTAINER="${VM_CONTAINER:-dockur-windows}"
OUT="${1:?usage: screenshot.sh <output.png>}"

TMP_PPM="/tmp/windows-vm-screenshot-$$.ppm"
docker exec "$VM_CONTAINER" python3 -c "
import socket, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('/run/shm/monitor.sock')
time.sleep(0.3); s.recv(65536)
s.sendall(b'screendump $TMP_PPM\n')
time.sleep(1); s.recv(65536)
s.close()
"
docker cp "$VM_CONTAINER:$TMP_PPM" "$TMP_PPM"
docker exec "$VM_CONTAINER" rm -f "$TMP_PPM"

python3 -c "
from PIL import Image
Image.open('$TMP_PPM').save('$OUT')
"
rm -f "$TMP_PPM"
echo "saved $OUT"
