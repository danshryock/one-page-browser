//! The startup "which pages should we open" decision — shared by every
//! frontend's own bootstrap code, the same way `switcher.rs` shares the
//! switcher's row list. Toolkit-free: this just turns a `Session` (loaded
//! from disk) plus the profile's configured `start_page` into an ordered
//! list of URLs to open and which one should end up active, leaving the
//! actual "call `add_page` for each" loop to each frontend (their `add_page`
//! equivalents differ too much in shape — return type, id-vs-nothing — to
//! usefully share that part too).

use browser_core::Session;

/// The result of `resolve_restore_plan`: `urls` in the order they should be
/// opened, and (if `Some`) the index into `urls` that should end up active
/// once they're all open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestorePlan {
    pub urls: Vec<String>,
    pub active_index: Option<usize>,
}

/// No saved session (first run, or an ephemeral profile, which never saves
/// one at all) falls back to the profile's single `start_page`, same as
/// every frontend's behavior before session restore existed. A saved
/// session's `active_index` is dropped if it's out of range for its own
/// `pages` list (e.g. a hand-edited or future-incompatible `session.json`)
/// rather than trusted blindly — callers index into `urls` with it, so an
/// out-of-range value must never reach them.
pub fn resolve_restore_plan(session: &Session, start_page: &str) -> RestorePlan {
    if session.pages.is_empty() {
        return RestorePlan { urls: vec![start_page.to_string()], active_index: Some(0) };
    }
    RestorePlan {
        urls: session.pages.iter().map(|p| p.url.clone()).collect(),
        active_index: session.active_index.filter(|&i| i < session.pages.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser_core::SessionPage;

    #[test]
    fn empty_session_falls_back_to_start_page() {
        let plan = resolve_restore_plan(&Session::default(), "https://start.example");
        assert_eq!(plan.urls, vec!["https://start.example".to_string()]);
        assert_eq!(plan.active_index, Some(0));
    }

    #[test]
    fn non_empty_session_passes_through_urls_and_active_index() {
        let session = Session {
            pages: vec![
                SessionPage { url: "https://a.example".to_string(), title: "A".to_string() },
                SessionPage { url: "https://b.example".to_string(), title: "B".to_string() },
            ],
            active_index: Some(1),
        };
        let plan = resolve_restore_plan(&session, "https://start.example");
        assert_eq!(plan.urls, vec!["https://a.example".to_string(), "https://b.example".to_string()]);
        assert_eq!(plan.active_index, Some(1));
    }

    #[test]
    fn out_of_range_active_index_is_dropped_not_trusted() {
        let session = Session {
            pages: vec![SessionPage { url: "https://a.example".to_string(), title: "A".to_string() }],
            active_index: Some(5),
        };
        let plan = resolve_restore_plan(&session, "https://start.example");
        assert_eq!(plan.active_index, None, "an out-of-range active_index must never reach callers who index with it");
    }
}
