# Linux → real Mac build/deploy/test pipeline

Cross-compiles `browser-macos-appkit` and the web-standards test driver on
Linux (via `cargo zigbuild`, see the repo root `.cargo/build-macos-appkit.sh`),
then deploys and runs both for real on actual Apple hardware over plain SSH —
no VM, no emulation, no Rust toolchain needed on the Mac itself. Verified
end-to-end against a real 2014 Intel MacBook running macOS 11 (Big Sur).

## How it works

Unlike `scripts/windows-vm/` (which has to fake a remote-exec channel out of
a Samba share and a polling batch file — see its own README for why), a real
Mac ships a real SSH daemon. So this is genuinely just `ssh`/`scp` against a
specific piece of hardware: `lib.sh`'s `mac_run`/`mac_deploy_file` are thin
wrappers, nothing more.

The interactions `web-standards-driver-macos` performs (open the switcher,
type a URL, click a link) don't go through OS-level synthetic input at all —
`browser-macos-appkit` accepts a `--test-command-socket <path>` flag that
starts a local Unix-socket listener (`AppState::start_test_command_listener`,
`crates/browser-macos-appkit/src/lib.rs`) calling the same internal methods a
real keypress/click would eventually reach. This exists specifically because
the alternative — `CGEvent`-based synthetic input — needs Accessibility/Input
Monitoring TCC permission, and that turned out to be a genuine dead end for
an SSH-driven workflow: confirmed directly (not assumed) that macOS requires
a live GUI session for every private-key/keychain operation involved in
setting up even a *stable* signing identity, which is what would otherwise
be needed to stop a TCC grant from being invalidated by every rebuild. The
command-socket approach sidesteps all of it — no TCC dependency, no signing
concerns, works identically on every macOS version.

`web-standards-driver-macos` still supports the original `CGEvent`-based path
too (`WEB_STANDARDS_MACOS_CLICK_MODE=native`), kept for direct comparison —
but expect it to need a manual Accessibility grant that won't survive a
rebuild of either binary (see Known limitations).

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
   AppKit apps launched over SSH have no WindowServer to render into. This
   is *necessary but not sufficient* — see the next step.
3. **Disable the screen lock/screensaver password, not just sleep.** These
   are two separate settings and both matter: `sudo pmset -c sleep 0`
   disables *system* sleep, but the screen can still lock on its own timer
   (Security & Privacy → General, or the Screen Saver/Lock Screen settings)
   even while the system stays awake and SSH stays reachable. Confirmed
   directly: a locked session still answers SSH and still shows a real
   console user in `bootstrap.sh`'s check, but the app's page JavaScript
   (including the console-capture shim) appears to stop executing while
   locked — every command reached the app fine, nothing the page itself did
   ever produced output, until the screen was unlocked. Disable both, not
   just sleep.
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

That's the whole checklist for the default (command-socket) path — no
Accessibility/TCC permission grant needed at all, and nothing here is tied
to a specific binary build, so none of it needs repeating after a rebuild.

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
   `browser-macos-appkit`. `x86_64`, not `aarch64`: a 2014 Mac is Intel.
2. Same script again with `-p web-standards-tests --bin web-standards-driver-macos`
   — cross-compiles the driver.
3. Deploys both binaries plus `web-standards-tests/fixtures/` to
   `$MAC_REMOTE_DIR` on the Mac (`lib.sh`'s `mac_deploy_file`/`mac_deploy_dir`
   — plain `scp`, with a quarantine-xattr strip + ad-hoc codesign as a
   defensive no-op against Gatekeeper). Skips the deploy entirely for a file
   whose content hasn't changed since the last run (see Known limitations —
   this matters more than it sounds like it should).
