use gtk::glib::object::Cast as _;
use gtk::Container;
use webkit2gtk::{URIRequestExt as _, WebViewExt as _};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix};

use crate::{RenderEngine, CONSOLE_CAPTURE_SCRIPT, FILL_LOGIN_SCRIPT};

/// A new-window request's real details, extracted from WebKitGTK's own
/// `NavigationAction` — see `WryEngine::new`'s `on_new_window_requested` doc
/// comment for why this needs to bypass `wry`'s `with_new_window_req_handler`
/// entirely (it only ever exposes the bare URL).
pub struct NewWindowInfo {
    pub uri: String,
    /// Whether a real user gesture (a click, not a script calling
    /// `window.open()` unprompted) triggered this request — WebKitGTK's own
    /// `webkit_navigation_action_is_user_gesture`, a real signal `wry`'s
    /// abstraction discards.
    pub is_user_gesture: bool,
}

pub struct WryEngine {
    webview: WebView,
}

impl WryEngine {
    /// `web_context` is shared across every page in the same profile (see
    /// `wry::WebContext`'s own doc comment: "A browser would have a context
    /// for all the normal tabs and a different context for all the
    /// private/incognito tabs") — the caller owns it for the lifetime of the
    /// whole profile, not per-page, so cookies/localStorage/cache persist
    /// (and are shared between tabs) instead of each page silently getting
    /// its own throwaway context (`WebViewBuilder::new()`'s default when no
    /// context is supplied at all).
    /// `on_new_window_requested` fires for a `target="_blank"` link click, a
    /// `window.open()` call, or the engine's own default right-click "Open
    /// Link in New Window" context-menu item — all three route through the
    /// same WebKitGTK `create` signal. Deliberately bypasses `wry`'s own
    /// `with_new_window_req_handler` (it only ever exposes the bare URL) and
    /// connects straight to the underlying signal instead, via the same
    /// Unix escape hatch `is-playing-audio`/`screenshot` already use — this
    /// is what makes the real `NewWindowInfo::is_user_gesture` signal
    /// available at all (see its doc comment). The handler gets a clone of
    /// this page's own raw `webkit2gtk::WebView` (needed to build a
    /// *related* popup via `new_related`, preserving `window.opener`/
    /// `postMessage`) and returns the constructed popup's `gtk::Widget` to
    /// satisfy the signal's contract, or `None` to suppress the request
    /// outright (no popup, no relationship — used for non-user-gesture
    /// requests, which get no other treatment: unlike the old
    /// deny-and-open-unrelated behavior, there's no orphan tab anymore).
    /// `on_console_message` fires for every `console.log(...)` call any page
    /// in this webview makes (via `CONSOLE_CAPTURE_SCRIPT`, injected into
    /// every page through `wry`'s `with_initialization_script`/
    /// `with_ipc_handler`) — a real, generic capability, not specific to any
    /// one test: production call sites can pass a no-op, or wire it to
    /// `eprintln!` for incidental debugging value; automated test call sites
    /// (see `web-standards-tests/`) use it to read a fixture page's own
    /// plain, standard `console.log` output as that test's result, instead
    /// of each test needing its own bespoke host-side check-and-report code.
    pub fn new<W: gtk::glib::IsA<Container>>(
        container: &W,
        initial_url: &str,
        web_context: &mut WebContext,
        on_title_changed: impl Fn(String) + 'static,
        on_audio_playing_changed: impl Fn(bool) + 'static,
        on_new_window_requested: impl Fn(NewWindowInfo, webkit2gtk::WebView) -> Option<gtk::Widget> + 'static,
        on_console_message: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let builder = WebViewBuilder::new_with_web_context(web_context).with_url(initial_url);
        Self::build(builder, container, on_title_changed, on_audio_playing_changed, on_new_window_requested, on_console_message)
    }

    /// Same as `new`, but for a page opened via `window.open()`/
    /// `target="_blank"`/"open in new tab" that should preserve a real
    /// opener relationship (`window.opener`, `postMessage`, the opener's own
    /// `window.open()` return value) instead of being a disconnected page —
    /// see `new`'s `on_new_window_requested` doc comment for where this gets
    /// called from. `related_to` should be the opener page's own raw
    /// `webkit2gtk::WebView` (the second parameter `on_new_window_requested`
    /// receives) — `WebViewBuilderExtUnix::with_related_view` shares the
    /// same web process with it, which is what makes WebKitGTK itself
    /// establish and honor the real browsing-context relationship. No
    /// `initial_url`/`.with_url(...)` call here: once the `create` signal
    /// handler returns this page's widget, WebKitGTK performs the actual
    /// requested navigation into it itself — setting a URL here would race
    /// or conflict with that.
    pub fn new_related<W: gtk::glib::IsA<Container>>(
        container: &W,
        related_to: &webkit2gtk::WebView,
        web_context: &mut WebContext,
        on_title_changed: impl Fn(String) + 'static,
        on_audio_playing_changed: impl Fn(bool) + 'static,
        on_new_window_requested: impl Fn(NewWindowInfo, webkit2gtk::WebView) -> Option<gtk::Widget> + 'static,
        on_console_message: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let builder = WebViewBuilder::new_with_web_context(web_context).with_related_view(related_to.clone());
        Self::build(builder, container, on_title_changed, on_audio_playing_changed, on_new_window_requested, on_console_message)
    }

