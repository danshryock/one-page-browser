// Real profile/settings page logic. The profile-menu dropdown's Settings/
// Passwords items (shared with assets/profile-menu/ — see profile_menu.js)
// navigate via navigation.open_settings/.open_passwords RPC calls, not a
// same-page link — see that file's own doc comment for why.

let settings = null;
let profileInfo = null;
let activeSection = "general";

async function loadProfileInfo() {
  profileInfo = await rpcCall("profile.info", {});
  document.getElementById("avatar-btn").textContent = profileInfo.initial;
}

function renderMenu(open) {
  const root = document.getElementById("menu-root");
  if (!open || !profileInfo) {
    root.innerHTML = "";
    return;
  }
  root.innerHTML = `<div class="scrim" id="menu-scrim"></div><div class="menu">${profileMenuHtml(profileInfo)}</div>`;
}

document.getElementById("avatar-btn").addEventListener("click", () => {
  const isOpen = document.getElementById("menu-root").children.length > 0;
  renderMenu(!isOpen);
});

document.getElementById("menu-root").addEventListener("click", (event) => {
  if (event.target.id === "menu-scrim") {
    renderMenu(false);
  }
});
wireProfileMenu(document.getElementById("menu-root"), () => renderMenu(false));

// ---- General section ----
function renderGeneral() {
  const isDark = settings.theme === "Dark";
  return `
    <div class="section">
      <div class="sidehead section-title">General</div>
      <div class="field-row">
        <div class="field-label">
          <div class="row-title">Start page</div>
          <div class="field-sub">Opened for a brand new page</div>
        </div>
        <input type="text" id="start-page-input" value="${escapeHtml(settings.start_page)}">
        <button class="btn btn-primary" id="save-start-page">Save</button>
      </div>
      <div class="field-row">
        <div class="field-label">
          <div class="row-title">Loaded pages limit</div>
          <div class="field-sub">How many pages may stay loaded at once — blank for unlimited</div>
        </div>
        <input type="number" id="max-loaded-input" min="1" value="${settings.max_loaded_pages == null ? "" : settings.max_loaded_pages}" style="width:80px">
        <button class="btn btn-primary" id="save-max-loaded">Save</button>
      </div>
      <div class="field-row">
        <div class="field-label">
          <div class="row-title">Dark theme</div>
        </div>
        <div class="toggle ${isDark ? "on" : ""}" id="theme-toggle"><div class="toggle-knob"></div></div>
      </div>
    </div>`;
}

function wireGeneral() {
  document.getElementById("save-start-page").addEventListener("click", async () => {
    const start_page = document.getElementById("start-page-input").value.trim();
    await rpcCall("profile.settings.update_general", { start_page });
    await reloadSettings();
  });
  document.getElementById("save-max-loaded").addEventListener("click", async () => {
    const raw = document.getElementById("max-loaded-input").value.trim();
    await rpcCall("profile.settings.update_general", { max_loaded_pages: raw === "" ? null : Number(raw) });
    await reloadSettings();
  });
  document.getElementById("theme-toggle").addEventListener("click", async () => {
    const next = settings.theme === "Dark" ? "Light" : "Dark";
    await rpcCall("profile.settings.update_general", { theme: next });
    await reloadSettings();
  });
}

// ---- Privacy & Security section (no backing settings yet) ----
function renderPrivacy() {
  return `
    <div class="section">
      <div class="sidehead section-title">Privacy &amp; Security</div>
      <div class="placeholder">Privacy &amp; Security settings aren't implemented yet.</div>
    </div>`;
}

// ---- Password Managers section ----
function renderPasswordManagers(managers) {
  const rows = managers
    .map((m) => {
      if (m.builtin) {
        return `
          <div class="listrow">
            <div class="swatch" style="background:var(--accent);width:14px;height:14px;border-radius:4px"></div>
            <div style="flex:1;min-width:0"><div class="row-title">${escapeHtml(m.label)}</div><div class="row-sub">${escapeHtml(m.detail)}</div></div>
            <div class="toggle on"><div class="toggle-knob"></div></div>
          </div>`;
      }
      const actionLabel = m.connected ? "Disconnect" : "Connect";
      return `
        <div class="listrow">
          <div class="swatch" style="background:var(--accent-bitwarden);width:14px;height:14px;border-radius:4px"></div>
          <div style="flex:1;min-width:0"><div class="row-title">${escapeHtml(m.label)}</div><div class="row-sub">${escapeHtml(m.detail)}</div></div>
          <button class="btn" data-bitwarden-action="${m.connected ? "disconnect" : "connect"}">${actionLabel}</button>
        </div>`;
    })
    .join("");
  return `
    <div class="section">
      <div class="sidehead section-title">Password managers</div>
      ${rows}
      <div class="add-row" id="bitwarden-connect-form" style="display:none">
        <input type="text" id="bitwarden-url-input" placeholder="http://127.0.0.1:8087" style="width:240px">
        <button class="btn btn-primary" id="bitwarden-connect-submit">Connect</button>
      </div>
    </div>`;
}

