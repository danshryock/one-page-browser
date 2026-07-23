# Windows CI via GitHub Actions (real hardware, not local VMs)

**Roadmap item:** run/test the Windows front end on real Windows, without relying on cross-compilation or
Wine emulation for anything beyond what already exists locally.

## Why not local VMs

Investigated running Windows (and macOS) test environments as local Docker/KVM VMs on this dev machine.
Findings:

- This machine (bare-metal AMD Ryzen 9 5900X, KVM/nested virtualization all present and usable) could
  technically run a Windows VM locally. But Apple's macOS Software License Agreement restricts running macOS
  in a VM to Apple-branded hardware — this machine is not Apple hardware, so a local macOS VM (even via
  community QEMU/KVM projects capable of it) would violate that license. Not pursued.
- The `cross` tool (referenced by the repo's existing `Cross.toml`, but not actually wired into any current
  build alias) only cross-*compiles* via Docker containers with the target's toolchain — it doesn't run a
  foreign-OS binary. It wouldn't add anything beyond what `cargo-xwin`/`cargo-zigbuild`/Wine already do here.

Given that, and since a local Windows VM would just reproduce what a genuine, license-clean, free
GitHub-hosted `windows-latest` runner already provides directly, the user redirected this to GitHub Actions:
write and compile locally as always, push to GitHub, let CI actually run it on real Windows, and pull back
results (logs, a screenshot) for review here.

## What this adds beyond the existing local Wine/xwin cross-compile setup

- `browser-windows-winui` (WinUI 3) has been **cross-compile/link-verified only** the whole time it's
  existed (see `ROADMAP.md`) — it has never actually launched, because WinUI 3 needs the real Windows App SDK
  runtime, which doesn't exist under Wine. A genuine `windows-latest` runner is real Windows, so this is the
  first environment that can actually launch it and confirm it doesn't immediately crash.
- `windows-latest` runners have a real MSVC toolchain and Windows SDK already installed, so building
  `browser-windows-winui` there needs nothing beyond plain `cargo build` — no `cargo-xwin`, no `xwin`-fetched
  SDK, sidestepping the `llvm-lib`-shaped cross-compile gaps this session has hit before with MSVC-only Cargo
  features.
- `windows-latest` runners have a real interactive desktop, unlike the Linux runners used for the existing
  headless GTK suite — so the workflow can launch the exe for real and screenshot it, genuine visual
  confirmation rather than "it compiled and linked."

## What was built

`.github/workflows/windows.yml` (new), two jobs, both on `windows-latest`:

- **`test-core`**: `cargo test -p browser-core` (native Windows — for the first time, `browser-core`'s
  platform-independent logic gets to run on an actual non-Linux target, not just get compile-checked) and
  `cargo check -p render-engine`.
- **`build-and-smoke-winui`**: builds `browser-windows-winui` with a plain `cargo build --target
  x86_64-pc-windows-msvc` (no cross-compile tooling), attempts to install the Windows App SDK v2.x runtime
  (via Microsoft's `aka.ms/windowsappsdk` redistributable — `continue-on-error: true`, since it's unverified
  whether `windows-latest` images already bundle a compatible one), then launches the built exe via
  PowerShell, waits, checks it hasn't already exited (a `HasExited` check catches an early
  "Class not registered"-style crash and fails the job with a pointer back to the runtime-install step),
  screenshots the primary display via `System.Windows.Forms.Screen`/`System.Drawing.Graphics.CopyFromScreen`,
  and uploads it as a build artifact.

Deliberately **out of scope**: `browser-windows-win32`, `browser-windows-nwg`, `browser-wx` — per standing
direction earlier this session to deprioritize those crates going forward (they already have a working local
Wine-based build+run loop; `browser-windows-winui` is the one under active development).

## Also fixed: `browser-core` test gating for a genuine non-Linux runner

Three `crates/browser-core/src/history.rs` tests (`encrypted_store_round_trips_with_the_right_passphrase`,
`encrypted_store_rejects_the_wrong_passphrase`, `a_plain_unencrypted_open_cannot_read_an_encrypted_store`)
call `HistoryStore::open_encrypted` and assert it succeeds — true on Linux (libsql's `encryption` feature,
scoped Linux-only per `summaries/profile-passphrase-encryption.md`), but on every other platform
`open_encrypted` is a stub that unconditionally returns `Err`. These would have failed the very first time
`cargo test -p browser-core` ran somewhere genuinely non-Linux, which this workflow is the first thing to do.
Gated all three `#[cfg(target_os = "linux")]`, and added a `#[cfg(not(target_os = "linux"))]` counterpart
(`open_encrypted_returns_an_error_rather_than_silently_opening_unencrypted_on_this_platform`) asserting the
stub's actual (error-returning) behavior, so non-Linux platforms still get real coverage of that code path
instead of just skipping it silently.

## Testing

- `cargo test -p browser-core`: 79/79 passing (this machine is Linux, so the `#[cfg(target_os = "linux")]`
  tests still run here as before; the new non-Linux counterpart test can only be confirmed correct by cfg
  logic + a genuine non-Linux run, which is exactly what this new CI workflow will provide once pushed).
- `cargo clippy --all-targets` on `browser-core`/`browser-linux-gtk3`/`render-engine`: clean (two
  pre-existing, unrelated `field_reassign_with_default` warnings in test helpers, untouched by this change).
- `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx` and `cargo build-windows-winui`:
  both succeed, confirming the `history.rs` test-gating edit doesn't affect either existing cross-compile path.
- Full headless GTK suite (`xwayland-run`/`cage`): 20/20 passing, unaffected by this change (no
  `browser-linux-gtk3` production code touched).

## Honest limitations / what's unverified

- **Nothing in `.github/workflows/windows.yml` has actually run yet.** No GitHub remote is connected to this
  repo yet (the user will create and configure one later) — everything here is written defensively from
  documentation and first-hand reading of this repo's own runtime requirements (see
  `crates/browser-windows-winui/src/main.rs`'s `PackageDependency::initialize_version` comment and
  `winio-winui3`'s vendored bootstrap source for the Windows App SDK v2.x requirement), not from an actual
  observed CI run. The Windows App SDK install step in particular is the most likely thing to need real
  iteration once it actually runs against a live runner image.
- No macOS workflow yet — planned as a separate, later piece once a `browser-macos-appkit` crate exists to
  build/smoke-test.
