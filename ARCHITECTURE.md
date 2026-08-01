# Architecture

A living reference for how this workspace is put together, where cross-platform reuse is already working,
where it isn't, and what's planned to close the gap. Complements `README.md` (what things are and how to
build them) and `ROADMAP.md` (status/what's next) rather than duplicating either — this document is about
*structure*, not status.

## 1. Current layering

```
browser-core        pure domain logic: Page/PageManager, Settings, Keybindings, Profile,
                     HistoryStore, Bookmarks, embedding/vector search. Zero native-UI
                     dependency. Generic over a RenderEngine impl for testing (mock engine,
                     no toolkit). 86 unit tests, all toolkit-free.
                          |
render-engine        RenderEngine trait (navigate/go_back/go_forward/reload/current_url/
                     screenshot) + one impl per platform family (linux via WebKitGTK,
                     windows-gnu via WebView2, macos via WKWebView). This is the one
                     dimension that's genuinely swappable today.
                          |
4 frontend crates    browser-linux-gtk3, browser-windows-{winui,reactor},
                     browser-macos-appkit. Each owns: window/toolbar construction,
                     PageManager wiring, switcher/settings/profile-picker overlays,
                     keybindings editor, and native event → core-call glue.
```

`browser-core` (2,522 lines) and `render-engine` (520 lines) are the well-modularized layer — see §2.
The 4 frontend crates are where the *same* decision logic gets rewritten, in a different native idiom,
every time a new one is added. See §3 for the evidence.

**Note on scope**: this document was originally written against 7 frontend crates; `browser-windows-win32`,
`browser-windows-nwg`, and `browser-wx` were deleted afterward to reduce the number of near-duplicate
implementations (unmaintained, behind the other frontends in feature scope, and Wine-cross-compile-only —
see `ROADMAP.md`). Several findings below cite their code specifically, since the patterns found there (both
good and bad) are still instructive even with the crates themselves gone — recoverable from git history if
needed. Sections marked accordingly.

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
  specifically to avoid pulling `windows-reactor`'s git dependency into `browser-windows-winui`'s build
  (documented in `engine.rs`'s own module comment) — a good, deliberate call, but nothing currently tells
  the *next* person adding a frontend when to make the same call versus defaulting to `render-engine`. (The
  now-deleted `browser-wx` was the other example of this — it kept its engine local because it wrapped a
  wholly different webview control, `wxWebView` rather than `wry`, that the trait didn't otherwise need.)

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
open) is hand-copied at least three times (`browser-linux-gtk3`, `browser-windows-reactor`,
`browser-macos-appkit`), each time re-deriving: is-switcher-open → exactly-one-open-match switches to it →
else exactly-one-history-match opens it → else resolve the text as a URL/search and open a new page.

**Drift already visible**: `browser-linux-gtk3` has a documented escape hatch — Ctrl+Enter always forces a
brand-new page even when the typed text matches an open page (`force_new_page_from_search`,
`browser-linux-gtk3/src/lib.rs:911`). Neither the `browser-windows-reactor` nor `browser-macos-appkit` port
written this session carried that escape hatch forward — a real feature silently dropped during porting,
not a documented scope decision. Concrete demonstration of the exact risk this document is about.

### 3.4 Settings save/cancel draft state

Copy `Settings` into local draft fields on open, validate/parse the loaded-pages-limit field, apply on Save,
discard on Cancel — same shape in `browser-linux-gtk3`, `browser-windows-winui`, `browser-windows-reactor`,
`browser-macos-appkit`. None of this touches a native widget except the two lines that read/write the
draft fields from/to actual controls.

### 3.5 Keybindings editor state machine

`listening_for: Option<Action>` (waiting for a new binding), add/remove/commit against `Keybindings`,
persist, refresh — identical in `browser-linux-gtk3`, `browser-windows-winui`, `browser-windows-reactor`,
`browser-macos-appkit`. The one genuine platform difference is *how* a chord is captured: `browser-linux-gtk3`
(and, per its own docs, the now-deleted `browser-windows-win32`/`browser-windows-nwg` had they built this
feature) can do live "press keys…" capture via a raw keydown event; `windows-reactor` and
`browser-macos-appkit` fell back to typed-text parsing (`"Ctrl+Shift+P"`) because neither toolkit's
declarative/high-level shortcut API exposes a generic "capture the next keypress" hook. So `parse_chord`
itself genuinely isn't shareable across every capture style — but everything *after* obtaining a `KeyChord`
(add/remove/persist/refresh) is 100% identical logic, currently copy-pasted across all four remaining
frontends regardless of capture method.

### 3.6 Profile picker

`list_profile_names()` → mark the current one → click launches a new process or closes the picker (if
already current). Same in `browser-linux-gtk3`, `browser-windows-winui`, `browser-windows-reactor`,
`browser-macos-appkit` — i.e. universal across every remaining frontend. It was absent in the now-deleted
`browser-windows-win32`/`browser-windows-nwg`/`browser-wx` (they only supported one profile via `--profile`,
no in-app switcher UI) — a real scope gap at the time, not a bug, and moot now that those crates are gone.

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
- `browser-macos-appkit` — checked directly this pass — **never calls `history.record_visit` anywhere**.
  It reads history (for switcher suggestions) but never writes to it. Browsing history silently never
  accumulates on this platform; the switcher's history-suggestion rows will always be empty. This is a real,
  currently-shipped bug from this session's work, not a hypothetical — flagged here rather than silently
  patched, since fixing it wasn't this pass's scope; see §5 for the concrete one-line fix.

This is the single most concrete piece of evidence for why this document exists: there is no single
"a page finished navigating" hook that *forces* every frontend to remember to record history. Each one has
to independently notice it needs to, on whatever event happens to be that platform's navigation-succeeded
signal — and one of them didn't.

### 3.9 A related, undocumented reference-cycle bug (found while checking §3.8)

`browser-macos-appkit`'s `add_page`/`ensure_engine_loaded` capture `Rc::clone(self)` (a strong reference to
the whole `AppState`) inside the per-page title-changed closure, which is stored inside that page's
`wry::WebView`, which lives inside `PageManager`, which lives inside `AppState.core`. That's a genuine
reference cycle: `AppState → core → PageManager → Page.engine → wry::WebView → closure → Rc<AppState>`.
`AppState` will never deallocate for the life of the process (harmless in practice here — the process exits
and the OS reclaims everything regardless of Rust-level `Drop` — but a real leak nonetheless, and exactly
the kind of subtle mistake hand-copying this pattern invites).

Two other frontends already avoided this, in two different ways, worth normalizing:
- `browser-linux-gtk3` uses `Rc::downgrade(self)`/`.upgrade()` in the same callback
  (`browser-linux-gtk3/src/lib.rs:153`).
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
- **`SwitcherModel<E>`**: given `&PageManager<E>`, `&HistoryStore`, and a query string, returns
  `Vec<SwitcherRow>` (already almost exactly `browser-windows-reactor`'s `Tile`/`browser-macos-appkit`'s
  `SwitcherRow` — this is mostly a *move*, not a rewrite, plus restoring the dropped `color` field from
  §3.2). Each frontend's job shrinks to "render this `Vec<SwitcherRow>` as native widgets" + "translate a
  click on index N into `model.activate(idx)`."
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

## 5. Quick wins (don't require the full refactor, worth doing regardless)

Found during this pass, not yet fixed (documentation was this turn's scope) — flagging precisely rather
than silently patching, since these are real, cheap, independent fixes:

1. **`browser-macos-appkit`: wire up `history.record_visit`** (§3.8) — call it from the title-changed
   callback (matching `browser-linux-gtk3`'s approach) or a navigation-completed signal if one becomes
   available. One missing call, currently a silent, total feature gap on this platform.
2. **`browser-macos-appkit`: break the `Rc` cycle** (§3.9) — change the title-changed callback's captured
   `Rc::clone(self)` to `Rc::downgrade(self)` + `.upgrade()` inside the closure, matching
   `browser-linux-gtk3`'s existing pattern.
3. **Restore or explicitly scope-cut the Ctrl+Enter "force new page" escape hatch** (§3.3) in
   `browser-windows-reactor`/`browser-macos-appkit` — currently silently missing rather than a documented
   decision.

## 6. Testability

`browser-core`'s `MockEngine`-based test pattern (86 tests, zero native toolkit, `crates/browser-core/src/
lib.rs`'s `#[cfg(test)] mod tests`) is the thing to extend, not replace. Once the controllers/models in §4
exist, they're testable the exact same way — meaning the switcher/settings/keybindings/profile-picker
decision logic across all four frontends, which currently has **zero automated test coverage** (only
exercised by manual clicking), gets real unit tests essentially for free.

This is complementary to, not a replacement for, `browser-linux-gtk3`'s existing `tests/gtk_tests.rs`
(using `gtk-test`, replacing what used to be manually-run `examples/nav_test.rs`/`switcher_test.rs`
binaries — see `README.md`'s Testing section): that suite verifies the *real native rendering* end-to-end
for one platform, driving actual live GTK widgets with synthetic input; the controller/model extraction
here verifies the *decision logic* with no toolkit at all, for every platform at once. Both are worth
having; neither substitutes for the other.

## 7. Suggested rollout order

Staged so each step is independently buildable/testable before the next starts — not a big-bang rewrite:

1. **Quick wins** (§5) — cheap, already-understood, no design work needed.
2. **`SwitcherModel`** first — appears in all 4 remaining frontends, and two of them
   (`browser-windows-reactor`, `browser-macos-appkit`) already have it in nearly extractable shape. Migrate
   one frontend at a time, confirming existing behavior/tests still hold after each.
3. **`SettingsController` + `KeybindingsController`** — same treatment.
4. **`PageController`'s decision logic** — trickiest, since container strategy genuinely differs by platform
   (§3.7); only the *decision* half (what should happen) extracts, containers stay native/local.
