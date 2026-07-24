//! Converts `browser_core::KeyChord` to/from `windows-reactor`'s
//! `KeyboardAccelerator` — a real, working global-shortcut mechanism (see
//! `lib.rs`'s module doc comment), unlike `winio-winui3`'s raw `HWND`
//! subclass workaround (`browser-windows-winui`'s `install_hwnd_subclass`),
//! needed there only because that crate has no working `KeyDown` event at
//! all.
//!
//! `VirtualKey`'s letter/digit variants share their numeric values with the
//! classic Win32 virtual-key codes (`'A'` is `0x41`, matching `VK_A`; `'0'`
//! is `0x30`, matching `VK_0`) — a well-established fact, not a guess — so
//! single alphanumeric characters convert via a plain cast rather than
//! needing 36 named constants. F-keys (`VK_F1` = `0x70`) and the arrow/
//! escape keys are the other well-known standard values.
use browser_core::KeyChord;
use windows_reactor::{KeyboardAccelerator, VirtualKey, VirtualKeyModifiers};

pub fn chord_to_accelerator(chord: &KeyChord, on_invoked: impl Fn() + 'static) -> Option<KeyboardAccelerator> {
    let key = key_to_virtual_key(&chord.key)?;
    let mut modifiers = VirtualKeyModifiers::None;
    if chord.ctrl {
        modifiers |= VirtualKeyModifiers::Control;
    }
    if chord.alt {
        modifiers |= VirtualKeyModifiers::Menu;
    }
    if chord.shift {
        modifiers |= VirtualKeyModifiers::Shift;
    }
    Some(KeyboardAccelerator::new(key, modifiers, on_invoked))
}

fn key_to_virtual_key(key: &str) -> Option<VirtualKey> {
    if key.len() == 1 {
        let c = key.chars().next()?;
        if c.is_ascii_alphanumeric() {
            return Some(VirtualKey(c.to_ascii_uppercase() as i32));
        }
    }
    if let Some(n) = key.strip_prefix('F') {
        if let Ok(n) = n.parse::<i32>() {
            if (1..=24).contains(&n) {
                return Some(VirtualKey(111 + n)); // VK_F1 == 0x70 == 112
            }
        }
    }
    match key {
        "Escape" => Some(VirtualKey(27)),
        "Left" => Some(VirtualKey(37)),
        "Up" => Some(VirtualKey(38)),
        "Right" => Some(VirtualKey(39)),
        "Down" => Some(VirtualKey(40)),
        _ => None,
    }
}

/// Parses the keybindings editor's text-entry format (`"Ctrl+Shift+P"`) into
/// a `KeyChord`. `windows-reactor` has no generic "capture the next keypress"
/// API (checked by reading `element.rs`/`widget.rs`: only `.on_tapped(..)`/
/// `.on_pointer_*(..)`/`.keyboard_accelerator(..)` exist — the last is for
/// *known* combinations, not arbitrary capture), unlike `winio-winui3`'s
/// `WM_KEYDOWN`-subclass-driven "press keys…" capture UI
/// (`browser-windows-winui`'s `listening_for`/`assign_listening_binding`).
/// Text entry is a real, honest alternative given that constraint, not a
/// stopgap — reintroducing a raw `HWND` subclass just for this one feature
/// would undercut the point of moving off `winio-winui3` in the first place.
pub fn parse_chord(text: &str) -> Option<KeyChord> {
    let parts: Vec<&str> = text.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    let (key_part, modifiers) = parts.split_last()?;
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    for m in modifiers {
        match m.to_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" => alt = true,
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
    fn parses_modifiers_in_any_order() {
        assert_eq!(parse_chord("Ctrl+Shift+P"), Some(KeyChord::new(true, false, true, "P")));
        assert_eq!(parse_chord("shift+ctrl+p"), Some(KeyChord::new(true, false, true, "P")));
    }

    #[test]
    fn parses_named_keys() {
        assert_eq!(parse_chord("Alt+Left"), Some(KeyChord::new(false, true, false, "Left")));
        assert_eq!(parse_chord("F5"), Some(KeyChord::new(false, false, false, "F5")));
    }

    #[test]
    fn rejects_unknown_modifier_or_key() {
        assert_eq!(parse_chord("Super+T"), None);
        assert_eq!(parse_chord("Ctrl+"), None);
        assert_eq!(parse_chord(""), None);
    }
}
