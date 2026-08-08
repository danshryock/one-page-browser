//! Toolkit-agnostic *decision* logic shared by every native chrome
//! (`browser-linux-gtk3`, `browser-windows-winui`, `browser-windows-reactor`,
//! `browser-macos-appkit`) — the layer `ARCHITECTURE.md` §4 proposed: sits
//! between `browser-core`'s raw data types (`Page`, `PageManager`,
//! `Settings`, `Keybindings`) and each frontend's native UI, owning "what
//! should happen" without ever touching a widget.
//!
//! Deliberately a separate crate from `browser-core` rather than more
//! modules there: `browser-core` today has zero UI concepts at all: "which
//! rows the switcher should show" or "what happens when row 3 is clicked"
//! is a different, UI-adjacent concern even though it still requires no
//! actual widget toolkit — see `ARCHITECTURE.md` §4's opening paragraph for
//! the full reasoning.
//!
//! Every piece here is generic over `render_engine::RenderEngine` the same
//! way `browser_core::PageManager` already is, and unit-tested with
//! `browser_core::testing::MockEngine` — the same mock `browser-core`'s own
//! `PageManager` tests use, not new test infrastructure (see that module's
//! doc comment for why it's exposed, not `#[cfg(test)]`-gated, in the first
//! place).
//!
//! Currently `switcher` (`ARCHITECTURE.md` §7's rollout starts there —
//! highest duplication count, and already nearly pure data in two of the
//! four frontends that have it) and `restore` (the startup "which pages
//! should we open" decision — one of the `PageController`-shaped follow-ups
//! that doc comment already called out). `SettingsController`/
//! `KeybindingsController`/`ProfilePickerModel` remain future work.
//!
//! `test_commands` is a different kind of shared piece — not a real UI
//! decision, just a parser for the local test-command channel
//! `browser-macos-appkit`/`browser-windows-reactor` each run (see that
//! module's own doc comment) — kept here anyway rather than a new crate,
//! since it was hand-duplicated the exact same way the real decision logic
//! above used to be before this crate existed.

mod restore;
mod switcher;
mod test_commands;

pub use restore::{resolve_restore_plan, RestorePlan};
pub use switcher::{activate_row, build_switcher_rows, next_activatable_row, SwitcherActivation, SwitcherNavDirection, SwitcherRow};
pub use test_commands::{parse_test_command, TestCommand};
