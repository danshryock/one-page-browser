// Real passwords page logic. `passwords.list` never includes plaintext —
// `passwords.reveal` fetches one entry's password on demand (the eye/copy
// icons), so a page load never pulls every saved password into the DOM.

const SOURCE_COLOR = { browser: "var(--accent)", bitwarden: "var(--accent-bitwarden)" };
const SOURCE_LABEL = { browser: "Browser", bitwarden: "Bitwarden" };

let allEntries = [];
let locked = false;
let hiddenSources = new Set();
let revealed = {}; // "source:id" -> password

function entryKey(entry) {
  return `${entry.source}:${entry.id}`;
}

function renderFilterRow() {
  const sources = [...new Set(allEntries.map((e) => e.source))];
  document.getElementById("filter-row").innerHTML = sources
    .map(
      (source) => `
      <div class="filter-toggle" data-toggle-source="${source}">
        <div class="toggle ${hiddenSources.has(source) ? "" : "on"}"><div class="toggle-knob"></div></div>
        <span style="width:9px;height:9px;border-radius:3px;background:${SOURCE_COLOR[source] || "#999"}"></span>
        <span>${escapeHtml(SOURCE_LABEL[source] || source)}</span>
      </div>`
    )
    .join("");
}

function renderContent(query) {
  const content = document.getElementById("content");
  if (locked) {
    content.innerHTML = '<div class="empty-state" style="padding:24px">The password vault is locked. Unlock it from the Profile page\'s Password Managers section.</div>';
    return;
  }
  const q = (query || "").toLowerCase();
  const visible = allEntries.filter((e) => !hiddenSources.has(e.source) && (!q || e.site.toLowerCase().includes(q) || e.username.toLowerCase().includes(q)));
  if (visible.length === 0) {
    content.innerHTML = '<div class="empty-state" style="padding:24px">No saved logins.</div>';
    return;
  }
  content.innerHTML = visible
    .map((entry) => {
      const key = entryKey(entry);
      const shown = revealed[key];
      return `
      <div class="listrow" data-key="${escapeHtml(key)}" data-id="${escapeHtml(entry.id)}" data-source="${escapeHtml(entry.source)}">
        <div class="swatch" style="background:${SOURCE_COLOR[entry.source] || "#999"}"></div>
        <div style="flex:1;min-width:0">
          <div style="display:flex;align-items:center;gap:8px">
            <span class="row-title">${escapeHtml(entry.site)}</span>
            <span class="tag" style="background:${SOURCE_COLOR[entry.source] || "#999"}">${escapeHtml((SOURCE_LABEL[entry.source] || entry.source).toUpperCase())}</span>
          </div>
          <div class="row-sub">${escapeHtml(entry.username)}</div>
        </div>
        <span class="dots">${shown ? escapeHtml(shown) : "••••••••"}</span>
        <button class="iconbtn" data-action="reveal" title="Show password">👁</button>
        <button class="iconbtn" data-action="copy" title="Copy password">⧉</button>
      </div>`;
    })
    .join("");
}

async function load() {
  const result = await rpcCall("passwords.list", {});
  locked = result.locked;
  allEntries = result.entries;
  renderFilterRow();
  renderContent(document.getElementById("search").value.trim());
  console.log(`passwords_loaded locked=${locked} entries=${allEntries.length}`);
}

async function revealPassword(row) {
  const id = row.dataset.id;
  const source = row.dataset.source;
  const key = `${source}:${id}`;
  if (!revealed[key]) {
    const { password } = await rpcCall("passwords.reveal", { id, source });
    revealed[key] = password || "";
  }
  return revealed[key];
}

document.getElementById("filter-row").addEventListener("click", (event) => {
  const toggle = event.target.closest("[data-toggle-source]");
  if (!toggle) return;
  const source = toggle.dataset.toggleSource;
  if (hiddenSources.has(source)) {
    hiddenSources.delete(source);
  } else {
    hiddenSources.add(source);
  }
  renderFilterRow();
  renderContent(document.getElementById("search").value.trim());
});

document.getElementById("content").addEventListener("click", async (event) => {
  const row = event.target.closest(".listrow");
  if (!row) return;
  const actionBtn = event.target.closest("[data-action]");
  if (!actionBtn) return;
  const password = await revealPassword(row);
  if (actionBtn.dataset.action === "reveal") {
    renderContent(document.getElementById("search").value.trim());
  } else if (actionBtn.dataset.action === "copy" && navigator.clipboard) {
    navigator.clipboard.writeText(password).catch((err) => console.error(err));
  }
});

document.getElementById("search").addEventListener("input", (event) => {
  renderContent(event.target.value.trim());
});

load().catch((err) => console.error(err));
