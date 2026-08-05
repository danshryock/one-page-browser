# Linux → Windows VM build/deploy/test pipeline

Cross-compiles `browser-windows-reactor` and its test suite on Linux, runs
both for real inside a local `dockur/windows` VM, and does a screenshot-based
visual smoke test — all without installing Rust, MSVC, or any build toolchain
inside the VM itself.

## How it works

`cargo xwin` (already used by the `cargo build-windows-reactor` alias, see
the repo root `.cargo/config.toml`) cross-compiles genuine
`x86_64-pc-windows-msvc` binaries — the app itself and the test harness —
entirely on Linux. The only things that ever need to run *inside* Windows are
those binaries themselves, so the VM never needs a Rust toolchain, MSVC, or
even `git`.

Getting files in and running them is done through the one host↔VM channel
`dockur/windows` already provides for free: a guest-only Samba share,
`\\host.lan\Data`, backed by a bind-mounted host directory (see
`docker inspect dockur-windows`'s `Mounts`). A tiny batch-file polling loop
(`poll.bat`) runs inside the VM's real interactive desktop session, watches
that share for a request, runs it, and writes the result back. No SSH, no
RDP automation — see `lib.sh`'s header comment for the full protocol.

`poll.bat` has to run in the VM's real interactive session (not a SYSTEM-context
Scheduled Task) because of Windows' Session 0 isolation: a SYSTEM process
can build things but can't usefully launch or interact with a GUI window,
which the visual smoke test in `build-and-test.sh` needs. That's also why
starting it the very first time needs synthetic keystrokes rather than a
remote-exec mechanism — see `bootstrap.sh`.

## Prerequisites (one-time)

1. `dockur/windows` already running as the `dockur-windows` container, with
   Windows installed and logged in to a real desktop session. (Doing the
   Windows install itself isn't scripted here — it needs an interactive
   GUI/VNC session at `127.0.0.1:8006` the one time you set the VM up.)
2. `jq` on the Linux side (used to parse `cargo xwin`'s `--message-format=json`
   output).
3. `python3` + Pillow (`PIL`) on the Linux side, for `screenshot.sh`'s PPM→PNG
   conversion. `python3` itself is also expected already inside the VM
   container (it ships in the `dockurr/windows` image).
4. **Windows App SDK Runtime 2.3.1 installed in the VM** — the app links
   against it (see `windows-reactor-setup`'s `RUNTIME_VER`), and unlike
   VCRUNTIME140 (see below) this one really is a runtime dependency, not
   something a build flag can make go away. One-time, from inside the VM
   (`vm_run`, or interactively):
   ```
   curl.exe -sS -L -o wasdk.exe https://aka.ms/windowsappsdk/2.3/2.3.1/windowsappruntimeinstall-x64.exe
   wasdk.exe --quiet
   ```
   (`curl.exe`'s `-L` matters — the `aka.ms` link 301s to the real
   `download.microsoft.com` URL, and a plain `curl.exe -o` without it silently
   downloads a 0-byte file. Also note the *installer's* own flag is
   `--quiet`/`-q`, not `/quiet` — it prints its own usage and does nothing if
   you get that wrong.) Without this, the app doesn't just fail to render —
   it doesn't start at all, with "Required components of the Windows App
   Runtime are missing".

Cargo binaries built for `x86_64-pc-windows-msvc` are already statically
linked against the MSVC C runtime (`target.x86_64-pc-windows-msvc.rustflags`
in the repo-root `.cargo/config.toml`), so — unlike the Windows App SDK
Runtime above — a fresh VM does *not* need the Visual C++ Redistributable
installed just to run these binaries. That wasn't always true here: this
pipeline's own first real run hit "VCRUNTIME140.dll was not found" on a
totally clean VM, which is what prompted adding `crt-static` in the first
place.

## Usage

```sh
./bootstrap.sh        # idempotent — makes sure poll.bat is running in the VM
./build-and-test.sh   # build, deploy, run tests, screenshot the real app
```

Run `bootstrap.sh` any time you're not sure the poller is still up (after a
VM reboot, for instance) — it's cheap and does nothing if the poller already
responds. `build-and-test.sh` assumes it's already been done and will just
fail fast with a clear message if not.

`build-and-test.sh --no-smoke` skips the "launch the real app and screenshot
it" step and only runs the test binaries — faster for a quick pass/fail check.

### What `build-and-test.sh` actually does

