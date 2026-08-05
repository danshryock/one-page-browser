# Feature matrix

What's implemented, and how well it's actually verified, across the four front ends
(`browser-linux-gtk3`, `browser-macos-appkit`, `browser-windows-winui`, `browser-windows-reactor`). Built by
reading current source directly (not ROADMAP.md's chronological narrative, which documents *how* things
happened but drifts from exact current scope) — see ROADMAP.md for the story behind any given row.

Two tables: a high-level summary (one row per feature area), then a low-level breakdown grouped by the same
areas (one row per concrete behavior). Testing-status legend applies to both:

| Symbol | Meaning |
| --- | --- |
| **real tests** | Automated tests that actually execute this behavior (`cargo test`, run for real — not just compiled) |
| **compile-only** | Compiles/links (Linux workspace check, or a cross-compile via `cargo build-windows-*`/`.cargo/build-macos-appkit.sh`), never executed |
| **CI (real HW)** | Executed on real hardware in GitHub Actions (not this dev machine), but not via a unit test — a launch/smoke check |
| **VM-verified** | Manually, interactively verified running in a local Windows VM (`dockur/windows`) this session — real behavior, not automated |
| **untested** | Implemented, compiles, but no automated test and no real-hardware run has ever exercised it |

Implementation legend: ✅ Full · 🔶 Partial (works, with a real named gap) · ❌ Not implemented / not reachable.

## Summary matrix

| Feature area | `browser-linux-gtk3` | `browser-macos-appkit` | `browser-windows-winui` | `browser-windows-reactor` |
| --- | --- | --- | --- | --- |
| Core browsing & navigation | ✅ real tests | ✅ untested | 🔶 compile-only, no unified bar | 🔶 VM-verified nav, no unified bar |
| Multi-page / tabs & switcher | ✅ real tests | 🔶 untested (list, not tile grid) | 🔶 compile-only (fixed-grid workaround) | 🔶 VM-verified (real grid control) |
| Session & webview-data persistence | ✅ real tests | ✅ untested (eager restore) | ✅ compile-only (eager restore) | ✅ compile-only (eager restore) |
| Bookmarks | ✅ real tests | ✅ untested | ❌ | ❌ |
| History (+ similarity search) | ✅ real tests | ✅ mostly untested | 🔶 no encryption, CI-tested query paths | 🔶 no encryption, VM-verified tiles |
| Password manager (vault, Bitwarden, autofill) | ✅ real tests | ✅ untested | ❌ | ❌ |
| Settings & customization | ✅ real tests | 🔶 untested (no search-engine mgmt, no theme) | 🔶 compile-only (no theme, pick-only engine) | 🔶 VM-verified overlay, same gaps |
| Keybindings (configurable + editor) | ✅ real tests | ✅ real tests (CI, real HW) | ✅ compile-only | ✅ real unit tests (not run in CI) |
| Profiles (picker, ephemeral, encrypted) | ✅ real tests | 🔶 untested (ephemeral has no UI hook) | 🔶 compile-only (ephemeral unreachable, no encryption) | 🔶 same as winui |
| External link launch / chooser | ✅ real tests | ✅ untested | 🔶 compile-only, never run for real | ✅ VM-verified end-to-end |
| Reader mode | ✅ real tests | ❌ | ❌ | ❌ |
| Screenshot capture | ✅ real tests | ❌ | ❌ | ❌ |
| App identity / rename infrastructure | ✅ real tests | ✅ (shared-layer tests) | ✅ (shared-layer tests) | ✅ (shared-layer tests) |
| Window lifecycle / quit | ✅ real tests, both paths save | 🔶 app-menu Quit bypasses save (real gap) | ✅ compile-only, both paths save | 🔶 only Ctrl+Q saves (documented gap) |

**At a glance**: `browser-linux-gtk3` is the only front end with real, automated, end-to-end test coverage for
nearly everything (headless GUI tests via `xwfb-run`/`cage`). `browser-macos-appkit` is functionally the
closest *second*, with real feature parity across most areas — reader mode, screenshot, search-engine
management, and audio tracking are its main gaps — but almost none of that parity is covered by automated
tests, only compile/link checks and a shallow CI launch-and-screenshot smoke test. The two Windows front ends
share the smallest feature set (no bookmarks, password manager, encrypted history/profiles, theme, reader
mode, screenshot, or search-engine management) — `browser-windows-reactor` has been interactively verified
running in a real local VM for a surprising amount of its scope, while `browser-windows-winui`'s only
real-hardware attempt currently *crashes* before first paint in CI (an open, unresolved bug), leaving it the
least-proven front end despite compiling and linking cleanly.

## Low-level matrix

### Core browsing & navigation

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Back / forward / reload / navigate | ✅ real tests | ✅ untested | ✅ compile-only | ✅ VM-verified (~40 real render cycles) |
| Address bar doubles as switcher search (unified bar) | ✅ real tests | ✅ untested | ❌ separate address bar + search box | ❌ separate; search box has no plain-Enter behavior at all |

