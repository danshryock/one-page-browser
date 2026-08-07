# Linux → real Mac build/deploy/test pipeline

Cross-compiles `browser-macos-appkit` and the web-standards test driver on
Linux (via `cargo zigbuild`, see the repo root `.cargo/build-macos-appkit.sh`),
then deploys and runs both for real on actual Apple hardware over plain SSH —
no VM, no emulation, no Rust toolchain needed on the Mac itself.

## How it works

Unlike `scripts/windows-vm/` (which has to fake a remote-exec channel out of
a Samba share and a polling batch file — see its own README for why), a real
Mac ships a real SSH daemon. So this is genuinely just `ssh`/`scp` against a
specific piece of hardware: `lib.sh`'s `mac_run`/`mac_deploy_file` are thin
wrappers, nothing more.

The one thing SSH alone doesn't get you: `browser-macos-appkit` is a real
windowed AppKit app, and `web-standards-driver-macos` drives it with real
synthetic mouse clicks (`CGEvent`, posted to the HID event system) — both
need an actual logged-in GUI (Aqua/WindowServer) session to render into and
inject input through, which a bare SSH login doesn't provide on its own
unless someone is *already* logged into the console. That's what most of the
Phase 0 checklist below is actually about.

## Prerequisites (one-time, physical)

These need to happen once, at the laptop itself — none of it can be done
over SSH before SSH is even configured.

1. **Turn on Remote Login.** System Settings → General → Sharing → Remote
   Login: on. Note the computer's name shown there — that's what `<name>.local`
   resolves to over mDNS on the LAN. Note the account name too (`MAC_USER`
   below).
2. **Turn on automatic login.** System Settings → Users & Groups → Login
   Options → Automatic login: set to the test account. Without this, the
   Mac boots to the login screen with nobody in the console session, and
   AppKit apps launched over SSH have no WindowServer to render into or
   receive clicks through — they may not even error, just silently fail to
   show a window. `bootstrap.sh` checks for a real console session and
   warns if this isn't set.
3. **Disable sleep on power.** System Settings → Battery/Energy → Power
   Adapter: turn off "Put hard disks to sleep" / enable "Prevent automatic
   sleeping when the display is off", or just run once, logged in locally:
   ```
   sudo pmset -c sleep 0
   ```
   System sleep drops the SSH connection *and* suspends the WindowServer
   session — either breaks this whole pipeline until someone's physically
   there to wake it again.
4. **Authorize this pipeline's SSH key.** A dedicated key (not your personal
   one) was generated for this — append its public half to the Mac's
   `~/.ssh/authorized_keys` (create the file/`.ssh` dir if it doesn't exist,
   `chmod 700 ~/.ssh` / `chmod 600 ~/.ssh/authorized_keys`):
   ```
   ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIaOuwa9Z2atiKTiEekk0DRhMyTaPwWJntA7weZMEVjB claude-browser-mac-testing
   ```
   Easiest path if you can already get one interactive SSH/password session
   or physical Terminal access:
   ```
   echo "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIaOuwa9Z2atiKTiEekk0DRhMyTaPwWJntA7weZMEVjB claude-browser-mac-testing" >> ~/.ssh/authorized_keys
   ```
5. **Grant Accessibility/Input Monitoring permission, once, by hand.**
   `web-standards-driver-macos` posts synthetic `CGEvent`s, which macOS
   gates behind a TCC permission prompt the very first time — this can't be
   pre-approved remotely on stock macOS. After the first deploy
   (`./build-and-test.sh`), physically launch
   `~/ClaudeBrowserTests/web-standards-driver-macos` once at the laptop (or
   just let a `build-and-test.sh` run fail once) and click "Allow" on the
   Accessibility prompt (System Settings → Privacy & Security →
   Accessibility, `web-standards-driver-macos` should appear there —
   confirm it's toggled on). Persists across future runs.

## Usage

From this directory:

```sh
export MAC_HOST=your-mac-name.local   # or a LAN IP
export MAC_USER=your-macos-username
./bootstrap.sh        # checks SSH reachability + a real console session
./build-and-test.sh   # cross-compile, deploy, run the real driver, screenshot
```

`build-and-test.sh --no-smoke` skips the launch-and-screenshot step and only
runs the driver — faster for a quick pass/fail check.

`MAC_SSH_KEY` (default `~/.ssh/id_ed25519_claude_browser_mac_test`) and
`MAC_REMOTE_DIR` (default `~/ClaudeBrowserTests`) are also overridable — see
`lib.sh`'s header comment.

### What `build-and-test.sh` actually does

1. `.cargo/build-macos-appkit.sh x86_64-apple-darwin` — cross-compiles
   `browser-macos-appkit`. `x86_64`, not `aarch64`: a 2015 Mac is Intel.
2. Same script again with `-p web-standards-tests --bin web-standards-driver-macos`
   — cross-compiles the driver.
3. Deploys both binaries plus `web-standards-tests/fixtures/` to
   `$MAC_REMOTE_DIR` on the Mac (`lib.sh`'s `mac_deploy_file`/`mac_deploy_dir`
   — plain `scp`, with a quarantine-xattr strip + ad-hoc codesign as a
   defensive no-op against Gatekeeper).
4. Runs `web-standards-driver-macos` for real over SSH — genuine execution
   on real Apple hardware, driving the real app with real `CGEvent` clicks,
   reading its real `console.log` output relayed to stdout.
5. Unless `--no-smoke`: launches the app, waits, grabs a real screenshot via
   the built-in `screencapture` CLI, and pulls it back to
   `target/macos-mac-screenshots/` — eyeball it, same caveat as the Windows
   pipeline's smoke test (confirms the process launched and rendered
   *something*, not that it's pixel-correct).

## Files

| File | Purpose |
|---|---|
| `lib.sh` | Shared helpers (`mac_run`, `mac_deploy_file`, `mac_deploy_dir`, `mac_fetch_file`) — sourced, not run directly. |
| `bootstrap.sh` | Idempotent: SSH reachability, real-console-session check, macOS version/arch, remote dir setup. |
| `build-and-test.sh` | The main orchestrator — see above. |

## Also covered in CI

The same driver + fixtures also run on GitHub's real `macos-13` (Intel,
matching this same `x86_64-apple-darwin` target) and `macos-14` (Apple
Silicon) Actions runners — see `.github/workflows/macos.yml`'s
`web-standards-macos` job. That path needs none of this directory (GitHub's
runners come pre-logged-into a real console session), but running the exact
same driver+fixtures against a genuinely different piece of hardware here is
still worth doing: real hardware, real display geometry, real timing, and
the only way to validate `web-standards-tests/src/bin/macos_driver.rs`'s
`content_area_origin()` calibration is against something CI itself can't
promise stays identical to your laptop's actual window chrome.

## Known limitations

- `content_area_origin()` in `macos_driver.rs` is calibrated by eye against
  whatever `--no-smoke`'s screenshot shows — same as `windows_driver.rs`'s
  `switcher_button_pos`/`search_box_pos` needed a real screenshot session to
  get right rather than being guessed correctly on the first try. Expect to
  need at least one calibration pass against a real screenshot from this
  laptop specifically.
- No equivalent of `windows-vm`'s `close_dialog.ps1` recovery tool yet — if
  a run leaves a crashed/stuck dialog on the Mac's screen, the fix for now
  is `mac_run "killall browser-macos-appkit web-standards-driver-macos"` or
  a real screenshot (`screencapture`) to see what's actually on screen.
- The Accessibility/Input Monitoring permission grant (Phase 0, step 5) is
  tied to the exact binary path/signature. Re-signing or moving the deployed
  driver binary may require re-granting it.
