//! Converts `browser_core::KeyChord`/`Keybindings` to/from AppKit's
//! `NSMenuItem` key-equivalent mechanism — the real, standard way macOS
//! apps register global keyboard shortcuts (unlike `windows-reactor`'s
//! `KeyboardAccelerator`s or GTK's raw `key-press-event`, an `NSMenuItem`'s
//! key equivalent fires regardless of which view has focus, including
//! inside a `WKWebView` — no `WebView2`-focus-swallows-shortcuts-style gap
//! here at all, since AppKit's menu key-equivalent dispatch happens at the
//! `NSApplication` event-loop level, before it's routed to any responder).
//!
//! See `lib.rs`'s module doc comment for why `KeyChord::ctrl` maps to ⌘
//! Command (not literal Control) on this platform.
use objc2::rc::Retained;
use objc2::{sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

use browser_core::{Action, KeyChord, Keybindings};

use crate::AppDelegate;

/// Builds the app's whole main menu — an application menu (Quit) plus one
/// top-level "Browser" menu with one item per `Action::ALL`, each routed to
/// `AppDelegate`'s shared `dispatchAction:` selector (tagged with its index
/// into `Action::ALL`, the same sender-tag convention `lib.rs` uses for
/// toolbar/overlay rows) and given whatever key equivalent `keybindings`
/// currently binds it to.
pub fn build_menu(delegate: &Retained<AppDelegate>, keybindings: &Keybindings, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let main_menu = NSMenu::new(mtm);

    let app_menu_item = NSMenuItem::new(mtm);
    let app_menu = NSMenu::new(mtm);
    let quit_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        )
    };
    quit_item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    app_menu.addItem(&quit_item);
    app_menu_item.setSubmenu(Some(&app_menu));
    main_menu.addItem(&app_menu_item);

    let browser_menu_item = NSMenuItem::new(mtm);
    let browser_menu = NSMenu::new(mtm);
    for (idx, &action) in Action::ALL.iter().enumerate() {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(action.label()),
                Some(sel!(dispatchAction:)),
                &NSString::from_str(""),
            )
        };
        item.setTag(idx as isize);
        // Deref-coerces `&AppDelegate` up through its `NSObject` superclass
        // to the `&AnyObject` this expects — same pattern `lib.rs` already
        // uses for every button's `setTarget`.
        unsafe { item.setTarget(Some(delegate)) };
        browser_menu.addItem(&item);
    }
    // Appended *after* the `Action::ALL` loop, not interspersed — kept out
    // of the loop so `apply_key_equivalents`'s index-based walk over
    // `browser_items` (paired against `Action::ALL[idx]`) still lines up.
    // Escape is otherwise the one hardcoded, non-`Keybindings`-configurable
    // shortcut in this app (matches gtk3/reactor — see
    // `browser_core::keybindings`'s doc comment). Reuses the existing
    // `closeAnyOverlay:` selector (already idempotent with nothing open).
    // A menu item's key equivalent always fires and swallows the event when
    // enabled, which would otherwise steal Escape from web page content
    // (in-page fullscreen, JS modals) even with no overlay open — gated via
    // `NSMenuItemValidation` (see `lib.rs`) so it only fires while one
    // actually is.
    let close_overlay_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Close Overlay"),
            Some(sel!(closeAnyOverlay:)),
            &NSString::from_str("\u{1b}"), // literal ESC
        )
    };
    unsafe { close_overlay_item.setTarget(Some(delegate)) };
    browser_menu.addItem(&close_overlay_item);
    browser_menu_item.setSubmenu(Some(&browser_menu));
    main_menu.addItem(&browser_menu_item);

    apply_key_equivalents(&main_menu, keybindings);
    main_menu
}

/// Walks every item in the "Browser" submenu (the second top-level item —
/// see `build_menu`) and refreshes its key equivalent from `keybindings`'
/// *first* bound chord for that action (an `NSMenuItem` can only show one
/// key equivalent, unlike the list-of-chords model `Keybindings` itself
/// supports — a real, documented simplification, not an oversight). Called
/// once at startup and again after every keybinding add/remove.
pub fn apply_key_equivalents(main_menu: &NSMenu, keybindings: &Keybindings) {
    let items = main_menu.itemArray();
    if items.len() < 2 {
        return;
    }
    let browser_menu_item = items.objectAtIndex(1);
    let Some(browser_menu) = browser_menu_item.submenu() else { return };
    let browser_items = browser_menu.itemArray();
    for (idx, &action) in Action::ALL.iter().enumerate() {
        if idx >= browser_items.len() {
            continue;
        }
        let item = browser_items.objectAtIndex(idx);
        match keybindings.bindings_for(action).first().and_then(chord_to_key_equivalent) {
            Some((key, modifiers)) => {
                item.setKeyEquivalent(&NSString::from_str(&key));
                item.setKeyEquivalentModifierMask(modifiers);
            }
            None => {
                item.setKeyEquivalent(&NSString::from_str(""));
                item.setKeyEquivalentModifierMask(NSEventModifierFlags::empty());
            }
        }
    }
}

