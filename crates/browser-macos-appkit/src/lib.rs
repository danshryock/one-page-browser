//! Native AppKit chrome — brought to feature parity with
//! `browser-windows-reactor`'s scope (multi-page via `PageManager`,
//! switcher/settings/profile overlays, global keyboard shortcuts,
//! external-link chooser) and then some: bookmarks, light/dark theme, and
//! encrypted history (see `ROADMAP.md`) have since been added, closing the
//! remaining gap with `browser-linux-gtk3` (still no `NSCollectionView` tile
//! grid — see "Known gaps" below). The Windows front ends don't have these
//! three yet.
//!
//! Theme and encrypted history both diverge from `browser-linux-gtk3`'s
//! literal implementation, each for a concrete reason:
//! - **Theme** is an application-wide `NSApplication::setAppearance` override
//!   (`Theme::Light`/`Dark` → `NSAppearanceNameAqua`/`NSAppearanceNameDarkAqua`),
//!   not a manual color palette — this crate sets zero explicit `NSColor`s
//!   anywhere, so every control is already correctly styled by AppKit in
//!   both system appearances; gtk3 needs a manual `CssProvider` palette only
//!   because GTK's stylesheet is otherwise static.
//! - **Encrypted history**'s passphrase is collected via a synchronous
//!   `NSAlert` + `NSSecureTextField` accessory view (`runModal()`), run
//!   *before* the main window is built — not gtk3's separate `gtk::Window`
//!   (this crate's only second-window precedent, `run_chooser`, is a
//!   spawn-and-exit standalone mini-app, architecturally wrong for "collect
//!   one input, then keep building the same window in the same process").
//!
//! # Written on Linux, cross-compiled from Linux, never *run* from Linux
//!
//! Written entirely on a Linux dev machine with no macOS hardware available
//! (a local macOS VM would violate Apple's EULA on this non-Apple hardware
//! — see `summaries/windows-github-actions-ci.md`'s "why not local VMs"
//! section). This crate *does* have a real cross-compile story from Linux —
//! `cargo zigbuild` plus an unofficial macOS SDK mirror (see README.md's
//! "browser-macos-appkit: building" section) — so every change here is at
//! least compile-and-link checked (real Mach-O binaries, real framework
//! linking) before it's pushed, not just eyeballed against `objc2-app-kit`'s
//! generated source. But there's still no way to *run* a macOS binary from
//! this Linux machine (no Wine-for-macOS equivalent), so real behavioral
//! verification still only happens on GitHub's native `macos-latest`
//! runners (see `.github/workflows/macos.yml`) — treat runtime behavior
//! here as link-checked, not yet proven correct end-to-end.
//!
//! # Layout: manual frames, not `NSStackView`/Auto Layout
//!
//! Every panel here (toolbar, switcher/settings/profile overlays) positions
//! its subviews via explicit `NSRect` frames recomputed in `relayout()`,
//! the same approach the original scaffold used for the toolbar — not
//! `NSStackView`'s Auto Layout engine, despite that being the more
//! idiomatic modern AppKit approach for a from-scratch codebase. Auto
//! Layout's constraint-conflict failures are a runtime phenomenon (logged
//! warnings, sometimes silently-wrong layout) that can't be caught at
//! compile/link time — exactly the class of bug this crate currently has no
//! way to catch before a real human runs it, unlike everything else checked
//! by cross-compiling. Manual frames are more verbose but every failure
//! mode is a compile error or an obviously-wrong number, not a runtime
//! constraint solver falling over in a way only visible on real hardware.
//!
//! # `ctrl` → ⌘, `alt` → ⌥: `browser_core::KeyChord`'s modifiers on macOS
//!
//! `KeyChord::ctrl`/`alt` are cross-platform abstractions for "the OS's
//! primary command modifier" and "the OS's secondary modifier", not
//! literally always physical Control/Alt — Windows/Linux happen to use the
//! same physical keys, but macOS's platform convention for exactly these
//! app-level shortcuts (new tab, close tab, reload, ...) is ⌘ Command, not
//! Control (which mostly does nothing useful at this level in real Mac
//! apps) — so this crate maps `ctrl` to `NSEventModifierFlags::Command` and
//! `alt` to `NSEventModifierFlags::Option` (a literal match — Option *is*
//! Alt) when building `NSMenuItem` key equivalents. See `shortcuts.rs`.
//!
//! Known gaps, to be honest about up front:
//! - The switcher overlay is a plain vertical list, not a wrapping tile
//!   grid — `NSCollectionView` (the real AppKit equivalent of GTK's
//!   `FlowBox`/reactor's `grid_view`) is a much bigger lift than this pass
//!   had time for; a list is a real, working simplification, not a stub.
//! - The address bar doesn't update when navigating via in-page links —
//!   `RenderEngine` only offers a document-title-changed callback, not a
//!   URL-changed one, and wiring that up (`WKNavigationDelegate` on the
//!   `WKWebView` `wry` hands back) is future work, same gap the original
//!   scaffold already had.
#![cfg(target_os = "macos")]

mod shortcuts;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSApplicationActivationPolicy, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSButtonType, NSColor,
    NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags, NSMenuItemValidation, NSPasteboard,
    NSPasteboardTypeString, NSPopUpButton, NSSecureTextField, NSTextAlignment, NSTextField, NSTitlePosition,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowDelegate, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use browser_core::{
    decide_vault_unlock_action, domain_of, launch_new_encrypted_profile_process, launch_new_profile_process,
    list_profile_names, resolve_address_input, resolve_profile_name, resolve_url_argument, Action, BitwardenBackend,
    BitwardenStatus, Bookmark, Bookmarks, HistoryStore, Keybindings, Login, LoginFields, PageManager, PasswordBackend,
    PasswordStore, Profile, Session, SessionPage, Settings, Theme, VaultUnlockAction, APP_TITLE, HOME_URL,
};
use render_engine::{RenderEngine, WKWebView, WKWebViewConfiguration, WebContext, WryEngine};

const TOOLBAR_HEIGHT: f64 = 36.0;
const BUTTON_WIDTH: f64 = 32.0;
const BUTTON_MARGIN: f64 = 4.0;
const ROW_HEIGHT: f64 = 44.0;
const OVERLAY_MARGIN: f64 = 16.0;
const OVERLAY_WIDTH: f64 = 480.0;
const CLOSE_BUTTON_SIZE: f64 = 28.0;
const HINT_WIDTH: f64 = 130.0;

/// Mutually exclusive — opening any one of these closes whichever else was
/// open, mirroring every other front end's `close_switcher`/`close_settings`/
/// `close_profile_picker` convention. No separate `Keybindings` variant: the
/// editor lives as a section within `Settings` (see `rebuild_keybindings_rows`),
/// same design `browser-windows-reactor` settled on per explicit user
/// feedback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Overlay {
    None,
    Switcher,
    Settings,
    Profile,
    Passwords,
    Bookmarks,
}

/// The password vault's session state — mirrors `browser-linux-gtk3`'s
/// `VaultState` exactly (see that crate's doc comment): UI-level
/// bookkeeping, not a storage concept, distinct from `PasswordStore`/
/// `PasswordBackend` in `browser-core`.
enum VaultState {
    NotSetUp,
    Locked,
    Unlocked(PasswordStore),
}

/// Which backend a `Login` shown in the password manager overlay actually
/// came from — mirrors `browser-linux-gtk3`'s local (non-`browser-core`)
/// `LoginSource` enum for the same reason: `Edit`/`Delete`/`Fill` must
/// route to whichever backend a row actually came from, and there's no
/// "move a login between backends" operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginSource {
    Local,
    Bitwarden,
}

// The switcher's row list/activation logic used to be a local `SwitcherRow`
// enum + hand-copied row-building here — now
// `browser_chrome_core::{SwitcherRow, build_switcher_rows, activate_row}`
// (see `ARCHITECTURE.md` §3.2/§4: the exact same decision logic was
// independently hand-copied in `browser-linux-gtk3`/`browser-windows-
// reactor` too, and is now unit-tested once, toolkit-free, instead of
// manually in three places).

struct AppState {
    window: Retained<NSWindow>,
    toolbar_view: Retained<NSView>,
    /// The switcher's search/URL box, living entirely inside the switcher
    /// overlay's own layout (not the toolbar — see `title_chip`). Doubles
    /// as both filtering open pages/history and editing the active page's
    /// URL depending on how the switcher was opened — same unified design
    /// as `browser-linux-gtk3`'s `address_bar` (see that crate's field doc
    /// for the full reasoning): one widget for both roles, not two.
    address_bar: Retained<NSTextField>,
    /// The toolbar's clickable "title chip" — an `NSBox` (border/fill,
    /// toggled at-rest vs. hover-looks-like-an-input by `mouseEntered:`/
    /// `mouseExited:`), a non-editable `NSTextField` on top showing the
    /// active page's title (see `refresh_title_label`), and a borderless
    /// `NSButton` on top of both purely for click detection
    /// (`openSwitcherEditingUrlAction:`).
    title_chip: Retained<NSBox>,
    title_label: Retained<NSTextField>,
    title_chip_button: Retained<NSButton>,
    switcher_button: Retained<NSButton>,
    settings_button: Retained<NSButton>,
    profile_button: Retained<NSButton>,
    passwords_button: Retained<NSButton>,
    /// Toggles whether the active page is bookmarked — title glyph swaps
    /// between a hollow/filled star (see `refresh_bookmark_toggle_button`).
    bookmark_toggle_button: Retained<NSButton>,
    bookmarks_button: Retained<NSButton>,
    /// Hosts every page's container view (see `pages`) below the toolbar —
    /// the AppKit equivalent of GTK's `Stack`/reactor's `Grid`: every loaded
    /// page's container is a sibling subview here, only the active one
    /// visible (`isHidden` toggled), an unloaded page's simply isn't
    /// created at all.
    content_view: Retained<NSView>,
    /// One container `NSView` per page, each hosting that page's `WKWebView`
    /// (via `WryEngine`) as its own child — `browser_core::Page` doesn't
    /// hold this since it's an AppKit-only concept, same reasoning as
    /// `browser-linux-gtk3`'s `containers` field.
    containers: RefCell<HashMap<String, Retained<NSView>>>,
    core: RefCell<PageManager<WryEngine>>,
    /// One `wry::WebContext` shared by every page this profile ever opens —
    /// what actually makes cookies/localStorage/cache persist across
    /// restarts (and be shared between tabs in the same session), instead of
    /// each page silently getting its own throwaway context. Same fix and
    /// reasoning as `browser-linux-gtk3`'s field of the same name.
    web_context: RefCell<WebContext>,
    overlay: Cell<Overlay>,

    switcher_view: Retained<NSView>,
    switcher_chrome: OverlayChrome,
    switcher_rows_container: Retained<NSView>,
    /// Rebuilt every time the switcher opens or its query changes — row
    /// buttons are tagged with their index into this so a click can look up
    /// which row it was (AppKit's `target`/`action` dispatch has no
    /// built-in "which item" beyond the sender itself).
    switcher_rows: RefCell<Vec<browser_chrome_core::SwitcherRow>>,

    settings_view: Retained<NSView>,
    settings_chrome: OverlayChrome,
    start_page_field: Retained<NSTextField>,
    unlimited_checkbox: Retained<NSButton>,
    limit_field: Retained<NSTextField>,
    keybindings_rows_container: Retained<NSView>,
    keybindings: RefCell<Keybindings>,
    /// `Some(action)` while the "Add binding" flow is waiting for text in
    /// `new_binding_field` to be committed as that action's new binding —
    /// mirrors `browser-windows-reactor`'s `listening_for` state.
    listening_for: Cell<Option<Action>>,
    new_binding_field: Retained<NSTextField>,

    profile_view: Retained<NSView>,
    profile_chrome: OverlayChrome,
    profile_rows_container: Retained<NSView>,
    new_profile_field: Retained<NSTextField>,

    passwords_view: Retained<NSView>,
    passwords_chrome: OverlayChrome,
    /// Whether the local vault has ever been set up, is set up but not
    /// unlocked this run, or is open and ready to use — see
    /// `browser_core::decide_vault_unlock_action`'s doc comment for how a
    /// profile that already has a passphrase (for the vault specifically —
    /// unrelated to "encrypted profiles"/history encryption, which this
    /// crate has never implemented) reuses it silently.
    passwords: RefCell<VaultState>,
    /// The passphrase, if any, this run has already used to unlock *some*
    /// store — history at startup (see `open_history`), or the vault —
    /// reused silently instead of re-prompting, mirroring
    /// `browser-linux-gtk3`'s identically-named field and its
    /// `note_unlocked_with_passphrase`. Never written to disk.
    session_passphrase: RefCell<Option<String>>,
    /// Vault locked/setup sub-group — shown instead of the contents
    /// sub-group (below) while `passwords` isn't `VaultState::Unlocked`.
    passwords_unlock_label: Retained<NSTextField>,
    passwords_unlock_field: Retained<NSSecureTextField>,
    passwords_unlock_button: Retained<NSButton>,
    passwords_unlock_error_label: Retained<NSTextField>,
    /// Vault contents sub-group — the credential list plus the add/edit
    /// form, shown once `passwords` is `VaultState::Unlocked`.
    passwords_rows_container: Retained<NSView>,
    /// Flat, combined (local vault + Bitwarden) index list backing every
    /// row's `NSButton::setTag(idx)` — AppKit's target/action dispatch has
    /// no built-in "which item" beyond the sender itself (same reasoning as
    /// `switcher_rows`), so this is what `idx` indexes into.
    passwords_rows: RefCell<Vec<(Login, LoginSource)>>,
    passwords_site_field: Retained<NSTextField>,
    passwords_username_field: Retained<NSTextField>,
    passwords_password_field: Retained<NSSecureTextField>,
    passwords_notes_field: Retained<NSTextField>,
    /// Chooses which backend a brand-new login (`editing_login: None`)
    /// gets saved to — populated with "Local vault" always, "Bitwarden"
    /// only when `Settings::bitwarden_server_url` is set (see
    /// `rebuild_passwords_rows`).
    passwords_destination_popup: Retained<NSPopUpButton>,
    /// Labeled "Add" or "Save" depending on `editing_login`.
    passwords_submit_button: Retained<NSButton>,
    /// Only visible while `editing_login` is `Some` — abandons the edit and
    /// returns the form to "add new" mode.
    passwords_cancel_edit_button: Retained<NSButton>,
    passwords_error_label: Retained<NSTextField>,
    /// Which existing login (if any) the form above is currently editing,
    /// and which backend it came from — `None` means "add new" mode.
    editing_login: RefCell<Option<(String, LoginSource)>>,
    /// Bitwarden's own inline unlock — shown (fixed position, not part of
    /// the dynamic `passwords_rows_container`) only when Bitwarden is
    /// configured but its own `status()` reports locked. Unrelated to the
    /// local vault's unlock fields above; this crate has no "modal that
    /// hands back to the main window" precedent (see `run_chooser`), so —
    /// same as the local vault's own setup/unlock flow — this folds into
    /// the passwords overlay itself rather than a second popup window.
    bitwarden_unlock_field: Retained<NSSecureTextField>,
    bitwarden_unlock_button: Retained<NSButton>,

    bookmarks: RefCell<Bookmarks>,
    bookmarks_view: Retained<NSView>,
    bookmarks_chrome: OverlayChrome,
    bookmarks_rows_container: Retained<NSView>,
    /// Backs each row's `NSButton::setTag(idx)` the same way `passwords_rows`/
    /// `switcher_rows` do — `Bookmarks::all()`'s order (most-recently-added
    /// first), snapshotted fresh every time `rebuild_bookmarks_rows` runs.
    bookmarks_rows: RefCell<Vec<Bookmark>>,

