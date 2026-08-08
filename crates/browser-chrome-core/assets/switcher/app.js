// Real switcher page logic — fetches open pages, bookmarks, and history
// from the host over RPC (see rpc.js) and renders them; a real webview
// test (browser-linux-gtk3/tests/gtk_tests.rs) asserts these console.log
// lines against seeded real data, not just that the page loaded.

let openPages = [];
let bookmarks = [];
let history = [];

function rowHtml({ swatchColor, title, sub, closable, id }) {
  return `
    <div class="listrow" data-id="${escapeHtml(id)}">
      <div class="swatch" style="background:${escapeHtml(swatchColor)}"></div>
      <div style="flex:1;min-width:0">
        <div class="row-title">${escapeHtml(title)}</div>
        <div class="row-sub">${escapeHtml(sub)}</div>
      </div>
      ${closable ? '<button class="iconbtn close-page-btn" data-action="close">✕</button>' : ""}
    </div>`;
}

function renderColumn(elementId, items, closable) {
  const el = document.getElementById(elementId);
  if (items.length === 0) {
    el.innerHTML = '<div class="empty-state">Nothing here yet.</div>';
    return;
  }
  el.innerHTML = items
    .map((item) => rowHtml({ swatchColor: item.color, title: item.title, sub: item.sub, closable, id: item.id }))
    .join("");
}

function renderAll() {
  renderColumn("open-rows", openPages.map((p) => ({ id: p.id, color: p.color, title: p.title, sub: p.url })), true);
  renderColumn("bookmarks-rows", bookmarks.map((b) => ({ id: b.url, color: b.color, title: b.title, sub: b.domain })), false);
  renderColumn("history-rows", history.map((h) => ({ id: h.url, color: h.color, title: h.title, sub: h.domain })), false);
}

async function loadAll(query) {
  const [openResult, bookmarksResult, historyResult] = await Promise.all([
    rpcCall("switcher.open_pages", {}),
    rpcCall("switcher.bookmarks", { query: query || "" }),
    rpcCall("switcher.history", { query: query || "" }),
  ]);
  openPages = query ? openResult.pages.filter((p) => matches(p, query)) : openResult.pages;
  bookmarks = bookmarksResult.bookmarks;
  history = historyResult.history;
  renderAll();
  console.log(`switcher_loaded pages=${openPages.length} bookmarks=${bookmarks.length} history=${history.length}`);
}

function matches(page, query) {
  const q = query.toLowerCase();
  return page.title.toLowerCase().includes(q) || page.url.toLowerCase().includes(q);
}

function setActiveTab(tab) {
  document.querySelectorAll(".tabbtn").forEach((btn) => btn.classList.toggle("active", btn.dataset.tab === tab));
  document.querySelectorAll(".column").forEach((col) => {
    col.style.display = tab === "all" || col.dataset.column === tab ? "flex" : "none";
  });
}

document.getElementById("tabbar").addEventListener("click", (event) => {
  const btn = event.target.closest(".tabbtn");
  if (btn) setActiveTab(btn.dataset.tab);
});

document.getElementById("search").addEventListener("input", (event) => {
  loadAll(event.target.value.trim()).catch((err) => console.error(err));
});

document.getElementById("columns").addEventListener("click", (event) => {
  const closeBtn = event.target.closest('[data-action="close"]');
  const row = event.target.closest(".listrow");
  if (!row) return;
  const id = row.dataset.id;
  if (closeBtn) {
    rpcCall("switcher.close_page", { id }).then(() => loadAll(document.getElementById("search").value.trim())).catch((err) => console.error(err));
    event.stopPropagation();
    return;
  }
  if (row.closest('[data-column="open"]')) {
    rpcCall("switcher.switch_to", { id }).catch((err) => console.error(err));
  }
});

setActiveTab("all");
loadAll("").catch((err) => console.error(err));