### Multi-page / tabs & switcher

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Switcher grid UI | ✅ tile grid, real tests | 🔶 plain vertical list, not a tile grid (`NSCollectionView` gap) | 🔶 fixed-column manual grid (no wrap-panel binding available) | ✅ real wrapping `grid_view` control, VM-verified |
| Next/Previous page — dispatch | ✅ real tests | ✅ wired | ✅ wired | ✅ wired |
| Next/Previous page — physical key reachable | ✅ `Ctrl+Tab`/`Ctrl+PageDown` really fire | ❌ `Tab`/`PageUp`/`PageDown` unmapped in `chord_to_key_equivalent`; default binding unreachable out of the box | ❌ same gap in `winui_vk_to_chord` | ❌ same gap in `key_to_virtual_key` |
| Loaded-page limit / eviction (unload + lazy reload on switch-back) | ✅ real tests | ✅ untested | ✅ compile-only | ✅ untested (implicit unload via reconciler) |
| Audio-playing tracking (speaker icon) | ✅ real tests (no audio backend in headless CI, so the real signal itself is untestable) | ❌ no public WKWebView API | ❌ not wired | ❌ not wired |

### Session & webview-data persistence

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Session restore across restarts | ✅ real tests | ✅ untested | ✅ compile-only | ✅ compile-only |
| Restore strategy | ✅ **lazy** — only the active page is eagerly constructed | 🔶 **eager** — every saved page gets a real engine up front, trimmed after via eviction | 🔶 eager, same as macOS | 🔶 eager, same as macOS/winui |
| Cookies/localStorage/cache persist per profile | ✅ real tests (shared `wry::WebContext`) | ✅ untested (shared `wry::WebContext`) | ✅ compile-only (`WEBVIEW2_USER_DATA_FOLDER`, profile-scoped) | ✅ compile-only (same mechanism — root-caused a real user-reported bug) |
| Ephemeral-profile data isolation | ✅ real tests (unique temp dir per session) | ✅ untested (same mechanism) | ✅ but dead code — ephemeral is unreachable on this front end (see Profiles) | ✅ but dead code, same reason |

### Bookmarks

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Toggle / list / open bookmarks | ✅ real tests | ✅ untested | ❌ no-op dispatch arm | ❌ no-op dispatch arm |

### History

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Visit tracking + search | ✅ real tests | ✅ untested | ✅ real tests (dedicated smoke-test bins + shared `browser-core` tests) | ✅ shared-layer tests + VM-verified switcher tiles |
| Lexical-similarity ("vector") search in switcher | ✅ real tests | ✅ untested (via shared `build_switcher_rows`) | ✅ via shared `build_switcher_rows` | ✅ via shared `build_switcher_rows` |
| Encrypted history (passphrase) | ✅ real tests | ✅ untested in-crate, but the shared encryption code is real-hardware-tested via `cargo test -p browser-core` in macOS CI | ❌ Windows `libsql-ffi` cross-compile gap (needs `llvm-lib`) | ❌ same gap |

### Password manager

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Local vault (add/view/copy/edit/delete) | ✅ real tests | ✅ untested | ❌ | ❌ |
| Bitwarden/Vaultwarden integration | ✅ real tests (fake `bw serve`) | ✅ untested | ❌ | ❌ |
| In-page autofill ("Fill") | ✅ real tests (fixture pages + `evaluate_script_for_test`) | ✅ untested (same `fill_login` heuristic) | ❌ | ❌ |