    settings: RefCell<Settings>,
    /// Enables Bitwarden integration and sets `Settings::bitwarden_server_url`
    /// — same construction (`NSButtonType::Switch`) as `unlimited_checkbox`.
    bitwarden_checkbox: Retained<NSButton>,
    bitwarden_url_field: Retained<NSTextField>,
    /// Light/Dark picker for `Settings::theme` — an `NSPopUpButton`, the
    /// same widget/pattern already proven in this crate
    /// (`passwords_destination_popup`), read via `titleOfSelectedItem()`.
    theme_popup: Retained<NSPopUpButton>,
    /// Encrypts a brand-new profile's history from the start — read once by
    /// `create_and_open_profile`, which then calls
    /// `launch_new_encrypted_profile_process` instead of
    /// `launch_new_profile_process` when checked.
    encrypted_checkbox: Retained<NSButton>,
    history: HistoryStore,
    /// Resolved once at startup (from `--profile`, defaulting to
    /// `"default"`) — kept so the settings overlay's Save action re-saves to
    /// the same place `Settings::load`/`Keybindings::load` read from.
    profile: Profile,
}

impl AppState {
    /// Recomputes every frame from the window's current content size —
    /// AppKit has no layout manager doing this automatically for views added
    /// without an autoresizing mask (see this module's doc comment on why
    /// that's a deliberate choice, not an oversight).
    fn relayout(&self) {
        let content_size = self.window.contentView().map(|view| view.frame().size).unwrap_or(NSSize::new(0.0, 0.0));
        self.toolbar_view.setFrame(NSRect::new(
            NSPoint::new(0.0, content_size.height - TOOLBAR_HEIGHT),
            NSSize::new(content_size.width, TOOLBAR_HEIGHT),
        ));

        let button_count = 9.0; // back, forward, reload, switcher, settings, profile, passwords, bookmark toggle, bookmarks
        let title_chip_x = 3.0 * BUTTON_WIDTH + 4.0 * BUTTON_MARGIN;
        let title_chip_end = content_size.width - (button_count - 3.0) * (BUTTON_WIDTH + BUTTON_MARGIN) - BUTTON_MARGIN;
        let title_chip_frame = NSRect::new(
            NSPoint::new(title_chip_x, BUTTON_MARGIN),
            NSSize::new((title_chip_end - title_chip_x).max(0.0), TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN),
        );
        self.title_chip.setFrame(title_chip_frame);
        self.title_label.setFrame(title_chip_frame);
        self.title_chip_button.setFrame(title_chip_frame);
        let mut x = title_chip_end + BUTTON_MARGIN;
        for button in [
            &self.switcher_button,
            &self.settings_button,
            &self.profile_button,
            &self.passwords_button,
            &self.bookmark_toggle_button,
            &self.bookmarks_button,
        ] {
            button.setFrame(NSRect::new(NSPoint::new(x, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
            x += BUTTON_WIDTH + BUTTON_MARGIN;
        }

        let content_height_below_toolbar = (content_size.height - TOOLBAR_HEIGHT).max(0.0);
        let content_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(content_size.width, content_height_below_toolbar));
        for container in self.containers.borrow().values() {
            container.setFrame(content_frame);
        }
        if let Some(page) = self.core.borrow().active() {
            if let Some(engine) = &page.engine {
                if let Err(err) = engine.set_bounds(0, 0, content_frame.size.width.max(0.0) as u32, content_frame.size.height.max(0.0) as u32) {
                    eprintln!("failed to resize webview: {err}");
                }
            }
        }

        for overlay_view in [&self.switcher_view, &self.settings_view, &self.profile_view, &self.passwords_view, &self.bookmarks_view] {
            overlay_view.setFrame(content_frame);
        }
        for chrome in [&self.switcher_chrome, &self.settings_chrome, &self.profile_chrome, &self.passwords_chrome, &self.bookmarks_chrome] {
            relayout_overlay_chrome(chrome, content_frame);
        }
    }

    fn with_active(&self, action: impl FnOnce(&WryEngine) -> anyhow::Result<()>) {
        let core = self.core.borrow();
        if let Some(page) = core.active() {
            if let Some(engine) = &page.engine {
                if let Err(err) = action(engine) {
                    eprintln!("action on active page failed: {err}");
                }
            }
        }
    }

    fn is_switcher_open(&self) -> bool {
        self.overlay.get() == Overlay::Switcher
    }

    /// Real, pre-existing bug this fixes: the 5 overlay views are added as
    /// `content_view` subviews once, at window-construction time — *before*
    /// any page container exists. `add_page` adds each page's container to
    /// `content_view` later, so every page ends up *after* (AppKit: later
    /// subview = painted on top of) all 5 overlays in z-order, permanently.
    /// Without this, opening any overlay renders it *behind* the active
    /// page's `WKWebView`, invisible. Called from every `open_*` method,
    /// right before showing that overlay's view.
    fn bring_overlay_to_front(&self, view: &NSView) {
        self.content_view.addSubview_positioned_relativeTo(view, NSWindowOrderingMode::Above, None);
    }

    // ---- overlay open/close -------------------------------------------

    fn close_all_overlays(&self) {
        self.overlay.set(Overlay::None);
        self.switcher_view.setHidden(true);
        self.settings_view.setHidden(true);
        self.profile_view.setHidden(true);
        self.passwords_view.setHidden(true);
        self.bookmarks_view.setHidden(true);
        if let Some(page) = self.core.borrow().active() {
            self.address_bar.setStringValue(&NSString::from_str(&page.current_url()));
        }
        self.address_bar.setPlaceholderString(None);
    }

    fn open_switcher(self: &Rc<Self>) {
        self.close_all_overlays();
        self.address_bar.setStringValue(&NSString::from_str(""));
        self.address_bar.setPlaceholderString(Some(&NSString::from_str("Type to filter open pages\u{2026}")));
        self.overlay.set(Overlay::Switcher);
        self.rebuild_switcher_rows();
        self.bring_overlay_to_front(&self.switcher_view);
        self.switcher_view.setHidden(false);
        self.window.makeFirstResponder(Some(&self.address_bar));
    }

    /// `EditUrl` (⌘L): opens the switcher preloaded with the active page's
    /// current URL, fully selected — a real "edit the URL" affordance,
    /// unlike `browser-windows-reactor`'s `EditUrl`, which dispatches
    /// correctly but can't actually focus anything (`windows-reactor`
    /// exposes no `Focus()`-style API at all — see that crate's
    /// `dispatch_action` doc comment). AppKit *does* expose real,
    /// unrestricted programmatic focus (`NSWindow::makeFirstResponder`),
    /// so this is implemented for real here.
    fn open_switcher_editing_url(self: &Rc<Self>) {
        self.close_all_overlays();
        let current_url = self.core.borrow().active().map(|p| p.current_url()).unwrap_or_default();
        self.address_bar.setStringValue(&NSString::from_str(&current_url));
        self.address_bar.setPlaceholderString(None);
        self.overlay.set(Overlay::Switcher);
        self.rebuild_switcher_rows();
        self.bring_overlay_to_front(&self.switcher_view);
        self.switcher_view.setHidden(false);
        self.window.makeFirstResponder(Some(&self.address_bar));
        if let Some(editor) = self.address_bar.currentEditor() {
            unsafe { editor.selectAll(None) };
        }
    }

    fn toggle_switcher(self: &Rc<Self>) {
        if self.is_switcher_open() {
            self.close_all_overlays();
        } else {
            self.open_switcher();
        }
    }

    fn open_settings(self: &Rc<Self>) {
        self.close_all_overlays();
        self.listening_for.set(None);
        let settings = self.settings.borrow();
        self.start_page_field.setStringValue(&NSString::from_str(&settings.start_page));
        match settings.max_loaded_pages {
            Some(n) => {
                self.unlimited_checkbox.setState(NSControlStateValueOff);
                self.limit_field.setStringValue(&NSString::from_str(&n.to_string()));
                self.limit_field.setEnabled(true);
            }
            None => {
                self.unlimited_checkbox.setState(NSControlStateValueOn);
                self.limit_field.setStringValue(&NSString::from_str(""));
                self.limit_field.setEnabled(false);
            }
        }
        match &settings.bitwarden_server_url {
            Some(url) => {
                self.bitwarden_checkbox.setState(NSControlStateValueOn);
                self.bitwarden_url_field.setStringValue(&NSString::from_str(url));
            }
            None => {
                self.bitwarden_checkbox.setState(NSControlStateValueOff);
                self.bitwarden_url_field.setStringValue(&NSString::from_str(""));
            }
        }
        self.theme_popup.selectItemWithTitle(&NSString::from_str(match settings.theme {
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }));
        drop(settings);
        self.overlay.set(Overlay::Settings);
        self.rebuild_keybindings_rows();
        self.bring_overlay_to_front(&self.settings_view);
        self.settings_view.setHidden(false);
    }

    /// The settings toolbar button's target — see `toggle_switcher`.
    fn toggle_settings(self: &Rc<Self>) {
        if self.overlay.get() == Overlay::Settings {
            self.close_all_overlays();
        } else {
            self.open_settings();
        }
    }

    /// Live toggle: clicking "Unlimited loaded pages" enables/disables
    /// `limit_field` immediately, matching `browser-linux-gtk3`'s
    /// `unlimited_check`'s toggle handler — without this, the field would
    /// only ever reflect the setting as of when the overlay was last
    /// opened, a real, worth-fixing UX gap, not just a cosmetic one.
    fn toggle_unlimited(&self) {
        let unlimited = self.unlimited_checkbox.state() == NSControlStateValueOn;
        self.limit_field.setEnabled(!unlimited);
    }

    fn save_settings(self: &Rc<Self>) {
        let unlimited = self.unlimited_checkbox.state() == NSControlStateValueOn;
        let limit_text = self.limit_field.stringValue().to_string();
        let new_limit = if unlimited { None } else { limit_text.trim().parse::<usize>().ok().map(|n| n.max(1)) };
        let bitwarden_enabled = self.bitwarden_checkbox.state() == NSControlStateValueOn;
        let bitwarden_url_text = self.bitwarden_url_field.stringValue().to_string();
        let bitwarden_url_text = bitwarden_url_text.trim();
        let new_bitwarden_url = bitwarden_enabled
            .then(|| if bitwarden_url_text.is_empty() { "http://127.0.0.1:8087".to_string() } else { bitwarden_url_text.to_string() });
        let new_theme = match self.theme_popup.titleOfSelectedItem().map(|s| s.to_string()).as_deref() {
            Some("Light") => Theme::Light,
            _ => Theme::Dark,
        };
        {
            let mut settings = self.settings.borrow_mut();
            settings.start_page = self.start_page_field.stringValue().to_string();
            settings.max_loaded_pages = new_limit;
            settings.bitwarden_server_url = new_bitwarden_url;
            settings.theme = new_theme;
        }
        let evicted = self.core.borrow_mut().set_max_loaded_pages(new_limit);
        self.unload_engines(&evicted);
        if let Err(err) = self.settings.borrow().save(&self.profile) {
            eprintln!("failed to save settings: {err}");
        }
        self.apply_theme();
        self.close_all_overlays();
    }

    /// Applies `Settings::theme` by overriding the whole app's `NSAppearance`
    /// — every control here is already correctly styled by AppKit in both
    /// system light and dark mode (this crate sets no explicit `NSColor`s
    /// anywhere), so this one call is the entire feature; no per-overlay
    /// color palette to maintain, unlike `browser-linux-gtk3`'s
    /// `theme_css`/`CssProvider` (GTK's stylesheet is otherwise static).
    /// Called once right after `AppState` is constructed and again at the
    /// end of `save_settings`, mirroring gtk3's two call sites.
    fn apply_theme(&self) {
        let name = match self.settings.borrow().theme {
            Theme::Light => unsafe { NSAppearanceNameAqua },
            Theme::Dark => unsafe { NSAppearanceNameDarkAqua },
        };
        let appearance = NSAppearance::appearanceNamed(name);
        NSApplication::sharedApplication(self.mtm()).setAppearance(appearance.as_deref());
    }

    fn open_profile_picker(self: &Rc<Self>) {
        self.close_all_overlays();
        self.new_profile_field.setStringValue(&NSString::from_str(""));
        self.encrypted_checkbox.setState(NSControlStateValueOff);
        self.overlay.set(Overlay::Profile);
        self.rebuild_profile_rows();
        self.bring_overlay_to_front(&self.profile_view);
        self.profile_view.setHidden(false);
    }

    /// The profile toolbar button's target — see `toggle_switcher`.
    fn toggle_profile_picker(self: &Rc<Self>) {
        if self.overlay.get() == Overlay::Profile {
            self.close_all_overlays();
        } else {
            self.open_profile_picker();
        }
    }

    /// Launches a new process for the new profile — encrypted (its history
    /// passphrase-protected from the start, via `--setup-passphrase`; see
    /// `open_history`) if `encrypted_checkbox` is checked, plain otherwise.
    fn create_and_open_profile(&self) {
        let name = self.new_profile_field.stringValue().to_string();
        let name = name.trim();
        if !name.is_empty() {
            let encrypted = self.encrypted_checkbox.state() == NSControlStateValueOn;
            let result = if encrypted { launch_new_encrypted_profile_process(name) } else { launch_new_profile_process(name) };
            if let Err(err) = result {
                eprintln!("failed to launch a new process for profile {name:?}: {err}");
            }
        }
        self.close_all_overlays();
    }

    // ---- password manager -----------------------------------------------

    /// Builds a fresh `BitwardenBackend` from the current settings, if
    /// Bitwarden integration is enabled — cheap to construct (no network
    /// I/O happens until a real call is made), so there's nothing to cache:
    /// `bw serve` — a separate, already-running process — is what actually
    /// owns the vault's lock state, not anything here. Mirrors
    /// `browser-linux-gtk3`'s identically-named method.
    fn bitwarden_backend(&self) -> Option<BitwardenBackend> {
        self.settings.borrow().bitwarden_server_url.clone().map(BitwardenBackend::new)
    }

    /// Shows the password manager overlay. Unlike every other overlay here,
    /// this first has to resolve the local vault's unlock/setup state (see
    /// `decide_vault_unlock_action`'s doc comment) — this crate has no
    /// separate-window passphrase-prompt precedent (see `run_chooser`'s doc
    /// comment), so both the "not set up yet"/"locked" states and the real
    /// credential list are all rendered within this one overlay, toggled by
    /// `rebuild_passwords_view`.
    fn open_passwords(self: &Rc<Self>) {
        self.close_all_overlays();
        self.overlay.set(Overlay::Passwords);
        self.cancel_editing_login();
        self.passwords_unlock_error_label.setStringValue(&NSString::from_str(""));
        self.rebuild_passwords_view();
        self.bring_overlay_to_front(&self.passwords_view);
        self.passwords_view.setHidden(false);
    }

    /// The passwords toolbar button's target — see `toggle_switcher`.
    fn toggle_passwords(self: &Rc<Self>) {
        if self.overlay.get() == Overlay::Passwords {
            self.close_all_overlays();
        } else {
            self.open_passwords();
        }
    }

    /// Tries to open the vault with `passphrase`, updating `self.passwords`/
    /// `self.session_passphrase` on success. `is_setup` marks this as the
    /// vault's first-ever passphrase (calls `enable_vault_passphrase`) —
    /// see `PasswordStore::open_encrypted`'s doc comment for why "setup"
    /// and "unlock" are otherwise the same call. Returns whether it
    /// succeeded, so callers can show an error rather than silently doing
    /// nothing. Mirrors `browser-linux-gtk3`'s identically-named method.
    fn try_open_vault_with(&self, passphrase: &str, is_setup: bool) -> bool {
        match PasswordStore::open_encrypted(&self.profile, passphrase) {
            Ok(store) => {
                if is_setup {
                    if let Err(err) = self.profile.enable_vault_passphrase() {
                        eprintln!("failed to mark profile as vault-passphrase-protected: {err}");
                    }
                }
                if self.session_passphrase.borrow().is_none() {
                    *self.session_passphrase.borrow_mut() = Some(passphrase.to_string());
                }
                *self.passwords.borrow_mut() = VaultState::Unlocked(store);
                true
            }
            Err(err) => {
                eprintln!("failed to open password vault: {err}");
                false
            }
        }
    }

    /// Decides whether to show the locked/setup sub-group or the vault
    /// contents sub-group, based on `decide_vault_unlock_action` — a
    /// `SilentlySetUpWith`/`SilentlyUnlockWith` result (a passphrase already
    /// known this session) unlocks immediately with no prompt shown at all,
    /// same "same passphrase, no second prompt" behavior
    /// `browser-linux-gtk3` implements.
    fn rebuild_passwords_view(self: &Rc<Self>) {
        if !matches!(&*self.passwords.borrow(), VaultState::Unlocked(_)) {
            let action = decide_vault_unlock_action(&self.profile, self.session_passphrase.borrow().as_deref());
            match action {
                VaultUnlockAction::SilentlySetUpWith(passphrase) => {
                    self.try_open_vault_with(&passphrase, true);
                }
                VaultUnlockAction::SilentlyUnlockWith(passphrase) => {
                    self.try_open_vault_with(&passphrase, false);
                }
                VaultUnlockAction::PromptToSetUp | VaultUnlockAction::PromptToUnlock => {}
            }
        }

        let unlocked = matches!(&*self.passwords.borrow(), VaultState::Unlocked(_));
        let is_setup = !self.profile.has_vault_passphrase();
        self.passwords_unlock_label.setStringValue(&NSString::from_str(if is_setup {
            "Choose a passphrase to encrypt your password vault."
        } else {
            "Enter your vault passphrase to unlock it."
        }));
        self.passwords_unlock_button.setTitle(&NSString::from_str(if is_setup { "Set Up" } else { "Unlock" }));
        self.passwords_unlock_label.setHidden(unlocked);
        self.passwords_unlock_field.setHidden(unlocked);
        self.passwords_unlock_button.setHidden(unlocked);
        self.passwords_unlock_error_label.setHidden(unlocked);
        self.passwords_rows_container.setHidden(!unlocked);
        self.passwords_site_field.setHidden(!unlocked);
        self.passwords_username_field.setHidden(!unlocked);
        self.passwords_password_field.setHidden(!unlocked);
        self.passwords_notes_field.setHidden(!unlocked);
        self.passwords_destination_popup.setHidden(!unlocked);
        self.passwords_submit_button.setHidden(!unlocked);

        if unlocked {
            self.rebuild_passwords_rows();
        } else {
            self.window.makeFirstResponder(Some(&self.passwords_unlock_field));
        }
    }

    /// The local vault unlock/setup button's action.
    fn unlock_vault_clicked(self: &Rc<Self>) {
        let passphrase = self.passwords_unlock_field.stringValue().to_string();
        if passphrase.is_empty() {
            self.passwords_unlock_error_label.setStringValue(&NSString::from_str("Passphrase can't be empty."));
            return;
        }
        let is_setup = !self.profile.has_vault_passphrase();
        if self.try_open_vault_with(&passphrase, is_setup) {
            self.passwords_unlock_field.setStringValue(&NSString::from_str(""));
            self.rebuild_passwords_view();
        } else {
            self.passwords_unlock_error_label.setStringValue(&NSString::from_str("Couldn't open the vault with that passphrase. Try again."));
            self.passwords_unlock_field.setStringValue(&NSString::from_str(""));
            self.window.makeFirstResponder(Some(&self.passwords_unlock_field));
        }
    }

    /// Rebuilds `passwords_destination_popup`'s entries — "Local vault"
    /// always, plus "Bitwarden" when it's enabled — preserving the current
    /// selection if it's still valid, defaulting to "Local vault" otherwise.
    fn refresh_passwords_destination_popup(&self) {
        let bitwarden_enabled = self.bitwarden_backend().is_some();
        let previous = self.passwords_destination_popup.titleOfSelectedItem().map(|s| s.to_string());
        self.passwords_destination_popup.removeAllItems();
        self.passwords_destination_popup.addItemWithTitle(&NSString::from_str("Local vault"));
        if bitwarden_enabled {
            self.passwords_destination_popup.addItemWithTitle(&NSString::from_str("Bitwarden"));
        }
        let restore = previous.filter(|t| t == "Local vault" || (t == "Bitwarden" && bitwarden_enabled));
        self.passwords_destination_popup.selectItemWithTitle(&NSString::from_str(restore.as_deref().unwrap_or("Local vault")));
    }

    /// Rebuilds the password manager overlay's credential list — local
    /// vault entries, then (if Bitwarden is configured) Bitwarden entries,
    /// mirroring `browser-linux-gtk3`'s `rebuild_passwords_list` (see its
    /// doc comment for why the two sources are sectioned rather than
    /// interleaved by timestamp). Each row gets Fill (gated on
    /// `entry.domain` matching the active page's domain — enforced again,
    /// not just here, inside the actual fill action), Copy, Edit, and
    /// Delete buttons, all `setTag(idx)`-ed into `passwords_rows`.
    fn rebuild_passwords_rows(&self) {
        clear_subviews(&self.passwords_rows_container);
        self.passwords_error_label.setStringValue(&NSString::from_str(""));
        self.refresh_passwords_destination_popup();

        let active_domain = self.core.borrow().active().map(|p| domain_of(&p.current_url()));
        let mut rows: Vec<(Login, LoginSource)> = Vec::new();

        let local_entries = match &*self.passwords.borrow() {
            VaultState::Unlocked(store) => store.list().unwrap_or_else(|err| {
                eprintln!("failed to list password entries: {err}");
                Vec::new()
            }),
            VaultState::Locked | VaultState::NotSetUp => Vec::new(),
        };
        for entry in local_entries {
            rows.push((entry, LoginSource::Local));
        }

        let bitwarden = self.bitwarden_backend();
        let bitwarden_status = bitwarden.as_ref().map(|b| b.status());
        if let Some(Ok(BitwardenStatus::Unlocked)) = &bitwarden_status {
            match bitwarden.as_ref().unwrap().list() {
                Ok(entries) => {
                    for entry in entries {
                        rows.push((entry, LoginSource::Bitwarden));
                    }
                }
                Err(err) => eprintln!("failed to list Bitwarden items: {err}"),
            }
        }

        let mtm = self.mtm();
        let width = self.passwords_rows_container.frame().size.width;
        for (idx, (entry, _source)) in rows.iter().enumerate() {
            let y = (rows.len() - 1 - idx) as f64 * ROW_HEIGHT;

            let label_text = format!("{} \u{2014} {}", entry.domain, entry.username);
            let label = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(&label_text), None, None, mtm) };
            label.setButtonType(NSButtonType::MomentaryLight);
            label.setEnabled(false);
            label.setFrame(NSRect::new(NSPoint::new(0.0, y), NSSize::new(width * 0.4, ROW_HEIGHT - BUTTON_MARGIN)));
            self.passwords_rows_container.addSubview(&label);

            // `target: None` — dispatched via the responder chain (up to
            // the window's delegate, `AppDelegate`) rather than an explicit
            // target reference, the same nil-targeted-action pattern
            // `rebuild_switcher_rows`/`rebuild_profile_rows` already use for
            // every dynamically-created row button in this crate.
            let mut x = width * 0.4;
            if entry.password.is_some() && active_domain.as_deref() == Some(entry.domain.as_str()) {
                let fill = unsafe {
                    NSButton::buttonWithTitle_target_action(&NSString::from_str("Fill"), None, Some(sel!(passwordRowFillClicked:)), mtm)
                };
                fill.setTag(idx as isize);
                fill.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(60.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.passwords_rows_container.addSubview(&fill);
            }
            x += 64.0;

            let copy = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str("Copy"), None, Some(sel!(passwordRowCopyClicked:)), mtm)
            };
            copy.setTag(idx as isize);
            copy.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(60.0, ROW_HEIGHT - BUTTON_MARGIN)));
            self.passwords_rows_container.addSubview(&copy);
            x += 64.0;

            let edit = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str("Edit"), None, Some(sel!(passwordRowEditClicked:)), mtm)
            };
            edit.setTag(idx as isize);
            edit.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(60.0, ROW_HEIGHT - BUTTON_MARGIN)));
            self.passwords_rows_container.addSubview(&edit);
            x += 64.0;

            let delete = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str("\u{d7}"), None, Some(sel!(passwordRowDeleteClicked:)), mtm)
            };
            delete.setTag(idx as isize);
            delete.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(30.0, ROW_HEIGHT - BUTTON_MARGIN)));
            self.passwords_rows_container.addSubview(&delete);
        }

        // Bitwarden's own inline unlock — fixed position, not part of the
        // dynamic row list above (see this crate's `bitwarden_unlock_field`
        // doc comment for why).
        let bitwarden_locked = matches!(bitwarden_status, Some(Ok(BitwardenStatus::Locked)));
        self.bitwarden_unlock_field.setHidden(!bitwarden_locked);
        self.bitwarden_unlock_button.setHidden(!bitwarden_locked);
        if let Some(Err(err)) = &bitwarden_status {
            eprintln!("Bitwarden: could not connect (is `bw serve` running?): {err}");
        }

        *self.passwords_rows.borrow_mut() = rows;
    }

    fn password_row_fill_clicked(self: &Rc<Self>, idx: usize) {
        let Some((entry, _)) = self.passwords_rows.borrow().get(idx).cloned() else { return };
        self.fill_active_page_with_login(&entry);
    }

    /// Fills the active page's login form with `entry`'s username/password
    /// and closes the overlay. Re-checks `entry.domain` against the active
    /// page's domain itself (a no-op if it doesn't match, or if there's no
    /// password to fill) — `rebuild_passwords_rows` already gates whether
    /// the Fill button is shown on the same check, but the restriction
    /// needs to be real and enforced here too, not just a UI affordance.
    fn fill_active_page_with_login(self: &Rc<Self>, entry: &Login) {
        let Some(password) = entry.password.clone() else { return };
        let active_domain = self.core.borrow().active().map(|p| domain_of(&p.current_url()));
        if active_domain.as_deref() != Some(entry.domain.as_str()) {
            return;
        }
        let username = entry.username.clone();
        self.with_active(|engine| engine.fill_login(&username, &password));
        self.close_all_overlays();
    }

    fn password_row_copy_clicked(&self, idx: usize) {
        let Some((entry, _)) = self.passwords_rows.borrow().get(idx).cloned() else { return };
        let password = entry.password.unwrap_or_default();
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        unsafe {
            pasteboard.setString_forType(&NSString::from_str(&password), NSPasteboardTypeString);
        }
    }

    fn password_row_edit_clicked(self: &Rc<Self>, idx: usize) {
        let Some((entry, source)) = self.passwords_rows.borrow().get(idx).cloned() else { return };
        self.start_editing_login(&entry, source);
    }

    fn password_row_delete_clicked(self: &Rc<Self>, idx: usize) {
        let Some((entry, source)) = self.passwords_rows.borrow().get(idx).cloned() else { return };
        self.delete_login(&entry.id, source);
    }

    /// Fills the add/edit form from `entry` and switches it into "edit"
    /// mode — reuses the exact same form the add-new-credential flow does,
    /// rather than a second, separate edit form. Mirrors
    /// `browser-linux-gtk3`'s identically-named method.
    fn start_editing_login(self: &Rc<Self>, entry: &Login, source: LoginSource) {
        self.passwords_site_field.setStringValue(&NSString::from_str(&entry.site));
        self.passwords_username_field.setStringValue(&NSString::from_str(&entry.username));
        self.passwords_password_field.setStringValue(&NSString::from_str(entry.password.as_deref().unwrap_or("")));
        self.passwords_notes_field.setStringValue(&NSString::from_str(&entry.notes));
        *self.editing_login.borrow_mut() = Some((entry.id.clone(), source));
        self.passwords_destination_popup.setEnabled(false);
        self.passwords_submit_button.setTitle(&NSString::from_str("Save"));
        self.passwords_cancel_edit_button.setHidden(false);
    }

    /// Returns the add/edit form to "add new" mode.
    fn cancel_editing_login(&self) {
        self.passwords_site_field.setStringValue(&NSString::from_str(""));
        self.passwords_username_field.setStringValue(&NSString::from_str(""));
        self.passwords_password_field.setStringValue(&NSString::from_str(""));
        self.passwords_notes_field.setStringValue(&NSString::from_str(""));
        *self.editing_login.borrow_mut() = None;
        self.passwords_destination_popup.setEnabled(true);
        self.passwords_submit_button.setTitle(&NSString::from_str("Add"));
        self.passwords_cancel_edit_button.setHidden(true);
    }

    /// Submits the add/edit form — adds a new login (routed to whichever
    /// backend `passwords_destination_popup` selects) if `editing_login` is
    /// `None`, or updates the existing one it names otherwise. Mirrors
    /// `browser-linux-gtk3`'s identically-named method.
    fn submit_login_from_fields(self: &Rc<Self>) {
        let site = self.passwords_site_field.stringValue().to_string();
        let site = site.trim().to_string();
        if site.is_empty() {
            return;
        }
        let username = self.passwords_username_field.stringValue().to_string();
        let password_text = self.passwords_password_field.stringValue().to_string();
        let notes = self.passwords_notes_field.stringValue().to_string();
        let password = if password_text.trim().is_empty() { None } else { Some(password_text) };
        let fields = LoginFields { site, username, password, passkey: None, notes };

        let editing = self.editing_login.borrow().clone();
        let result: anyhow::Result<()> = match editing {
            Some((id, LoginSource::Local)) => match &*self.passwords.borrow() {
                VaultState::Unlocked(store) => store.update(&id, fields),
                VaultState::Locked | VaultState::NotSetUp => return,
            },
            Some((id, LoginSource::Bitwarden)) => match self.bitwarden_backend() {
                Some(backend) => backend.update(&id, fields),
                None => return,
            },
            None => match self.passwords_destination_popup.titleOfSelectedItem().map(|s| s.to_string()).as_deref() {
                Some("Bitwarden") => match self.bitwarden_backend() {
                    Some(backend) => backend.add(fields).map(|_| ()),
                    None => return,
                },
                _ => match &*self.passwords.borrow() {
                    VaultState::Unlocked(store) => store.add(fields).map(|_| ()),
                    VaultState::Locked | VaultState::NotSetUp => return,
                },
            },
        };

        if let Err(err) = result {
            let action = if self.editing_login.borrow().is_some() { "save" } else { "add" };
            self.passwords_error_label.setStringValue(&NSString::from_str(&format!("Failed to {action} login: {err}")));
            return;
        }
        self.cancel_editing_login();
        self.rebuild_passwords_rows();
    }

    /// Deletes the login identified by `id` from whichever backend `source`
    /// names. Mirrors `browser-linux-gtk3`'s identically-named method.
    fn delete_login(&self, id: &str, source: LoginSource) {
        let result: anyhow::Result<()> = match source {
            LoginSource::Local => match &*self.passwords.borrow() {
                VaultState::Unlocked(store) => store.delete(id),
                VaultState::Locked | VaultState::NotSetUp => Ok(()),
            },
            LoginSource::Bitwarden => match self.bitwarden_backend() {
                Some(backend) => backend.delete(id),
                None => Ok(()),
            },
        };
        if let Err(err) = result {
            self.passwords_error_label.setStringValue(&NSString::from_str(&format!("Failed to delete login: {err}")));
        }
        self.rebuild_passwords_rows();
    }

    /// Bitwarden's inline "Unlock" button's action.
    fn unlock_bitwarden_clicked(&self) {
        let Some(backend) = self.bitwarden_backend() else { return };
        let password = self.bitwarden_unlock_field.stringValue().to_string();
        if password.is_empty() {
            return;
        }
        match backend.unlock(&password) {
            Ok(()) => {
                self.bitwarden_unlock_field.setStringValue(&NSString::from_str(""));
                self.rebuild_passwords_rows();
            }
            Err(err) => {
                self.passwords_error_label.setStringValue(&NSString::from_str(&format!("Couldn't unlock Bitwarden: {err}")));
                self.bitwarden_unlock_field.setStringValue(&NSString::from_str(""));
            }
        }
    }

    // ---- bookmarks --------------------------------------------------------

    /// Shows the bookmarks overlay, rebuilt from the current `Bookmarks`
    /// each time. Mirrors `browser-linux-gtk3`'s identically-named method.
    fn open_bookmarks(self: &Rc<Self>) {
        self.close_all_overlays();
        self.overlay.set(Overlay::Bookmarks);
        self.rebuild_bookmarks_rows();
        self.bring_overlay_to_front(&self.bookmarks_view);
        self.bookmarks_view.setHidden(false);
    }

    /// The bookmarks toolbar button's target — see `toggle_switcher`.
    fn toggle_bookmarks(self: &Rc<Self>) {
        if self.overlay.get() == Overlay::Bookmarks {
            self.close_all_overlays();
        } else {
            self.open_bookmarks();
        }
    }

    /// Rebuilds the bookmarks overlay's rows from scratch, most-recently-
    /// added first (`Bookmarks::all()`'s order). Each row's Open button
    /// opens it as a new page; Remove deletes it without opening anything.
    /// Mirrors `browser-linux-gtk3`'s `rebuild_bookmarks_list`.
    fn rebuild_bookmarks_rows(&self) {
        clear_subviews(&self.bookmarks_rows_container);
        let mtm = self.mtm();
        let rows: Vec<Bookmark> = self.bookmarks.borrow().all().into_iter().cloned().collect();
        let width = self.bookmarks_rows_container.frame().size.width;

        for (idx, bookmark) in rows.iter().enumerate() {
            let y = (rows.len() - 1 - idx) as f64 * ROW_HEIGHT;

            let label_text = if bookmark.title.is_empty() {
                bookmark.url.clone()
            } else {
                format!("{} \u{2014} {}", bookmark.title, bookmark.domain)
            };
            let label = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(&label_text), None, None, mtm) };
            label.setButtonType(NSButtonType::MomentaryLight);
            label.setEnabled(false);
            label.setFrame(NSRect::new(NSPoint::new(0.0, y), NSSize::new(width - 200.0, ROW_HEIGHT - BUTTON_MARGIN)));
            self.bookmarks_rows_container.addSubview(&label);

            // `target: None` — same nil-targeted-action pattern every other
            // dynamically-created row button in this crate uses (see
            // `rebuild_passwords_rows`'s doc comment).
            let open = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str("Open"), None, Some(sel!(bookmarkRowOpenClicked:)), mtm)
            };
            open.setTag(idx as isize);
            open.setFrame(NSRect::new(NSPoint::new(width - 190.0, y), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
            self.bookmarks_rows_container.addSubview(&open);

            let remove = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str("\u{d7}"), None, Some(sel!(bookmarkRowRemoveClicked:)), mtm)
            };
            remove.setTag(idx as isize);
            remove.setFrame(NSRect::new(NSPoint::new(width - 90.0, y), NSSize::new(60.0, ROW_HEIGHT - BUTTON_MARGIN)));
            self.bookmarks_rows_container.addSubview(&remove);
        }

        *self.bookmarks_rows.borrow_mut() = rows;
    }

    fn bookmark_row_open_clicked(self: &Rc<Self>, idx: usize) {
        let Some(bookmark) = self.bookmarks_rows.borrow().get(idx).cloned() else { return };
        if let Err(err) = self.add_page(&bookmark.url) {
            eprintln!("failed to open bookmark: {err}");
        }
        self.close_all_overlays();
    }

    fn bookmark_row_remove_clicked(&self, idx: usize) {
        let Some(bookmark) = self.bookmarks_rows.borrow().get(idx).cloned() else { return };
        self.bookmarks.borrow_mut().remove(&bookmark.url);
        if let Err(err) = self.bookmarks.borrow().save(&self.profile) {
            eprintln!("failed to save bookmarks: {err}");
        }
        self.rebuild_bookmarks_rows();
        self.refresh_bookmark_toggle_button();
    }

    /// Adds or removes a bookmark for the active page — the toolbar star
    /// button's action, and the `ToggleBookmark` keybinding (default ⌘D).
    /// Mirrors `browser-linux-gtk3`'s identically-named method.
    fn toggle_bookmark_for_active(&self) {
        let (url, title) = {
            let core = self.core.borrow();
            let Some(page) = core.active() else { return };
            let title = page.title.borrow().clone();
            (page.current_url(), title)
        };
        self.bookmarks.borrow_mut().toggle(&url, &title, now_unix());
        if let Err(err) = self.bookmarks.borrow().save(&self.profile) {
            eprintln!("failed to save bookmarks: {err}");
        }
        self.refresh_bookmark_toggle_button();
    }

    /// Updates the toolbar star button's title glyph to reflect whether the
    /// active page is currently bookmarked — called whenever the active
    /// page changes (see `set_active`) or a bookmark is toggled/removed, so
    /// it never shows stale state. Mirrors `browser-linux-gtk3`'s
    /// `refresh_bookmark_toggle_button`.
    fn refresh_bookmark_toggle_button(&self) {
        let is_bookmarked = self
            .core
            .borrow()
            .active()
            .map(|p| self.bookmarks.borrow().is_bookmarked(&p.current_url()))
            .unwrap_or(false);
        self.bookmark_toggle_button
            .setTitle(&NSString::from_str(if is_bookmarked { "\u{2605}" } else { "\u{2606}" }));
    }

    /// Updates the toolbar's title chip to reflect the active page's current
    /// title — called whenever the active page changes (`set_active`) or its
    /// title changes (`WryEngine::new`'s title-changed callback). Falls back
    /// to "New Page" for an empty title, matching `browser_chrome_core::
    /// switcher`'s existing convention for the same case. Mirrors
    /// `browser-linux-gtk3`'s `refresh_title_label`.
    fn refresh_title_label(&self) {
        let title = self.core.borrow().active().map(|p| p.title.borrow().clone()).unwrap_or_default();
        self.title_label.setStringValue(&NSString::from_str(if title.is_empty() { "New Page" } else { &title }));
    }

    // ---- pages ----------------------------------------------------------

    /// Allocates a fresh page id, builds its container view + `WryEngine`,
    /// unloads whatever `PageManager::insert` evicted to make room, and
    /// makes it active — the shared core of both the first-page bootstrap
    /// and the switcher's "+" tile.
    fn add_page(self: &Rc<Self>, url: &str) -> anyhow::Result<String> {
        let mtm = self.mtm();
        let mut core = self.core.borrow_mut();
        let id = core.allocate_id();
        drop(core);

        let container = NSView::initWithFrame(NSView::alloc(mtm), self.content_view.frame());
        self.content_view.addSubview(&container);
        // Weak, not `Rc::clone(self)` — this closure is stored inside the
        // `wry::WebView` that ends up owned (via `PageManager`) by this same
        // `AppState`, so a strong reference here would be a genuine `Rc`
        // cycle (`AppState -> core -> PageManager -> Page.engine ->
        // wry::WebView -> this closure -> Rc<AppState>`), keeping `AppState`
        // alive forever. Matches `browser-linux-gtk3`'s
        // `Rc::downgrade(self)` in the same spot.
        let self_for_title = Rc::downgrade(self);
        let id_for_title = id.clone();
        let self_for_new_window = Rc::downgrade(self);
        let engine = WryEngine::new(
            &container,
            url,
            &mut *self.web_context.borrow_mut(),
            move |title| {
                let Some(app) = self_for_title.upgrade() else { return };
                if let Some(page) = app.core.borrow_mut().page_mut(&id_for_title) {
                    *page.title.borrow_mut() = title;
                }
                app.record_visit(&id_for_title);
                if app.core.borrow().active_id() == id_for_title {
                    app.refresh_title_label();
                }
            },
            move |_new_window_url, target_configuration| {
                let app = self_for_new_window.upgrade()?;
                app.add_page_related(target_configuration).ok()
            },
        )?;

        let title = Rc::new(RefCell::new(String::new()));
        let evicted = self.core.borrow_mut().insert(id.clone(), engine, title);
        self.unload_engines(&evicted);
        self.containers.borrow_mut().insert(id.clone(), container);
        self.set_active(&id);
        Ok(id)
    }

    /// Opens a page related to `target_configuration` — used for a page
    /// opened via `window.open()`/`target="_blank"`/"open in new tab" (see
    /// `render_engine::macos::WryEngine::new`'s `on_new_window_requested`
    /// doc comment), preserving `window.opener`/`postMessage`/the opener's
    /// own `window.open()` return value via `WryEngine::new_related` —
    /// since macOS has no way to gate on a real user gesture (see that same
    /// doc comment), every request that reaches this function is accepted.
    /// Returns the new page's raw `WKWebView` so the caller (`wry`'s own
    /// new-window handler) can hand it straight back as
    /// `NewWindowResponse::Create`'s payload, rather than tracking it
    /// internally only. Otherwise mirrors `add_page`: calls
    /// `insert_background` instead of `insert`, never calls `set_active` so
    /// the new tab doesn't steal focus, and — matching this function's
    /// callers' own convention (see `force_new_page_from_search`) — leaves
    /// refreshing the switcher grid to its caller.
    fn add_page_related(self: &Rc<Self>, target_configuration: Retained<WKWebViewConfiguration>) -> anyhow::Result<Retained<WKWebView>> {
        let mtm = self.mtm();
        let mut core = self.core.borrow_mut();
        let id = core.allocate_id();
        drop(core);

        let container = NSView::initWithFrame(NSView::alloc(mtm), self.content_view.frame());
        self.content_view.addSubview(&container);
        let self_for_title = Rc::downgrade(self);
        let id_for_title = id.clone();
        let self_for_new_window = Rc::downgrade(self);
        let engine = WryEngine::new_related(
            &container,
            target_configuration,
            move |title| {
                let Some(app) = self_for_title.upgrade() else { return };
                if let Some(page) = app.core.borrow_mut().page_mut(&id_for_title) {
                    *page.title.borrow_mut() = title;
                }
                app.record_visit(&id_for_title);
                if app.core.borrow().active_id() == id_for_title {
                    app.refresh_title_label();
                }
            },
            move |_new_window_url, target_configuration| {
                let app = self_for_new_window.upgrade()?;
                app.add_page_related(target_configuration).ok()
            },
        )?;

        let raw_webview = engine.raw_webview();
        let title = Rc::new(RefCell::new(String::new()));
        let evicted = self.core.borrow_mut().insert_background(id.clone(), engine, title);
        self.unload_engines(&evicted);
        self.containers.borrow_mut().insert(id.clone(), container);
        Ok(raw_webview)
    }

    /// Records a history visit for `id`'s current URL/title — called from
    /// every page's title-changed callback (see `add_page`/
    /// `ensure_engine_loaded`). Previously missing entirely on this
    /// platform (a real, silent gap: browsing history never accumulated on
    /// macOS at all — see `ARCHITECTURE.md` §3.8); mirrors
    /// `browser-linux-gtk3`'s `AppState::record_visit`.
    fn record_visit(&self, id: &str) {
        let core = self.core.borrow();
        let Some(page) = core.page(id) else { return };
        let url = page.current_url();
        let title = page.title.borrow().clone();
        drop(core);
        if let Err(err) = self.history.record_visit(&url, &title) {
            eprintln!("failed to record history visit: {err}");
        }
    }

    /// Drops the live engine/container for every id `PageManager` evicted —
    /// the actual resource reclamation `enforce_loaded_limit`'s bookkeeping
    /// alone doesn't perform (mirrors every other front end's
    /// `unload_engines`/equivalent).
    fn unload_engines(&self, ids: &[String]) {
        for id in ids {
            self.core.borrow_mut().take_engine(id);
            if let Some(container) = self.containers.borrow_mut().remove(id) {
                container.removeFromSuperview();
            }
        }
    }

    /// Rebuilds a page's container/engine if it was unloaded — mirrors
    /// `browser-windows-reactor`'s `ensure_engine_loaded`.
    fn ensure_engine_loaded(self: &Rc<Self>, id: &str) {
        let needs_engine = self.core.borrow().page(id).map(|p| p.engine.is_none()).unwrap_or(false);
        if !needs_engine {
            return;
        }
        let mtm = self.mtm();
        let last_url = self.core.borrow().page(id).map(|p| p.current_url()).unwrap_or_default();
        let url = if last_url.is_empty() { HOME_URL.to_string() } else { last_url };
        let container = NSView::initWithFrame(NSView::alloc(mtm), self.content_view.frame());
        self.content_view.addSubview(&container);
        // Weak — see the identical comment in `add_page`.
        let self_for_title = Rc::downgrade(self);
        let id_for_title = id.to_string();
        let self_for_new_window = Rc::downgrade(self);
        match WryEngine::new(
            &container,
            &url,
            &mut *self.web_context.borrow_mut(),
            move |title| {
                let Some(app) = self_for_title.upgrade() else { return };
                if let Some(page) = app.core.borrow_mut().page_mut(&id_for_title) {
                    *page.title.borrow_mut() = title;
                }
                app.record_visit(&id_for_title);
                if app.core.borrow().active_id() == id_for_title {
                    app.refresh_title_label();
                }
            },
            move |_new_window_url, target_configuration| {
                let app = self_for_new_window.upgrade()?;
                app.add_page_related(target_configuration).ok()
            },
        ) {
            Ok(engine) => {
                self.core.borrow_mut().install_engine(id, engine);
                self.containers.borrow_mut().insert(id.to_string(), container);
            }
            Err(err) => eprintln!("failed to reload page {id}: {err}"),
        }
    }

    /// Opens either the saved session's pages (if any) or `start_page` —
    /// this crate's single startup call site. Individual `add_page`
    /// failures are logged and skipped rather than aborting the whole
    /// restore (a URL that no longer resolves shouldn't cost the user
    /// every *other* restored tab).
    fn open_start_page_or_restored_session(self: &Rc<Self>, start_page: &str) {
        let session = Session::load(&self.profile);
        let plan = browser_chrome_core::resolve_restore_plan(&session, start_page);
        for url in &plan.urls {
            if let Err(err) = self.add_page(url) {
                eprintln!("failed to open restored page {url:?}: {err}");
            }
        }
        // `add_page` makes each newly-added page active in turn, so without
        // this the *last* URL in `plan.urls` would end up active regardless
        // of which one was actually active when the session was saved. The
        // id is copied out of `core`'s borrow into its own statement before
        // calling `set_active` (which needs its own borrow) rather than
        // held across it — otherwise this would panic on the `RefCell`'s
        // already-borrowed check.
        let active_page_id = plan.active_index.and_then(|idx| self.core.borrow().pages().get(idx).map(|p| p.id.clone()));
        if let Some(id) = active_page_id {
            self.set_active(&id);
        }
    }

    /// Snapshots the currently-open pages (URL + title, in `PageManager`'s
    /// own creation order) plus which one is active, for `windowWillClose:`
    /// to save before the app actually terminates.
    fn build_session(&self) -> Session {
        let core = self.core.borrow();
        let active_id = core.active_id();
        let active_index = core.pages().iter().position(|p| p.id == active_id);
        let pages = core.pages().iter().map(|p| SessionPage { url: p.current_url(), title: p.title.borrow().clone() }).collect();
        Session { pages, active_index }
    }

    /// The real "the whole app is closing" hook's save half — called from
    /// `windowWillClose:` (both the red-traffic-light button and
    /// `Action::Quit`'s `self.window.close()` route through it, since
    /// closing the window is what triggers that delegate method) before
    /// `NSApplication::terminate` actually exits the process.
    fn save_session(&self) {
        let session = self.build_session();
        if let Err(err) = session.save(&self.profile) {
            eprintln!("failed to save session: {err}");
        }
    }

    fn switch_to(self: &Rc<Self>, id: &str) {
        self.ensure_engine_loaded(id);
        self.set_active(id);
        self.close_all_overlays();
    }

    /// `Action::NextPage` (Ctrl+Tab/Ctrl+PageDown on gtk3 — this platform has
    /// no physical key recognition for either yet, see `ROADMAP.md`, but the
    /// dispatch itself is real, working code, not a stub). The id is copied
    /// out of `core`'s borrow before calling `switch_to` (which needs its
    /// own borrow) rather than held across it.
    fn switch_to_next_page(self: &Rc<Self>) {
        let id = self.core.borrow().next_page_id().map(|s| s.to_string());
        if let Some(id) = id {
            self.switch_to(&id);
        }
    }

    /// `Action::PreviousPage` (Ctrl+Shift+Tab/Ctrl+PageUp on gtk3) — same as
    /// `switch_to_next_page`, one position earlier.
    fn switch_to_previous_page(self: &Rc<Self>) {
        let id = self.core.borrow().previous_page_id().map(|s| s.to_string());
        if let Some(id) = id {
            self.switch_to(&id);
        }
    }

    fn set_active(&self, id: &str) {
        self.core.borrow_mut().set_active(id);
        for (page_id, container) in self.containers.borrow().iter() {
            container.setHidden(page_id != id);
        }
        self.refresh_title_label();
        self.refresh_bookmark_toggle_button();
        self.relayout();
    }

    fn close_page(self: &Rc<Self>, id: &str) {
        let was_active = self.core.borrow().active_id() == id;
        self.core.borrow_mut().remove(id);
        if let Some(container) = self.containers.borrow_mut().remove(id) {
            container.removeFromSuperview();
        }
        if was_active {
            let next_id = self.core.borrow().pages().first().map(|p| p.id.clone());
            match next_id {
                Some(nid) => self.set_active(&nid),
                None => {
                    let start_page = self.settings.borrow().start_page.clone();
                    if let Err(err) = self.add_page(&start_page) {
                        eprintln!("failed to open replacement page: {err}");
                    }
                }
            }
        }
        self.rebuild_switcher_rows();
    }

    /// Enter in the address bar: navigates the active page, unless the
    /// switcher is open, in which case the same widget is acting as its
    /// search box — same unified design as `browser-linux-gtk3`'s
    /// `connect_activate` handler (see that crate's comment for the full
    /// reasoning): an exactly-one open-page match switches to it, else an
    /// exactly-one history match opens that entry, else the typed text is
    /// resolved (URL or search) into a brand-new page.
    /// ⌘Enter (the platform mapping of `browser-linux-gtk3`'s Ctrl+Enter —
    /// see this module's doc comment on `ctrl` → ⌘) while the switcher is
    /// open always opens a brand-new page from the typed text, even when it
    /// matches an open page or history entry (which plain Enter would
    /// instead switch to/open) — the escape hatch for deliberately wanting a
    /// second page at the same URL. Mirrors
    /// `browser-linux-gtk3`'s `force_new_page_from_search`; dropped
    /// (silently, not as a deliberate scope cut) when this crate was first
    /// ported from that one — see `ARCHITECTURE.md` §3.3.
    fn force_new_page_from_search(self: &Rc<Self>, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let url = resolve_address_input(trimmed, &self.settings.borrow());
        if let Err(err) = self.add_page(&url) {
            eprintln!("failed to open new page: {err}");
        }
        self.close_all_overlays();
    }

    /// Whether ⌘ was held for the key event that triggered the control
    /// action currently being handled — `NSApplication.currentEvent` is the
    /// standard AppKit way to recover this from inside an action method
    /// (there's no argument carrying it directly, unlike a raw `keyDown:`
    /// override).
    fn command_key_held(&self) -> bool {
        NSApplication::sharedApplication(self.mtm())
            .currentEvent()
            .is_some_and(|event| event.modifierFlags().contains(NSEventModifierFlags::Command))
    }

    /// The address bar's Enter handler — filters/edits are its only roles
    /// now that it lives entirely inside the switcher panel (see the field
    /// doc on `AppState::address_bar`), so this no longer needs to branch
    /// on whether the switcher is open: it always is, by construction,
    /// whenever this widget is reachable at all.
    fn address_bar_activated(self: &Rc<Self>) {
        let text = self.address_bar.stringValue().to_string();
        if self.command_key_held() {
            self.force_new_page_from_search(&text);
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let matches = self.core.borrow().matching_ids(trimmed);
        match matches.as_slice() {
            [only] => self.switch_to(&only.clone()),
            _ => {
                let history_matches = self.history.search(trimmed, 2).unwrap_or_default();
                if let [only] = history_matches.as_slice() {
                    let url = only.url.clone();
                    if let Err(err) = self.add_page(&url) {
                        eprintln!("failed to open history entry: {err}");
                    }
                    self.close_all_overlays();
                } else {
                    let url = resolve_address_input(trimmed, &self.settings.borrow());
                    if let Err(err) = self.add_page(&url) {
                        eprintln!("failed to open new page: {err}");
                    }
                    self.close_all_overlays();
                }
            }
        }
    }

    /// The title chip's click handler — see `browser-linux-gtk3`'s
    /// `title_chip_clicked` for why it's guarded on `!is_switcher_open()`:
    /// the toolbar stays clickable even while the switcher is showing (the
    /// overlay only covers the content area below the toolbar), and
    /// re-clicking while it's already open must not clobber whatever the
    /// user already typed.
    fn open_switcher_editing_url_clicked(self: &Rc<Self>) {
        if !self.is_switcher_open() {
            self.open_switcher_editing_url();
        }
    }

    fn mtm(&self) -> MainThreadMarker {
        MainThreadMarker::new().expect("AppState is only ever touched from the main thread")
    }

    // ---- rebuilding overlay row lists ------------------------------------

    /// Rebuilds every switcher row from scratch — open pages matching the
    /// search box's current text (or all of them, if empty —
    /// `PageManager::matching_ids` already handles that), a trailing
    /// "+ New Page" row, then, only once there's a query, matching history
    /// entries not already open. Mirrors `browser-windows-reactor`'s
    /// `switcher_overlay` closely; see this module's doc comment for why
    /// it's a plain list here instead of a wrapping tile grid.
    fn rebuild_switcher_rows(&self) {
        let mtm = self.mtm();
        let query = self.address_bar.stringValue().to_string();
        let bookmarks = self.bookmarks.borrow();
        let rows = browser_chrome_core::build_switcher_rows(&self.core.borrow(), &self.history, Some(&bookmarks), &query);
        drop(bookmarks);

        clear_subviews(&self.switcher_rows_container);
        let width = self.switcher_rows_container.frame().size.width;
        for (idx, row) in rows.iter().enumerate() {
            use browser_chrome_core::SwitcherRow;
            let (label, sub) = match row {
                SwitcherRow::Open { title, domain, .. } => (title.clone(), domain.clone()),
                SwitcherRow::Add => ("+ New Page".to_string(), String::new()),
                // No per-variant visual styling here — this crate's switcher
                // rows have never had any (even `Open`'s palette `color` is
                // discarded above), so `History`/`Bookmark`/`Similar` all
                // render identically: title + domain text, the domain
                // already carrying a "· history"/"· bookmark"/"· similar"
                // suffix from `build_switcher_rows` itself.
                SwitcherRow::History { title, domain, .. }
                | SwitcherRow::Bookmark { title, domain, .. }
                | SwitcherRow::Similar { title, domain, .. } => (title.clone(), domain.clone()),
            };
            let text = if sub.is_empty() { label } else { format!("{label}\n{sub}") };
            let button = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(&text), None, None, mtm) };
            button.setTag(idx as isize);
            button.setFrame(NSRect::new(
                NSPoint::new(0.0, (rows.len() - 1 - idx) as f64 * ROW_HEIGHT),
                NSSize::new(width, ROW_HEIGHT - BUTTON_MARGIN),
            ));
            self.switcher_rows_container.addSubview(&button);
        }
        *self.switcher_rows.borrow_mut() = rows;
    }

    fn switcher_row_clicked(self: &Rc<Self>, idx: usize) {
        let rows = self.switcher_rows.borrow();
        let start_page = self.settings.borrow().start_page.clone();
        let Some(activation) = browser_chrome_core::activate_row(&rows, idx, &start_page) else { return };
        drop(rows);
        match activation {
            browser_chrome_core::SwitcherActivation::SwitchTo(id) => self.switch_to(&id),
            browser_chrome_core::SwitcherActivation::OpenNewPage(url) => {
                if let Err(err) = self.add_page(&url) {
                    eprintln!("failed to open page: {err}");
                }
                self.close_all_overlays();
            }
        }
    }

    /// Rebuilds the keybindings editor's rows — one per `Action::ALL`,
    /// showing its label, current chords as removable "×" buttons, and
    /// either an "Add binding" button or (while `listening_for ==
    /// Some(action)`) a text field to type the new chord in
    /// `"Cmd+Shift+P"` format. See `shortcuts::parse_chord`'s doc comment
    /// for why text entry rather than live key capture.
    fn rebuild_keybindings_rows(&self) {
        let mtm = self.mtm();
        clear_subviews(&self.keybindings_rows_container);
        let listening_for = self.listening_for.get();
        let actions = Action::ALL;
        for (row_idx, &action) in actions.iter().enumerate() {
            let y = (actions.len() - 1 - row_idx) as f64 * ROW_HEIGHT;
            let label = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str(action.label()), None, None, mtm)
            };
            label.setButtonType(NSButtonType::MomentaryLight);
            label.setEnabled(false);
            label.setFrame(NSRect::new(NSPoint::new(0.0, y), NSSize::new(200.0, ROW_HEIGHT - BUTTON_MARGIN)));
            self.keybindings_rows_container.addSubview(&label);

            let mut x = 210.0;
            let chords = self.keybindings.borrow().bindings_for(action).to_vec();
            for chord in &chords {
                let remove = unsafe {
                    NSButton::buttonWithTitle_target_action(&NSString::from_str(&format!("{chord} \u{d7}")), None, None, mtm)
                };
                remove.setTag(row_idx as isize);
                remove.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.keybindings_rows_container.addSubview(&remove);
                x += 94.0;
            }

            if listening_for == Some(action) {
                self.new_binding_field.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(140.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.new_binding_field.setHidden(false);
                self.keybindings_rows_container.addSubview(&self.new_binding_field);
                x += 144.0;
                let ok = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str("OK"), None, None, mtm) };
                ok.setTag(row_idx as isize);
                ok.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(40.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.keybindings_rows_container.addSubview(&ok);
            } else {
                let add = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str("Add binding"), None, None, mtm) };
                add.setTag(row_idx as isize);
                add.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(100.0, ROW_HEIGHT - BUTTON_MARGIN)));
                self.keybindings_rows_container.addSubview(&add);
            }
        }
        if listening_for.is_none() {
            self.new_binding_field.setHidden(true);
            self.new_binding_field.setStringValue(&NSString::from_str(""));
        }
    }

    fn keybinding_add_clicked(&self, action_idx: usize) {
        let Some(&action) = Action::ALL.get(action_idx) else { return };
        self.new_binding_field.setStringValue(&NSString::from_str(""));
        self.listening_for.set(Some(action));
        self.rebuild_keybindings_rows();
        self.window.makeFirstResponder(Some(&self.new_binding_field));
    }

    fn keybinding_remove_clicked(&self, action_idx: usize) {
        // Tag only identifies the row/action here — with possibly several
        // chords per action, the leftmost chord button removed first is an
        // acceptable simplification (real removal-by-exact-chord would need
        // per-chord tags, not just per-row).
        let Some(&action) = Action::ALL.get(action_idx) else { return };
        let mut chords = self.keybindings.borrow().bindings_for(action).to_vec();
        if !chords.is_empty() {
            chords.remove(0);
        }
        self.keybindings.borrow_mut().set_bindings(action, chords);
        if let Err(err) = self.keybindings.borrow().save(&self.profile) {
            eprintln!("failed to save keybindings: {err}");
        }
        self.rebuild_keybindings_rows();
        self.rebuild_menu_key_equivalents();
    }

    fn keybinding_commit(&self, action_idx: usize) {
        let Some(&action) = Action::ALL.get(action_idx) else { return };
        let text = self.new_binding_field.stringValue().to_string();
        if let Some(chord) = shortcuts::parse_chord(&text) {
            let mut chords = self.keybindings.borrow().bindings_for(action).to_vec();
            if !chords.contains(&chord) {
                chords.push(chord);
            }
            self.keybindings.borrow_mut().set_bindings(action, chords);
            if let Err(err) = self.keybindings.borrow().save(&self.profile) {
                eprintln!("failed to save keybindings: {err}");
            }
        }
        self.listening_for.set(None);
        self.rebuild_keybindings_rows();
        self.rebuild_menu_key_equivalents();
    }

    /// Rebuilds the profile list — existing profiles (from
    /// `list_profile_names()`, fresh every time this overlay opens), the
    /// current one marked and closing the picker instead of launching a
    /// duplicate process of itself.
    fn rebuild_profile_rows(&self) {
        let mtm = self.mtm();
        clear_subviews(&self.profile_rows_container);
        let width = self.profile_rows_container.frame().size.width;
        let current_profile = self.profile.name.clone();
        let names = list_profile_names();
        for (idx, name) in names.iter().enumerate() {
            let is_current = *name == current_profile;
            let label = if is_current { format!("{name} (current)") } else { name.clone() };
            let button = unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(&label), None, None, mtm) };
            button.setTag(idx as isize);
            button.setFrame(NSRect::new(
                NSPoint::new(0.0, (names.len() - 1 - idx) as f64 * ROW_HEIGHT),
                NSSize::new(width, ROW_HEIGHT - BUTTON_MARGIN),
            ));
            self.profile_rows_container.addSubview(&button);
        }
    }

    fn profile_row_clicked(self: &Rc<Self>, idx: usize) {
        let names = list_profile_names();
        let Some(name) = names.get(idx) else { return };
        if *name == self.profile.name {
            self.close_all_overlays();
            return;
        }
        if let Err(err) = launch_new_profile_process(name) {
            eprintln!("failed to launch a new process for profile {name:?}: {err}");
        } else {
            self.close_all_overlays();
        }
    }

    /// Rebuilds the app's main menu bar's key equivalents from the current
    /// `Keybindings` — called after every keybinding change, since
    /// `NSMenuItem`'s key equivalent (unlike reactor's per-render
    /// `KeyboardAccelerator` rebuild) is imperative, persistent state that
    /// has to be explicitly refreshed rather than being recomputed for free.
    fn rebuild_menu_key_equivalents(&self) {
        let Some(menu) = self.window.menu().or_else(|| NSApplication::sharedApplication(self.mtm()).mainMenu()) else { return };
        shortcuts::apply_key_equivalents(&menu, &self.keybindings.borrow());
    }
}

