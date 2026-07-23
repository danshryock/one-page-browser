# Fix colors in settings/bookmarks overlays

**Roadmap item:** "fix colors in settings/bookmarks overlays."

## The actual bug

The `.settings-box` CSS class (shared by the settings, profile picker, keybindings editor, and bookmarks
overlays — they're all built from the same dark `#2e2e2c` box) only ever styled its own **title** label
(`.settings-title`, explicit white/bold). Every other `gtk::Label` placed directly inside one of these boxes
— the settings overlay's "Start page"/"Search engine"/"Loaded pages limit" row labels, the "Unlimited"
checkbox's own label, the bookmarks overlay's "No bookmarks yet" empty-state label, and each bookmark row's
title/domain label — never got any color of their own. They fell back to the system GTK theme's default
label foreground color, which on a dark background reads as very low-contrast or effectively invisible,
depending on the active theme. Buttons using the `.flat` style class (profile picker rows, each bookmark's
open button) have the same problem for their own label text, on top of Adwaita's flat-button hover state
potentially drawing an unwanted background gradient (the same issue already documented and fixed for
`.page-tile` earlier in this file).

## What changed

`crates/browser-linux-gtk3/src/lib.rs` — three new rules added to the existing global `CssProvider` block
(the same one that already styles `.tile-title`/`.switcher-hint`/etc.):

```css
.settings-box label:not(.settings-title) { color: rgba(255, 255, 255, 0.92); }
.settings-box button.flat, .settings-box button.flat:hover {
  background-image: none; background-color: transparent; }
.settings-box button.flat label { color: rgba(255, 255, 255, 0.92); }
```

These are descendant selectors scoped to `.settings-box` (which every one of the four overlays' content boxes
carries), so one rule covers all four overlays at once rather than needing four separate near-duplicate
rules. `:not(.settings-title)` keeps the title labels on their own existing, more specific color/weight rule
rather than being overridden by the new broader one (a plain class+type descendant selector like
`.settings-box label` would otherwise have *higher* specificity than the single-class `.settings-title`
selector, and could have won a color conflict between the two — excluding it sidesteps that entirely rather
than relying on rule ordering).

## Testing

- `cargo check --tests` / `cargo clippy --all-targets` on `browser-linux-gtk3`: clean (pure CSS string
  change, no Rust API surface touched).
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing (no behavior changed, just colors).
- **What I could not verify**: I attempted to actually screenshot the rendered overlays for a real visual
  check, using `gdk::Window::pixbuf()` to grab the live window's backing store during a temporary test (GDK's
  own screenshot mechanism — the same technique the "page screenshotting" backlog item would likely use).
  Every capture came back a blank white frame under this headless `cage`/`xwayland-run` setup, even after
  pumping the GTK main loop for 500ms first — I couldn't get a compositor-backed framebuffer to actually
  read from in this environment, and didn't want to sink further time into it. I removed the temporary test
  entirely rather than leave a non-functional screenshot attempt in the suite. The color fix itself is
  reasoned from the CSS rules and selector specificity directly (and mirrors the exact pattern this file
  already uses successfully for `.tile-title`/`.tile-subtitle` on the switcher grid's tiles, which do
  visibly render correctly per every prior session's UI work) rather than confirmed with a rendered
  screenshot — worth a quick look on a real desktop to be sure.

## Scope notes

CSS-only change in `browser-linux-gtk3`; no `browser-core` or other-frontend changes needed.