### Settings & customization

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Settings overlay (start page, loaded-page limit) | ✅ real tests | ✅ untested | ✅ compile-only | ✅ VM-verified (opens pre-filled, Escape closes) |
| Search engine management (add/remove) | ✅ real tests | ❌ no UI at all | 🔶 pick a default only, no add/remove | 🔶 same as winui |
| Light/dark theme | ✅ real tests | ✅ untested (app-wide `NSAppearance`, arguably a stronger implementation than gtk3's manual CSS palette) | ❌ | ❌ |

### Keybindings

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Configurable bindings + editor UI | ✅ real tests | ✅ real unit tests (`shortcuts.rs`, run on real hardware in CI, tag/manual trigger only) | ✅ compile-only (live "press keys…" capture, no unit tests in the crate at all) | ✅ real unit tests (`shortcuts.rs`, 4 tests) — **not run by any CI workflow** |

### Profiles

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Profile picker (create/switch) | ✅ real tests | ✅ untested | ✅ compile-only | ✅ untested |
| Ephemeral / private / incognito | ✅ real tests, `--incognito`/`--private`/`--guest` | 🔶 backend fully supports it (unique `WebContext` dir etc.), but **no UI or CLI entry point wired in this crate** — unreachable | ❌ `resolve_ephemeral_requested` never called from `main.rs` — unreachable | ❌ same — unreachable |
| Encrypted profile passphrase UI | ✅ real tests | ✅ untested (vault + history, shared passphrase) | ❌ | ❌ |

### External link launch / chooser

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| "Open with" URL handoff → profile chooser window | ✅ real tests | ✅ untested (CI smoke-tests the main window only, not the chooser) | ✅ compile-only, never confirmed on real hardware | ✅ **VM-verified end-to-end** — confirmed via `tasklist`/`taskkill` that Open spawns exactly one new process and the chooser exits |

### Reader mode

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Content-extraction reading view | ✅ real tests | ❌ explicit no-op | ❌ explicit no-op | ❌ explicit no-op |

### Screenshot capture

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| Save current page as an image | ✅ real tests (real PNG file written) | ❌ stub returns `Err` (`WKWebView` snapshot API not wired up) | ❌ stub returns `Err` | ❌ stub returns `Err` (`windows-webview` only has an unwrapped vtable slot) |

### App identity / rename infrastructure

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| `APP_ID`/`APP_TITLE`, `init_app_id` at startup | ✅ real tests + a live smoke test against the real binary | ✅ wired (relies on shared `browser-core` tests) | ✅ wired (relies on shared `browser-core` tests) | ✅ wired (relies on shared `browser-core` tests) |
| `--app-id`/`CLAUDE_BROWSER_APP_ID` override + legacy-id migration | ✅ 10 real `browser-core` tests, incl. one against real `directories::ProjectDirs` paths | ✅ (shared logic) | ✅ (shared logic) | ✅ (shared logic) |

### Window lifecycle / quit

| Behavior | gtk3 | macos-appkit | windows-winui | windows-reactor |
| --- | --- | --- | --- | --- |
| `Ctrl+Q`/`⌘Q`-equivalent quit saves session | ✅ real tests | ✅ (via `Keybindings`-dispatched Quit → `windowWillClose:`) | ✅ (`Action::Quit` → `window.Close()` → same `WM_DESTROY` path) | ✅ (`Action::Quit` saves synchronously, then `std::process::exit(0)`) |
| Native OS close button also saves session | ✅ real tests, same hook as Quit | ❌ **real gap**: the hardcoded `NSMenu` "Quit" item calls `terminate:` directly, bypassing `windowWillClose:`/`save_session` entirely (`AppDelegate` is only the *window's* delegate, never installed as the *application's*) | ✅ same `WM_DESTROY` path as Quit, confirmed | ❌ **documented gap**: `windows-reactor`'s `Window::Closed`/`Close` are `pub(crate)` in the vendored crate — no reachable hook from this crate at all, confirmed by direct compile error |

## Testing methodology per platform

- **`browser-core`/`browser-chrome-core`** (shared, toolkit-free logic): 136 + several real unit tests, run
  natively via `cargo test` on Linux always, and for real on native `windows-latest`/`macos-14`/`macos-13`
  GitHub Actions runners. The foundation every front end's feature-parity claims above ultimately rest on.
- **`browser-linux-gtk3`**: real, automated, end-to-end GUI tests (`cargo test` driving actual GTK/WebKitGTK
  widgets headlessly via `xwayland-run`/`cage`) — 39 tests, the only front end with this level of coverage.
  The reference implementation and the only one both feature-complete and test-complete.
- **`browser-macos-appkit`**: one real, executed unit-test slice (`shortcuts.rs`'s chord-parsing tests, 6
  tests) that runs on real `arm64`/`x64` hardware in CI — but only on a pushed tag or manual dispatch, not
  every push. Everything else is compile/link-checked via cross-compilation (`cargo zigbuild` +
  `.cargo/build-macos-appkit.sh`) plus a shallow CI launch-and-screenshot smoke test (proves the process
  doesn't crash for ~8 seconds, not that any specific feature works).
- **`browser-windows-winui`**: zero unit tests in the crate itself. CI attempts a real launch + screenshot on
  a native Windows runner, but currently **crashes** (`STATUS_STOWED_EXCEPTION`) just before first paint — an
  open, unresolved bug, so real visual/functional confirmation has never actually been reached for this front
  end. Otherwise compile/link-checked only via `cargo build-windows-winui`.
- **`browser-windows-reactor`**: 4 real unit tests (`shortcuts.rs`), ironically **not run by any CI
  workflow at all** — but uniquely among the non-reference front ends, has been extensively hand-verified
  interactively in a real local Windows VM (`dockur/windows`, Docker + QEMU/KVM) this session: navigation,
  multi-page, the switcher grid, settings/profile/keybindings overlays, and the external-link chooser were
  all confirmed actually working, not just compiling. The most real-world-proven of the three non-gtk3 front
  ends despite the weakest CI story.