/// Current time as Unix seconds — used as a bookmark's `created_at` when
/// added. Mirrors `browser-linux-gtk3`'s identically-named free function.
fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn clear_subviews(view: &NSView) {
    for subview in view.subviews().iter() {
        subview.removeFromSuperview();
    }
}

struct AppDelegateIvars {
    state: RefCell<Option<Rc<AppState>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements we're violating —
    // AppDelegate doesn't override `dealloc`/`init` in a way that skips
    // superclass behavior.
    #[unsafe(super(NSObject))]
    // Every ivar here is only safe to touch from the main thread anyway, so
    // this object is main-thread-only rather than trying to make it
    // `Send`/`Sync`.
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    impl AppDelegate {
        #[unsafe(method(goBack:))]
        fn go_back(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.with_active(|e| e.go_back());
            }
        }

        #[unsafe(method(goForward:))]
        fn go_forward(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.with_active(|e| e.go_forward());
            }
        }

        #[unsafe(method(reloadPage:))]
        fn reload_page(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.with_active(|e| e.reload());
            }
        }

        #[unsafe(method(addressBarActivated:))]
        fn address_bar_activated(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.address_bar_activated();
            }
        }

        #[unsafe(method(toggleSwitcher:))]
        fn toggle_switcher(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_switcher();
            }
        }

        #[unsafe(method(openSwitcherAction:))]
        fn open_switcher_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.open_switcher();
            }
        }

        #[unsafe(method(openSwitcherEditingUrlAction:))]
        fn open_switcher_editing_url_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.open_switcher_editing_url_clicked();
            }
        }

        #[unsafe(method(openSwitcherEditingUrl:))]
        fn open_switcher_editing_url(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.open_switcher_editing_url();
            }
        }

        #[unsafe(method(closePageAction:))]
        fn close_page_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                let id = state.core.borrow().active_id().to_string();
                state.close_page(&id);
            }
        }

        #[unsafe(method(closeAnyOverlay:))]
        fn close_any_overlay(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.close_all_overlays();
            }
        }

        #[unsafe(method(openSettingsAction:))]
        fn open_settings_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_settings();
            }
        }

        #[unsafe(method(saveSettingsAction:))]
        fn save_settings_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.save_settings();
            }
        }

        #[unsafe(method(toggleUnlimitedAction:))]
        fn toggle_unlimited_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_unlimited();
            }
        }

        #[unsafe(method(openProfilePickerAction:))]
        fn open_profile_picker_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_profile_picker();
            }
        }

        #[unsafe(method(createAndOpenProfileAction:))]
        fn create_and_open_profile_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.create_and_open_profile();
            }
        }

        #[unsafe(method(switcherRowClicked:))]
        fn switcher_row_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.switcher_row_clicked(idx);
                }
            }
        }

        #[unsafe(method(keybindingAddClicked:))]
        fn keybinding_add_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.keybinding_add_clicked(idx);
                }
            }
        }

        #[unsafe(method(keybindingRemoveClicked:))]
        fn keybinding_remove_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.keybinding_remove_clicked(idx);
                }
            }
        }

        #[unsafe(method(keybindingCommit:))]
        fn keybinding_commit(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.keybinding_commit(idx);
                }
            }
        }

        #[unsafe(method(profileRowClicked:))]
        fn profile_row_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.profile_row_clicked(idx);
                }
            }
        }

        #[unsafe(method(openPasswordsAction:))]
        fn open_passwords_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_passwords();
            }
        }

        #[unsafe(method(unlockVaultAction:))]
        fn unlock_vault_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.unlock_vault_clicked();
            }
        }

        #[unsafe(method(unlockBitwardenAction:))]
        fn unlock_bitwarden_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.unlock_bitwarden_clicked();
            }
        }

        #[unsafe(method(submitLoginAction:))]
        fn submit_login_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.submit_login_from_fields();
            }
        }

        #[unsafe(method(cancelEditingLoginAction:))]
        fn cancel_editing_login_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.cancel_editing_login();
            }
        }

        #[unsafe(method(passwordRowFillClicked:))]
        fn password_row_fill_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.password_row_fill_clicked(idx);
                }
            }
        }

        #[unsafe(method(passwordRowCopyClicked:))]
        fn password_row_copy_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.password_row_copy_clicked(idx);
                }
            }
        }

        #[unsafe(method(passwordRowEditClicked:))]
        fn password_row_edit_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.password_row_edit_clicked(idx);
                }
            }
        }

        #[unsafe(method(passwordRowDeleteClicked:))]
        fn password_row_delete_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.password_row_delete_clicked(idx);
                }
            }
        }

        #[unsafe(method(toggleBookmarkAction:))]
        fn toggle_bookmark_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_bookmark_for_active();
            }
        }

        #[unsafe(method(openBookmarksAction:))]
        fn open_bookmarks_action(&self, _sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.toggle_bookmarks();
            }
        }

        #[unsafe(method(bookmarkRowOpenClicked:))]
        fn bookmark_row_open_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.bookmark_row_open_clicked(idx);
                }
            }
        }

        #[unsafe(method(bookmarkRowRemoveClicked:))]
        fn bookmark_row_remove_clicked(&self, sender: Option<&AnyObject>) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                if let Some(idx) = sender_tag(sender) {
                    state.bookmark_row_remove_clicked(idx);
                }
            }
        }

        #[unsafe(method(dispatchAction:))]
        fn dispatch_action(&self, sender: Option<&AnyObject>) {
            let Some(state) = self.ivars().state.borrow().clone() else { return };
            let Some(idx) = sender_tag(sender) else { return };
            let Some(&action) = Action::ALL.get(idx) else { return };
            match action {
                Action::OpenSwitcher => state.open_switcher(),
                Action::EditUrl => state.open_switcher_editing_url(),
                Action::ClosePage => {
                    let id = state.core.borrow().active_id().to_string();
                    state.close_page(&id);
                }
                Action::Reload => state.with_active(|e| e.reload()),
                Action::GoBack => state.with_active(|e| e.go_back()),
                Action::GoForward => state.with_active(|e| e.go_forward()),
                Action::OpenSettings => state.open_settings(),
                Action::OpenProfilePicker => state.open_profile_picker(),
                Action::OpenPasswords => state.open_passwords(),
                Action::ToggleBookmark => state.toggle_bookmark_for_active(),
                Action::OpenBookmarks => state.open_bookmarks(),
                Action::NextPage => state.switch_to_next_page(),
                Action::PreviousPage => state.switch_to_previous_page(),
                // Routes through the same `windowWillClose:` delegate
                // method the red-traffic-light button already uses (see
                // its doc comment) — one save-then-quit implementation,
                // not two.
                Action::Quit => state.window.close(),
                // Reader mode isn't implemented on this front end either yet
                // — matches browser-windows-winui/reactor's scope.
                Action::ToggleReaderMode => {}
            }
        }

        // The title chip's hover state — `NSTrackingArea`'s `owner` just
        // needs to respond to these two selectors via ordinary Objective-C
        // message dispatch (confirmed: it doesn't require `AppDelegate` to
        // formally subclass/implement `NSResponder` in the Rust type
        // system), so these are plain inherent methods here rather than a
        // new `unsafe impl NSResponder for AppDelegate` block or a
        // dedicated `NSView` subclass just for this one widget.
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                set_title_chip_hovered(&state.title_chip, true);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                set_title_chip_hovered(&state.title_chip, false);
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

        // The real "the whole app is closing" hook — both the window's own
        // red-traffic-light button and `Action::Quit` (via
        // `self.window.close()`) end up here, so this is the one place
        // that needs to save the session before the process actually
        // exits, rather than each route separately implementing
        // save-then-quit.
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.save_session();
            }
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    }

    // Gates the "Close Overlay" (Escape) menu item built in
    // `shortcuts::build_menu` — see its own comment for why: an enabled
    // menu item's key equivalent always fires and swallows the event, which
    // would otherwise steal Escape from web page content (in-page
    // fullscreen, JS modals) whenever no overlay is open. AppKit calls this
    // as part of key-equivalent dispatch itself, not just for visible-menu
    // display, so returning `false` here really does let the event fall
    // through to the responder chain instead.
    unsafe impl NSMenuItemValidation for AppDelegate {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, menu_item: &objc2_app_kit::NSMenuItem) -> bool {
            if menu_item.action() == Some(sel!(closeAnyOverlay:)) {
                return self.ivars().state.borrow().as_ref().is_some_and(|s| s.overlay.get() != Overlay::None).into();
            }
            true.into()
        }
    }
);

