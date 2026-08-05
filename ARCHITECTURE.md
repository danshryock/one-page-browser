# Architecture

A living reference for how this workspace is put together, where cross-platform reuse is already working,
where it isn't, and what's planned to close the gap. Complements `README.md` (what things are and how to
build them) and `ROADMAP.md` (status/what's next) rather than duplicating either — this document is about
*structure*, not status.

## 1. Current layering

```
browser-core         pure domain logic: Page/PageManager, Settings, Keybindings, Profile,
                      HistoryStore, Bookmarks, embedding/vector search. Zero native-UI
                      dependency. Generic over a RenderEngine impl for testing (MockEngine,
                      no toolkit — exposed via `testing`, see §4). 86 unit tests, toolkit-free.
                           |
render-engine         RenderEngine trait (navigate/go_back/go_forward/reload/current_url/
                      screenshot) + one impl per platform family (linux via WebKitGTK,
                      macos via WKWebView). `browser-windows-reactor` doesn't use this
                      crate for its engine at all — it keeps `ReactorWebViewEngine` local
                      (see §2's placement-policy note) — so render-engine today only
                      covers Linux/macOS. (windows-gnu's WryEngine was removed along with
                      browser-windows-win32/nwg, and the msvc winio-winui3-based
                      WebView2Engine was removed along with browser-windows-winui itself
                      — see the scope note below; nothing left targets either.)
                           |
browser-chrome-core   NEW (§4/§7): toolkit-agnostic decision logic sitting between
                      browser-core's raw data and each frontend's native UI — currently
                      just `switcher` (SwitcherRow/build_switcher_rows/activate_row), unit-
                      tested with browser_core::testing::MockEngine + MemoryHistoryStore.
                           |
3 frontend crates     browser-linux-gtk3, browser-windows-reactor, browser-macos-appkit.
                      Each owns: window/toolbar construction, PageManager wiring,
                      switcher/settings/profile-picker overlays, keybindings editor, and
                      native event → core-call glue — though the switcher piece is now
                      `browser-chrome-core`'s job for the frontends already migrated (§4).
```

`browser-core` (2,522 lines), `render-engine` (520 lines), and the new `browser-chrome-core` are the
well-modularized layers — see §2/§4. The 4 frontend crates are where the *same* decision logic gets
rewritten, in a different native idiom, every time a new one is added — less of it than before now that
`browser-chrome-core` exists, but plenty still remains (settings/keybindings/profile-picker/page-lifecycle).
See §3 for the evidence.

**Note on scope**: this document was originally written against 7 frontend crates; `browser-windows-win32`,
`browser-windows-nwg`, and `browser-wx` were deleted afterward to reduce the number of near-duplicate
implementations (unmaintained, behind the other frontends in feature scope, and Wine-cross-compile-only —
see `ROADMAP.md`). `browser-windows-winui` was later deleted too — real WinUI 3 apps crash on GitHub Actions'
GPU-less Windows runners just before first paint (`STATUS_STOWED_EXCEPTION`, confirmed via crash-dump
analysis to be inside WinUI 3's own Composition/WinRT internals, not this codebase's own code), with no
workaround found after an extensive bisection pass, while `browser-windows-reactor` — the other WinUI 3
front end, built on a different underlying crate — worked fine; too problematic to keep maintaining
alongside a working alternative (see `ROADMAP.md`). Several findings below cite deleted crates' code
specifically, since the patterns found there (both good and bad) are still instructive even with the crates
themselves gone — recoverable from git history if needed. Sections marked accordingly.

## 2. What's already well-modularized (keep doing this)

- **`browser-core`**: `PageManager<E>` is generic purely over the `RenderEngine` trait, so its tests
  (load/unload eviction, `matching_ids`, active-page tracking) run against a `MockEngine` with zero GTK/
  AppKit/WinUI involved. This is the model to extend — see §4.
- **`render_engine::RenderEngine`**: a genuinely minimal, stable 5-method trait. Every platform supplies its
  own struct implementing it; callers (`PageManager<E>`, every frontend's `with_active`) never know which.
- **Per-crate chord↔native-shortcut conversion** (`shortcuts.rs` in `browser-windows-reactor` and
  `browser-macos-appkit`): small, pure functions (`parse_chord`, `chord_to_accelerator`/
  `chord_to_key_equivalent`) each with their own `#[cfg(test)] mod tests` that runs with no native toolkit
  at all. Right pattern — just currently rewritten per-crate rather than sharing a home. See §3.5.
- **The `render-engine` placement policy, once made explicit** (it isn't currently written down anywhere,
  which is itself a gap — see §4.6): an engine impl lives in `render-engine` *unless* its native dependency
  would burden other frontends' builds. `browser-windows-reactor` keeps its `ReactorWebViewEngine` local
  specifically to avoid pulling `windows-reactor`'s git dependency into other frontends' builds — originally
  written to avoid burdening the now-deleted `browser-windows-winui` specifically, but the same reasoning
  still holds for any future frontend that might depend on `render-engine` (documented in `engine.rs`'s own
  module comment) — a good, deliberate call, but nothing currently tells the *next* person adding a frontend
  when to make the same call versus defaulting to `render-engine`. (The now-deleted `browser-wx` was the
  other example of this — it kept its engine local because it wrapped a wholly different webview control,
  `wxWebView` rather than `wry`, that the trait didn't otherwise need.)

## 3. Duplication found, with evidence

Compared the equivalent function across crates rather than guessing — each item below cites real file:line
references checked this pass.

### 3.1 Page lifecycle orchestration

`add_page`/`ensure_engine_loaded`/`unload_engines`/`set_active`/`close_page` follow the identical five-step
dance in every `PageManager`-backed frontend, including the now-deleted `browser-windows-win32`/
`browser-wx` (kept below as evidence — the pattern held across all 7 original frontends, not just the 4
remaining): allocate an id from `PageManager`, build a native container (or not — see §3.7), construct the
engine with a title-changed callback, `insert`/evict via `PageManager`, activate. Compare:

```rust
// browser-linux-gtk3/src/lib.rs:143
pub fn add_page(self: &Rc<Self>, url: &str) -> anyhow::Result<()> {
    let id = self.core.borrow_mut().allocate_id();
    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    self.stack.add_named(&container, &id);
    ...
    let engine = WryEngine::new(&container, url, move |new_title| { ... })?;
    let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
    self.unload_engines(&evicted);
    self.set_active(&id);
    self.rebuild_switcher_grid();
    Ok(())
}

// browser-windows-win32/src/lib.rs:157 (crate since deleted) — same shape, no container widget (see §3.7)
pub fn add_page(&self, url: &str) -> anyhow::Result<()> {
    let id = self.core.borrow_mut().allocate_id();
    let engine = WryEngine::new(self.hwnd, url, build_title_changed_callback(...))?;
    let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
    self.unload_engines(&evicted);
    self.set_active(&id);
    ...
}

// browser-macos-appkit/src/lib.rs:363 — same shape again, written this session
fn add_page(self: &Rc<Self>, url: &str) -> anyhow::Result<String> {
    let id = core.allocate_id();
    let container = NSView::initWithFrame(...);
    let engine = WryEngine::new(&container, url, move |title| { ... })?;
    let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
    self.unload_engines(&evicted);
    self.set_active(&id);
    Ok(id)
}
```

Seven independent implementations of the same orchestration, none of it toolkit-specific except the one
line that builds a container.

### 3.2 Switcher row computation

`matching_ids` (query filter) + an "Add" row + history-search fallback (only once there's a query, only for
URLs not already open) is reimplemented per frontend as a different-shaped list (GTK `FlowBox` tiles,
`windows-reactor`'s `Tile` enum + `grid_view`, `browser-macos-appkit`'s `SwitcherRow` enum + plain list). The
*decision* logic — which rows exist, in what order — is identical; only the native rendering differs. This
one is nearly ready to extract as-is: `browser-windows-reactor`'s `Tile`/`switcher_overlay` and
`browser-macos-appkit`'s `SwitcherRow`/`rebuild_switcher_rows` are *already* pure data + a builder function
over `PageManager`/`HistoryStore` — they just live inside the frontend crate instead of somewhere shared.

**Drift already visible**: `browser-linux-gtk3`'s tiles carry a per-page `color` (from `PageManager`'s
palette) rendered as real background CSS; neither `browser-windows-reactor`'s `Tile` nor
`browser-macos-appkit`'s `SwitcherRow` carry color at all — a visual feature quietly lost in every later
port, not a deliberate scope cut.

### 3.3 Unified address-bar/search-box state machine

The address bar doubling as the switcher's search box (Enter behavior depends on whether the switcher is
open) is hand-copied in `browser-linux-gtk3` and `browser-macos-appkit`: is-switcher-open →
exactly-one-open-match switches to it → else exactly-one-history-match opens it → else resolve the text as
a URL/search and open a new page. **Correction to an earlier version of this section**: `browser-windows-
reactor` doesn't actually share this design — its switcher has its own, separate search box
(`switcher_overlay`'s `search_box`, independent of the toolbar's `address` state) with no plain-Enter
behavior at all, only click/selection on a tile. A real, smaller-than-first-described divergence, not the
close three-way duplicate originally claimed here.

**Drift found, now fixed**: `browser-linux-gtk3` has a documented escape hatch — Ctrl+Enter always forces a
brand-new page even when the typed text matches an open page (`force_new_page_from_search`,
`browser-linux-gtk3/src/lib.rs:911`). Neither `browser-windows-reactor` nor `browser-macos-appkit` carried
that escape hatch forward when first built — a real feature silently dropped during porting, not a
documented scope decision, and the concrete demonstration that motivated this whole document. **Fixed** in
both: `browser-macos-appkit`'s `force_new_page_from_search` (checks ⌘ via `NSApplication.currentEvent`'s
modifier flags — there's no argument carrying it directly into an AppKit action method) and
`browser-windows-reactor`'s search box now has a real `Ctrl+Enter` `KeyboardAccelerator` wired to the same
behavior (`resolve_address_input` + `add_page_and_switch`), the first keyboard interaction that search box
has ever had.

### 3.4 Settings save/cancel draft state

Copy `Settings` into local draft fields on open, validate/parse the loaded-pages-limit field, apply on Save,
discard on Cancel — same shape in `browser-linux-gtk3`, `browser-windows-reactor`, `browser-macos-appkit`
(and the now-deleted `browser-windows-winui` had it too). None of this touches a native widget except the
two lines that read/write the draft fields from/to actual controls.

### 3.5 Keybindings editor state machine

`listening_for: Option<Action>` (waiting for a new binding), add/remove/commit against `Keybindings`,
persist, refresh — identical in `browser-linux-gtk3`, `browser-windows-reactor`, `browser-macos-appkit` (the
now-deleted `browser-windows-winui` had it too). The one genuine platform difference is *how* a chord is
captured: `browser-linux-gtk3` (and, per its own docs, the now-deleted `browser-windows-win32`/
`browser-windows-nwg` had they built this feature, and `browser-windows-winui` actually did) can do live
"press keys…" capture via a raw keydown event; `windows-reactor` and `browser-macos-appkit` fell back to
typed-text parsing (`"Ctrl+Shift+P"`) because neither toolkit's declarative/high-level shortcut API exposes
a generic "capture the next keypress" hook. So `parse_chord` itself genuinely isn't shareable across every
capture style — but everything *after* obtaining a `KeyChord` (add/remove/persist/refresh) is 100% identical
logic, currently copy-pasted across all three remaining frontends regardless of capture method.

### 3.6 Profile picker

`list_profile_names()` → mark the current one → click launches a new process or closes the picker (if
already current). Same in `browser-linux-gtk3`, `browser-windows-reactor`, `browser-macos-appkit` — i.e.
universal across every remaining frontend (the now-deleted `browser-windows-winui` had it too). It was
absent in the now-deleted `browser-windows-win32`/`browser-windows-nwg`/`browser-wx` (they only supported
one profile via `--profile`, no in-app switcher UI) — a real scope gap at the time, not a bug, and moot now
that those crates are gone.

### 3.7 Page-container strategy — a genuine platform difference, not just duplication

Not everything here *can* unify:

- `browser-linux-gtk3`/`browser-macos-appkit`: a real native container widget per page (`gtk::Box`/`NSView`)
  that owns visibility — hide the container, its embedded webview hides too. (The now-deleted
  `browser-windows-nwg` used the same strategy, via `nwg::Frame`.)
- The now-deleted `browser-windows-win32` had no container concept at all (documented in its own module doc
  as a deliberate departure) — the webview's raw HWND was shown/hidden directly.
- `browser-windows-reactor`: no container either, for a different reason — `windows-reactor`'s declarative
  model has no visibility primitive at all (checked directly against its source); every loaded page's
  element stays mounted, z-order alone decides what's on top.

This is real, load-bearing platform variance, not something a future refactor should try to paper over —
worth naming explicitly so it isn't mistaken for an oversight later.

### 3.8 History-visit recording point — inconsistent, and one crate drops it entirely

- `browser-linux-gtk3` records a visit from the webview's title-changed callback
  (`AppState::record_visit`, `browser-linux-gtk3/src/lib.rs:176`).
- `browser-windows-reactor` records it from the navigation-completed callback instead
  (`on_navigation_completed`'s `reflect` closure, `browser-windows-reactor/src/lib.rs`).
- `browser-macos-appkit` — checked directly this pass — **never called `history.record_visit` anywhere**.
  It read history (for switcher suggestions) but never wrote to it; browsing history silently never
  accumulated on this platform. **Fixed**: added the same `record_visit(id)` method
  `browser-linux-gtk3` has, called from both title-changed callbacks (`add_page`/`ensure_engine_loaded`).

This is the single most concrete piece of evidence for why this document exists: there is no single
"a page finished navigating" hook that *forces* every frontend to remember to record history. Each one has
to independently notice it needs to, on whatever event happens to be that platform's navigation-succeeded
signal — and one of them didn't.

### 3.9 A related reference-cycle bug (found while checking §3.8), now fixed

`browser-macos-appkit`'s `add_page`/`ensure_engine_loaded` captured `Rc::clone(self)` (a strong reference to
the whole `AppState`) inside the per-page title-changed closure, which is stored inside that page's
`wry::WebView`, which lives inside `PageManager`, which lives inside `AppState.core`. That was a genuine
reference cycle: `AppState → core → PageManager → Page.engine → wry::WebView → closure → Rc<AppState>` —
`AppState` would never deallocate for the life of the process (harmless in practice — the process exits and
the OS reclaims everything regardless of Rust-level `Drop` — but a real leak nonetheless, and exactly the
kind of subtle mistake hand-copying this pattern invites). **Fixed**: both callbacks now capture
`Rc::downgrade(self)` and `.upgrade()` inside the closure, matching the pattern below.

Other frontends already avoided this, in two different ways, worth normalizing:
- `browser-linux-gtk3` uses `Rc::downgrade(self)`/`.upgrade()` in the same callback
  (`browser-linux-gtk3/src/lib.rs:153`) — the pattern `browser-macos-appkit` now also uses.
- The now-deleted `browser-windows-win32` sidestepped it structurally — its title-changed callback only
  closed over the title `RefCell` and the raw `HWND`, never a strong reference to the whole app state at all
  (`build_title_changed_callback`, `browser-windows-win32/src/lib.rs:430`) — a good pattern worth keeping in
  mind even with that crate gone.

## 4. Future state: a shared, toolkit-agnostic decision layer

Proposal: introduce a new crate, `browser-chrome-core`, sitting between `browser-core`'s raw data types and
each frontend's native UI. Not more modules inside `browser-core` itself — `browser-core` today has zero UI
concepts (`Page`, `Settings`, `Keybindings`, `Profile`) and should stay that way; "what happens when switcher
row 3 is clicked" is a different, UI-adjacent concern even though it still requires no actual widget
toolkit. Each piece below is a plain struct/enum + methods, generic over `RenderEngine` the same way
`PageManager` already is, unit-tested with the same `MockEngine` `browser-core`'s own tests already use —
no new test infrastructure needed.

- **`PageController<E>`**: wraps `PageManager<E>`; exposes `add_page`, `switch_to`, `close_page`,
  `ensure_engine_loaded`, `unload_engines`, and an `address_bar_activated(text, is_switcher_open) ->
  AddressBarOutcome` decision function, where
  ```rust
  enum AddressBarOutcome { Navigate(String), SwitchToPage(String), OpenNewPage(String), NoOp }
  ```
  so every frontend just matches on the result and does native things — the branching itself
  (§3.3, including restoring the Ctrl+Enter escape hatch) runs once, tested once.
- ✅ **`SwitcherModel`** — done, as the new `browser-chrome-core` crate's `switcher` module:
  `build_switcher_rows(&PageManager<E>, &impl HistoryBackend, query) -> Vec<SwitcherRow>` plus
  `activate_row(&[SwitcherRow], idx, start_page) -> Option<SwitcherActivation>`, restoring the dropped
  `color` field from §3.2. Generic over `HistoryBackend` (not just `HistoryStore`), so it's tested with
  `MemoryHistoryStore` — real `MockEngine`/`MemoryHistoryStore`, zero real webview/SQLite I/O, reusing
  `browser-core`'s own test doubles exactly as planned (see `browser_core::testing`, newly exposed for this
  — previously private to `browser-core`'s own `#[cfg(test)] mod tests`). Generalized once more (`Bookmark`/
  `Similar` row variants, `bookmarks: Option<&Bookmarks>` parameter) before migrating `browser-linux-gtk3` —
  its real switcher also searches bookmarks and lexically-similar history matches, which the original
  3-variant shape (modeled on the simpler reactor/macos-appkit switchers) would have silently dropped.
  Every frontend that existed at the time was migrated: `browser-windows-reactor` and `browser-macos-appkit`
  (their local `Tile`/`SwitcherRow` enums and hand-copied row-building removed entirely), `browser-linux-gtk3`
  (its tile-building split into `build_open_tile`/`build_add_tile`/`build_search_result_tile` helpers keyed
  by `SwitcherRow` variant, with every tile's `widget_name` now its index into a stored `switcher_rows`
  snapshot — a bonus fix along the way: history/bookmark/similar tiles previously had no `widget_name` at
  all, so keyboard Enter/Space only ever worked on open-page/add tiles; routing every tile through the same
  index-based `activate_switcher_row` fixed that gap for free), and the now-deleted `browser-windows-winui`
  (`None` passed for `bookmarks` — no bookmarks integration in that crate — and each tile's `Click` closure
  just captured the `SwitcherActivation` computed once at build time, rather than a separate closure shape
  per row kind).