function wirePasswordManagers() {
  const content = document.getElementById("content");
  content.addEventListener("click", async (event) => {
    const actionBtn = event.target.closest("[data-bitwarden-action]");
    if (!actionBtn) return;
    if (actionBtn.dataset.bitwardenAction === "disconnect") {
      await rpcCall("profile.password_managers.disconnect_bitwarden", {});
      await renderSection();
    } else {
      document.getElementById("bitwarden-connect-form").style.display = "flex";
    }
  });
  const submit = document.getElementById("bitwarden-connect-submit");
  if (submit) {
    submit.addEventListener("click", async () => {
      const server_url = document.getElementById("bitwarden-url-input").value.trim();
      if (!server_url) return;
      await rpcCall("profile.password_managers.connect_bitwarden", { server_url });
      await renderSection();
    });
  }
}

// ---- Search Engines section ----
function renderSearchEngines() {
  const rows = settings.search_engines
    .map(
      (eng) => `
      <div class="listrow">
        <div style="width:8px;height:8px;border-radius:50%;background:${eng.name === settings.default_search_engine ? "var(--accent)" : "transparent"};border:1.5px solid rgba(0,0,0,.2)"></div>
        <div style="flex:1;min-width:0"><div class="row-title">${escapeHtml(eng.name)}</div><div class="row-sub">${escapeHtml(eng.query_url_template)}</div></div>
        <button class="btn" data-set-default="${escapeHtml(eng.name)}">Set default</button>
        <button class="iconbtn" data-remove-engine="${escapeHtml(eng.name)}">✕</button>
      </div>`
    )
    .join("");
  return `
    <div class="section">
      <div class="sidehead section-title">Search engines</div>
      ${rows}
      <div class="add-row" style="display:flex;gap:8px">
        <input type="text" id="new-engine-name" placeholder="Name" style="width:120px">
        <input type="text" id="new-engine-url" placeholder="https://example.com/search?q={query}" style="flex:1">
        <button class="btn btn-dashed" id="add-engine-btn">+ Add search engine</button>
      </div>
    </div>`;
}

function wireSearchEngines() {
  const content = document.getElementById("content");
  content.addEventListener("click", async (event) => {
    const setDefaultBtn = event.target.closest("[data-set-default]");
    const removeBtn = event.target.closest("[data-remove-engine]");
    if (setDefaultBtn) {
      await rpcCall("profile.search_engines.set_default", { name: setDefaultBtn.dataset.setDefault });
      await reloadSettings();
    } else if (removeBtn) {
      await rpcCall("profile.search_engines.remove", { name: removeBtn.dataset.removeEngine });
      await reloadSettings();
    }
  });
  const addBtn = document.getElementById("add-engine-btn");
  if (addBtn) {
    addBtn.addEventListener("click", async () => {
      const name = document.getElementById("new-engine-name").value.trim();
      const query_url_template = document.getElementById("new-engine-url").value.trim();
      if (!name || !query_url_template) return;
      await rpcCall("profile.search_engines.add", { name, query_url_template });
      await reloadSettings();
    });
  }
}

// ---- Keyboard Shortcuts section ----
// Chords are full objects ({ctrl,alt,shift,key,display}), not plain
// strings — set_bindings needs the structured form back, so the display
// string alone (what a read-only view would need) isn't enough here.
let capturingAction = null;
let latestBindingsByAction = {};

function chordTag(action, chord) {
  return `<span class="row-sub" style="background:var(--hover);padding:2px 6px;border-radius:5px;display:inline-flex;align-items:center;gap:4px">
    ${escapeHtml(chord.display)}
    <button class="iconbtn" style="width:14px;height:14px" data-remove-chord="${escapeHtml(action)}" data-chord='${escapeHtml(JSON.stringify(chord))}'>✕</button>
  </span>`;
}