    /// Shared tail of `new`/`new_related` — everything after the builder's
    /// initial `.with_url(...)`/`.with_related_view(...)` divergence.
    fn build<W: gtk::glib::IsA<Container>>(
        builder: WebViewBuilder<'_>,
        container: &W,
        on_title_changed: impl Fn(String) + 'static,
        on_audio_playing_changed: impl Fn(bool) + 'static,
        on_new_window_requested: impl Fn(NewWindowInfo, webkit2gtk::WebView) -> Option<gtk::Widget> + 'static,
        on_console_message: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let webview = builder
            .with_initialization_script(CONSOLE_CAPTURE_SCRIPT)
            .with_ipc_handler(move |request| on_console_message(request.body().clone()))
            .with_document_title_changed_handler(move |title| on_title_changed(title))
            .build_gtk(container)?;

        // `is-playing-audio` isn't a wry builder hook — connect straight to
        // the underlying webkit2gtk object via the same Unix escape hatch
        // `screenshot()` below already uses.
        webview.webview().connect_is_playing_audio_notify(move |wv| {
            on_audio_playing_changed(wv.is_playing_audio());
        });

        webview.webview().connect_create(move |opener, action| {
            let uri = action.request().and_then(|request| request.uri()).map(|uri| uri.to_string()).unwrap_or_default();
            let info = NewWindowInfo { uri, is_user_gesture: action.is_user_gesture() };
            on_new_window_requested(info, opener.clone())
        });

        // WebKitGTK's fire-and-forget `evaluate_script` (no callback, used by
        // go_back/go_forward) is unreliable on a freshly-built webview — it
        // can silently fail to execute unless preceded by a callback-based
        // evaluation, and that priming call itself only works reliably once
        // at least one GTK main-loop iteration has run after `build_gtk`
        // (draining whatever realize/map events that call queued). Skipping
        // either half of this reproduced the bug ~50-100% of the time in
        // testing; doing both fixed it 10/10. Root cause is unclear — this
        // is a cheap, empirically-verified workaround, not a full fix upstream.
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        webview.evaluate_script_with_callback("true", |_| {})?;

        Ok(Self { webview })
    }

    /// The webview's own raw GTK widget — lets a caller return the result of
    /// `new_related` from WebKitGTK's `create` signal (see `new`'s
    /// `on_new_window_requested` doc comment) without needing to know
    /// anything about `webkit2gtk` beyond the `NewWindowInfo`/opener-handle
    /// types this module already re-exports.
    pub fn widget(&self) -> gtk::Widget {
        self.webview.webview().clone().upcast()
    }

    /// Toggles reader mode: strips chrome/nav/ads and re-renders the page's
    /// main content with clean, readable typography, via a hand-rolled
    /// extraction heuristic — favors `<article>`/`<main>`/`[role=main]` if
    /// present, otherwise the highest-scoring `<div>`/`<section>` by
    /// paragraph count and text length. This is **not** a vendored copy of
    /// Mozilla's Readability.js (not available to bundle here — no network
    /// access to fetch it, and no existing dependency provides it); it's a
    /// simpler, self-contained approximation, less robust on pages that
    /// don't fit the "one clear content block" shape. Calling this again
    /// while already active restores the original page — the script stashes
    /// the pre-reader-mode HTML on `window` before replacing it, purely a
    /// per-page-load DOM mutation with nothing tracked on the Rust side.
    ///
    /// Not part of the `RenderEngine` trait: this is pure JS injection
    /// specific to what a webview-backed engine can do, not something every
    /// possible engine (a hypothetical non-web renderer) would meaningfully
    /// implement the same way navigate/reload/screenshot do.
    pub fn toggle_reader_mode(&self) -> anyhow::Result<()> {
        self.webview.evaluate_script(READER_MODE_SCRIPT)?;
        Ok(())
    }

    /// Fills the page's login form with `username`/`password`, via a real
    /// per-page-load DOM mutation with nothing tracked on the Rust side —
    /// same "hand-rolled heuristic, not perfect" spirit as
    /// `toggle_reader_mode`'s Readability.js approximation. Field detection
    /// prefers the standard `autocomplete` attribute (`"current-password"`
    /// for the password field, `"username"`/`"email"` for the identifier
    /// field) when a page marks its fields that way — the explicit, reliable
    /// signal sites are supposed to provide — falling back to a positional
    /// heuristic (the page's first `input[type="password"]`, and whichever
    /// text/email/tel input immediately precedes it in DOM order, within the
    /// same `<form>` if there is one) only when `autocomplete` isn't present.
    /// Doesn't handle multi-step (username-page-then-password-page) login
    /// flows, or pages with more than one login form.
    ///
    /// `username`/`password` are JSON-encoded (`serde_json::to_string` on a
    /// plain string produces a valid, properly-escaped JS string literal —
    /// JSON string syntax is a subset of JS's) before being spliced into the
    /// script, rather than interpolated as raw text — the correct way to
    /// safely embed untrusted content in injected JS. Not part of the
    /// `RenderEngine` trait, same reasoning as `toggle_reader_mode`.
    pub fn fill_login(&self, username: &str, password: &str) -> anyhow::Result<()> {
        let username_json = serde_json::to_string(username)?;
        let password_json = serde_json::to_string(password)?;
        let script = FILL_LOGIN_SCRIPT
            .replace("\"__USERNAME__\"", &username_json)
            .replace("\"__PASSWORD__\"", &password_json);
        self.webview.evaluate_script(&script)?;
        Ok(())
    }

