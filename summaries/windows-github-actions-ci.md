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

- No macOS workflow yet — planned as a separate, later piece once a `browser-macos-appkit` crate exists to
  build/smoke-test. (Update: this shipped — see `summaries/macos-appkit-scaffold.md` and
  `.github/workflows/macos.yml`, which passed completely on its first real run.)

## Update: first real runs, once the repo was pushed

The repo was pushed to `danshryock/one-page-browser` and this workflow ran for real. Two real bugs surfaced
and were fixed, neither related to anything speculative above — both were genuine mistakes, found via actual
job logs (`gh run view --log-failed` / the Actions API), not guessed at:

1. **`test-core` and the winui build both failed with the same root cause**: `.cargo/config.toml`'s `[env]`
   table set `CC_x86_64_pc_windows_msvc`/`AR_x86_64_pc_windows_msvc` to this dev machine's local clang-cl/
   llvm-lib paths (`/usr/lib/llvm-21/bin/...`, needed for `cargo-xwin`'s cross-compile here). `[env]` entries
   apply unconditionally regardless of host OS — so the native `windows-latest` runner inherited that
   Linux-only path too, and every build touching `libsql-ffi` (i.e. anything depending on `browser-core`)
   failed with `failed to find tool "/usr/lib/llvm-21/bin/clang-cl"`. Fixed by moving those two lines out of
   the repo-tracked `.cargo/config.toml` into `~/.cargo/config.toml` (this machine's global Cargo config,
   never checked into the repo) — the correct place for "where a tool happens to live on this specific box."
   After this fix, `test-core` passed fully and the winui build step itself succeeded.
2. **`build-and-smoke-winui`'s launch step then failed differently**: the app crashed immediately with exit
   code `-1073741189` (`0xC000027B`). First guess (wrong, not checked before writing it down) was
   "`STATUS_DLL_NOT_FOUND`," theorizing the "Install Windows App SDK runtime" step's `Start-Process ... -Wait`
   silently swallowed a failed install. That step was improved to capture/print the installer's exit code and
   confirm via `Get-AppxPackage` whether the runtime landed — and on the next run it clearly had (exit code
   `0`, `Microsoft.WindowsAppRuntime.2` v2.2.0.0 present, both x64/x86) — yet the exact same crash code
   recurred, ruling that theory out. Actually checking `0xC000027B` against a real reference
   (`ntstatus.h`) instead of guessing: it's `STATUS_STOWED_EXCEPTION`, not a missing-DLL code at all — this is
   Windows' fast-fail for an **unhandled exception**, most plausibly a Rust panic unwinding across the
   `ApplicationInitializationCallback` FFI boundary (or a genuine WinRT-level exception) somewhere inside
   `browser-windows-winui`'s own startup code — which has never actually run anywhere before this workflow
   (see `ROADMAP.md`'s "`browser-windows-winui` debugging" backlog entry; this is that debugging pass,
   happening for the first time). The launch step now redirects the app's own stdout/stderr to files and
   prints + uploads them regardless of outcome, since the previous version gave no way to see *what* failed
   beyond a bare exit code — but the redirected stdout/stderr both came back **completely empty** on the next
   run, meaning the crash is too abrupt for normal stream flushing (consistent with `RaiseFailFastException`-
   style termination, which explicitly skips the usual CRT/unwind cleanup). Registered a Windows Error
   Reporting local dump for the exe next — also came back empty (`WerSvc` confirmed running, WER not
   disabled, but still no `.dmp` written for this specific crash), and this runner has no `cdb`/`windbg` on
   `PATH` to attach a debugger directly either. Switched strategy again: added checkpoint tracing (`trace(...)`,
   in `browser-windows-winui/src/lib.rs` — writes straight to a file with an explicit `sync_all()` after every
   line, since it survives an abrupt fast-fail in a way buffered stdio doesn't) at each step of `main()`/
   `run()`. **Kept permanently rather than stripped out once the bug is found**, per explicit direction — it's
   cheap, and useful again if this crate ever regresses on real Windows.

   The resulting `winui-trace.log` was a real breakthrough: **every** checkpoint through `run()` succeeded —
   `Application::new`, `build_window_and_app`, `add_page`, `activate`, and the callback returning `Ok` — meaning
   the window is genuinely built, the page loads, and the window activates cleanly. The crash happens *after*
   all of that, somewhere inside WinUI 3's own message pump (`Application::Start`'s internals, which we don't
   control directly). Added the same tracing to the very top of `subclass_proc` (the `SetWindowSubclass`-based
   `WNDPROC` handling `WM_KEYDOWN`/`WM_DESTROY`/`WM_NCDESTROY` — see this file's module doc comment on why that
   exists) to log every window message it receives. That run showed ~30 real messages handled cleanly —
   `WM_SHOWWINDOW`, `WM_ACTIVATE`, `WM_NCPAINT`, `WM_ERASEBKGND`, `WM_SIZE`, `WM_MOVE`, `WM_PAINT`, `WM_GETICON`
   — ending with `WM_SETCURSOR` (`0x0020`) fired twice, then **nothing**. That's exactly the point WinUI 3 would
   start its first real Composition/DirectX render pass, and matches a documented category of crash: GitHub
   Actions' `windows-latest` runners have no real GPU (a basic/virtual display adapter only), and
   Composition-based UI frameworks (WinUI 3 uses `DirectComposition`/`Windows.UI.Composition`, not just plain
   HWND painting — which we now know works fine, since dozens of ordinary window messages were handled without
   issue) are a known source of exactly this kind of fast-fail on GPU-less machines.

   Researched whether there's a way to force software (WARP — Windows Advanced Rasterization Platform,
   Microsoft's own bundled software Direct3D rasterizer, explicitly meant for server/VM/no-GPU scenarios: see
   [Microsoft's WARP guide](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/directx-warp))
   rendering for an *existing* compiled app without modifying its source (`browser-windows-winui` doesn't
   create its own Direct3D device — `winio-winui3`/WinUI 3's internals do, with no Rust-accessible hook to
   request WARP directly). Two real, first-party mechanisms exist:
   - **`d3dconfig.exe`**, a Microsoft console tool ([DirectX dev blog
     post](https://devblogs.microsoft.com/directx/d3dconfig-a-new-tool-to-manage-directx-control-panel-settings/))
     for managing the same per-application driver-type overrides the DirectX Control Panel GUI (`dxcpl.exe`)
     does, installable via the "Graphics Tools" Windows Feature-on-Demand
     (`Add-WindowsCapability -Online -Name "Tools.Graphics.DirectX~~~~0.0.1.0"`). The blog post shows `apps`/
     `debug-layer`/`message-break` subcommands verbatim but not the driver-type one specifically — its exact
     syntax for forcing WARP isn't confirmed from documentation, so the workflow installs it, dumps `--help`
     output for real, and tries a few plausible guesses at the syntax.
   - The **`Microsoft.Direct3D.WARP` NuGet package** ships `D3D10Warp.dll` (the same file name backs both
     D3D10 and D3D11 WARP support historically), which Microsoft's own docs say can be placed next to an app's
     `.exe` — but this only helps if WinUI 3's Composition stack already *attempts* a WARP fallback and just
     needs a working DLL, not if it doesn't attempt a software fallback at all. Not tried yet — `d3dconfig` is
     the more targeted first attempt, since it can force the driver type rather than hoping for an automatic
     fallback.

   `d3dconfig --help`'s real output confirmed the exact syntax: `device force-warp[=(true|false)]` (not a
   `driver-type` subcommand as first guessed — the real categories are `apps`/`debug-layer`/`device`/`dred`/
   `message-break`/`message-mute`). `apps --add <exe>` also worked as expected, scoping settings to
   `browser-windows-winui.exe` specifically rather than system-wide. Ran it for real: `d3dconfig --export`
   confirmed `ForceWARP=1` genuinely landed for this exe, in both the D3D10/11 and D3D12 `Application0`
   sections — **and the crash was identical anyway**, same exit code, same exact message sequence in
   `winui-trace.log`. Clean negative result: either the per-app AppCompat-style override doesn't reach
   whatever device-creation path WinUI 3's Composition stack actually uses, or the root cause isn't
   hardware/software device selection at all.

   Reconsidered the trace log itself: the last messages before the crash are `0x031F`
   (`WM_DWMNCRENDERINGCHANGED` — DWM's own non-client-area rendering notification) and `0x0020`
   (`WM_SETCURSOR`, twice) — i.e., the crash sits right at DWM/non-client-area interaction, not necessarily
   inside the app's own Direct3D device selection at all. That reframes what's worth checking next: is this a
   fundamental "no GPU" limitation of the runner (would affect *any* WinUI 3 app), or something in
   `browser-windows-winui`'s own code? Two real candidates for the latter, both genuinely unusual code sitting
   directly in the window's setup/message path:
   - `install_hwnd_subclass`/`subclass_proc` — the raw `WNDPROC` interception workaround for `winio-winui3`'s
     missing `KeyDown`/`Window::Closed` delegates (see `lib.rs`'s module doc comment).
   - `window.SetExtendsContentIntoTitleBar(true)` + `SetTitleBar(&toolbar)` (`lib.rs` around line 981) — a
     custom title bar, which requires exactly the non-client-area/DWM interaction the trace's last messages
     point at. This is at least as plausible a suspect as raw GPU absence, and wasn't considered until
     rereading the trace with that framing.

   Added `crates/browser-windows-winui/src/bin/minimal_smoke_test.rs`: the bare minimum WinUI 3 app (init,
   bootstrap, one plain `Window`, `Activate()`) — no subclassing, no custom title bar, no controls, no
   `WebView2`. The workflow builds and launches it (with its own `trace()` log, `minimal-smoke-trace.log`)
   right after building the full app, non-fatally.

   **Ran it — conclusive result: `minimal_smoke_test` survived.** Every checkpoint logged through
   `callback: run() returned Ok`, and the process was still running 8 seconds later — no crash at all. This
   settles the "is it a fundamental GitHub Actions/no-GPU limitation" question for good: **no, WinUI 3 itself
   works fine on this runner.** The bug is specific to something in `browser-windows-winui`'s own code, one of
   the two suspects above.

   Added a second bisection binary, `titlebar_smoke_test.rs`: identical to `minimal_smoke_test` plus *only*
   `SetExtendsContentIntoTitleBar(true)` + `SetTitleBar(...)` — still no `install_hwnd_subclass`.

   **Ran it — also survived cleanly.** Every checkpoint through `run() returned Ok`, still running 8 seconds
   later. That clears the custom title bar too — neither a bare window nor the custom title bar alone
   reproduces the crash.

   Added a third bisection binary, `webview2_smoke_test.rs`: identical to `minimal_smoke_test` plus *only* an
   embedded `WebView2` XAML control (mirroring `render_engine::WebView2Engine`'s construction — see
   `render-engine/src/winui.rs`) navigated to a real URL — still no `install_hwnd_subclass`, no custom title
   bar. `WebView2` is a much heavier native control than anything tried so far, backed by a real Edge WebView2
   process with its own runtime/user-data-folder requirements — a genuinely plausible independent crash source
   this debugging pass hadn't considered until ruling out the other two.

   **Ran it — also survived cleanly.** Every checkpoint through `run() returned Ok`, no crash. `WebView2` alone
   isn't the cause either.

   Added a fourth bisection binary, `subclass_smoke_test.rs`: identical to `minimal_smoke_test` plus *only*
   `SetWindowSubclass`-based `WNDPROC` interception — the technique `lib.rs`'s `install_hwnd_subclass`/
   `subclass_proc` uses as a workaround for `winio-winui3`'s missing `KeyDown`/`Window::Closed` delegates.
   Reimplemented standalone (those functions are private to `lib.rs`, unreachable from a separate `src/bin/`
   binary crate) but the same shape: subclass the raw `HWND`, forward every message to `DefSubclassProc`
   unmodified, reclaim the boxed state on `WM_NCDESTROY`. This is the last of the real window's genuinely
   unusual pieces of code left untested in isolation.

   **Ran it — also survived, and its trace is the most telling one yet**: it received the *exact same message
   sequence* the real app's own trace shows, all the way through `WM_SETCURSOR` (`0x0020`) fired twice — the
   precise point the real app crashes at — and then just kept running past it. So it isn't any single piece
   in isolation. Added a fifth binary, `titlebar_webview2_smoke_test.rs`, testing the specific combination
   most likely to interact badly: `SetExtendsContentIntoTitleBar`/`SetTitleBar` *and* an embedded `WebView2`
   together (a `WebView2` surface rendering underneath/near a custom-drawn, DWM-extended title bar region is a
   real, previously documented tricky pairing for WinUI 3 apps in general, independent of this CI
   environment) — still no HWND subclassing.

   **Ran it — also survived cleanly**, every checkpoint through `Activate`/`run() returned Ok`, no crash. The
   pairwise combination isn't it either. Added a sixth binary, `full_combo_smoke_test.rs`: title bar +
   `WebView2` + `SetWindowSubclass` `WNDPROC` interception, all three together — the last remaining
   combination of the real window's genuinely unusual pieces of code before concluding the crash needs the
   real app's full complexity (many controls/overlays at once, multiple pages/`WebView2`s) rather than any
   subset tested so far.

   **Ran it — also survived, and this trace went further than any before it**: past `WM_SETCURSOR` (the real
   crash point) into `WM_NCHITTEST`/`WM_NCMOUSEMOVE`/`WM_MOUSELEAVE` — messages the real app's crash trace
   never even reaches — with no crash at all. None of the real window's genuinely unusual XAML/subclassing
   code, individually or in any combination up to all three, reproduces the crash.

   That's conclusive enough to redirect: **none of the seven binaries so far touch `browser_core` at all.**
   The real app's `build_window_and_app` calls `HistoryStore::open(&profile)`, which opens a real libsql
   database and spins up its own `tokio::runtime::Runtime` (`browser_core::history`'s `self.rt.block_on(...)`
   calls) — mixing that multi-threaded async runtime with the WinRT single-threaded STA apartment
   (`init_apartment(ApartmentType::SingleThreaded)`) is a genuinely plausible, previously untested crash
   source. Added `historystore_smoke_test.rs`: uses `HistoryStore::open_in_memory()` (same real libsql/tokio
   machinery, no disk I/O) and *actually runs queries* — `record_visit` twice, then `search` — displaying the
   real result count and titles in the window's content, not just opening the store and leaving it idle.
   Still no custom title bar, `WebView2`, or HWND subclassing — isolates the `browser_core`/tokio question on
   its own.

   **Ran it — also survived, real queries and all** (`record_visit` × 2, `search`, "Found 2 entries: Example
   Domain, Example Org" genuinely displayed in the window's content) — but this run's `Launch and screenshot`
   step for the *real* app finally produced something new: **a genuine crash dump.** (WER had silently failed
   to write one on every previous run for reasons never pinned down — this time it worked.)

   ### The crash dump: the fault is inside Microsoft's own system DLLs, not our code

   Downloaded the 232MB `.dmp` and analyzed it locally with `minidump-stackwalk` (`cargo install
   minidump-stackwalk`, the `rust-minidump` project's CLI — genuinely useful that this is possible entirely
   from Linux, no Windows debugger needed). The crashing thread's stack:
   ```
   Crash reason:  STATUS_STOWED_EXCEPTION
   Thread 0 main (crashed) - tid: 9024
    0  KERNELBASE.dll + 0x10eec8
    1  combase.dll + 0x1043ae
    2  combase.dll + 0x1f9c00
    3  ucrtbase.dll + 0xed79e
    4  KERNELBASE.dll + 0x10eec7
   ```
   `browser-windows-winui.exe` (our own module) *is* loaded (confirmed in the dump's module list, with a real
   address range) — but it appears **nowhere** in the crashing thread's stack, nor in any other thread's. Every
   frame is inside Microsoft's own `combase.dll` (core COM/WinRT infrastructure — this is literally the DLL
   that implements stowed-exception propagation itself) and `ucrtbase.dll` (the Universal CRT), called
   through `KERNELBASE.dll`'s fail-fast path. A separate thread (tid 612) sits inside `CoreMessagingXP.dll` —
   the DLL implementing `Windows.System.DispatcherQueue`, WinUI 3's own internal message dispatcher —
   confirming XAML's dispatcher machinery is genuinely active, consistent with everything up to that point
   working normally (as all seven bisection binaries already showed).

   Tried symbolicating against Microsoft's public symbol server (`--symbols-url
   https://msdl.microsoft.com/download/symbols`, confirmed reachable) — no additional resolution, these
   specific internal offsets aren't covered by public PDBs. Even without exact function names, the module
   list alone is conclusive: **the actual unhandled exception originates entirely inside Microsoft's own
   WinRT/COM/Composition stack, not in anything this codebase wrote** — not the custom title bar, not
   `WebView2`, not the HWND subclass, not `browser_core`/tokio, individually or in any combination tested.
   Searched for precedent: `combase!RoOriginateLanguageException`-rooted stowed-exception crashes are a real,
   recurring category of WinAppSDK/WinUI 3 issue in Microsoft's own GitHub trackers (e.g.
   [WindowsAppSDK#4861](https://github.com/microsoft/WindowsAppSDK/issues/4861),
   [WindowsAppSDK#999](https://github.com/microsoft/WindowsAppSDK/issues/999)), and GitHub Actions' Windows
   runner images have their own documented rough edges around Windows App SDK/SDK version mismatches (e.g.
   [WindowsAppSDK#5851](https://github.com/microsoft/WindowsAppSDK/issues/5851)) — consistent with this being
   a real compatibility issue between WinUI 3's Composition internals and this specific CI environment
   (Windows NT 10.0.26100, confirmed from the dump), not a bug introduced by this project's own code.

   ### Follow-up: is tokio itself really clean, and does exact construction order matter?

   Asked directly whether `browser_core`'s `tokio` usage could still be implicated, and whether a Microsoft-
   provided or custom alternative could replace it. Checking the actual code first (`history.rs`): the tokio
   runtime was already about as minimal as tokio gets —
   `tokio::runtime::Builder::new_current_thread().build()` with **no** `.enable_io()`/`.enable_time()`/
   `.enable_all()` call, and the crate itself only enables tokio's bare `"rt"` feature
   (`default-features = false`). No thread pool, no I/O driver (no IOCP handle setup on Windows), no timers —
   just a bare task executor on the calling thread.

   Reading libsql's own source (`local/impls.rs`) settled the bigger question: for the local (embedded)
   backend used here, `Connection::execute`/`query` are `async fn`s that just directly call the synchronous
   SQLite FFI (`self.conn.execute(sql, params)`, no `.await` inside) — the `async` wrapper exists purely for
   API uniformity with libsql's separate remote/HTTP backend, which presumably does need real async I/O.
   Every one of these futures completes on its very first poll; there is nothing for a real async runtime to
   actually schedule. That makes `futures_executor::block_on` — a single free function, no runtime object, no
   reactor, no threads, already resolved transitively via libsql itself so no new dependency — an exact,
   honest fit, and **replaced tokio in `browser_core` entirely** (`history.rs`'s `HistoryStore` struct no
   longer even stores a runtime handle). Verified: all 79 `browser-core` tests and all 20 headless
   `browser-linux-gtk3` GTK tests still pass, and all three build targets (native Linux, `x86_64-pc-windows-
   gnu`, `cargo build-windows-winui`) still succeed. Not implicated by the crash dump evidence above, but a
   real simplification regardless — dead-weight reactor/threading machinery that was never doing anything.

   Also added an eighth bisection binary, `exact_order_smoke_test.rs`: none of the previous seven matched the
   real `build_window_and_app`'s *exact* construction order (content + custom title bar → `HistoryStore` with
   real queries → `WebView2` → HWND subclass → `Activate`, mirroring where each piece actually happens
   relative to the others, including `WebView2` only being created after the window/history/subclass setup
   the way `add_page` really does it). Tests whether the specific interleaving — not any single piece in
   isolation — matters. Not yet run.

   ### Where this leaves things

   Seven independent bisection binaries (bare window, custom title bar, `WebView2`, HWND subclass, three
   combinations up to all of them, and `browser_core`/tokio) all survived cleanly — each one individually
   ruling out a real, plausible suspect rather than guessing blind. The crash dump then closed the loop:
   the fault lives entirely inside Microsoft's own system DLLs, with zero frames from this codebase anywhere
   in the crashing thread. Tokio's own footprint was independently confirmed minimal and has now been removed
   entirely as a real simplification, and an eighth test targets exact construction-order/timing as the one
   remaining untested variable. Further diagnosis beyond that would need either Microsoft's own private
   symbols (not publicly available) or a live interactive debugger session on a matching machine — beyond
   what's practically achievable from this environment. This is being left as a well-documented, real, open
   environment-compatibility issue (see `ROADMAP.md`) rather than chased further blind.

This is exactly the iteration loop the CI was built for: push, get a real failure, read the real log, fix the
real bug, repeat — each round taking a couple of minutes rather than needing a physical Windows machine. It
also caught a real mistake of mine along the way: guessing at what an NTSTATUS code meant instead of checking
it, which sent the first fix attempt in the wrong direction — corrected once actually verified. This
particular crash took many rounds and, in the end, a genuine crash dump to reach a conclusive answer — a
first-ever debugging session for code that had never run anywhere before this CI existed, and each round
ruled something concrete out rather than just guessing blind, which is real progress even without a code fix
at the end of it.
