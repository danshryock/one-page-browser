//! The shared vocabulary for each frontend's local test-command channel
//! (`browser-macos-appkit`'s `AppState::start_test_command_listener` over a
//! Unix domain socket, `browser-windows-reactor`'s `test_command_server`
//! over a `127.0.0.1` TCP socket — see each module's own doc comment for
//! why a real OS-level input driver isn't used instead) — a different kind
//! of shared piece than `switcher`/`restore`'s real UI decisions, but
//! duplicated the same way those used to be before this crate existed:
//! both platforms independently hand-rolled an identical
//! `line.split_once(' ')` parser and `match` over the same command names.
//! One parser here means adding a command updates the vocabulary in one
//! place, and every frontend's own exhaustive `match` over [`TestCommand`]
//! is the compiler's own guarantee that "this frontend doesn't handle X" is
//! a deliberate, visible choice, not a silent gap between two copies
//! quietly drifting apart.
//!
//! Deliberately not JSON — this is test-only surface area (real user input
//! never reaches this parser), and nothing here needs nested structure.
//!
//! Only the commands every socket-based frontend's driver actually
//! exercises today (`web-standards-tests/src/bin/macos_driver.rs`/
//! `windows_driver.rs`) are modeled as real, exhaustively-matched variants.
//! `browser-linux-gtk3` doesn't have (or need) a socket at all — its own
//! `tests/gtk_tests.rs` drives the real app in-process, via direct
//! `Rc<AppState>` method calls, which is strictly more capable than any
//! string it could send itself over a channel like this one.

/// One parsed command line. `Unknown` carries the original command word
/// (not the whole line — an unrecognized command's argument is whatever
/// that platform's own extension expects, not this module's business) so a
/// caller can still try its own platform-specific commands beyond this
/// shared core before giving up and logging it, the same fallback order
/// every current caller already used before this type existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestCommand {
    /// Opens the switcher, cleared and ready to filter/type a URL — every
    /// frontend's own `open_switcher`/`toggle_switcher`(`Overlay::Switcher`)
    /// method.
    OpenSwitcher,
    /// Opens a URL directly. Each frontend wires this to whatever its own
    /// "just open this URL" primitive is (`add_page`/`add_page_and_switch`)
    /// rather than simulating typing into the address bar and pressing
    /// Enter — simpler, and reliable is what a test fixture URL needs, not
    /// a UI heuristic in between.
    Navigate(String),
    /// Runs `document.querySelector('[data-test-target="<name>"]')?.click()`
    /// against the active page — not a "trusted" click in the DOM sense
    /// (see each driver's own doc comment for where that already matters:
    /// `windows_driver.rs`'s `KNOWN_UNSUPPORTED_CASES`).
    ClickJs(String),
    /// Not one of the commands above — `String` is just the command word
    /// (`line.split_once(' ')`'s first half), so a caller can still resolve
    /// its own additional commands from the original line.
    Unknown(String),
}

/// Parses one line of the shared vocabulary. Never fails — an unrecognized
/// command word becomes `TestCommand::Unknown`, not an `Err`, so a caller
/// can seamlessly fall through to its own platform-specific extensions
/// (see this module's doc comment) without needing to re-split the line
/// itself.
pub fn parse_test_command(line: &str) -> TestCommand {
    let (command, arg) = line.split_once(' ').unwrap_or((line, ""));
    match command {
        "open_switcher" => TestCommand::OpenSwitcher,
        "navigate" => TestCommand::Navigate(arg.to_string()),
        "click_js" => TestCommand::ClickJs(arg.to_string()),
        other => TestCommand::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_switcher_with_no_argument() {
        assert_eq!(parse_test_command("open_switcher"), TestCommand::OpenSwitcher);
    }

    #[test]
    fn parses_navigate_with_its_url_argument() {
        assert_eq!(parse_test_command("navigate https://example.com"), TestCommand::Navigate("https://example.com".to_string()));
    }

    #[test]
    fn parses_click_js_with_its_target_argument() {
        assert_eq!(parse_test_command("click_js link"), TestCommand::ClickJs("link".to_string()));
    }

    #[test]
    fn unrecognized_command_becomes_unknown_with_just_the_command_word() {
        assert_eq!(parse_test_command("switcher_nav_down"), TestCommand::Unknown("switcher_nav_down".to_string()));
        assert_eq!(parse_test_command("open_settings"), TestCommand::Unknown("open_settings".to_string()));
    }

    #[test]
    fn navigate_with_no_argument_gets_an_empty_url() {
        assert_eq!(parse_test_command("navigate"), TestCommand::Navigate(String::new()));
    }
}