- **`SettingsController`**: draft-state handling (start page, search engine index, unlimited/limit), Save/
  Cancel, `String`/`bool`/`Option<usize>` fields only — no native widgets anywhere in this type.
- **`KeybindingsController`**: add/remove/commit against `Keybindings`, decoupled from *how* the `KeyChord`
  was obtained (accepts one regardless of live-capture vs. text-parse origin — see §3.5).
- **`ProfilePickerModel`**: list rows (current marked), click-to-launch-or-close decision.

### 4.6 Formalize the `render-engine` placement policy

Write down, in `render-engine/src/lib.rs`'s module doc comment, the rule already being followed
inconsistently-documented in individual crates (§2): an engine impl belongs in `render-engine` by default;
it stays local to a frontend crate only when (a) its native dependency would burden *other* frontends'
builds (the `windows-reactor` git-dependency case), or (b) it wraps a fundamentally different webview
control the trait doesn't otherwise need (the now-deleted `browser-wx`'s `wxWebView` case was the only
example of this) — and either exception should be a one-line comment at the impl site saying which reason
applies, so the next new frontend doesn't have to rediscover the reasoning from scratch.

## 5. Quick wins — done

All three found during the original pass over this document, fixed in the follow-up session that also
deleted `browser-windows-win32`/`browser-windows-nwg`/`browser-wx` (see `ROADMAP.md`):

