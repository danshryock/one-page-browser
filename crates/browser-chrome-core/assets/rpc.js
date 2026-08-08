// Tiny shared RPC client for the switcher/profile/passwords pages — talks
// to the WebviewRpcServer instance the host started alongside this page's
// own EmbeddedAssetServer (see internal_pages::resolve, which is what put
// `?rpc_port=` on this page's own URL in the first place).
const RPC_PORT = new URLSearchParams(location.search).get("rpc_port");

async function rpcCall(method, params) {
  const response = await fetch(`http://127.0.0.1:${RPC_PORT}/rpc/${method}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(params === undefined ? {} : params),
  });
  const body = await response.json();
  if (!response.ok) {
    throw new Error(body && body.message ? body.message : `rpc call ${method} failed`);
  }
  return body;
}

function escapeHtml(text) {
  const div = document.createElement("div");
  div.textContent = text == null ? "" : String(text);
  return div.innerHTML;
}