/// Reads an `NSButton`'s `tag` (set via `setTag` when the row/button was
/// created) back out as a `usize` index — AppKit's `target`/`action`
/// dispatch hands back only the sender itself, so this is how a shared
/// handler method learns *which* row fired it.
fn sender_tag(sender: Option<&AnyObject>) -> Option<usize> {
    let sender = sender?;
    let button: &NSButton = sender.downcast_ref()?;
    usize::try_from(button.tag()).ok()
}

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars { state: RefCell::new(None) });
        unsafe { msg_send![super(this), init] }
    }
}

/// Owns the delegate (and, through its ivars, the whole window/webview
/// tree) for as long as the app is meant to run — dropping this would tear
/// the app down, so `main.rs` is expected to hold it until `run()` returns.
pub struct App {
    mtm: MainThreadMarker,
    _delegate: Retained<AppDelegate>,
}

impl App {
    /// Hands control to `NSApplication`'s run loop — same role as GTK's
    /// `gtk::main()`, Win32's message loop, or WinUI 3's
    /// `Application::Start`. Doesn't return until the app quits (see
    /// `AppDelegate::window_will_close`, the only quit path this wires up).
    pub fn run(&self) {
        NSApplication::sharedApplication(self.mtm).run();
    }
}

/// A borderless-style toolbar/overlay button — small helper so every call
/// site doesn't repeat the same three-call dance.
fn make_button(title: &str, target: &AnyObject, action: objc2::runtime::Sel, mtm: MainThreadMarker) -> Retained<NSButton> {
    unsafe { NSButton::buttonWithTitle_target_action(&NSString::from_str(title), Some(target), Some(action), mtm) }
}

