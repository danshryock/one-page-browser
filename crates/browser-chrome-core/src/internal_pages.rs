//! Maps `browser://...` URLs to the real loopback URL that actually serves
//! them — see `ROADMAP.md`'s backlog item this is the first real consumer
//! of: "move UI that isn't part of the toolbar into the webview itself,
//! using RPC to expose whatever native calls that UI needs." Deliberately
//! toolkit-agnostic (no GTK/AppKit/WinUI types here) and pure — no server
//! is started by this module, it only knows how to compute the URL once one
//! already is.
//!
//! `resolve_address_input` (in `browser-core`) already passes a
//! `browser://...` URL through unchanged (it contains `"://"`), so nothing
//! about address-bar parsing needs to change — a caller about to hand a URL
//! to a real `RenderEngine` calls `resolve` first, and only navigates to
//! its own `browser://...` string as a fallback if it returns `None`.

pub const SWITCHER: &str = "browser://switcher";
pub const PROFILE: &str = "browser://profile";
pub const PASSWORDS: &str = "browser://passwords";

/// Shared by `resolve` and `display_url` — one table, not two, so the two
/// stay in sync by construction rather than by convention.
const PAGES: &[(&str, &str)] = &[(SWITCHER, "switcher/index.html"), (PROFILE, "profile/index.html"), (PASSWORDS, "passwords/index.html")];

/// The real, loopback HTTP URL to navigate to instead of `url`, given the
/// ports of the running `EmbeddedAssetServer` (serving `assets/`) and
/// `WebviewRpcServer` (serving `/rpc/<method>`) — or `None` if `url` isn't
/// one of the known internal pages, in which case the caller should treat
/// it as an ordinary URL. `rpc_port` is passed through as a `?rpc_port=`
/// query parameter, the only way a served page (which has no other channel
/// to the host) can learn which port to `fetch()` its RPC calls against.
pub fn resolve(url: &str, asset_port: u16, rpc_port: u16) -> Option<String> {
    let path = PAGES.iter().find(|(internal, _)| *internal == url)?.1;
    Some(format!("http://127.0.0.1:{asset_port}/{path}?rpc_port={rpc_port}"))
}

/// The inverse of `resolve`, minus the `?rpc_port=` it added: given a page's
/// real, current URL and the running `EmbeddedAssetServer`'s port, the
/// `browser://...` form to show for it wherever a URL is displayed for
/// editing — or `None` if `url` isn't one of the known internal pages'
/// served addresses, in which case the caller should show `url` unchanged.
/// Used to keep the address bar showing `browser://switcher`, not the raw
/// loopback URL, once a page has actually loaded there (also see
/// `is_internal_url`, for keeping these pages out of history).
pub fn display_url(url: &str, asset_port: u16) -> Option<&'static str> {
    let prefix = format!("http://127.0.0.1:{asset_port}/");
    let path = url.strip_prefix(&prefix)?.split('?').next().unwrap_or("");
    PAGES.iter().find(|(_, served_path)| *served_path == path).map(|(internal, _)| *internal)
}

/// Whether `url` is one of the real loopback addresses `resolve` can produce
/// for `asset_port` — i.e. an internal page, not a page the user actually
/// navigated to on the web. Internal pages are deliberately excluded from
/// history (see `AppState::record_visit`): they're host UI, not browsing.
pub fn is_internal_url(url: &str, asset_port: u16) -> bool {
    display_url(url, asset_port).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_each_known_internal_page() {
        assert_eq!(resolve(SWITCHER, 1111, 2222), Some("http://127.0.0.1:1111/switcher/index.html?rpc_port=2222".to_string()));
        assert_eq!(resolve(PROFILE, 1111, 2222), Some("http://127.0.0.1:1111/profile/index.html?rpc_port=2222".to_string()));
        assert_eq!(resolve(PASSWORDS, 1111, 2222), Some("http://127.0.0.1:1111/passwords/index.html?rpc_port=2222".to_string()));
    }

    #[test]
    fn returns_none_for_an_unrecognized_internal_url() {
        assert_eq!(resolve("browser://not-a-real-page", 1111, 2222), None);
    }

    #[test]
    fn returns_none_for_an_ordinary_url() {
        assert_eq!(resolve("https://example.com", 1111, 2222), None);
    }

    #[test]
    fn display_url_is_the_inverse_of_resolve_regardless_of_the_rpc_port() {
        for &(internal, _) in PAGES {
            let resolved = resolve(internal, 1111, 2222).unwrap();
            assert_eq!(display_url(&resolved, 1111), Some(internal));
            // The rpc_port query string is irrelevant to display_url — the
            // page's own URL never changes as its RPC calls happen.
            let resolved_other_rpc_port = resolve(internal, 1111, 9999).unwrap();
            assert_eq!(display_url(&resolved_other_rpc_port, 1111), Some(internal));
        }
    }

    #[test]
    fn display_url_returns_none_for_an_ordinary_url() {
        assert_eq!(display_url("https://example.com", 1111), None);
        assert_eq!(display_url("http://127.0.0.1:1111/not-a-real-page.html", 1111), None);
    }

    #[test]
    fn display_url_requires_the_right_asset_port() {
        let resolved = resolve(SWITCHER, 1111, 2222).unwrap();
        assert_eq!(display_url(&resolved, 3333), None, "a URL served on a different port isn't one of ours");
    }

    #[test]
    fn is_internal_url_matches_display_url() {
        let resolved = resolve(PASSWORDS, 1111, 2222).unwrap();
        assert!(is_internal_url(&resolved, 1111));
        assert!(!is_internal_url("https://example.com", 1111));
    }
}
