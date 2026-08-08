// Example script, served from the embedded zip — see index.html's own
// comment. Reports both its own execution and the stylesheet's real,
// computed effect via console.log, so a real webview test can assert that
// HTML *and* CSS *and* JS all actually loaded from the unextracted
// archive, not just that the top-level page request succeeded.
const message = document.getElementById("message");
message.textContent = "loaded from the embedded zip";
const color = getComputedStyle(message).color;
console.log("embedded_assets_loaded color=" + color);