4. Runs `web-standards-driver-macos` for real over SSH — genuine execution
   on real Apple hardware, driving the real app over its local command
   socket, reading its real `console.log` output relayed to stdout. The
   driver itself starts a local `http://127.0.0.1` server for the fixtures
   (see Known limitations' `file://` note) rather than navigating to
   `file://` URLs directly.
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

`.github/workflows/macos.yml`'s `web-standards-macos` job runs the exact same
driver, in the same default (command-socket) mode, on GitHub's real
`macos-13`/`macos-14` Actions runners — no SSH involved there (the runner
already has its own console session), but the same fixtures, same driver
binary logic, same assertions. This only became possible once the driver
stopped needing `CGEvent`/TCC at all: an earlier version of that job could
only compile-check the driver, since GitHub's runners are fully ephemeral
and can never have Accessibility permission granted to a freshly-built
binary (see that workflow's own git history for the abandoned attempts).
Running the exact same driver+fixtures against a genuinely different piece
of hardware here is still worth doing on top of CI: real hardware, real
display geometry, real timing, an actual older macOS version CI's runner
matrix doesn't cover, and the only way to validate anything `native` click
mode still depends on (`content_area_origin()`'s calibration).

## Real bugs found this way

Testing on genuine, older hardware (not just cross-compiling and hoping)
surfaced several real, previously-unknown bugs no amount of Linux-side
testing could have caught:

- **`browser-macos-appkit`**: `sender_tag` only handled `NSButton` senders,
  so every keyboard-shortcut-triggered action (menu key equivalents send an
  `NSMenuItem` as the sender, not a `NSButton`) silently did nothing —
  toolbar buttons worked, keyboard shortcuts never did, on any macOS
  version. `NSApplication::activate()` (used unconditionally) doesn't exist
  before macOS 14 Sonoma — an uncatchable Objective-C exception that aborted
  the whole process on Big Sur.
- **`wry`** (this repo's cross-compile pins a specific upstream commit, see
  `render-engine/Cargo.toml`): a `WKUIDelegate` override
  (`requestMediaCapturePermissionForOrigin:...`) doesn't exist before macOS
  12 and crashed the app outright on launch on Big Sur — fixed upstream, but
  only via *build-machine* OS detection, which never fires when cross-
  compiling from Linux (worked around via a forced `--cfg`, see
  `.cargo/config.toml`). Separately, macOS `navigate_to_url` uses a plain
  `loadRequest:` for every URL scheme including `file://`, which WKWebView
  doesn't reliably load that way at all — navigation silently never
  committed. Fixed the same way as the analogous GTK-side wry bug: serve
  fixtures over a local `http://127.0.0.1` server instead of `file://` URLs
  (`FixtureServer` in `macos_driver.rs`, mirroring
  `browser-linux-gtk3/tests/gtk_tests.rs`'s).
- **`crates/browser-core/src/profile.rs`**: `resolve_url_argument` (the
  shared `--url`-vs-flag argument scanner every front end's `main.rs` uses)
  didn't know to skip `--test-command-socket`'s value, so it mistook the
  socket path for a bare positional URL and routed to the external-link
  chooser window instead of a normal launch — silently, with the app
  looking idle rather than erroring.

## Known limitations

- `content_area_origin()` in `macos_driver.rs` (only used by `native` click
  mode) is calibrated by eye, never against a real screenshot on this
  laptop specifically — unlike `windows_driver.rs`'s `switcher_button_pos`/
  `search_box_pos`. Not a blocker for the default command-socket path, which
  doesn't use screen coordinates at all.
- No equivalent of `windows-vm`'s `close_dialog.ps1` recovery tool yet — if
  a run leaves a crashed/stuck dialog on the Mac's screen, the fix for now
  is `mac_run "killall browser-macos-appkit web-standards-driver-macos"` or
  a real screenshot (`screencapture`) to see what's actually on screen.
- `native` click mode's Accessibility/Input Monitoring permission grant is
  tied to the exact binary's code signature. `mac_deploy_file` skips
  re-signing (and therefore re-deploying) a file whose content is unchanged
  from what's already there specifically to avoid invalidating this — but
  any real rebuild of either binary still needs a fresh manual grant for
  that mode. A dedicated, persistent signing identity was attempted as a
  fix and abandoned: generating and using a non-ad-hoc code-signing identity
  turns out to need a live GUI session for every private-key operation
  (import, trust, *and* each actual signing use), not just once — confirmed
  directly, several different ways, not assumed. This is why the default
  path doesn't use `CGEvent` at all rather than trying to make the
  permission grant durable.