/// Toggles the toolbar's title chip between its at-rest look (a subtle
/// border, no fill) and hover-looks-like-a-text-input (filled with
/// `textBackgroundColor` — literally the system color real text inputs use
/// for their background, not a hand-picked approximation). Both colors are
/// semantic/dynamic `NSColor`s, so they already adapt to light/dark mode
/// automatically — mirrors `browser-linux-gtk3`'s `@theme_fg_color`/
/// `@theme_base_color` GTK CSS variables serving the same purpose there.
fn set_title_chip_hovered(chip: &NSBox, hovered: bool) {
    if hovered {
        chip.setFillColor(&NSColor::textBackgroundColor());
        chip.setBorderColor(&NSColor::labelColor());
    } else {
        chip.setFillColor(&NSColor::clearColor());
        chip.setBorderColor(&NSColor::separatorColor());
    }
}

fn make_overlay_container(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
    let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
    view.setHidden(true);
    view
}

/// The three pieces every overlay's chrome adds on top of its own content —
/// see `make_overlay_chrome`. Kept (not discarded after construction) so
/// `AppState::relayout` can reposition them on window resize, the same
/// reason every other overlay sub-widget here is a stored field rather than
/// a local built once and forgotten.
struct OverlayChrome {
    backdrop: Retained<NSBox>,
    close_button: Retained<NSButton>,
    esc_hint: Retained<NSTextField>,
}

