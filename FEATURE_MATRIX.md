# Feature matrix

What's implemented, and how well it's actually verified, across the three front ends
(`browser-linux-gtk3`, `browser-macos-appkit`, `browser-windows-reactor`). Built by reading current source
directly (not ROADMAP.md's chronological narrative, which documents *how* things happened but drifts from
exact current scope) — see ROADMAP.md for the story behind any given row.

A fourth front end, `browser-windows-winui`, was covered here too until it was deleted (crashed on every
real-hardware CI run, unfixable after an extensive bisection pass, too problematic to keep maintaining
alongside a working alternative — see ROADMAP.md's "Deleted" entry). Its rows are gone, not marked ❌.

Two tables: a high-level summary (one row per feature area), then a low-level breakdown grouped by the same
areas (one row per concrete behavior). Testing-status legend applies to both:

<div class="matrix-legend">

| Symbol | Meaning |
| --- | --- |
| **real tests** | Automated tests that actually execute this behavior (`cargo test`, run for real — not just compiled) |
| **compile-only** | Compiles/links (Linux workspace check, or a cross-compile via `cargo build-windows-reactor`/`.cargo/build-macos-appkit.sh`), never executed |
| **CI (real HW)** | Executed on real hardware in GitHub Actions (not this dev machine), but not via a unit test — a launch/smoke check |
| **VM-verified** | Manually, interactively verified running in a local Windows VM (`dockur/windows`) this session — real behavior, not automated |
| **untested** | Implemented, compiles, but no automated test and no real-hardware run has ever exercised it |

Implementation legend: ✅ Full · 🔶 Partial (works, with a real named gap) · ❌ Not implemented / not reachable.

</div>

## Summary matrix

| Feature area | `browser-linux-gtk3` | `browser-macos-appkit` | `browser-windows-reactor` |
| --- | --- | --- | --- |
| Core browsing & navigation | ✅ real tests | 🔶 untested, except opener/popup (CI + real HW) | 🔶 VM-verified nav, no unified bar |
| Multi-page / tabs & switcher | ✅ real tests | 🔶 untested (list, not tile grid) | 🔶 VM-verified (real grid control) |
| Session & webview-data persistence | ✅ real tests | ✅ untested (eager restore) | ✅ compile-only (eager restore) |
| Bookmarks | ✅ real tests | ✅ untested | ❌ |
| History (+ similarity search) | ✅ real tests | ✅ mostly untested | 🔶 no encryption, VM-verified tiles |
| Password manager (vault, Bitwarden, autofill) | ✅ real tests | ✅ untested | ❌ |
| Settings & customization | ✅ real tests | 🔶 untested (no search-engine mgmt, no theme) | 🔶 VM-verified overlay, same gaps |
| Keybindings (configurable + editor) | ✅ real tests | ✅ real tests (CI, real HW) | ✅ real unit tests (not run in CI) |
| Profiles (picker, ephemeral, encrypted) | ✅ real tests | 🔶 untested (ephemeral has no UI hook) | 🔶 ephemeral unreachable, no encryption |
| External link launch / chooser | ✅ real tests | ✅ untested | ✅ VM-verified end-to-end |
| Reader mode | ✅ real tests | ❌ | ❌ |
| Screenshot capture | ✅ real tests | ❌ | ❌ |
| App identity / rename infrastructure | ✅ real tests | ✅ (shared-layer tests) | ✅ (shared-layer tests) |
| Window lifecycle / quit | ✅ real tests, both paths save | 🔶 app-menu Quit bypasses save (real gap) | 🔶 only Ctrl+Q saves (documented gap) |

**At a glance**: `browser-linux-gtk3` is the only front end with real, automated, end-to-end test coverage for
nearly everything (headless GUI tests via `xwfb-run`/`cage`). `browser-macos-appkit` is functionally the
closest *second*, with real feature parity across most areas — reader mode, screenshot, search-engine
management, and audio tracking are its main gaps — but almost none of that parity is covered by automated
tests, only compile/link checks and a shallow CI launch-and-screenshot smoke test, plus one real exception:
opener/popup behavior (`web-standards-tests/`) is genuinely verified, both in CI (real macOS runners) and
against a real 2014 Intel Mac running Big Sur over SSH (`scripts/macos-mac/`). Getting there took ruling out
CGEvent-based synthetic input entirely — TCC/Accessibility permission can't be granted non-interactively on
either an ephemeral CI runner or (confirmed directly, not assumed) over SSH on real hardware, even with a
dedicated signing identity — in favor of `AppState::start_test_command_listener`, a local Unix-socket command
channel the driver talks to directly, bypassing OS-level input and TCC entirely. `browser-windows-reactor`
has the smallest feature set (no bookmarks, password manager, encrypted history/profiles, theme, reader
mode, screenshot, or search-engine management) but has been interactively verified running in a real local
VM for a surprising amount of its scope — more real-world-proven behavior than its thin CI/unit-test
footprint would suggest on its own.

## Low-level matrix

### Core browsing & navigation

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Back / forward / reload / navigate | ✅ real tests | ✅ untested | ✅ VM-verified (~40 real render cycles) |
| Address bar doubles as switcher search (unified bar) | ✅ real tests | ✅ untested | ❌ separate; search box has no plain-Enter behavior at all |

### Multi-page / tabs & switcher

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Switcher grid UI | ✅ tile grid, real tests | 🔶 plain vertical list, not a tile grid (`NSCollectionView` gap) | ✅ real wrapping `grid_view` control, VM-verified |
| Next/Previous page — dispatch | ✅ real tests | ✅ wired | ✅ wired |
| Next/Previous page — physical key reachable | ✅ `Ctrl+Tab`/`Ctrl+PageDown` really fire | ❌ `Tab`/`PageUp`/`PageDown` unmapped in `chord_to_key_equivalent`; default binding unreachable out of the box | ❌ same gap in `key_to_virtual_key` |
| Loaded-page limit / eviction (unload + lazy reload on switch-back) | ✅ real tests | ✅ untested | ✅ untested (implicit unload via reconciler) |
| Audio-playing tracking (speaker icon) | ✅ real tests (no audio backend in headless CI, so the real signal itself is untestable) | ❌ no public WKWebView API | ❌ not wired |

### Session & webview-data persistence

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Session restore across restarts | ✅ real tests | ✅ untested | ✅ compile-only |
| Restore strategy | ✅ **lazy** — only the active page is eagerly constructed | 🔶 **eager** — every saved page gets a real engine up front, trimmed after via eviction | 🔶 eager, same as macOS |
| Cookies/localStorage/cache persist per profile | ✅ real tests (shared `wry::WebContext`) | ✅ untested (shared `wry::WebContext`) | ✅ compile-only (`WEBVIEW2_USER_DATA_FOLDER`, profile-scoped — root-caused a real user-reported bug) |
| Ephemeral-profile data isolation | ✅ real tests (unique temp dir per session) | ✅ untested (same mechanism) | ✅ but dead code — ephemeral is unreachable on this front end (see Profiles) |

### Bookmarks

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Toggle / list / open bookmarks | ✅ real tests | ✅ untested | ❌ no-op dispatch arm |

### History

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Visit tracking + search | ✅ real tests | ✅ untested | ✅ shared-layer tests + VM-verified switcher tiles |
| Lexical-similarity ("vector") search in switcher | ✅ real tests | ✅ untested (via shared `build_switcher_rows`) | ✅ via shared `build_switcher_rows` |
| Encrypted history (passphrase) | ✅ real tests | ✅ untested in-crate, but the shared encryption code is real-hardware-tested via `cargo test -p browser-core` in macOS CI | ❌ Windows `libsql-ffi` cross-compile gap (needs `llvm-lib`) |

### Password manager

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Local vault (add/view/copy/edit/delete) | ✅ real tests | ✅ untested | ❌ |
| Bitwarden/Vaultwarden integration | ✅ real tests (fake `bw serve`) | ✅ untested | ❌ |
| In-page autofill ("Fill") | ✅ real tests (fixture pages + `evaluate_script_for_test`) | ✅ untested (same `fill_login` heuristic) | ❌ |

### Settings & customization

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Settings overlay (start page, loaded-page limit) | ✅ real tests | ✅ untested | ✅ VM-verified (opens pre-filled, Escape closes) |
| Search engine management (add/remove) | ✅ real tests | ❌ no UI at all | 🔶 pick a default only, no add/remove |
| Light/dark theme | ✅ real tests | ✅ untested (app-wide `NSAppearance`, arguably a stronger implementation than gtk3's manual CSS palette) | ❌ |

### Keybindings

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Configurable bindings + editor UI | ✅ real tests | ✅ real unit tests (`shortcuts.rs`, run on real hardware in CI, tag/manual trigger only) | ✅ real unit tests (`shortcuts.rs`, 4 tests) — **not run by any CI workflow** |

### Profiles

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Profile picker (create/switch) | ✅ real tests | ✅ untested | ✅ untested |
| Ephemeral / private / incognito | ✅ real tests, `--incognito`/`--private`/`--guest` | 🔶 backend fully supports it (unique `WebContext` dir etc.), but **no UI or CLI entry point wired in this crate** — unreachable | ❌ `resolve_ephemeral_requested` never called from `main.rs` — unreachable |
| Encrypted profile passphrase UI | ✅ real tests | ✅ untested (vault + history, shared passphrase) | ❌ |

### External link launch / chooser

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| "Open with" URL handoff → profile chooser window | ✅ real tests | ✅ untested (CI smoke-tests the main window only, not the chooser) | ✅ **VM-verified end-to-end** — confirmed via `tasklist`/`taskkill` that Open spawns exactly one new process and the chooser exits |

### Reader mode

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Content-extraction reading view | ✅ real tests | ❌ explicit no-op | ❌ explicit no-op |

### Screenshot capture

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| Save current page as an image | ✅ real tests (real PNG file written) | ❌ stub returns `Err` (`WKWebView` snapshot API not wired up) | ❌ stub returns `Err` (`windows-webview` only has an unwrapped vtable slot) |

### App identity / rename infrastructure

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| `APP_ID`/`APP_TITLE`, `init_app_id` at startup | ✅ real tests + a live smoke test against the real binary | ✅ wired (relies on shared `browser-core` tests) | ✅ wired (relies on shared `browser-core` tests) |
| `--app-id`/`CLAUDE_BROWSER_APP_ID` override + legacy-id migration | ✅ 10 real `browser-core` tests, incl. one against real `directories::ProjectDirs` paths | ✅ (shared logic) | ✅ (shared logic) |

### Window lifecycle / quit

| Behavior | gtk3 | macos-appkit | windows-reactor |
| --- | --- | --- | --- |
| `Ctrl+Q`/`⌘Q`-equivalent quit saves session | ✅ real tests | ✅ (via `Keybindings`-dispatched Quit → `windowWillClose:`) | ✅ (`Action::Quit` saves synchronously, then `std::process::exit(0)`) |
| Native OS close button also saves session | ✅ real tests, same hook as Quit | ❌ **real gap**: the hardcoded `NSMenu` "Quit" item calls `terminate:` directly, bypassing `windowWillClose:`/`save_session` entirely (`AppDelegate` is only the *window's* delegate, never installed as the *application's*) | ❌ **documented gap**: `windows-reactor`'s `Window::Closed`/`Close` are `pub(crate)` in the vendored crate — no reachable hook from this crate at all, confirmed by direct compile error |

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
- **`browser-windows-reactor`**: 4 real unit tests (`shortcuts.rs`), ironically **not run by any CI
  workflow at all** — but has been extensively hand-verified interactively in a real local Windows VM
  (`dockur/windows`, Docker + QEMU/KVM) this session: navigation, multi-page, the switcher grid, settings/
  profile/keybindings overlays, and the external-link chooser were all confirmed actually working, not just
  compiling. More real-world-proven than its thin CI/unit-test footprint alone would suggest.


<style>
/* make all tables default to the identical column width, except for the feature matrix legend table */
:not(.matrix-legend) > table { table-layout: fixed; width: 100%; }
</style>
