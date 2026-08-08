//! A small, bidirectional JSON-RPC-shaped protocol for talking to a child
//! process over its own stdin/stdout — see `stdio`'s module doc comment for
//! the wire format, and `stdio_client::RpcChildProcess` for a usable client
//! over it. Not a general-purpose crate: this exists specifically to host
//! extension backends (`ROADMAP.md`'s "bidirectional JSON-RPC over stdio"
//! backlog item), so its scope stays narrow rather than growing into a
//! generic RPC framework.
//!
//! `RpcError` is also reused by `browser_chrome_core::webview_rpc`'s
//! separate (HTTP-based, not stdio-based — see that module's own doc
//! comment for why the two protocols don't share a single envelope) host↔
//! webview channel, so error shapes stay consistent across both without
//! forcing the two transports themselves to be the same thing.

mod stdio;
mod stdio_client;

pub use stdio::{read_message, write_message};
pub use stdio_client::RpcChildProcess;

use serde::{Deserialize, Serialize};

/// One message on the wire — see `stdio`'s doc comment for framing.
/// Deliberately not spec-literal JSON-RPC 2.0 (no `"jsonrpc": "2.0"` field,
/// an explicit `kind` tag instead of id-presence sniffing): this codebase's
/// existing small envelopes (e.g. `browser_core::bitwarden`'s `BwEnvelope`)
/// already favor small, explicit shapes over adopting a spec/crate
/// wholesale, and there's no consumer here that needs interop with a real
/// JSON-RPC 2.0 peer.
///
/// Both directions are symmetric — either side of the pipe can send a
/// `Request` and expects a `Response` with the same `id` back, matching the
/// "bidirectional" requirement this protocol exists for (an extension
/// process can call back into the host, not just answer the host's own
/// calls).
///
/// `binary_len`, when `Some(n)`, means exactly `n` raw, unencoded bytes
/// immediately follow this message on the wire, logically attached to it —
/// see `stdio`'s doc comment for exactly how that's read/written. `None`
/// means this message carries no binary attachment; `params`/`result` alone
/// (or nothing, for `Notification`) is the whole payload.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum RpcMessage {
    Request {
        id: u64,
        method: String,
        params: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binary_len: Option<u64>,
    },
    Response {
        id: u64,
        result: Result<serde_json::Value, RpcError>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binary_len: Option<u64>,
    },
    Notification {
        method: String,
        params: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binary_len: Option<u64>,
    },
}

/// The one type this crate's stdio protocol and `browser_chrome_core::
/// webview_rpc`'s HTTP protocol share — everything else about the two is
/// intentionally separate (see this crate's own top-of-file doc comment).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

impl RpcMessage {
    /// The declared length of this message's raw binary attachment, if
    /// any — see `stdio`'s doc comment for how this drives framing.
    pub fn binary_len(&self) -> Option<u64> {
        match self {
            RpcMessage::Request { binary_len, .. }
            | RpcMessage::Response { binary_len, .. }
            | RpcMessage::Notification { binary_len, .. } => *binary_len,
        }
    }
}
