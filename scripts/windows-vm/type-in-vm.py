#!/usr/bin/env python3
"""Types stdin into the Windows VM as synthetic keystrokes, via QEMU's HMP
monitor socket (bind-mounted into the dockur/windows container at
/run/shm/monitor.sock, no host-side port needed). Only used for the one-time
bootstrap.sh setup (getting poll.bat started for the first time, or after a
VM reboot if the Startup-folder entry didn't fire) — everything else talks
to the VM through the shared-folder poll loop instead (see lib.sh), which
doesn't need any of this.

Meant to be run with `docker exec -i <container> python3 type-in-vm.py`
(python3 is already present in the dockur/windows image itself; nothing
extra to install). Ends stdin with a trailing newline to submit an Enter
keypress, same as typing into a real terminal/dialog and pressing Enter.
"""
import socket, sys, time

SOCK = "/run/shm/monitor.sock"

CHARMAP = {
    ' ': 'spc', '\n': 'ret',
    '/': 'slash', '\\': 'backslash',
    ':': 'shift-semicolon', ';': 'semicolon',
    '-': 'minus', '_': 'shift-minus',
    '.': 'dot', ',': 'comma',
    '*': 'shift-8', '&': 'shift-7', '>': 'shift-dot', '<': 'shift-comma',
    '(': 'shift-9', ')': 'shift-0',
    '=': 'equal', '+': 'shift-equal',
    "'": 'apostrophe', '"': 'shift-apostrophe',
    '|': 'shift-backslash',
    '{': 'shift-bracket_left', '}': 'shift-bracket_right',
    '[': 'bracket_left', ']': 'bracket_right',
    '%': 'shift-5', '#': 'shift-3', '@': 'shift-2', '!': 'shift-1',
    '^': 'shift-6', '~': 'shift-grave_accent', '`': 'grave_accent',
    '$': 'shift-4',
}

def keyname(c):
    if c in CHARMAP:
        return CHARMAP[c]
    if c.isupper():
        return f'shift-{c.lower()}'
    if c.isalnum():
        return c
    raise ValueError(f"no mapping for char {c!r}")

def main():
    text = sys.stdin.read()
    # Resolve every keyname up front — an unmapped char raising mid-loop
    # already bit us once: it left a half-typed command sitting in whatever
    # had focus in the VM, which then got walked into by the *next* attempt
    # and produced a garbled, hard-to-diagnose mess. Fail before sending
    # anything instead.
    keys = [keyname(c) for c in text]
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(SOCK)
    time.sleep(0.3)
    s.recv(65536)
    for k in keys:
        s.sendall(f"sendkey {k}\n".encode())
        time.sleep(0.04)
        s.recv(65536)
    s.close()

if __name__ == "__main__":
    main()
