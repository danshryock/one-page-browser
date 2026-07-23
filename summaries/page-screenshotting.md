# Page screenshotting

**Roadmap item:** "Page screenshotting."

## Design

Every platform's real screenshot capability is async/callback-based (WebKitGTK's `webkit_web_view_get_snapshot`,
WebView2's `CapturePreview`) rather than a synchronous call, so `RenderEngine` gained a new trait method that
stays async at that level too, rather than forcing every implementation to block internally:

```rust
fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>);
```

`Ok(bytes)` is PNG-encoded image data, ready to write straight to a file.

## What changed

- **`crates/render-engine`**:
  - `RenderEngine` trait gained `screenshot`.
  - `linux.rs` (`WryEngine`, used by `browser-linux-gtk3`) implements it for real: `wry::WebViewExtUnix::webview()`
    hands back the raw `webkit2gtk::WebView`, whose `.snapshot(SnapshotRegion::FullDocument, SnapshotOptions::NONE,
    ...)` captures the whole page (not just the visible viewport) into a `cairo::Surface`, converted via
    `TryFrom` to a `cairo::ImageSurface` and PNG-encoded via `write_to_png` (cairo-rs's `png` feature, not
    enabled by default — added as a direct dependency specifically for this, which unifies with the same
    cairo-rs already pulled in transitively via `gtk`/`wry`, so nothing new actually gets built beyond the
    feature itself).
  - `windows.rs` (win32/nwg's `WryEngine`) and `winui.rs` (winui3's `WebView2Engine`) both got a **stub**
    implementation (`callback(Err(...))`, "not yet implemented on this platform") — needed just to keep them
    compiling against the new trait method, not a feature implementation. `browser-wx`'s own separate `WxEngine`
    (it doesn't use `render-engine`'s per-platform modules at all — see its own doc comment) needed the same
    stub for the same reason, plus `browser-core`'s test-only `MockEngine` needed a trivial `Ok(Vec::new())`
    implementation to keep the existing `PageManager` unit tests compiling.
- **`crates/browser-core`**: new `Profile::screenshots_dir()` (mirrors `history_db_path()`'s existing shape) —
  where a profile's screenshots are suggested to save by default. Deliberately per-profile rather than the
  user's shared system Pictures folder, for the same "a profile keeps its own things to itself" reasoning as
  every other piece of profile data; it's just a starting suggestion for the save dialog below, not a forced
  location, and isn't created on disk until a screenshot is actually about to be saved there.
- **`crates/browser-linux-gtk3`**: a new toolbar camera-icon button ("Save screenshot"). Split into two methods,
  since the save dialog can't run inside an automated test:
  - `take_screenshot()`: shows a native `GtkFileChooserNative` "Save Screenshot" dialog (suggested filename
    `<domain>-<unix-timestamp>.png`, starting folder `Profile::screenshots_dir()`, created on demand), and on
    confirm, hands the chosen path to —
  - `save_screenshot_to(path)`: the actual capture-and-write logic, calling `RenderEngine::screenshot` on the
    active page's engine and writing the PNG bytes to `path` in the callback. Errors are logged rather than
    propagated, matching this codebase's established fire-and-forget UI-action style.

## Testing

- `browser-core`: new `screenshots_dir_is_scoped_under_the_profile_name` test. `cargo test -p browser-core`:
  62/62 passing.
- `crates/browser-linux-gtk3/tests/gtk_tests.rs`: new `screenshot_saves_a_real_png_file` — calls
  `save_screenshot_to` directly (not `take_screenshot`, which blocks on a real dialog with nothing to drive it
  in a test), waits for the file to appear, and confirms the written bytes actually start with the PNG magic
  number (`\x89PNG`) — this is a real end-to-end check of the whole
  WryEngine → webkit2gtk snapshot → cairo → PNG pipeline, not just a "did it not crash" smoke test, and it
  **passed on the first real run**, confirming the capture genuinely works even under this headless
  `cage`/`xwayland-run` setup (unlike the earlier `gdk_window.pixbuf()`-based screenshot attempt from the
  "fix overlay colors" task, which came back blank in this same environment — `webkit_web_view_get_snapshot`
  clearly works through a different rendering path that doesn't depend on the window actually being composited
  on screen).
- `cargo clippy --all-targets` on `render-engine`/`browser-linux-gtk3`/`browser-core`/`browser-windows-winui`/
  `browser-wx`: clean.
- `cargo build` (workspace), `cargo build --target x86_64-pc-windows-gnu --workspace --exclude browser-wx`,
  `cargo build-windows-winui`: all succeed with the new trait method's stub implementations in place.
- Full headless GTK suite via `wlheadless-run -c cage -- xwayland-run -- cargo test -p browser-linux-gtk3`:
  all passing.

## Scope notes

Real implementation is `browser-core` + `render-engine::linux` + `browser-linux-gtk3` only. Every other
`RenderEngine` implementer (win32/nwg's `WryEngine`, winui3's `WebView2Engine`, `browser-wx`'s `WxEngine`) got
the minimal stub needed to keep compiling — real screenshot support for winui3 would use WebView2's own
`CoreWebView2.CapturePreview`, noted in a comment at the stub site for whoever picks that up later.
