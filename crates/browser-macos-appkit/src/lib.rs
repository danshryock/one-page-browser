//! Native AppKit chrome — a **minimal scaffold**, not yet at feature parity
//! with `browser-linux-gtk3` (no switcher grid, no settings/bookmarks/
//! keybindings/profile-picker overlays, no history/bookmarks integration
//! yet — those are still `browser-windows-winui`/`browser-linux-gtk3`-only,
//! see `ROADMAP.md`'s Backlog). What exists: a single native `NSWindow` with
//! a toolbar strip (back/forward/reload `NSButton`s + an address-bar
//! `NSTextField`) and a `WKWebView` (via `render_engine::macos::WryEngine`)
//! filling the rest, embedded as a real AppKit view hierarchy (no `tao`/
//! `winit`), following the same "native widget wraps native webview child"
//! pattern as every other front end here.
//!
//! # This has never been compiled — real, but unverified, code
//!
//! Written entirely on a Linux dev machine with no macOS toolchain
//! available (this repo cross-compiles for Windows here, but nothing
//! provides a way to cross-compile *or* run macOS code from Linux — see
//! `summaries/windows-github-actions-ci.md`'s "why not local VMs" section:
//! a local macOS VM would violate Apple's EULA on this non-Apple hardware).
//! Every `objc2`/`objc2-app-kit` API used below was checked against that
//! crate's actual generated source (vendored under
//! `~/.cargo/registry/src/.../objc2-app-kit-0.3.2`) rather than guessed, but
//! that's verification of *signatures*, not of this file actually
//! compiling and running end-to-end. Treat this the same way
//! `browser-windows-winui` was treated for months (see `ROADMAP.md`): a
//! real implementation that needs a real debugging pass against real
//! hardware (or a `macos-latest` GitHub Actions runner) before it's
//! trustworthy, not something to build on further unverified.
//!
//! Known gaps, to be honest about up front:
//! - The address bar doesn't update when navigating via in-page links —
//!   `RenderEngine` only offers a document-title-changed callback, not a
//!   URL-changed one, and wiring that up is future work.
//! - No `Cmd+W`/`Cmd+Q` menu bar or keyboard shortcuts beyond what AppKit
//!   provides for free — no `NSMenu` has been built at all.
//! - Only one page at a time — no multi-page switcher grid (`PageManager`
//!   isn't used here at all yet).
#![cfg(target_os = "macos")]

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSTextField, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use browser_core::{resolve_address_input, Profile, Settings};
use render_engine::{RenderEngine, WryEngine};

const TOOLBAR_HEIGHT: f64 = 36.0;
const BUTTON_WIDTH: f64 = 32.0;
const BUTTON_MARGIN: f64 = 4.0;

struct AppState {
    window: Retained<NSWindow>,
    toolbar_view: Retained<NSView>,
    address_bar: Retained<NSTextField>,
    engine: WryEngine,
    settings: Settings,
}

impl AppState {
    /// Recomputes the toolbar strip's and webview's frames from the
    /// window's current content size — AppKit has no layout manager doing
    /// this automatically for views added without an autoresizing mask, the
    /// same situation `render_engine::windows::WryEngine::set_bounds`'s doc
    /// comment describes for Win32.
    fn relayout(&self) {
        let content_size = self.window.contentView().map(|view| view.frame().size).unwrap_or(NSSize::new(0.0, 0.0));
        self.toolbar_view.setFrame(NSRect::new(NSPoint::new(0.0, content_size.height - TOOLBAR_HEIGHT), NSSize::new(content_size.width, TOOLBAR_HEIGHT)));

        let address_bar_x = 3.0 * BUTTON_WIDTH + 4.0 * BUTTON_MARGIN;
        self.address_bar.setFrame(NSRect::new(
            NSPoint::new(address_bar_x, BUTTON_MARGIN),
            NSSize::new((content_size.width - address_bar_x - BUTTON_MARGIN).max(0.0), TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN),
        ));

        let content_height_below_toolbar = (content_size.height - TOOLBAR_HEIGHT).max(0.0);
        if let Err(err) = self.engine.set_bounds(0, 0, content_size.width.max(0.0) as u32, content_height_below_toolbar as u32) {
            eprintln!("failed to resize webview: {err}");
        }
    }

    fn navigate_to_address_bar_text(&self) {
        let input = self.address_bar.stringValue().to_string();
        let resolved = resolve_address_input(&input, &self.settings);
        if let Err(err) = self.engine.navigate(&resolved) {
            eprintln!("failed to navigate: {err}");
        }
    }
}