1. `cargo build-windows-reactor` — builds `browser-windows-reactor.exe`.
2. `cargo xwin test --no-run --target x86_64-pc-windows-msvc -p browser-windows-reactor`
   — builds (doesn't run) the test-harness `.exe`s, one per `src/lib.rs` and
   `src/main.rs` test target. The exact executable paths are read out of
   `--message-format=json` rather than guessed from the hash-suffixed
   filenames, so this doesn't break if Cargo's hashing changes.
3. Deploys the app + both test binaries into the VM's `C:\ClaudeBrowser\` via
   the shared folder (`lib.sh`'s `vm_deploy_file`) — plus three sidecar files
   that have to sit next to the app exe or it won't even launch
   (`Microsoft.Web.WebView2.Core.dll`, `microsoft.windowsappruntime.bootstrap.dll`,
   `resources.pri` — the app is framework-dependent, not self-contained; see
   `browser-windows-reactor/build.rs`'s own comments for why).
4. Runs each test binary for real inside the VM and reports pass/fail — this
   is genuine Windows execution, not a cross-compile-and-hope.
5. Unless `--no-smoke`: launches the real app inside the VM, waits a few
   seconds, grabs a real screenshot via QEMU's HMP `screendump` (the same
   mechanism dockur/windows' own web viewer uses), and kills the app. The
   screenshot is saved under `target/windows-vm-screenshots/` — eyeball it;
   the script only confirms the process launched and didn't immediately
   error out, not that the window actually rendered correctly.

## Files

| File | Purpose |
|---|---|
| `lib.sh` | Shared helpers (`vm_shared_dir`, `vm_run`, `vm_deploy_file`) — sourced, not run directly. |
| `bootstrap.sh` | Idempotent: makes sure `poll.bat` is running inside the VM, bootstrapping it via synthetic keystrokes if not, and installs a Startup-folder entry so it (best-effort) survives VM reboots. |
| `poll.bat` | Runs inside the VM; polls the shared folder for work. |
| `type-in-vm.py` | Synthetic-keystroke typer via QEMU's HMP monitor socket — only used by `bootstrap.sh`'s one-time setup. |
| `screenshot.sh` | Grabs a real screenshot of the VM's current display. |
| `close_dialog.ps1` | Recovery tool (not part of the normal flow) — force-closes a stuck error dialog by window title. See its own header comment and "If poll.bat stops responding" below. |
| `build-and-test.sh` | The main orchestrator — see above. |

## If poll.bat stops responding

`vm_run` times out after 60s with "no response from the VM's poll.bat". The
most likely cause: something the poller `call`ed is blocked on a GUI dialog
(a crashed launch, a missing-dependency error) — `call` doesn't return until
the child process does, so poll.bat itself is stuck, not crashed. Confirm
with `./screenshot.sh` before assuming something deeper is wrong.

Ordinary interaction doesn't reliably reach these dialogs from here — mouse
clicks need QEMU absolute-tablet coordinates scaled to the VM's actual
resolution (`pixel / screen_dim * 32767`, not raw pixels), and the *screen's
cursor itself often doesn't show up in `screendump` captures at all*, so you
can't calibrate by eye. Worse, the dialog isn't always owned by the process
you'd expect: a classic "X.dll was not found" hard-error box is shown by
`csrss.exe` (confirmed via `tasklist /V` — the owning PID was a "critical
system process" `taskkill` refused to touch), not by the crashed exe, so
killing the exe doesn't close it.

What reliably works: open a fresh `cmd` via the same keystroke mechanism
`bootstrap.sh` uses (Win+R → `cmd` → Enter — see its `type-in-vm.py` calls),
then from that fresh, definitely-focused window, deploy and run
`close_dialog.ps1` (`vm_deploy_file` + `vm_run <<< "powershell
-ExecutionPolicy Bypass -File C:\ClaudeBrowser\close_dialog.ps1"`). It sends
`WM_CLOSE` straight to the window handle via `SendMessage`, which doesn't
need focus or a real click. If the dialog belongs to the app itself (not
csrss), `taskkill /IM browser-windows-reactor.exe /F` also works and is
simpler — try that first.

## Known limitations

- The Startup-folder auto-launch `bootstrap.sh` installs (so `poll.bat`
  survives a VM reboot without re-running the keystroke bootstrap) hasn't
  been verified against a real reboot — if the VM ever comes back up
  unresponsive, just run `bootstrap.sh` again; it'll notice and re-bootstrap.
- `bootstrap.sh`'s keystroke-driving step needs the VM's desktop session to
  actually be idle/unlocked (nothing else has focus) — it doesn't check this
  first. If it doesn't work, check the VM's screen (`127.0.0.1:8006`, or
  `./screenshot.sh`) before assuming something deeper is wrong.
- `type-in-vm.py` resolves every character to a keystroke *before* sending
  anything, specifically so an unmappable character fails loudly instead of
  leaving a half-typed command sitting in whatever has focus in the VM
  (that half-typed-command state is confusing to recover from — learned the
  hard way). If you hit "no mapping for char", add it to `CHARMAP` rather
  than working around it by hand.
- `browser-windows-winui` isn't covered here — it was deleted (see
  `ROADMAP.md`). This pipeline is `browser-windows-reactor`-only.
