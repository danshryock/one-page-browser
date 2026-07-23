# Color themes + overlay backgrounds

**Roadmap items:** "Color themes (light/dark, possibly custom) across the native chrome in each frontend" and
"Overlay backgrounds." Done together: a real light/dark theme system necessarily covers the overlay
backgrounds too, since that's the one place in the current UI with an actual themeable background surface
(see the design note below) — building them separately would have meant redoing the same CSS split twice.

## Design: what actually needed to vary by theme

Before touching any CSS, I checked which rendered surfaces have a real background of their own versus which
just float over the switcher/settings/etc. overlays' **scrim** (`.switcher-scrim`, a fixed dark
`rgba(20,20,18,0.55)` dimmer over the page behind, shared by every overlay). Only the scrim's own children —
the settings/profile/keybindings/bookmarks boxes (`.settings-box`) and the switcher grid's history/bookmark
search-result tiles — have a background whose *theme* actually matters for text contrast. Everything else
(the switcher grid's page tiles, the "+" add-tile, hint text) sits directly over the scrim, which stays the
same dark tone in both themes — the same convention most apps' modal dimmers use regardless of system theme —
so none of that needed to change. This kept the scope honest: not a full app-wide light/dark reskin, but the
specific surfaces this app actually has that need one.

**"Custom" themes** (mentioned in the roadmap item) were not implemented — only a Light/Dark choice. Building
an arbitrary user-defined color/CSS editor is a meaningfully larger feature on its own; flagging the omission
here rather than silently dropping it.

## What changed

- **`crates/browser-core/src/settings.rs`**: new `Theme` enum (`Light`/`Dark`), `Settings` gained a `theme`
  field, defaulting to `Theme::Dark` (preserves this app's existing look — every overlay/tile style was
  written assuming a dark background — for anyone upgrading from before themes existed).
- **`crates/browser-linux-gtk3/src/lib.rs`**:
  - Split the single, monolithic CSS string that used to be loaded once at startup into two `CssProvider`s: a
    `base_provider` for the theme-invariant rules (unchanged from before, loaded once, never reloaded), and a
    new `theme_provider` holding only the theme-*dependent* rules (`.settings-box`/`.settings-title`/
    `.history-tile`/`.bookmark-tile`/the settings-box label and flat-button text colors).
  - New `theme_css(Theme) -> &'static str` function producing each theme's version of those rules.
  - New `AppState::apply_theme()` reloads `theme_provider` from the current `Settings::theme` — called once
    right after `AppState` is constructed (so the correct theme is live before the first page even opens),
    and again at the end of `save_settings` (so a theme change takes effect immediately, no restart needed).
  - Settings overlay gained a "Theme" row with Dark/Light radio buttons, populated in `open_settings` and read
    back in `save_settings`.

## Testing

- `browser-core`: `round_trips_through_disk` extended to also cover `Theme::Light` round-tripping through
  JSON. `cargo test -p browser-core`: 62/62 passing.
- `crates/browser-linux-gtk3/tests/gtk_tests.rs`: new `switching_to_light_theme_reloads_the_theme_css`, using
  two new test helpers (`select_light_theme_radio`, and `theme_provider_css()` which reads back the actual
  loaded CSS text via `CssProvider::to_str()`) — confirms the provider starts with dark-theme CSS by default,
  and that switching to Light and saving reloads it with light-theme CSS, with the old dark-theme CSS gone.
  This checks the real provider content, not just that the `Settings` struct field changed.
  - **A real discovery while writing this test**: my first version asserted `theme_provider_css().contains("#2e2e2c")`
    (the literal hex color from the source CSS) and it failed immediately. `CssProvider::to_str()` doesn't
    echo back the input text — it returns GTK's own re-serialization of the *parsed* stylesheet, which
    canonicalizes hex colors to `rgb(46,46,44)` form and expands shorthand properties like `border-radius`
    into their four longhand equivalents. Confirmed by actually printing the string rather than guessing;
    fixed by asserting on `rgb(46,46,44)`/`rgb(242,242,240)` instead.
- `cargo clippy --all-targets` on `browser-linux-gtk3`/`browser-core`: clean (only the same pre-existing
  `field_reassign_with_default` note on `settings.rs`'s test helper, now also mentioning the new `theme`
  field in its suggested fix — not a new warning category).
- `cargo build` (workspace), `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx`,
  `cargo build-windows-winui`: all succeed unchanged — no other `RenderEngine`/frontend code touches
  `Settings::theme`, so nothing else needed changes.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing.

## Scope notes

`browser-core` + `browser-linux-gtk3` only, as with every other item this session — `browser-windows-winui`'s
own settings/theming is untouched and left as a follow-up.