function renderShortcuts(bindings) {
  const rows = bindings
    .map((b) => {
      const isCapturing = capturingAction === b.action;
      const addBtn = isCapturing
        ? `<span class="row-sub" style="padding:2px 6px">Press a key…</span>`
        : `<button class="iconbtn" data-add-chord="${escapeHtml(b.action)}" title="Add shortcut">+</button>`;
      return `
      <div class="listrow">
        <span style="flex:1">${escapeHtml(b.label)}</span>
        <div style="display:flex;gap:4px;align-items:center">
          ${b.chords.map((chord) => chordTag(b.action, chord)).join("") || (isCapturing ? "" : '<span class="row-sub">unbound</span>')}
          ${addBtn}
        </div>
      </div>`;
    })
    .join("");
  return `
    <div class="section">
      <div class="sidehead section-title">Keyboard shortcuts</div>
      ${rows}
    </div>`;
}

function keyEventToChord(event) {
  // Named keys keep their event.key value (e.g. "F1", "Escape", "ArrowLeft"
  // trimmed to "Left" to match KeyChord's existing normalized vocabulary —
  // see browser_core::keybindings.rs); a single printable character is
  // uppercased to match the same normalized form every native frontend
  // already stores ("T", not "t").
  let key = event.key;
  if (key.startsWith("Arrow")) key = key.slice(5);
  else if (key.length === 1) key = key.toUpperCase();
  return { ctrl: event.ctrlKey, alt: event.altKey, shift: event.shiftKey, key };
}

// Registered exactly once at module load (not per-render, unlike the
// content click listener below — a re-registered `document`-level listener
// would never get cleaned up by `innerHTML` replacement the way listeners
// on replaced child elements do, and would silently pile up every time the
// Keyboard Shortcuts tab is revisited).
document.addEventListener("keydown", async (event) => {
  if (!capturingAction) return;
  event.preventDefault();
  const action = capturingAction;
  capturingAction = null;
  // Bare Escape cancels the capture instead of binding it — the same
  // convention as every overlay in this app already using Escape to
  // close/cancel, not something a user would expect to rebind by accident.
  if (event.key === "Escape" && !event.ctrlKey && !event.altKey && !event.shiftKey) {
    await renderShortcutsSection();
    return;
  }
  const chord = keyEventToChord(event);
  const existing = latestBindingsByAction[action] || [];
  await rpcCall("profile.keybindings.set_bindings", { action, chords: [...existing, chord] });
  await renderShortcutsSection();
});

function wireShortcuts() {
  const content = document.getElementById("content");
  content.addEventListener("click", async (event) => {
    const addBtn = event.target.closest("[data-add-chord]");
    const removeBtn = event.target.closest("[data-remove-chord]");
    if (addBtn) {
      capturingAction = addBtn.dataset.addChord;
      await renderSection();
    } else if (removeBtn) {
      const action = removeBtn.dataset.removeChord;
      const chord = JSON.parse(removeBtn.dataset.chord);
      const remaining = (latestBindingsByAction[action] || []).filter(
        (c) => !(c.ctrl === chord.ctrl && c.alt === chord.alt && c.shift === chord.shift && c.key === chord.key)
      );
      await rpcCall("profile.keybindings.set_bindings", { action, chords: remaining });
      await renderShortcutsSection();
    }
  });
}

async function renderShortcutsSection() {
  const { bindings } = await rpcCall("profile.keybindings.list", {});
  latestBindingsByAction = Object.fromEntries(bindings.map((b) => [b.action, b.chords]));
  document.getElementById("content").innerHTML = renderShortcuts(bindings);
  wireShortcuts();
  console.log(`profile_section_rendered section=shortcuts`);
}

async function reloadSettings() {
  settings = await rpcCall("profile.settings.get", {});
  await renderSection();
}

async function renderSection() {
  const content = document.getElementById("content");
  if (activeSection === "general") {
    content.innerHTML = renderGeneral();
    wireGeneral();
  } else if (activeSection === "privacy") {
    content.innerHTML = renderPrivacy();
  } else if (activeSection === "password-managers") {
    const { managers } = await rpcCall("profile.password_managers.list", {});
    content.innerHTML = renderPasswordManagers(managers);
    wirePasswordManagers();
  } else if (activeSection === "search-engines") {
    content.innerHTML = renderSearchEngines();
    wireSearchEngines();
  } else if (activeSection === "shortcuts") {
    await renderShortcutsSection();
    return; // renderShortcutsSection already logs profile_section_rendered
  }
  console.log(`profile_section_rendered section=${activeSection}`);
}

document.getElementById("sidebar").addEventListener("click", (event) => {
  const btn = event.target.closest(".sidebtn");
  if (!btn) return;
  activeSection = btn.dataset.section;
  document.querySelectorAll(".sidebtn").forEach((b) => b.classList.toggle("active", b === btn));
  renderSection().catch((err) => console.error(err));
});

async function init() {
  settings = await rpcCall("profile.settings.get", {});
  await loadProfileInfo();
  await renderSection();
}

init().catch((err) => console.error(err));
