// Shared profile-avatar dropdown menu — rendered either inline (inside
// assets/profile/index.html's own avatar button) or as the sole content of
// a small webview the native toolbar's avatar button shows in a
// gtk::Popover (assets/profile-menu/). Settings/Passwords route through
// navigation.open_settings/.open_passwords (real RPC calls into the host)
// rather than a same-page link: a page shown inside a small popover
// webview has no way to navigate the host's *real* active page other than
// RPC, and using the same mechanism from both contexts means there's only
// one behavior to keep correct, not two.

function profileMenuHtml(profileInfo) {
  const others = profileInfo.other_profiles
    .map((name) => `<button class="menuitem" data-switch-profile="${escapeHtml(name)}">${escapeHtml(name)}</button>`)
    .join("");
  return `
    <div style="display:flex;align-items:center;gap:10px;padding:8px 10px 10px">
      <div class="avbtn" style="width:34px;height:34px;background:var(--accent)">${escapeHtml(profileInfo.initial)}</div>
      <div style="font-weight:700">${escapeHtml(profileInfo.name)}</div>
    </div>
    <button class="menuitem" data-action="settings">Settings</button>
    <button class="menuitem" data-action="passwords">Passwords</button>
    <div class="menudiv"></div>
    <div class="menuhead">Switch profile</div>
    <button class="menuitem" data-action="incognito">Incognito</button>
    <button class="menuitem" data-action="guest">Guest</button>
    ${others}
    <div class="menudiv"></div>
    <button class="menuitem" data-action="add-profile" style="color:var(--accent)">Add profile</button>`;
}

// `onAction`, if given, runs after any handled click (switch-profile or a
// data-action button) — the in-page dropdown uses it to close itself;
// the popover page has nothing to do here (the native gtk::Popover owns
// its own dismissal).
function wireProfileMenu(root, onAction) {
  root.addEventListener("click", (event) => {
    const switchBtn = event.target.closest("[data-switch-profile]");
    if (switchBtn) {
      rpcCall("profile.switch", { name: switchBtn.dataset.switchProfile }).catch((err) => console.error(err));
      onAction?.();
      return;
    }
    const actionEl = event.target.closest("[data-action]");
    if (!actionEl) return;
    const action = actionEl.dataset.action;
    if (action === "settings") {
      rpcCall("navigation.open_settings", {}).catch((err) => console.error(err));
    } else if (action === "passwords") {
      rpcCall("navigation.open_passwords", {}).catch((err) => console.error(err));
    } else if (action === "incognito" || action === "guest") {
      rpcCall("profile.new_ephemeral", {}).catch((err) => console.error(err));
    } else if (action === "add-profile") {
      const name = window.prompt("New profile name:");
      if (name) {
        const encrypted = window.confirm("Encrypt this profile with a passphrase?");
        rpcCall("profile.create", { name, encrypted }).catch((err) => console.error(err));
      }
    }
    onAction?.();
  });
}
