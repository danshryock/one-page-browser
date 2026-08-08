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

/// The real, loopback HTTP URL to navigate to instead of `url`, given the
/// ports of the running `EmbeddedAssetServer` (serving `assets/`) and
/// `WebviewRpcServer` (serving `/rpc/<method>`) — or `None` if `url` isn't
/// one of the known internal pages, in which case the caller should treat
/// it as an ordinary URL. `rpc_port` is passed through as a `?rpc_port=`
/// query parameter, the only way a served page (which has no other channel
/// to the host) can learn which port to `fetch()` its RPC calls against.
pub fn resolve(url: &str, asset_port: u16, rpc_port: u16) -> Option<String> {
    let path = match url {
        SWITCHER => "switcher/index.html",
        PROFILE => "profile/index.html",
        PASSWORDS => "passwords/index.html",
        _ => return None,
    };
    Some(format!("http://127.0.0.1:{asset_port}/{path}?rpc_port={rpc_port}"))
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
}