1. ✅ **`browser-macos-appkit`: wire up `history.record_visit`** (§3.8) — added, called from both
   title-changed callbacks (`add_page`/`ensure_engine_loaded`), matching `browser-linux-gtk3`'s approach.
2. ✅ **`browser-macos-appkit`: break the `Rc` cycle** (§3.9) — both title-changed callbacks now capture
   `Rc::downgrade(self)` + `.upgrade()`, matching `browser-linux-gtk3`'s existing pattern.
3. ✅ **Restore the Ctrl+Enter "force new page" escape hatch** (§3.3) — added to both
   `browser-windows-reactor` (a real `KeyboardAccelerator` on the switcher's search box — its first keyboard
   interaction at all) and `browser-macos-appkit` (checks ⌘ via `NSApplication.currentEvent`'s modifier
   flags inside the address bar's action method).

All three verified via `cargo check --workspace`, `cargo test -p browser-core`, and cross-compiling both
Windows (`cargo build-windows-winui`/`cargo build-windows-reactor`) and macOS (`.cargo/build-macos-appkit.sh`,
both architectures) after the changes.

## 6. Testability

`browser-core`'s `MockEngine`-based test pattern (86 tests, zero native toolkit, `crates/browser-core/src/
lib.rs`'s `#[cfg(test)] mod tests`) is the thing to extend, not replace. Once the controllers/models in §4
exist, they're testable the exact same way — meaning the switcher/settings/keybindings/profile-picker
decision logic across all three remaining frontends, which currently has **zero automated test coverage**
(only exercised by manual clicking), gets real unit tests essentially for free.

This is complementary to, not a replacement for, `browser-linux-gtk3`'s existing `tests/gtk_tests.rs`
(using `gtk-test`, replacing what used to be manually-run `examples/nav_test.rs`/`switcher_test.rs`
binaries — see `README.md`'s Testing section): that suite verifies the *real native rendering* end-to-end
for one platform, driving actual live GTK widgets with synthetic input; the controller/model extraction
here verifies the *decision logic* with no toolkit at all, for every platform at once. Both are worth
having; neither substitutes for the other.

## 7. Suggested rollout order

Staged so each step is independently buildable/testable before the next starts — not a big-bang rewrite:

1. ✅ **Quick wins** (§5) — done.
2. ✅ **`SwitcherModel`** — done as a crate, migrated to every frontend that existed at the time
   (`browser-windows-reactor`, `browser-macos-appkit`, `browser-linux-gtk3`, and the now-deleted
   `browser-windows-winui`). One deliberately-deferred item remains: `browser-windows-reactor`'s
   `tile_element` still doesn't render `SwitcherRow::Open`'s `color` —
   no background-color builder exists in that crate's bound subset of the WinUI 3 API (see the comment at
   `tile_element`'s definition) — a real, narrow toolkit gap, not a modeling gap.
3. **`SettingsController` + `KeybindingsController`** — same treatment.
4. **`PageController`'s decision logic** — trickiest, since container strategy genuinely differs by platform
   (§3.7); only the *decision* half (what should happen) extracts, containers stay native/local.
