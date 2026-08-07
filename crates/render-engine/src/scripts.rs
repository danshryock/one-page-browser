//! JS injected into every page by more than one platform's engine — kept
//! here as the single source of truth instead of duplicated (as they used
//! to be, byte-for-byte, confirmed via `diff` before this module existed)
//! across `linux.rs`/`macos.rs`/`browser-windows-reactor`. Ungated (no
//! `#[cfg(target_os = ...)]`), unlike those platform modules themselves,
//! since the whole point is one copy usable from any of them — including
//! `browser-windows-reactor`, which is a separate crate but already depends
//! on this one for the `RenderEngine` trait.

/// Overrides `console.log` to relay every call through the platform's own
/// IPC channel (`window.chrome.webview.postMessage` on WebView2,
/// `window.ipc.postMessage` on `wry`) in addition to its normal behavior —
/// see `WryEngine::new`'s `on_console_message` doc comment for why. Checks
/// for `window.chrome.webview` *and* `window.ipc` so the exact same script
/// works unmodified on every backend.
///
/// Also reports where a fixture's `data-test-target="<name>"` elements are
/// on screen, via the *same* relayed `console.log` — `__test_target__ <name>
/// <rect-json>` — once the page finishes loading. This exists for
/// `web-standards-tests/`'s external drivers (`windows_driver.rs`/
/// `macos_driver.rs`): a separate OS process driving the app via
/// `SendInput`/`CGEvent` has no way to run arbitrary script against the
/// page and get a structured answer back, but it already reads this same
/// console-log relay off the app's stdout for the test's real assertion —
/// so reusing it for "where do I click" too avoids inventing a second,
/// bidirectional channel. A fixture's `actions.json` names which target to
/// click; nothing here needs to know about `actions.json` itself, just
/// which elements are marked as targets.
pub const CONSOLE_CAPTURE_SCRIPT: &str = r#"
(function () {
  var send = window.chrome && window.chrome.webview
    ? function (m) { window.chrome.webview.postMessage(m); }
    : window.ipc ? function (m) { window.ipc.postMessage(m); } : null;
  if (!send) return;
  var original = console.log;
  console.log = function () {
    original.apply(console, arguments);
    try { send(Array.prototype.map.call(arguments, String).join(' ')); } catch (e) {}
  };
  window.addEventListener('load', function () {
    var targets = document.querySelectorAll('[data-test-target]');
    for (var i = 0; i < targets.length; i++) {
      var el = targets[i];
      var r = el.getBoundingClientRect();
      console.log('__test_target__ ' + el.getAttribute('data-test-target') + ' ' + JSON.stringify({ x: r.x, y: r.y, width: r.width, height: r.height }));
    }
  });
})();
"#;

/// Finds the password field (preferring `autocomplete="current-password"`,
/// falling back to the page's first `input[type="password"]`) and, if a
/// non-empty username was given, the identifier field (preferring
/// `autocomplete="username"`/`"email"` within the same scope, falling back
/// to whichever text-like input most immediately precedes the password
/// field in DOM order — see each platform's `fill_login`'s own doc comment).
/// Sets values via the native property setter rather than plain
/// `el.value = ...` — the latter doesn't trigger React/Vue-style
/// controlled-input state, a well-known gotcha — then dispatches real
/// `input`/`change` events so any such framework actually observes the
/// change.
///
/// `"__USERNAME__"`/`"__PASSWORD__"` are placeholders — callers replace
/// those exact quoted tokens with a JSON-encoded (`serde_json::to_string`
/// on a plain string produces a valid, properly-escaped JS string literal —
/// JSON string syntax is a subset of JS's) real value before evaluating
/// this, rather than interpolating raw text — the correct way to safely
/// embed untrusted content in injected JS.
pub const FILL_LOGIN_SCRIPT: &str = r#"
(function () {
  var password = document.querySelector('input[autocomplete="current-password"]') ||
                  document.querySelector('input[type="password"]');
  if (!password) return;

  function setNativeValue(el, value) {
    var proto = Object.getPrototypeOf(el);
    var setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }

  var username = "__USERNAME__";
  if (username.length > 0) {
    var form = password.closest('form');
    var scope = form || document;
    var usernameField = scope.querySelector('input[autocomplete="username"], input[autocomplete="email"]');
    if (!usernameField) {
      var candidates = scope.querySelectorAll('input');
      for (var i = 0; i < candidates.length; i++) {
        var el = candidates[i];
        if (el === password) break;
        var type = (el.getAttribute('type') || 'text').toLowerCase();
        if (type === 'text' || type === 'email' || type === 'tel') {
          usernameField = el;
        }
      }
    }
    if (usernameField) setNativeValue(usernameField, username);
  }

  setNativeValue(password, "__PASSWORD__");
})();
"#;