/// Shared chrome for every full-screen overlay (switcher/settings/profile/
/// bookmarks/passwords): a dim backdrop filling `frame`, a close (×) button
/// pinned to the top-right corner, and a "Press Esc to close" hint next to
/// it — the same three pieces `browser-linux-gtk3`'s `build_overlay_chrome`
/// builds (matching its rgba(20,20,18,0.88) backdrop color, a free
/// visual-consistency touch, not shared code — different toolkits). Every
/// overlay here closes through the one shared `closeAnyOverlay:` selector
/// already used by every Cancel/Close button (see
/// `AppState::close_all_overlays`), so it's hardcoded here rather than
/// threaded through as a parameter.
///
/// Caller adds `backdrop` as the *first* subview of the overlay's own
/// container view (so whatever real content gets added after paints on top
/// of it) and `close_button`/`esc_hint` as the *last* two (so they stay on
/// top of everything else in that overlay).
fn make_overlay_chrome(mtm: MainThreadMarker, frame: NSRect, delegate: &AnyObject) -> OverlayChrome {
    let backdrop = NSBox::initWithFrame(NSBox::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
    backdrop.setBoxType(NSBoxType::Custom);
    backdrop.setTitlePosition(NSTitlePosition::NoTitle);
    backdrop.setFillColor(&NSColor::colorWithSRGBRed_green_blue_alpha(20.0 / 255.0, 20.0 / 255.0, 18.0 / 255.0, 0.88));

    let close_button = make_button("\u{2715}", delegate, sel!(closeAnyOverlay:), mtm);
    close_button.setFrame(NSRect::new(
        NSPoint::new(frame.size.width - OVERLAY_MARGIN - CLOSE_BUTTON_SIZE, frame.size.height - OVERLAY_MARGIN - CLOSE_BUTTON_SIZE),
        NSSize::new(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE),
    ));

    let esc_hint = make_text_field(mtm, "Press Esc to close");
    esc_hint.setEditable(false);
    esc_hint.setBordered(false);
    esc_hint.setSelectable(false);
    esc_hint.setAlignment(NSTextAlignment::Right);
    esc_hint.setFrame(NSRect::new(
        NSPoint::new(frame.size.width - OVERLAY_MARGIN - CLOSE_BUTTON_SIZE - HINT_WIDTH - 6.0, frame.size.height - OVERLAY_MARGIN - CLOSE_BUTTON_SIZE),
        NSSize::new(HINT_WIDTH, CLOSE_BUTTON_SIZE),
    ));

    OverlayChrome { backdrop, close_button, esc_hint }
}

/// Recomputes an `OverlayChrome`'s three frames from the overlay's current
/// content frame — `AppState::relayout`'s per-chrome counterpart to the
/// construction-time math in `make_overlay_chrome` (kept in sync by hand,
/// same as every other manually-positioned widget in this crate).
fn relayout_overlay_chrome(chrome: &OverlayChrome, frame: NSRect) {
    chrome.backdrop.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
    chrome.close_button.setFrame(NSRect::new(
        NSPoint::new(frame.size.width - OVERLAY_MARGIN - CLOSE_BUTTON_SIZE, frame.size.height - OVERLAY_MARGIN - CLOSE_BUTTON_SIZE),
        NSSize::new(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE),
    ));
    chrome.esc_hint.setFrame(NSRect::new(
        NSPoint::new(frame.size.width - OVERLAY_MARGIN - CLOSE_BUTTON_SIZE - HINT_WIDTH - 6.0, frame.size.height - OVERLAY_MARGIN - CLOSE_BUTTON_SIZE),
        NSSize::new(HINT_WIDTH, CLOSE_BUTTON_SIZE),
    ));
}

fn make_text_field(mtm: MainThreadMarker, initial: &str) -> Retained<NSTextField> {
    NSTextField::textFieldWithString(&NSString::from_str(initial), mtm)
}

/// Opens (or creates) `profile`'s `HistoryStore`. The plain, unencrypted
/// fast path (`!setup_passphrase && !profile.has_passphrase()`) is
/// unchanged from before this profile had passphrase support at all.
/// Otherwise collects the passphrase via a synchronous `NSAlert` +
/// `NSSecureTextField` accessory view, run with `runModal()` — a blocking
/// call designed exactly for "ask one synchronous question, get an answer,
/// continue," which (unlike `gtk::main()`) can run before
/// `NSApplication::run()` has been entered, so this needs no second
/// `NSWindow` the way `browser-linux-gtk3`'s `show_passphrase_prompt` does
/// (see this module's doc comment for why `run_chooser`, this crate's only
/// second-window precedent, doesn't fit this shape). Cancelling exits the
/// whole process (`std::process::exit(0)`, the same convention
/// `run_chooser` already uses) — there's no main window yet to fall back
/// to. Returns the passphrase actually used, if any, so the caller can
/// cross-wire it into `AppState::session_passphrase` once `AppState` is
/// constructed, mirroring gtk3's `note_unlocked_with_passphrase`.
fn open_history(profile: &Profile, setup_passphrase: bool, mtm: MainThreadMarker) -> anyhow::Result<(HistoryStore, Option<String>)> {
    if !setup_passphrase && !profile.has_passphrase() {
        return Ok((HistoryStore::open(profile)?, None));
    }

    NSApplication::sharedApplication(mtm).setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let mut error_message: Option<String> = None;
    loop {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(if setup_passphrase {
            "Choose a passphrase to encrypt this profile's history."
        } else {
            "This profile's history is passphrase-protected."
        }));
        if let Some(message) = &error_message {
            alert.setInformativeText(&NSString::from_str(message));
        }
        let field = NSSecureTextField::new(mtm);
        field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 24.0)));
        alert.setAccessoryView(Some(&field));
        alert.addButtonWithTitle(&NSString::from_str(if setup_passphrase { "Set Up" } else { "Unlock" }));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));

        if alert.runModal() != NSAlertFirstButtonReturn {
            std::process::exit(0);
        }
        let passphrase = field.stringValue().to_string();
        if passphrase.is_empty() {
            error_message = Some("Passphrase can't be empty.".to_string());
            continue;
        }
        match HistoryStore::open_encrypted(profile, &passphrase) {
            Ok(store) => {
                if setup_passphrase {
                    if let Err(err) = profile.enable_passphrase() {
                        eprintln!("failed to mark profile as passphrase-protected: {err}");
                    }
                }
                return Ok((store, Some(passphrase)));
            }
            Err(_) => {
                error_message = Some("Couldn't open this profile with that passphrase. Try again.".to_string());
            }
        }
    }
}