    /// Evaluates `script`, delivering its JSON-serialized result to
    /// `callback` — test/inspection hook only (nothing in real app code
    /// needs a script's return value: `go_back`/`go_forward`/
    /// `toggle_reader_mode`/`fill_login` are all fire-and-forget). Exists
    /// because `evaluate_script_with_callback` itself is otherwise only
    /// used internally, in `new`'s init workaround.
    pub fn evaluate_script_for_test(&self, script: &str, callback: impl Fn(String) + Send + 'static) -> anyhow::Result<()> {
        self.webview.evaluate_script_with_callback(script, callback)?;
        Ok(())
    }
}

const READER_MODE_SCRIPT: &str = r#"
(function () {
  if (window.__claudeBrowserReaderModeActive) {
    document.documentElement.innerHTML = window.__claudeBrowserOriginalHTML;
    document.title = window.__claudeBrowserOriginalTitle;
    window.__claudeBrowserReaderModeActive = false;
    return;
  }
  window.__claudeBrowserOriginalHTML = document.documentElement.innerHTML;
  window.__claudeBrowserOriginalTitle = document.title;

  function textLength(el) {
    return (el.innerText || "").length;
  }

  var best = document.querySelector("article") || document.querySelector("main") || document.querySelector('[role="main"]');
  if (!best) {
    var candidates = document.body.querySelectorAll("div, section");
    var bestScore = 0;
    for (var i = 0; i < candidates.length; i++) {
      var el = candidates[i];
      var pCount = el.querySelectorAll("p").length;
      if (pCount < 2) continue;
      var score = textLength(el) + pCount * 100;
      if (score > bestScore) {
        bestScore = score;
        best = el;
      }
    }
  }
  if (!best) best = document.body;

  var title = document.title || "";
  var escTitle = title.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  var contentHtml = best.innerHTML;

  document.documentElement.innerHTML =
    "<head><meta charset=\"utf-8\"><title>" + escTitle + "</title>" +
    "<style>" +
      "body{max-width:700px;margin:40px auto;padding:0 24px 80px;font-family:Georgia,'Times New Roman',serif;" +
      "font-size:19px;line-height:1.65;color:#222;background:#fdfdfb;}" +
      "img,video{max-width:100%;height:auto;}" +
      "h1{font-size:30px;line-height:1.25;margin-bottom:8px;}" +
      "pre,code{white-space:pre-wrap;}" +
      "a{color:#2563eb;}" +
    "</style></head>" +
    "<body><h1>" + escTitle + "</h1>" + contentHtml + "</body>";

  // Set explicitly rather than relying solely on the embedded <title> tag
  // above being picked up from such a drastic single innerHTML replacement
  // — this is also what makes reader mode visually obvious in the window
  // title/task list, not just a testing convenience.
  document.title = "Reader: " + title;
  window.__claudeBrowserReaderModeActive = true;
})();
"#;

impl RenderEngine for WryEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.webview.load_url(url)?;
        Ok(())
    }

    fn current_url(&self) -> anyhow::Result<String> {
        Ok(self.webview.url()?)
    }

    fn go_back(&self) -> anyhow::Result<()> {
        self.webview.evaluate_script("window.history.back()")?;
        Ok(())
    }

    fn go_forward(&self) -> anyhow::Result<()> {
        self.webview.evaluate_script("window.history.forward()")?;
        Ok(())
    }

    fn reload(&self) -> anyhow::Result<()> {
        self.webview.reload()?;
        Ok(())
    }

    fn screenshot(&self, callback: Box<dyn Fn(anyhow::Result<Vec<u8>>)>) {
        self.webview.webview().snapshot(
            webkit2gtk::SnapshotRegion::FullDocument,
            webkit2gtk::SnapshotOptions::NONE,
            gtk::gio::Cancellable::NONE,
            move |result| {
                let outcome = result
                    .map_err(|err| anyhow::anyhow!("snapshot failed: {err}"))
                    .and_then(|surface| {
                        let image_surface: gtk::cairo::ImageSurface =
                            surface.try_into().map_err(|_| anyhow::anyhow!("snapshot surface wasn't an image surface"))?;
                        let mut bytes = Vec::new();
                        image_surface.write_to_png(&mut bytes).map_err(|err| anyhow::anyhow!("failed to encode PNG: {err}"))?;
                        Ok(bytes)
                    });
                callback(outcome);
            },
        );
    }
}