struct AppDelegateIvars {
    state: RefCell<Option<AppState>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements we're violating —
    // AppDelegate doesn't override `dealloc`/`init` in a way that skips
    // superclass behavior.
    #[unsafe(super(NSObject))]
    // Every ivar here (`NSWindow`/`NSView`/`NSTextField`/`WryEngine`'s
    // `WebView`) is only safe to touch from the main thread anyway, so this
    // object is main-thread-only rather than trying to make it `Send`/`Sync`.
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    impl AppDelegate {
        #[unsafe(method(goBack:))]
        fn go_back(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Err(err) = state.engine.go_back() {
                    eprintln!("failed to go back: {err}");
                }
            }
        }

        #[unsafe(method(goForward:))]
        fn go_forward(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Err(err) = state.engine.go_forward() {
                    eprintln!("failed to go forward: {err}");
                }
            }
        }

        #[unsafe(method(reloadPage:))]
        fn reload_page(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Err(err) = state.engine.reload() {
                    eprintln!("failed to reload: {err}");
                }
            }
        }

        #[unsafe(method(addressBarActivated:))]
        fn address_bar_activated(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.navigate_to_address_bar_text();
            }
        }
    }

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSWindowDelegate for AppDelegate {
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.relayout();
            }
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars { state: RefCell::new(None) });
        unsafe { msg_send![super(this), init] }
    }
}

/// Owns the delegate (and, through its ivars, the whole window/webview) for
/// as long as the app is meant to run — dropping this would tear the app
/// down, so `main.rs` is expected to hold it until `run()` returns.
pub struct App {
    mtm: MainThreadMarker,
    _delegate: Retained<AppDelegate>,
}

impl App {
    /// Hands control to `NSApplication`'s run loop — same role as GTK's
    /// `gtk::main()`, Win32's message loop, or WinUI 3's
    /// `Application::Start`. Doesn't return until the app quits (see
    /// `AppDelegate::window_will_close`, the only quit path this scaffold
    /// wires up).
    pub fn run(&self) {
        NSApplication::sharedApplication(self.mtm).run();
    }
}

/// Builds the window, toolbar, and webview (loaded to `settings.start_page`
/// — mirroring every other front end's `add_page(&app.settings().start_page)`
/// pattern, just without the multi-page `PageManager` layer this scaffold
/// doesn't have yet) and returns an [`App`] ready to `run()`.
pub fn build_window_and_app(profile: Profile) -> anyhow::Result<App> {
    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("build_window_and_app must be called from the main thread"))?;
    let settings = Settings::load(&profile);
    let initial_url = settings.start_page.clone();

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let delegate = AppDelegate::new(mtm);

    let window_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 700.0));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::Resizable;
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), window_rect, style, NSBackingStoreType::Buffered, false) };
    window.setTitle(&NSString::from_str("Claude Browser"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let content_view = window.contentView().ok_or_else(|| anyhow::anyhow!("NSWindow has no content view"))?;

    let toolbar_view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, window_rect.size.height - TOOLBAR_HEIGHT), NSSize::new(window_rect.size.width, TOOLBAR_HEIGHT)));
    content_view.addSubview(&toolbar_view);

    let back_button = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str("\u{2190}"), Some(&*delegate), Some(sel!(goBack:)), mtm)
    };
    back_button.setFrame(NSRect::new(NSPoint::new(BUTTON_MARGIN, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&back_button);

    let forward_button = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str("\u{2192}"), Some(&*delegate), Some(sel!(goForward:)), mtm)
    };
    forward_button.setFrame(NSRect::new(NSPoint::new(2.0 * BUTTON_MARGIN + BUTTON_WIDTH, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&forward_button);

    let reload_button = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str("\u{21BB}"), Some(&*delegate), Some(sel!(reloadPage:)), mtm)
    };
    reload_button.setFrame(NSRect::new(NSPoint::new(3.0 * BUTTON_MARGIN + 2.0 * BUTTON_WIDTH, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&reload_button);

    let address_bar = NSTextField::textFieldWithString(&NSString::from_str(&initial_url), mtm);
    unsafe {
        address_bar.setTarget(Some(&*delegate));
        address_bar.setAction(Some(sel!(addressBarActivated:)));
    }
    toolbar_view.addSubview(&address_bar);

    let engine = WryEngine::new(&content_view, &initial_url, |_title| {
        // Document-title-changed hook exists on the trait for parity with
        // other backends, but this scaffold has nowhere to show a title yet
        // (no window-title-follows-page-title behavior, no switcher grid).
    })?;

    let state = AppState { window: window.clone(), toolbar_view, address_bar, engine, settings };
    state.relayout();
    delegate.ivars().state.replace(Some(state));

    window.makeKeyAndOrderFront(None);
    app.activateIgnoringOtherApps(true);

    Ok(App { mtm, _delegate: delegate })
}