/// `ctrl` → ⌘ Command, `alt` → ⌥ Option, `shift` → ⇧ Shift (a literal
/// match) — see this module's doc comment. Named keys map to the same
/// private-use-area Unicode characters AppKit itself uses for function/
/// arrow-key equivalents (`NSF1FunctionKey` etc., checked directly against
/// `objc2-app-kit`'s generated bindings rather than guessed); a single
/// alphanumeric character is used as-is (lowercased — `NSMenuItem` key
/// equivalents are conventionally lowercase, with Shift expressed via the
/// modifier mask, not by capitalizing the character).
fn chord_to_key_equivalent(chord: &KeyChord) -> Option<(String, NSEventModifierFlags)> {
    let mut modifiers = NSEventModifierFlags::empty();
    if chord.ctrl {
        modifiers |= NSEventModifierFlags::Command;
    }
    if chord.alt {
        modifiers |= NSEventModifierFlags::Option;
    }
    if chord.shift {
        modifiers |= NSEventModifierFlags::Shift;
    }

    if chord.key.len() == 1 {
        let c = chord.key.chars().next()?;
        if c.is_ascii_alphanumeric() {
            return Some((c.to_ascii_lowercase().to_string(), modifiers));
        }
    }
    if let Some(n) = chord.key.strip_prefix('F') {
        if let Ok(n) = n.parse::<u32>() {
            if (1..=24).contains(&n) {
                let code = 0xF704 + (n - 1); // NSF1FunctionKey == 0xF704
                return Some((char::from_u32(code)?.to_string(), modifiers));
            }
        }
    }
    let code: u32 = match chord.key.as_str() {
        "Escape" => 0x1B, // literal ASCII ESC, not a function-key constant
        "Left" => 0xF702,  // NSLeftArrowFunctionKey
        "Up" => 0xF700,    // NSUpArrowFunctionKey
        "Right" => 0xF703, // NSRightArrowFunctionKey
        "Down" => 0xF701,  // NSDownArrowFunctionKey
        _ => return None,
    };
    Some((char::from_u32(code)?.to_string(), modifiers))
}

/// Parses the keybindings editor's text-entry format (`"Cmd+Shift+P"`) into
/// a `KeyChord` — same reasoning as `browser-windows-reactor`'s
/// `parse_chord` for why text entry rather than live key capture (no
/// generic "capture the next keypress" API here either; AppKit's
/// equivalent would mean installing a local event monitor just for this one
/// feature, a real escape hatch but a bigger one than a text field). Accepts
/// both "Cmd"/"Command" and "Ctrl"/"Control" as the primary-modifier token
/// (both map to `ctrl` in the stored `KeyChord`, matching how `ctrl` means
/// "the platform's primary modifier" everywhere else in this module) so
/// users typing either the macOS-native or the cross-platform-familiar name
/// both work.
pub fn parse_chord(text: &str) -> Option<KeyChord> {
    let parts: Vec<&str> = text.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    let (key_part, modifiers) = parts.split_last()?;
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    for m in modifiers {
        match m.to_lowercase().as_str() {
            "cmd" | "command" | "ctrl" | "control" => ctrl = true,
            "alt" | "option" | "opt" => alt = true,
            "shift" => shift = true,
            _ => return None,
        }
    }
    let key = normalize_key(key_part)?;
    Some(KeyChord::new(ctrl, alt, shift, key))
}

fn normalize_key(s: &str) -> Option<String> {
    let upper = s.to_uppercase();
    if upper.len() == 1 && upper.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
        return Some(upper);
    }
    match upper.as_str() {
        "ESCAPE" | "ESC" => Some("Escape".to_string()),
        "LEFT" => Some("Left".to_string()),
        "RIGHT" => Some("Right".to_string()),
        "UP" => Some("Up".to_string()),
        "DOWN" => Some("Down".to_string()),
        s if s.starts_with('F') && s[1..].parse::<u8>().is_ok() => Some(upper),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_letter() {
        assert_eq!(parse_chord("T"), Some(KeyChord::new(false, false, false, "T")));
    }

    #[test]
    fn parses_cmd_and_command_the_same() {
        assert_eq!(parse_chord("Cmd+T"), Some(KeyChord::new(true, false, false, "T")));
        assert_eq!(parse_chord("Command+T"), Some(KeyChord::new(true, false, false, "T")));
    }

    #[test]
    fn parses_modifiers_in_any_order() {
        assert_eq!(parse_chord("Cmd+Shift+P"), Some(KeyChord::new(true, false, true, "P")));
        assert_eq!(parse_chord("shift+cmd+p"), Some(KeyChord::new(true, false, true, "P")));
    }

    #[test]
    fn parses_named_keys() {
        assert_eq!(parse_chord("Alt+Left"), Some(KeyChord::new(false, true, false, "Left")));
        assert_eq!(parse_chord("F5"), Some(KeyChord::new(false, false, false, "F5")));
    }

    #[test]
    fn rejects_unknown_modifier_or_key() {
        assert_eq!(parse_chord("Super+T"), None);
        assert_eq!(parse_chord("Cmd+"), None);
        assert_eq!(parse_chord(""), None);
    }

    #[test]
    fn chord_to_key_equivalent_maps_ctrl_to_command() {
        let (key, modifiers) = chord_to_key_equivalent(&KeyChord::new(true, false, false, "T")).unwrap();
        assert_eq!(key, "t");
        assert_eq!(modifiers, NSEventModifierFlags::Command);
    }

    #[test]
    fn chord_to_key_equivalent_maps_named_keys() {
        let (key, _) = chord_to_key_equivalent(&KeyChord::new(false, false, false, "F5")).unwrap();
        assert_eq!(key.chars().next().unwrap() as u32, 0xF708);
    }
}