/// Builds the window, toolbar, overlay panels, and first page (loaded to
/// `settings.start_page`), wires the app's `NSMenu`, and returns an [`App`]
/// ready to `run()`. `setup_passphrase` mirrors gtk3's `--setup-passphrase`
/// flag (see `resolve_passphrase_setup_requested`'s doc comment for why
/// it's a flag rather than the passphrase itself ever crossing a process
/// boundary via argv).
pub fn build_window_and_app(profile: Profile, setup_passphrase: bool) -> anyhow::Result<App> {
    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("build_window_and_app must be called from the main thread"))?;
    let settings = Settings::load(&profile);
    let (history, history_passphrase) = open_history(&profile, setup_passphrase, mtm)?;
    let keybindings = Keybindings::load(&profile);
    let bookmarks = Bookmarks::load(&profile);
    let initial_url = settings.start_page.clone();

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let delegate = AppDelegate::new(mtm);

    let window_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 700.0));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::Resizable;
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), window_rect, style, NSBackingStoreType::Buffered, false) };
    window.setTitle(&NSString::from_str(APP_TITLE));
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let content_root = window.contentView().ok_or_else(|| anyhow::anyhow!("NSWindow has no content view"))?;

    let toolbar_view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, window_rect.size.height - TOOLBAR_HEIGHT), NSSize::new(window_rect.size.width, TOOLBAR_HEIGHT)));
    content_root.addSubview(&toolbar_view);

    let back_button = make_button("\u{2190}", &delegate, sel!(goBack:), mtm);
    back_button.setFrame(NSRect::new(NSPoint::new(BUTTON_MARGIN, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&back_button);

    let forward_button = make_button("\u{2192}", &delegate, sel!(goForward:), mtm);
    forward_button.setFrame(NSRect::new(NSPoint::new(2.0 * BUTTON_MARGIN + BUTTON_WIDTH, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&forward_button);

    let reload_button = make_button("\u{21BB}", &delegate, sel!(reloadPage:), mtm);
    reload_button.setFrame(NSRect::new(NSPoint::new(3.0 * BUTTON_MARGIN + 2.0 * BUTTON_WIDTH, BUTTON_MARGIN), NSSize::new(BUTTON_WIDTH, TOOLBAR_HEIGHT - 2.0 * BUTTON_MARGIN)));
    toolbar_view.addSubview(&reload_button);

    // A clickable "title chip", not a text field — shows the active page's
    // title (see `refresh_title_label`), styled (`title_chip`'s `NSBox`
    // border/fill, toggled between at-rest and hover-looks-like-an-input by
    // `mouseEntered:`/`mouseExited:` on `AppDelegate`) to hint that clicking
    // it opens the switcher in URL-editing mode. Three layered pieces: the
    // `NSBox` for the border/background, a non-editable `NSTextField` for
    // the text, and a borderless `NSButton` on top purely for click
    // detection — reusing `make_button`, this crate's existing click
    // primitive, rather than inventing gesture-recognizer plumbing. The
    // real editable text entry (`address_bar`) now lives entirely inside
    // the switcher overlay — see "---- switcher overlay ----" below.
    let title_chip = NSBox::initWithFrame(NSBox::alloc(mtm), NSRect::default());
    title_chip.setBoxType(NSBoxType::Custom);
    title_chip.setTitlePosition(NSTitlePosition::NoTitle);
    title_chip.setBorderWidth(1.0);
    title_chip.setCornerRadius(6.0);
    set_title_chip_hovered(&title_chip, false);
    toolbar_view.addSubview(&title_chip);
    // `InVisibleRect` tracks `title_chip`'s bounds automatically as it
    // moves/resizes (`relayout()` on window resize) — no manual remove/
    // re-add needed on every layout pass, unlike a fixed-rect tracking
    // area. Owner is the delegate itself (an `NSTrackingArea`'s owner just
    // needs to respond to `mouseEntered:`/`mouseExited:` via ordinary
    // Objective-C message dispatch — it doesn't have to be the view being
    // tracked), so no new `NSView` subclass is needed just for hover.
    let title_chip_tracking = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::default(),
            NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect,
            Some(&delegate),
            None,
        )
    };
    title_chip.addTrackingArea(&title_chip_tracking);
    let title_label = make_text_field(mtm, "New Page");
    title_label.setEditable(false);
    title_label.setBordered(false);
    title_label.setSelectable(false);
    title_label.setDrawsBackground(false);
    title_label.setAlignment(NSTextAlignment::Center);
    toolbar_view.addSubview(&title_label);
    let title_chip_button = make_button("", &delegate, sel!(openSwitcherEditingUrlAction:), mtm);
    title_chip_button.setBordered(false);
    toolbar_view.addSubview(&title_chip_button);

    let switcher_button = make_button("\u{229e}", &delegate, sel!(toggleSwitcher:), mtm);
    toolbar_view.addSubview(&switcher_button);
    let settings_button = make_button("\u{2699}", &delegate, sel!(openSettingsAction:), mtm);
    toolbar_view.addSubview(&settings_button);
    let profile_button = make_button("\u{1f464}", &delegate, sel!(openProfilePickerAction:), mtm);
    toolbar_view.addSubview(&profile_button);
    let passwords_button = make_button("\u{1f511}", &delegate, sel!(openPasswordsAction:), mtm);
    toolbar_view.addSubview(&passwords_button);
    let bookmark_toggle_button = make_button("\u{2606}", &delegate, sel!(toggleBookmarkAction:), mtm);
    toolbar_view.addSubview(&bookmark_toggle_button);
    let bookmarks_button = make_button("\u{1f516}", &delegate, sel!(openBookmarksAction:), mtm);
    toolbar_view.addSubview(&bookmarks_button);

    let content_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(window_rect.size.width, window_rect.size.height - TOOLBAR_HEIGHT));
    let content_view = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
    content_root.addSubview(&content_view);

    // ---- switcher overlay ----
    let switcher_view = make_overlay_container(mtm, content_frame);
    content_view.addSubview(&switcher_view);
    let switcher_chrome = make_overlay_chrome(mtm, content_frame, &delegate);
    switcher_view.addSubview(&switcher_chrome.backdrop);
    // The real, editable text entry — lives entirely inside the switcher now
    // (see the toolbar's `title_chip` above). Doubles as the switcher's
    // search box (filter open pages/history) and, when opened via the
    // title chip's click, the URL editor for the active page — one widget
    // for both roles, same as before, just relocated out of the toolbar.
    let address_bar = make_text_field(mtm, "");
    address_bar.setFrame(NSRect::new(
        NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - ROW_HEIGHT),
        NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, ROW_HEIGHT - BUTTON_MARGIN),
    ));
    unsafe {
        address_bar.setTarget(Some(&*delegate));
        address_bar.setAction(Some(sel!(addressBarActivated:)));
    }
    switcher_view.addSubview(&address_bar);
    let switcher_rows_container = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN), NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, content_frame.size.height - 2.0 * OVERLAY_MARGIN - ROW_HEIGHT)));
    switcher_view.addSubview(&switcher_rows_container);
    // Row clicks route through the shared `switcherRowClicked:` selector —
    // set on each row button individually in `rebuild_switcher_rows`, not
    // here (there's nothing to attach it to yet).
    switcher_view.addSubview(&switcher_chrome.close_button);
    switcher_view.addSubview(&switcher_chrome.esc_hint);

    // ---- settings overlay ----
    let settings_view = make_overlay_container(mtm, content_frame);
    content_view.addSubview(&settings_view);
    let settings_chrome = make_overlay_chrome(mtm, content_frame, &delegate);
    settings_view.addSubview(&settings_chrome.backdrop);
    let start_page_field = make_text_field(mtm, &settings.start_page);
    start_page_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&start_page_field);

    let unlimited_checkbox = make_button("Unlimited loaded pages", &delegate, sel!(toggleUnlimitedAction:), mtm);
    unlimited_checkbox.setButtonType(NSButtonType::Switch);
    unlimited_checkbox.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - 2.0 * ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&unlimited_checkbox);

    let limit_field = make_text_field(mtm, "");
    limit_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - 3.0 * ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&limit_field);

    // No live-toggle action needed (unlike `unlimited_checkbox`, which
    // enables/disables a sibling field) — this is only ever read from in
    // `save_settings`, so a plain constructor is enough, not `make_button`.
    let bitwarden_checkbox = NSButton::new(mtm);
    bitwarden_checkbox.setTitle(&NSString::from_str("Enable Bitwarden (via bw serve)"));
    bitwarden_checkbox.setButtonType(NSButtonType::Switch);
    bitwarden_checkbox.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - 4.0 * ROW_HEIGHT), NSSize::new(220.0, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&bitwarden_checkbox);
    let bitwarden_url_field = make_text_field(mtm, "");
    bitwarden_url_field.setPlaceholderString(Some(&NSString::from_str("http://127.0.0.1:8087")));
    bitwarden_url_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 224.0, content_frame.size.height - OVERLAY_MARGIN - 4.0 * ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH - 224.0, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&bitwarden_url_field);

    // Reuses the exact widget/pattern `passwords_destination_popup` already
    // proved out in this crate, rather than introducing `NSButtonType::Radio`
    // (no precedent here) or `NSSegmentedControl` (a new class) — see this
    // module's doc comment for the theme design rationale.
    let theme_popup = NSPopUpButton::new(mtm);
    theme_popup.addItemWithTitle(&NSString::from_str("Light"));
    theme_popup.addItemWithTitle(&NSString::from_str("Dark"));
    theme_popup.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, content_frame.size.height - OVERLAY_MARGIN - 5.0 * ROW_HEIGHT), NSSize::new(160.0, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&theme_popup);

    let keybindings_rows_container = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + ROW_HEIGHT), NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, content_frame.size.height - 6.0 * ROW_HEIGHT - 2.0 * OVERLAY_MARGIN)),
    );
    settings_view.addSubview(&keybindings_rows_container);
    let new_binding_field = make_text_field(mtm, "");
    new_binding_field.setPlaceholderString(Some(&NSString::from_str("e.g. Cmd+Shift+P")));
    unsafe {
        new_binding_field.setTarget(Some(&*delegate));
        new_binding_field.setAction(Some(sel!(keybindingCommit:)));
    }
    new_binding_field.setHidden(true);

    let cancel_button = make_button("Cancel", &delegate, sel!(closeAnyOverlay:), mtm);
    cancel_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&cancel_button);
    let save_button = make_button("Save", &delegate, sel!(saveSettingsAction:), mtm);
    save_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 94.0, OVERLAY_MARGIN), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    settings_view.addSubview(&save_button);
    settings_view.addSubview(&settings_chrome.close_button);
    settings_view.addSubview(&settings_chrome.esc_hint);

    // ---- profile overlay ----
    let profile_view = make_overlay_container(mtm, content_frame);
    content_view.addSubview(&profile_view);
    let profile_chrome = make_overlay_chrome(mtm, content_frame, &delegate);
    profile_view.addSubview(&profile_chrome.backdrop);
    let profile_rows_container = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + 2.0 * ROW_HEIGHT), NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, content_frame.size.height - 4.0 * ROW_HEIGHT)),
    );
    profile_view.addSubview(&profile_rows_container);
    let new_profile_field = make_text_field(mtm, "");
    new_profile_field.setPlaceholderString(Some(&NSString::from_str("New profile name\u{2026}")));
    new_profile_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + ROW_HEIGHT), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    profile_view.addSubview(&new_profile_field);
    let profile_cancel = make_button("Cancel", &delegate, sel!(closeAnyOverlay:), mtm);
    profile_cancel.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    profile_view.addSubview(&profile_cancel);
    let profile_create = make_button("Create & Open", &delegate, sel!(createAndOpenProfileAction:), mtm);
    profile_create.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 94.0, OVERLAY_MARGIN), NSSize::new(140.0, ROW_HEIGHT - BUTTON_MARGIN)));
    profile_view.addSubview(&profile_create);
    // Encrypts the new profile's history from the start (via
    // `launch_new_encrypted_profile_process`'s `--setup-passphrase`) — no
    // live-toggle action needed, same reasoning as `bitwarden_checkbox`.
    let encrypted_checkbox = NSButton::new(mtm);
    encrypted_checkbox.setTitle(&NSString::from_str("Encrypted (history)"));
    encrypted_checkbox.setButtonType(NSButtonType::Switch);
    encrypted_checkbox.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 238.0, OVERLAY_MARGIN), NSSize::new(180.0, ROW_HEIGHT - BUTTON_MARGIN)));
    profile_view.addSubview(&encrypted_checkbox);
    profile_view.addSubview(&profile_chrome.close_button);
    profile_view.addSubview(&profile_chrome.esc_hint);

    // ---- password manager overlay ----
    // Rows 1-4 (from the top) are shared, mutually-exclusive coordinates:
    // the locked/setup sub-group's fields and the add/edit form's fields
    // occupy the exact same positions, only one set ever visible at once
    // (see `rebuild_passwords_view`) — no separate popup window, this
    // crate's only precedent for a second `NSWindow` (`run_chooser`) is a
    // spawn-and-exit standalone mini-app, not a modal that hands back to a
    // running main window (see this module's doc comment on that).
    let passwords_view = make_overlay_container(mtm, content_frame);
    content_view.addSubview(&passwords_view);
    let passwords_chrome = make_overlay_chrome(mtm, content_frame, &delegate);
    passwords_view.addSubview(&passwords_chrome.backdrop);

    let row_y = |n: f64| content_frame.size.height - OVERLAY_MARGIN - n * ROW_HEIGHT;

    let passwords_unlock_label = make_text_field(mtm, "");
    passwords_unlock_label.setEditable(false);
    passwords_unlock_label.setBordered(false);
    passwords_unlock_label.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(1.0)), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_unlock_label);
    let passwords_unlock_field = NSSecureTextField::new(mtm);
    passwords_unlock_field.setPlaceholderString(Some(&NSString::from_str("Passphrase")));
    unsafe {
        passwords_unlock_field.setTarget(Some(&*delegate));
        passwords_unlock_field.setAction(Some(sel!(unlockVaultAction:)));
    }
    passwords_unlock_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(2.0)), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_unlock_field);
    let passwords_unlock_button = make_button("Unlock", &delegate, sel!(unlockVaultAction:), mtm);
    passwords_unlock_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(3.0)), NSSize::new(120.0, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_unlock_button);
    let passwords_unlock_error_label = make_text_field(mtm, "");
    passwords_unlock_error_label.setEditable(false);
    passwords_unlock_error_label.setBordered(false);
    passwords_unlock_error_label.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(4.0)), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_unlock_error_label);

    let passwords_site_field = make_text_field(mtm, "");
    passwords_site_field.setPlaceholderString(Some(&NSString::from_str("Site (e.g. https://example.com)")));
    passwords_site_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(1.0)), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_site_field);
    let passwords_username_field = make_text_field(mtm, "");
    passwords_username_field.setPlaceholderString(Some(&NSString::from_str("Username")));
    passwords_username_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(2.0)), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_username_field);
    let passwords_password_field = NSSecureTextField::new(mtm);
    passwords_password_field.setPlaceholderString(Some(&NSString::from_str("Password")));
    unsafe {
        passwords_password_field.setTarget(Some(&*delegate));
        passwords_password_field.setAction(Some(sel!(submitLoginAction:)));
    }
    passwords_password_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(3.0)), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_password_field);
    let passwords_notes_field = make_text_field(mtm, "");
    passwords_notes_field.setPlaceholderString(Some(&NSString::from_str("Notes (optional)")));
    passwords_notes_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(4.0)), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_notes_field);

    // Read via `titleOfSelectedItem()` only when the submit button is
    // clicked (see `submit_login_from_fields`) — no target/action needed on
    // selection change itself.
    let passwords_destination_popup = NSPopUpButton::new(mtm);
    passwords_destination_popup.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(5.0)), NSSize::new(160.0, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_destination_popup);
    let passwords_submit_button = make_button("Add", &delegate, sel!(submitLoginAction:), mtm);
    passwords_submit_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 164.0, row_y(5.0)), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_submit_button);
    let passwords_cancel_edit_button = make_button("Cancel edit", &delegate, sel!(cancelEditingLoginAction:), mtm);
    passwords_cancel_edit_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 258.0, row_y(5.0)), NSSize::new(110.0, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_cancel_edit_button.setHidden(true);
    passwords_view.addSubview(&passwords_cancel_edit_button);

    let passwords_error_label = make_text_field(mtm, "");
    passwords_error_label.setEditable(false);
    passwords_error_label.setBordered(false);
    passwords_error_label.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, row_y(6.0)), NSSize::new(OVERLAY_WIDTH, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_error_label);

    let passwords_rows_container = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(
            NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + 2.0 * ROW_HEIGHT),
            NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, (row_y(7.0) - OVERLAY_MARGIN - 2.0 * ROW_HEIGHT).max(0.0)),
        ),
    );
    passwords_view.addSubview(&passwords_rows_container);

    // Bitwarden's own inline unlock — fixed position (see this module's
    // `bitwarden_unlock_field` doc comment for why it isn't part of the
    // dynamic row list), just above the Close button.
    let bitwarden_unlock_field = NSSecureTextField::new(mtm);
    bitwarden_unlock_field.setPlaceholderString(Some(&NSString::from_str("Bitwarden master password")));
    unsafe {
        bitwarden_unlock_field.setTarget(Some(&*delegate));
        bitwarden_unlock_field.setAction(Some(sel!(unlockBitwardenAction:)));
    }
    bitwarden_unlock_field.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + ROW_HEIGHT), NSSize::new(260.0, ROW_HEIGHT - BUTTON_MARGIN)));
    bitwarden_unlock_field.setHidden(true);
    passwords_view.addSubview(&bitwarden_unlock_field);
    let bitwarden_unlock_button = make_button("Unlock Bitwarden", &delegate, sel!(unlockBitwardenAction:), mtm);
    bitwarden_unlock_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN + 264.0, OVERLAY_MARGIN + ROW_HEIGHT), NSSize::new(140.0, ROW_HEIGHT - BUTTON_MARGIN)));
    bitwarden_unlock_button.setHidden(true);
    passwords_view.addSubview(&bitwarden_unlock_button);

    let passwords_close_button = make_button("Close", &delegate, sel!(closeAnyOverlay:), mtm);
    passwords_close_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    passwords_view.addSubview(&passwords_close_button);
    passwords_view.addSubview(&passwords_chrome.close_button);
    passwords_view.addSubview(&passwords_chrome.esc_hint);

    // ---- bookmarks overlay ----
    let bookmarks_view = make_overlay_container(mtm, content_frame);
    content_view.addSubview(&bookmarks_view);
    let bookmarks_chrome = make_overlay_chrome(mtm, content_frame, &delegate);
    bookmarks_view.addSubview(&bookmarks_chrome.backdrop);
    let bookmarks_rows_container = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN + ROW_HEIGHT), NSSize::new(content_frame.size.width - 2.0 * OVERLAY_MARGIN, content_frame.size.height - ROW_HEIGHT - 2.0 * OVERLAY_MARGIN)),
    );
    bookmarks_view.addSubview(&bookmarks_rows_container);
    let bookmarks_close_button = make_button("Close", &delegate, sel!(closeAnyOverlay:), mtm);
    bookmarks_close_button.setFrame(NSRect::new(NSPoint::new(OVERLAY_MARGIN, OVERLAY_MARGIN), NSSize::new(90.0, ROW_HEIGHT - BUTTON_MARGIN)));
    bookmarks_view.addSubview(&bookmarks_close_button);
    bookmarks_view.addSubview(&bookmarks_chrome.close_button);
    bookmarks_view.addSubview(&bookmarks_chrome.esc_hint);

    let initial_vault_state = if profile.has_vault_passphrase() { VaultState::Locked } else { VaultState::NotSetUp };

    // One context for every page this profile ever opens (see
    // `WryEngine::new`'s doc comment) — see `browser-linux-gtk3`'s identical
    // field/comment for why an `ephemeral` profile gets its own uniquely-
    // named temp directory rather than simply `WebContext::new(None)`
    // (confirmed by a real test there that `None` falls back to wry's
    // shared default location, not a fresh one per context).
    let web_context = if profile.ephemeral {
        static EPHEMERAL_WEBVIEW_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = EPHEMERAL_WEBVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("claude-browser-ephemeral-{}-{n}", std::process::id()));
        WebContext::new(Some(dir))
    } else {
        WebContext::new(profile.webview_data_dir())
    };

    let state = Rc::new(AppState {
        window: window.clone(),
        toolbar_view,
        address_bar,
        title_chip,
        title_label,
        title_chip_button,
        switcher_button,
        settings_button,
        profile_button,
        passwords_button,
        bookmark_toggle_button,
        bookmarks_button,
        content_view,
        containers: RefCell::new(HashMap::new()),
        core: RefCell::new(PageManager::new(settings.max_loaded_pages)),
        web_context: RefCell::new(web_context),
        overlay: Cell::new(Overlay::None),
        switcher_view,
        switcher_chrome,
        switcher_rows_container,
        switcher_rows: RefCell::new(Vec::new()),
        settings_view,
        settings_chrome,
        start_page_field,
        unlimited_checkbox,
        limit_field,
        keybindings_rows_container,
        keybindings: RefCell::new(keybindings),
        listening_for: Cell::new(None),
        new_binding_field,
        profile_view,
        profile_chrome,
        profile_rows_container,
        new_profile_field,
        passwords_view,
        passwords_chrome,
        passwords: RefCell::new(initial_vault_state),
        session_passphrase: RefCell::new(None),
        passwords_unlock_label,
        passwords_unlock_field,
        passwords_unlock_button,
        passwords_unlock_error_label,
        passwords_rows_container,
        passwords_rows: RefCell::new(Vec::new()),
        passwords_site_field,
        passwords_username_field,
        passwords_password_field,
        passwords_notes_field,
        passwords_destination_popup,
        passwords_submit_button,
        passwords_cancel_edit_button,
        passwords_error_label,
        editing_login: RefCell::new(None),
        bitwarden_unlock_field,
        bitwarden_unlock_button,
        bookmarks: RefCell::new(bookmarks),
        bookmarks_view,
        bookmarks_chrome,
        bookmarks_rows_container,
        bookmarks_rows: RefCell::new(Vec::new()),
        settings: RefCell::new(settings),
        bitwarden_checkbox,
        bitwarden_url_field,
        theme_popup,
        encrypted_checkbox,
        history,
        profile,
    });
    delegate.ivars().state.replace(Some(Rc::clone(&state)));
    state.apply_theme();

    // Mirrors gtk3's `note_unlocked_with_passphrase`: if history was just
    // unlocked/set up with a passphrase, and the vault already has its own
    // passphrase marker set, silently try the same one against it too — the
    // concrete mechanism behind "the same passphrase unlocks both, when
    // both are on," no second prompt.
    if let Some(passphrase) = history_passphrase {
        *state.session_passphrase.borrow_mut() = Some(passphrase.clone());
        if matches!(&*state.passwords.borrow(), VaultState::Locked) {
            state.try_open_vault_with(&passphrase, false);
        }
    }

    state.open_start_page_or_restored_session(&initial_url);
    state.relayout();

    let menu = shortcuts::build_menu(&delegate, &state.keybindings.borrow(), mtm);
    app.setMainMenu(Some(&menu));

    window.makeKeyAndOrderFront(None);
    app.activate();

    Ok(App { mtm, _delegate: delegate })
}

