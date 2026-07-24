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

   ### The crash dump (see this document's later correction: read this section's own limitations carefully)

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
   specific internal offsets aren't covered by public PDBs.
   **Caveat, added on review rather than left unexamined:** every frame past frame 0 above is "found by stack
   scanning," `minidump-stackwalk`'s own label for a heuristic (scanning stack memory for values that look
   like return addresses) rather than a reliable CFI-based unwind — it can include stale/unrelated values left
   over on the stack from earlier calls. "No frames from `browser-windows-winui.exe` in this stack" is real
   data worth recording, but it is not strong enough on its own to conclude the fault originates inside
   Microsoft's code rather than in, say, a `winio-winui3` wrapper function whose own frame just wasn't
   recovered by the scan. (An earlier version of this section drew exactly that conclusion — corrected further
   down, since WinRT/WinUI 3 are heavily used, well-tested libraries elsewhere and "Microsoft's platform code
   has a bug" shouldn't be where this investigation lands without much stronger evidence than an unreliable
   stack scan.)

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
   isolation — matters.

   **Ran it — also survived**, trace going the same distance past the real crash point (through
   `WM_NCHITTEST`/`WM_NCMOUSEMOVE`/`WM_MOUSELEAVE`) as `full_combo_smoke_test` did. Eight for eight: every
   individual piece, every combination up to all of them, and now the exact real-app construction order too,
   all survive cleanly — but see this document's later correction: every one of these still used only
   `Window`/`Grid`/`TextBlock`/`WebView2` at the default window size, none of the real app's `Button`/
   `TextBox`/`ComboBox`/`CheckBox`/`AppWindow().Resize()` usage.

   ### Abstracting `HistoryStore` to isolate `libsql` itself

   Asked to go one step further: abstract over `HistoryStore` and build a `libsql`-free in-memory version, to
   see whether `libsql` itself (not just the `tokio`/`futures_executor` code bridging its `async fn`s) could
   be involved. Added a real `HistoryBackend` trait to `browser_core` (`record_visit`/`search`/
   `search_similar`) implemented by both the existing libsql-backed `HistoryStore` (an additive change — its
   own inherent methods are unchanged, so every existing call site keeps working exactly as before) and a new
   `MemoryHistoryStore`: a plain `Vec<(HistoryEntry, [f32; DIMS])>` behind a `RefCell`, no SQL at all —
   `search` is a manual substring scan, `search_similar` reuses `embedding::embed` plus a new public
   `embedding::cosine_distance` helper, compared directly in Rust instead of via libsql's
   `vector_distance_cos`. Mirrors `HistoryStore`'s exact behavior (same upsert semantics, same `< 0.9` cutoff,
   same ordering) rather than being a lesser stand-in, with 7 new tests (including one confirming it works as
   a trait object) — all 86 `browser-core` tests, all 20 GTK tests, and all three build targets still pass.

   Added a ninth bisection binary, `memory_history_smoke_test.rs`: the same exact construction order as
   `exact_order_smoke_test.rs`, but calling `MemoryHistoryStore` instead of `HistoryStore`. One honest caveat
   worth being explicit about: this does **not** actually remove `libsql-ffi` from the compiled exe —
   `libsql` is a Cargo *package*-level dependency of `browser_core`, which the whole `browser-windows-winui`
   crate depends on (for the real app), and Cargo doesn't support excluding a dependency for one binary target
   within a package. So `libsql-ffi`'s bundled SQLite is statically linked into every one of these nine test
   binaries regardless — including `minimal_smoke_test`, the very first one, which never calls any
   `browser_core` history code at all and still survived cleanly. That already answers "does `libsql-ffi`
   merely being present in the binary matter" (no). What this ninth binary actually isolates is narrower but
   still real: whether *calling* `MemoryHistoryStore`'s code path specifically, instead of libsql's, changes
   anything.

   **Ran it — also survived**, real queries and all ("found 2 entries" via `MemoryHistoryStore`'s own manual
   substring search). Nine for nine now: every individual piece, every combination, the exact real-app
   construction order, and now a `libsql`-free history backend too, all survive cleanly.

   ### The decisive test: the real production binary itself, with libsql removed

   All nine bisection binaries above *approximate* pieces of the real app — none of them are the actual
   `browser-windows-winui.exe` the user runs. Asked directly whether the real binary had actually been tested
   with `MemoryHistoryStore` — it hadn't. Temporarily swapped `AppState`'s `history` field (and its
   construction in `build_window_and_app`) from the real, libsql-backed `HistoryStore` to `MemoryHistoryStore`
   directly in `browser-windows-winui/src/lib.rs`, clearly marked `TEMPORARY` in both places, and let the
   existing `Launch and screenshot` CI step (which already launches whatever `browser-windows-winui.exe` the
   Build step just produced) test it — no new CI step needed.

   **Result: crashed identically.** Same `STATUS_STOWED_EXCEPTION`, same message sequence ending at
   `WM_SETCURSOR`, even with the real app's *full* complexity intact (switcher grid, settings/profile/
   keybindings/bookmarks overlays, real page/`WebView2` management via `add_page`) and zero `libsql` calls
   anywhere in the actual code path taken. `winui-trace.log` shows `build_window_and_app`, `add_page`, and
   `activate` all completing successfully — the crash happens exactly where it always has, entirely
   independent of whether `libsql` is involved at all.

   This is the most direct confirmation available: not an approximation, the actual production binary, with
   the one remaining untested variable (libsql calls in the real, full-complexity app) removed entirely, and
   it crashes exactly the same. Reverted the swap immediately afterward — `MemoryHistoryStore` has no
   persistence across restarts, which would be a real regression for actual users; the `HistoryBackend`
   trait and `MemoryHistoryStore` implementation stay in `browser_core` as genuine, tested, reusable additions
   (not reverted), just not wired into the real app's `AppState`.

   ### Correction: "it's Microsoft's fault" was premature

   An earlier version of this document concluded from the crash dump (frames in `combase.dll`/`ucrtbase.dll`/
   `KERNELBASE.dll`, none from this codebase's own module) that the fault must be inside WinUI 3's/WinRT's own
   Composition internals. That conclusion doesn't hold up: WinRT and WinUI 3 are heavily used, well-tested
   libraries running far more complex production apps than this one, elsewhere, without this problem. The
   crash-dump stack itself is also weaker evidence than it first looked — every frame past the innermost one
   was "found by stack scanning" (a heuristic, not a reliable unwind), and no public symbols resolved even
   with Microsoft's own symbol server reachable, so "no frames from our module" is suggestive, not proof of
   where the fault actually originates. The far more likely explanations, in roughly descending probability,
   are: (a) how this codebase calls a specific real API, (b) a bug in `winio-winui3` (the community-
   maintained Rust binding subset this crate depends on — its own module doc comment already documents real
   gaps, like several delegate types having no working `add` accessor at all, so it wrapping *some* API
   incorrectly is entirely plausible), or (c) something about the cross-compile/build setup.

   Revisiting what the ten bisection binaries actually covered, with that framing: every one of them used only
   `Window`/`Grid`/`TextBlock`/`WebView2`, and only one event handler
   (`WebView2::NavigationCompleted`) — at WinUI 3's *default* window size. None of them called
   `window.AppWindow()?.Resize(...)` (the real app's very first action after `Window::new()` — see
   `build_window_and_app` in `lib.rs`), and none exercised `Button.Click`, `TextBox` `GotFocus`/`LostFocus`/
   `TextChanged`, `ComboBox`, or `CheckBox` — all real, specific API calls the real app makes constantly (every
   toolbar button, the address bar, the search box, the whole settings overlay) via `winio-winui3`'s delegate
   types (`RoutedEventHandler`, `TextChangedEventHandler`). That's substantial, previously-untested surface
   area — a real usage or wrapper bug has plenty of room to hide in it. Added a tenth bisection binary,
   `appwindow_resize_smoke_test.rs`, testing `AppWindow().Resize(...)` in isolation — the single most
   specific, novel, previously-unexamined call available. Not yet run.

This is exactly the iteration loop the CI was built for: push, get a real failure, read the real log, fix the
real bug, repeat — each round taking a couple of minutes rather than needing a physical Windows machine. It
also caught two real mistakes of mine along the way, both corrected once actually checked rather than left
standing: guessing at what an NTSTATUS code meant instead of verifying it (sent an early fix attempt in the
wrong direction), and — more significantly — concluding the crash dump pointed to a bug in Microsoft's own
platform code, when the far more likely explanations were a usage bug, a wrapper-crate bug, or a build issue.
This investigation is not resolved yet; each round has ruled something concrete out, which is real progress,
but the actual cause remains open.

## The `windows-reactor` comparison test: `winio-winui3` is now a real suspect, not just a plausible one

Set up a real Windows 11 VM locally (`dockur/windows`, Docker + QEMU/KVM, storage on the host's `/`
partition — `/home` didn't have the ~64GB needed) specifically to build and run a comparison app against
Microsoft's own `windows-reactor`/`windows-webview` (in-tree in `microsoft/windows-rs`, built on the same
`windows-bindgen` WinMD codegen as the base `windows` crate — see the "Microsoft-developed Rust bindings"
research this session) instead of the community `winio-winui3` wrapper. Several real, non-obvious problems
came up getting there, each worth recording:

- **`restart: "no"` broke the Windows install itself.** Windows' unattended setup goes through multiple
  internal reboots; the official `dockur/windows` compose example uses `restart: always` specifically so
  Docker relaunches the container across those. Setting it to `"no"` (to avoid the container looping
  indefinitely in the background) meant the *first* internal reboot killed the whole install permanently,
  well before the OEM `install.bat` provisioning script ever ran. Fixed by using `restart: always` and just
  remembering to `docker compose down` explicitly when done with the VM.
- **The OEM auto-install mechanism never fired**, for reasons still unconfirmed (plausibly LTSC-specific —
  the logs showed a different unattend XML, `win11x64-ltsc.xml`, than the standard flow). Recovered via a
  genuinely useful fallback: QEMU exposes an HMP monitor socket (`/run/shm/monitor.sock`) even with no RDP
  port published, and it supports `screendump` (real screenshots, no VNC client needed) and `sendkey`
  (synthetic keyboard input) — enough to open a Command Prompt and drive the VM entirely from `docker exec`
  + Python, no GUI access required. (Mouse input via HMP `mouse_move`/`mouse_button` never worked reliably in
  this setup, despite the tablet device being active per `info mice` — worth real investigation if this
  approach is reused, but keyboard-only driving was sufficient here.)
- **The shared folder's `Z:` drive letter is session-scoped, not a global mount.** It's a guest-only Samba
  share (`\\host.lan\Data`, confirmed via the container's `smb.conf`), and Windows maps it to `Z:` per
  logon session — invisible from an elevated (UAC split-token) session, and, more importantly, invisible to
  a SYSTEM-context scheduled task. Since there's no documented remote-exec mechanism for an already-installed
  `dockur/windows` VM, the whole point was a file-based command channel (a scheduled task polling for a
  `request.flag`, running whatever's in `command.bat`, writing results back) — which meant it had to run as
  SYSTEM (to work regardless of interactive logon state), which meant it needed the shared folder to work
  under SYSTEM. Fixed by addressing the share via its UNC path (`\\host.lan\Data\...`) everywhere instead of
  `Z:\` — UNC paths aren't session-scoped, so this worked identically for install scripts, the SYSTEM-run
  scheduled task, and interactive sessions alike.
- **Repeated appends to a UNC-path log file caused a silent, permanent lock.** The first provisioning attempt
  logged every step via `>> \\host.lan\Data\setup.log`; partway through (right after `rustup-init` finished,
  before the next line), every subsequent append started failing with "The process cannot access the file
  because it is being used by another process" — apparently triggered by a collision with this session
  reading the same file from the Linux host mid-run. Since a failed output redirect in `cmd.exe` skips
  running that line's command entirely, everything after that point (VS Build Tools, the Windows App SDK,
  the scheduled task) silently never ran, even though the script "completed" and returned to a normal prompt.
  Fixed by logging to a local file (`C:\OEM\setup.log`) and mirroring it to the shared folder via single-shot
  `copy` calls after each milestone instead of holding the UNC file open across dozens of small appends.
- **VS Build Tools triggered a mid-install reboot** despite `--norestart` (likely a prerequisite redistributable
  outside the bootstrapper's own control). `vs_buildtools.exe` is designed to resume/repair on a subsequent
  run, so simply re-launching the same provisioning script after the VM came back up (via `restart: always`)
  completed it on the second attempt — no special resume logic needed.
- **`cargo` running as SYSTEM (via the scheduled task) could build the exe but not usefully launch it.** The
  build succeeded and reported `CRASHED_OR_EXITED` after launch, but with *zero* trace-log output — not even
  the very first line of `main()`. This matches Windows' Session 0 isolation: a GUI process launched from a
  SYSTEM-context scheduled task runs in the non-interactive services session and cannot render a window
  there, regardless of which UI wrapper crate is used. Confirms the file-based automation is fine for
  *building* but launching/observing a GUI app's crash-or-survive behavior needs to happen in the interactive
  session (done here via the same QEMU `sendkey` channel).
- **First real launch attempt in the interactive session hit a genuine, simple, diagnosable error**: `The
  code execution cannot proceed because microsoft.windowsappruntime.bootstrap.dll was not found.` Traced to
  having dropped the sample's `build.rs` (`windows_reactor_setup::as_self_contained()`) when adapting it,
  on the assumption that installing the Windows App SDK runtime system-wide would be enough. It isn't:
  `Microsoft.WindowsAppRuntime.Bootstrap.dll` is a thin shim that ships *with the app*, not as part of the
  OS-wide runtime install — confirmed by reading `windows-reactor-setup`'s actual source, which embeds this
  exact DLL as a build-time resource. Fixed with the lighter `windows_reactor_setup::as_framework_dependent()`
  (copies just the bootstrap DLL + `resources.pri` next to the exe, no self-contained bundling needed since
  the framework package is already installed).
- **Cargo didn't notice the `Cargo.toml`/`build.rs` fix at first** — a rebuild after adding the
  `windows-reactor-setup` build-dependency showed every crate as `Fresh` (cached), including the build script
  itself, and reproduced the identical error. Plausibly a clock-skew artifact of editing files from the Linux
  host over the same Samba share cargo's mtime-based fingerprinting reads from. Fixed by deleting `target/`
  for a clean rebuild rather than chasing the exact cause.

**The result, once all of that was worked around**: `reactor_smoke_test` — a `windows-reactor`/
`windows-webview` app with the same toolbar-plus-`WebView2` shape as `browser-windows-winui`'s real app,
adapted directly from `microsoft/windows-rs`'s own `crates/samples/reactor/webview` sample — built cleanly
and **launched successfully**: a real window, toolbar (back/forward/reload/address bar/Go), stable for over
10 seconds with no crash, no exception, no dialog. Trace log confirms `main: start` → `main: after bootstrap`
→ `app: render start` → `app: render end`, all clean. The `WebView2` content area itself stayed blank (no
`on_ready: WebView2 ready` trace line ever fired) — a separate, likely-environmental issue (this is a stripped
IoT/LTSC eval image; the WebView2 Runtime itself may not be present even though the Edge icon is on the
desktop) worth checking separately, not part of the crash comparison.

This is real, if not conclusive, evidence: the same general shape of app (window + toolbar + `WebView2`,
built against the same underlying Windows App SDK / WinRT / Composition stack) survives when built against
Microsoft's own `windows-reactor`/`windows-webview`, on the same category of machine where `winio-winui3`-based
smoke tests survive too but the real `browser-windows-winui` app crashes. It doesn't yet isolate *which* of
the real app's specific `winio-winui3` calls is the problem (this test doesn't exercise `AppWindow().Resize()`,
`ComboBox`, `CheckBox`, or the exact construction order the real app uses), but it's one more point toward
"usage or wrapper bug," not "Microsoft's platform code," and a working, fast, local VM to keep iterating in
rather than round-tripping through GitHub Actions for every next hypothesis.
