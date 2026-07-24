//! WinUI 3 chrome, take two: built on Microsoft's own `windows-reactor`/
//! `windows-webview` (in-tree in `microsoft/windows-rs`, same
//! `windows-bindgen` WinMD codegen as the base `windows` crate) instead of
//! the community `winio-winui3` wrapper `browser-windows-winui` depends on.
//! See `summaries/windows-github-actions-ci.md`'s "windows-reactor
//! comparison test" section for why: a comparison app of this same general
//! shape (window + toolbar + `WebView2`) survived on a real Windows VM where
//! `browser-windows-winui` crashes with `STATUS_STOWED_EXCEPTION`.
//!
//! Being built out incrementally, one feature at a time, toward parity with
//! `browser-windows-winui`/`browser-linux-gtk3` (see ROADMAP.md) rather than
//! ported in one pass — `windows-reactor`'s declarative, React-like model
//! (a render function of state, re-diffed against the live tree) is a
//! genuinely different shape from `winio-winui3`'s imperative
//! widget-tree-with-handles style, so this isn't a mechanical port.
//!
//! This first version: a single page (no multi-tab/switcher/settings/
//! profile/keybindings yet — see ROADMAP.md's task list), proving the core
//! navigation loop (address bar, back/forward/reload, real `WebView2`
//! content) on the new stack.
#![cfg(all(target_os = "windows", target_env = "msvc"))]

use browser_core::{resolve_address_input, Settings, HOME_URL};
use windows_reactor::*;
use windows_webview::{webview, EventRegistration, WebView};

/// Same checkpoint-tracing pattern as `browser_windows_winui::trace` — cheap
/// and has already paid for itself once diagnosing a real crash on Windows.
pub fn trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("reactor-trace.log") {
        let _ = writeln!(f, "{msg}");
        let _ = f.sync_all();
    }
}

fn app(cx: &mut RenderCx) -> Element {
    trace("app: render start");
    let (address, set_address) = cx.use_state(String::from(HOME_URL));
    let web = cx.use_ref::<Option<WebView>>(None);
    let registration = cx.use_ref::<Option<EventRegistration>>(None);

    let on_ready = {
        let web = web.clone();
        let set_address = set_address.clone();
        move |ready: WebView| {
            trace("on_ready: WebView2 ready");
            let reflect = {
                let ready = ready.clone();
                let set_address = set_address.clone();
                move |_args| {
                    let source = ready.source();
                    if !source.is_empty() {
                        set_address.call(source);
                    }
                }
            };
            *registration.borrow_mut() = ready.on_navigation_completed(reflect).ok();
            let _ = ready.navigate(HOME_URL);
            *web.borrow_mut() = Some(ready);
        }
    };

    let navigate_from_address_bar = {
        let web = web.clone();
        let address = address.clone();
        move || {
            if let Some(web) = web.borrow().as_ref() {
                let url = resolve_address_input(&address, &Settings::default());
                let _ = web.navigate(&url);
            }
        }
    };

    let back = with_web(&web, WebView::go_back);
    let forward = with_web(&web, WebView::go_forward);
    let reload = with_web(&web, WebView::reload);

    let toolbar = grid((
        Element::from(button("\u{25c0}").on_click(back)).grid_column(0),
        Element::from(button("\u{25b6}").on_click(forward)).grid_column(1),
        Element::from(button("\u{27f3}").on_click(reload)).grid_column(2),
        Element::from(
            text_box(address)
                .on_text_changed(set_address)
                .keyboard_accelerator(KeyboardAccelerator::new(
                    VirtualKey::Enter,
                    VirtualKeyModifiers::None,
                    navigate_from_address_bar,
                )),
        )
        .grid_column(3),
    ))
    .columns([GridLength::Auto, GridLength::Auto, GridLength::Auto, GridLength::STAR])
    .column_spacing(8.0)
    .margin(Thickness::uniform(8.0));

    trace("app: render end");
    grid((
        Element::from(toolbar).grid_row(0),
        Element::from(webview(on_ready)).grid_row(1),
    ))
    .rows([GridLength::Auto, GridLength::STAR])
    .into()
}

fn with_web(web: &HookRef<Option<WebView>>, action: fn(&WebView) -> Result<()>) -> impl Fn() + 'static {
    let web = web.clone();
    move || {
        if let Some(web) = web.borrow().as_ref() {
            let _ = action(web);
        }
    }
}

/// Runs the app — called from `main.rs` after `bootstrap()`. Blocks until
/// the window closes (reactor's own message loop; see `App::render`'s doc
/// comment upstream).
pub fn run() -> Result<()> {
    App::new().title("claude-browser").render(app)
}