/// Shows a small standalone window for launching with a URL argument (e.g.
/// from the OS's "open with"/default-browser handoff) — lets the user
/// confirm/pick which profile to open it in before the real browser window
/// appears. Mirrors `browser-windows-reactor`'s `run_chooser`: spawns a new
/// process rather than swapping in the real browser window in place (this
/// crate has no more of a way to hand off between two `NSApplication`
/// instances in one process than reactor does between two `windows-reactor`
/// windows).
pub fn run_chooser(url: String, default_profile: String) -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("run_chooser must be called from the main thread"))?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let delegate = ChooserDelegate::new(mtm, url, default_profile);

    let window_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 240.0));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), window_rect, style, NSBackingStoreType::Buffered, false) };
    window.setTitle(&NSString::from_str("Open link"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let content = window.contentView().ok_or_else(|| anyhow::anyhow!("NSWindow has no content view"))?;

    let url_label = make_text_field(mtm, delegate.ivars().url.as_str());
    url_label.setEditable(false);
    url_label.setBordered(false);
    url_label.setFrame(NSRect::new(NSPoint::new(16.0, 190.0), NSSize::new(448.0, 24.0)));
    content.addSubview(&url_label);

    let profile_field = make_text_field(mtm, &delegate.ivars().profile_name.borrow());
    profile_field.setFrame(NSRect::new(NSPoint::new(16.0, 150.0), NSSize::new(448.0, 28.0)));
    content.addSubview(&profile_field);
    *delegate.ivars().profile_field.borrow_mut() = Some(profile_field);

    let mut x = 16.0;
    for name in list_profile_names() {
        let button = make_button(&name, &delegate, sel!(pickSuggestion:), mtm);
        button.setFrame(NSRect::new(NSPoint::new(x, 110.0), NSSize::new(100.0, 28.0)));
        content.addSubview(&button);
        x += 104.0;
    }

    let cancel = make_button("Cancel", &delegate, sel!(cancelChooser:), mtm);
    cancel.setFrame(NSRect::new(NSPoint::new(16.0, 16.0), NSSize::new(100.0, 28.0)));
    content.addSubview(&cancel);
    let open = make_button("Open", &delegate, sel!(openChooser:), mtm);
    open.setFrame(NSRect::new(NSPoint::new(120.0, 16.0), NSSize::new(100.0, 28.0)));
    content.addSubview(&open);

    window.makeKeyAndOrderFront(None);
    app.activate();
    app.run();
    Ok(())
}

struct ChooserIvars {
    url: String,
    profile_name: RefCell<String>,
    profile_field: RefCell<Option<Retained<NSTextField>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ChooserIvars]
    struct ChooserDelegate;

    impl ChooserDelegate {
        #[unsafe(method(pickSuggestion:))]
        fn pick_suggestion(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let Some(button): Option<&NSButton> = sender.downcast_ref() else { return };
            let title = button.title().to_string();
            if let Some(field) = self.ivars().profile_field.borrow().as_ref() {
                field.setStringValue(&NSString::from_str(&title));
            }
        }

        #[unsafe(method(cancelChooser:))]
        fn cancel_chooser(&self, _sender: Option<&AnyObject>) {
            std::process::exit(0);
        }

        #[unsafe(method(openChooser:))]
        fn open_chooser(&self, _sender: Option<&AnyObject>) {
            let profile_name = self
                .ivars()
                .profile_field
                .borrow()
                .as_ref()
                .map(|f| f.stringValue().to_string())
                .unwrap_or_default();
            if let Ok(exe) = std::env::current_exe() {
                if let Err(err) = std::process::Command::new(exe).arg("--profile").arg(&profile_name).arg(&self.ivars().url).spawn() {
                    eprintln!("failed to launch the browser process: {err}");
                }
            }
            std::process::exit(0);
        }
    }

    unsafe impl NSObjectProtocol for ChooserDelegate {}

    unsafe impl NSWindowDelegate for ChooserDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            std::process::exit(0);
        }
    }
);

impl ChooserDelegate {
    fn new(mtm: MainThreadMarker, url: String, default_profile: String) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ChooserIvars { url, profile_name: RefCell::new(default_profile), profile_field: RefCell::new(None) });
        unsafe { msg_send![super(this), init] }
    }
}

/// Re-exported so `main.rs` can parse `--profile`/a bare URL argument
/// exactly like every other front end's CLI handling.
pub fn resolve_args(args: Vec<String>) -> (Option<String>, String) {
    let url = resolve_url_argument(args.clone());
    let profile = resolve_profile_name(args);
    (url, profile)
}
